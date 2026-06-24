package main

import (
	"context"
	"crypto/rand"
	"encoding/base64"
	"encoding/json"
	"log"
	"net/http"
	"os"
	"strings"
	"time"
)

type Server struct {
	store     *Store
	tokens    *TokenIssuer
	verifySig SignatureVerifier
}

func writeJSON(w http.ResponseWriter, status int, v any) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(status)
	_ = json.NewEncoder(w).Encode(v)
}

func writeErr(w http.ResponseWriter, status int, msg string) {
	writeJSON(w, status, map[string]string{"error": msg})
}

func b64decode(s string) ([]byte, error) {
	return base64.StdEncoding.DecodeString(s)
}

func b64encode(b []byte) string {
	return base64.StdEncoding.EncodeToString(b)
}

// POST /v1/accounts
// { "ml_dsa_pub": "<b64>", "kem_pub": "<b64>" } -> { "user_id": "..." }
func (s *Server) handleRegister(w http.ResponseWriter, r *http.Request) {
	var req struct {
		MlDsaPub  string `json:"ml_dsa_pub"`
		KemPub    string `json:"kem_pub"`
		X25519Pub string `json:"x25519_pub"`
	}
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		writeErr(w, http.StatusBadRequest, "invalid body")
		return
	}
	mlDsaPub, err1 := b64decode(req.MlDsaPub)
	kemPub, err2 := b64decode(req.KemPub)
	x25519Pub, err3 := b64decode(req.X25519Pub)
	if err1 != nil || err2 != nil || err3 != nil || len(mlDsaPub) == 0 || len(kemPub) == 0 {
		writeErr(w, http.StatusBadRequest, "ml_dsa_pub, kem_pub, and x25519_pub are required, base64-encoded")
		return
	}

	userID := randomID()
	s.store.PutAccount(Account{
		UserID:    userID,
		MlDsaPub:  mlDsaPub,
		KemPub:    kemPub,
		X25519Pub: x25519Pub,
		CreatedAt: time.Now(),
	})

	writeJSON(w, http.StatusCreated, map[string]string{"user_id": userID})
}

// GET /v1/accounts/{id}
func (s *Server) handleGetAccount(w http.ResponseWriter, r *http.Request) {
	id := strings.TrimPrefix(r.URL.Path, "/v1/accounts/")
	acc, ok := s.store.GetAccount(id)
	if !ok {
		writeErr(w, http.StatusNotFound, "no such account")
		return
	}
	writeJSON(w, http.StatusOK, map[string]string{
		"user_id":     acc.UserID,
		"ml_dsa_pub":  b64encode(acc.MlDsaPub),
		"kem_pub":     b64encode(acc.KemPub),
		"x25519_pub":  b64encode(acc.X25519Pub),
	})
}

// POST /v1/auth/challenge
// { "user_id": "..." } -> { "nonce": "<b64>" }
func (s *Server) handleChallenge(w http.ResponseWriter, r *http.Request) {
	var req struct {
		UserID string `json:"user_id"`
	}
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		writeErr(w, http.StatusBadRequest, "invalid body")
		return
	}

	if _, ok := s.store.GetAccount(req.UserID); !ok {
		writeErr(w, http.StatusNotFound, "no such account")
		return
	}

	nonce := make([]byte, 32)
	if _, err := rand.Read(nonce); err != nil {
		writeErr(w, http.StatusInternalServerError, "rng failure")
		return
	}

	s.store.PutChallenge(req.UserID, Challenge{Nonce: nonce, ExpiresAt: time.Now().Add(2 * time.Minute)})

	writeJSON(w, http.StatusOK, map[string]string{"nonce": b64encode(nonce)})
}

// POST /v1/auth/verify
// { "user_id": "...", "signature": "<b64>" } -> { "token": "...", "expires_in": 900 }
func (s *Server) handleVerify(w http.ResponseWriter, r *http.Request) {
	var req struct {
		UserID    string `json:"user_id"`
		Signature string `json:"signature"`
	}
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		writeErr(w, http.StatusBadRequest, "invalid body")
		return
	}
	sig, err := b64decode(req.Signature)
	if err != nil {
		writeErr(w, http.StatusBadRequest, "bad signature encoding")
		return
	}

	ch, ok := s.store.PopChallenge(req.UserID)
	acc, accOk := s.store.GetAccount(req.UserID)

	if !ok || time.Now().After(ch.ExpiresAt) {
		writeErr(w, http.StatusUnauthorized, "no active challenge, request a new one")
		return
	}
	if !accOk {
		writeErr(w, http.StatusNotFound, "no such account")
		return
	}
	if !s.verifySig(acc.MlDsaPub, ch.Nonce, sig) {
		writeErr(w, http.StatusUnauthorized, "signature verification failed")
		return
	}

	token, expiresIn := s.tokens.Issue(req.UserID)
	writeJSON(w, http.StatusOK, map[string]any{"token": token, "expires_in": expiresIn})
}

// authMiddleware extracts and verifies the bearer token, injecting the user_id into context.
func (s *Server) authMiddleware(next func(w http.ResponseWriter, r *http.Request, userID string)) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		authz := r.Header.Get("Authorization")
		if !strings.HasPrefix(authz, "Bearer ") {
			writeErr(w, http.StatusUnauthorized, "missing bearer token")
			return
		}
		userID, err := s.tokens.Verify(strings.TrimPrefix(authz, "Bearer "))
		if err != nil {
			writeErr(w, http.StatusUnauthorized, "invalid or expired token")
			return
		}
		next(w, r, userID)
	}
}

