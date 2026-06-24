package main

import (
	"bytes"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"testing"
	"time"
)

func newTestServer() *Server {
	secret := []byte("test-secret-do-not-use-in-prod")
	return &Server{
		store:     NewStore(),
		tokens:    NewTokenIssuer(secret, 15*time.Minute),
		verifySig: func(_, _, _ []byte) bool { return true }, // accept-all: this suite doesn't test PQ sig verification itself
	}
}

func doJSON(t *testing.T, mux http.Handler, method, path string, body any, headers map[string]string) (*httptest.ResponseRecorder, map[string]any) {
	t.Helper()
	var buf bytes.Buffer
	if body != nil {
		if err := json.NewEncoder(&buf).Encode(body); err != nil {
			t.Fatalf("encode request body: %v", err)
		}
	}
	req := httptest.NewRequest(method, path, &buf)
	for k, v := range headers {
		req.Header.Set(k, v)
	}
	rec := httptest.NewRecorder()
	mux.ServeHTTP(rec, req)

	var parsed map[string]any
	if rec.Body.Len() > 0 {
		_ = json.Unmarshal(rec.Body.Bytes(), &parsed)
	}
	return rec, parsed
}

func registerAccount(t *testing.T, mux http.Handler, mlDsaPub, kemPub string) string {
	t.Helper()
	rec, body := doJSON(t, mux, "POST", "/v1/accounts", map[string]string{
		"ml_dsa_pub": mlDsaPub,
		"kem_pub":    kemPub,
	}, nil)
	if rec.Code != http.StatusCreated {
		t.Fatalf("register: expected 201, got %d (%v)", rec.Code, body)
	}
	return body["user_id"].(string)
}

func login(t *testing.T, mux http.Handler, userID string) string {
	t.Helper()
	doJSON(t, mux, "POST", "/v1/auth/challenge", map[string]string{"user_id": userID}, nil)
	rec, body := doJSON(t, mux, "POST", "/v1/auth/verify", map[string]string{
		"user_id":   userID,
		"signature": "AAAA",
	}, nil)
	if rec.Code != http.StatusOK {
		t.Fatalf("verify: expected 200, got %d (%v)", rec.Code, body)
	}
	return body["token"].(string)
}

func TestRegisterAndFetchAccount(t *testing.T) {
	s := newTestServer()
	mux := s.routes()

	userID := registerAccount(t, mux, "QQ==", "QQ==")

	rec, body := doJSON(t, mux, "GET", "/v1/accounts/"+userID, nil, nil)
	if rec.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d", rec.Code)
	}
	if body["ml_dsa_pub"] != "QQ==" || body["kem_pub"] != "QQ==" {
		t.Fatalf("unexpected account body: %v", body)
	}
}

func TestAuthChallengeVerifyAndRejectBadSignature(t *testing.T) {
	s := newTestServer()
	mux := s.routes()
	userID := registerAccount(t, mux, "QQ==", "QQ==")

	token := login(t, mux, userID)
	if token == "" {
		t.Fatal("expected non-empty token")
	}

	// A failing verifier should reject regardless of input.
	s2 := newTestServer()
	s2.verifySig = func(_, _, _ []byte) bool { return false }
	mux2 := s2.routes()
	userID2 := registerAccount(t, mux2, "QQ==", "QQ==")
	doJSON(t, mux2, "POST", "/v1/auth/challenge", map[string]string{"user_id": userID2}, nil)
	rec, _ := doJSON(t, mux2, "POST", "/v1/auth/verify", map[string]string{
		"user_id":   userID2,
		"signature": "AAAA",
	}, nil)
	if rec.Code != http.StatusUnauthorized {
		t.Fatalf("expected 401 with RejectAllVerifier, got %d", rec.Code)
	}
}