// PUT /v1/locations/{recipient_id}
// { "ciphertext": "<b64>", "wrapped_key": "<b64>", "kem_ciphertext": "<b64>", "nonce": "<b64>", "wrap_nonce": "<b64>" }
func (s *Server) handlePutLocation(w http.ResponseWriter, r *http.Request, userID string) {
	to := strings.TrimPrefix(r.URL.Path, "/v1/locations/")
	if to == "" {
		writeErr(w, http.StatusBadRequest, "missing recipient id")
		return
	}

	var req struct {
		Ciphertext    string `json:"ciphertext"`
		WrappedKey    string `json:"wrapped_key"`
		KemCiphertext string `json:"kem_ciphertext"`
		Nonce         string `json:"nonce"`
		WrapNonce     string `json:"wrap_nonce"`
	}
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		writeErr(w, http.StatusBadRequest, "invalid body")
		return
	}

	ciphertext, e1 := b64decode(req.Ciphertext)
	wrappedKey, e2 := b64decode(req.WrappedKey)
	kemCt, e3 := b64decode(req.KemCiphertext)
	nonce, e4 := b64decode(req.Nonce)
	wrapNonce, e5 := b64decode(req.WrapNonce)
	if e1 != nil || e2 != nil || e3 != nil || e4 != nil || e5 != nil {
		writeErr(w, http.StatusBadRequest, "all fields must be base64")
		return
	}

	if _, recipientExists := s.store.GetAccount(to); !recipientExists {
		writeErr(w, http.StatusNotFound, "recipient does not exist")
		return
	}

	s.store.PutRelay(RelayEntry{
		From:          userID,
		To:            to,
		Ciphertext:    ciphertext,
		WrappedKey:    wrappedKey,
		KemCiphertext: kemCt,
		Nonce:         nonce,
		WrapNonce:     wrapNonce,
		UpdatedAt:     time.Now(),
	})

	w.WriteHeader(http.StatusNoContent)
}

// GET /v1/locations/inbox
func (s *Server) handleInbox(w http.ResponseWriter, r *http.Request, userID string) {
	type item struct {
		From          string `json:"from"`
		Ciphertext    string `json:"ciphertext"`
		WrappedKey    string `json:"wrapped_key"`
		KemCiphertext string `json:"kem_ciphertext"`
		Nonce         string `json:"nonce"`
		WrapNonce     string `json:"wrap_nonce"`
		UpdatedAt     int64  `json:"updated_at"`
	}

	entries := s.store.InboxFor(userID)
	out := make([]item, 0, len(entries))
	for _, e := range entries {
		out = append(out, item{
			From:          e.From,
			Ciphertext:    b64encode(e.Ciphertext),
			WrappedKey:    b64encode(e.WrappedKey),
			KemCiphertext: b64encode(e.KemCiphertext),
			Nonce:         b64encode(e.Nonce),
			WrapNonce:     b64encode(e.WrapNonce),
			UpdatedAt:     e.UpdatedAt.Unix(),
		})
	}
	writeJSON(w, http.StatusOK, out)
}

// DELETE /v1/locations/{recipient_id}
func (s *Server) handleDeleteLocation(w http.ResponseWriter, r *http.Request, userID string) {
	to := strings.TrimPrefix(r.URL.Path, "/v1/locations/")
	s.store.DeleteRelay(userID, to)
	w.WriteHeader(http.StatusNoContent)
}

func randomID() string {
	b := make([]byte, 16)
	_, _ = rand.Read(b)
	return base64.RawURLEncoding.EncodeToString(b)
}

func (s *Server) routes() *http.ServeMux {
	mux := http.NewServeMux()
	mux.HandleFunc("POST /v1/accounts", s.handleRegister)
	mux.HandleFunc("GET /v1/accounts/{id}", s.handleGetAccount)
	mux.HandleFunc("POST /v1/auth/challenge", s.handleChallenge)
	mux.HandleFunc("POST /v1/auth/verify", s.handleVerify)
	mux.HandleFunc("PUT /v1/locations/{id}", s.authMiddleware(s.handlePutLocation))
	mux.HandleFunc("GET /v1/locations/inbox", s.authMiddleware(s.handleInbox))
	mux.HandleFunc("DELETE /v1/locations/{id}", s.authMiddleware(s.handleDeleteLocation))
	return mux
}

func main() {
	ctx := context.Background()

	var store *Store
	connStr := os.Getenv("DATABASE_URL")
	if connStr != "" {
		var err error
		store, err = OpenStorePG(ctx, connStr)
		if err != nil {
			log.Fatalf("failed to open PostgreSQL store: %v", err)
		}
		log.Println("using PostgreSQL store")
	} else {
		store = NewStore()
		log.Println("DATABASE_URL not set — using in-memory store")
	}

	secret := make([]byte, 32)
	if _, err := rand.Read(secret); err != nil {
		log.Fatal(err)
	}

	s := &Server{
		store:     store,
		tokens:    NewTokenIssuer(secret, 15*time.Minute),
		verifySig: circlMldsa65Verifier,
	}

	log.Println("locshare relay listening on :8080")
	log.Fatal(http.ListenAndServe(":8080", s.routes()))
}