func TestLocationRelayPutInboxDelete(t *testing.T) {
	s := newTestServer()
	mux := s.routes()

	aID := registerAccount(t, mux, "QQ==", "QQ==")
	bID := registerAccount(t, mux, "Qg==", "Qg==")
	aToken := login(t, mux, aID)
	bToken := login(t, mux, bID)

	share := map[string]string{
		"ciphertext":     "Y2lwaGVy",
		"wrapped_key":    "d3JhcA==",
		"kem_ciphertext": "a2Vt",
		"nonce":          "bm9uY2U=",
		"wrap_nonce":     "d3JhcG5vbmNl",
	}

	// No token: rejected.
	rec, _ := doJSON(t, mux, "PUT", "/v1/locations/"+bID, share, nil)
	if rec.Code != http.StatusUnauthorized {
		t.Fatalf("expected 401 without token, got %d", rec.Code)
	}

	// A shares to B.
	rec, _ = doJSON(t, mux, "PUT", "/v1/locations/"+bID, share, map[string]string{"Authorization": "Bearer " + aToken})
	if rec.Code != http.StatusNoContent {
		t.Fatalf("expected 204 on PUT, got %d", rec.Code)
	}

	// B's inbox has exactly one entry from A.
	rec, _ = doJSON(t, mux, "GET", "/v1/locations/inbox", nil, map[string]string{"Authorization": "Bearer " + bToken})
	if rec.Code != http.StatusOK {
		t.Fatalf("expected 200 on inbox, got %d", rec.Code)
	}
	inbox := mustDecodeList(t, rec.Body.Bytes())
	if len(inbox) != 1 || inbox[0]["from"] != aID {
		t.Fatalf("expected one share from %s, got %v", aID, inbox)
	}

	// A's own inbox is empty: A is the sender, not a recipient here.
	rec, _ = doJSON(t, mux, "GET", "/v1/locations/inbox", nil, map[string]string{"Authorization": "Bearer " + aToken})
	if list := mustDecodeList(t, rec.Body.Bytes()); len(list) != 0 {
		t.Fatalf("expected empty inbox for sender, got %v", list)
	}

	// A second PUT overwrites, doesn't append: latest-only, no history (design doc section 2).
	doJSON(t, mux, "PUT", "/v1/locations/"+bID, share, map[string]string{"Authorization": "Bearer " + aToken})
	rec, _ = doJSON(t, mux, "GET", "/v1/locations/inbox", nil, map[string]string{"Authorization": "Bearer " + bToken})
	if list := mustDecodeList(t, rec.Body.Bytes()); len(list) != 1 {
		t.Fatalf("expected exactly one entry after overwrite, got %d", len(list))
	}

	// Revoke: DELETE removes it immediately.
	rec, _ = doJSON(t, mux, "DELETE", "/v1/locations/"+bID, nil, map[string]string{"Authorization": "Bearer " + aToken})
	if rec.Code != http.StatusNoContent {
		t.Fatalf("expected 204 on DELETE, got %d", rec.Code)
	}
	rec, _ = doJSON(t, mux, "GET", "/v1/locations/inbox", nil, map[string]string{"Authorization": "Bearer " + bToken})
	if list := mustDecodeList(t, rec.Body.Bytes()); len(list) != 0 {
		t.Fatalf("expected empty inbox after revoke, got %v", list)
	}
}

func mustDecodeList(t *testing.T, raw []byte) []map[string]any {
	t.Helper()
	var out []map[string]any
	if err := json.Unmarshal(raw, &out); err != nil {
		t.Fatalf("decode list: %v (%s)", err, raw)
	}
	return out
}

func TestExpiredTokenIsRejected(t *testing.T) {
	s := newTestServer()
	s.tokens = NewTokenIssuer([]byte("test-secret"), -1*time.Minute) // already expired
	mux := s.routes()
	userID := registerAccount(t, mux, "QQ==", "QQ==")
	token := login(t, mux, userID)

	rec, _ := doJSON(t, mux, "GET", "/v1/locations/inbox", nil, map[string]string{"Authorization": "Bearer " + token})
	if rec.Code != http.StatusUnauthorized {
		t.Fatalf("expected 401 for expired token, got %d", rec.Code)
	}
}
