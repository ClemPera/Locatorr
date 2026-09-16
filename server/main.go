package main

import (
	"context"
	"crypto/rand"
	"encoding/hex"
	"fmt"
	"net/http"
	"os"
	"time"

	"github.com/ClemPera/Locatorr/server/db"
	"github.com/gin-gonic/gin"
	"github.com/jackc/pgx/v5/pgtype"
	"github.com/jackc/pgx/v5/pgxpool"
	"github.com/joho/godotenv"
)

type State struct {
	queries *db.Queries
	ctx     context.Context
}

func Must[T any](val T, err error) T {
	if err != nil {
		panic(fmt.Sprintf("%s", err))
	}
	return val
}

func main() {
	Must("", godotenv.Load())

	//Setup db
	ctx := context.Background()

	pool := Must(pgxpool.New(ctx, os.Getenv("DATABASE_URL")))
	defer pool.Close()

	state := State{
		queries: db.New(pool),
		ctx:     ctx,
	}

	//Create schema if they don't already exists
	//TODO: make migrations system with goose or manually
	schema := Must(os.ReadFile("schema.sql"))
	conn := Must(pool.Acquire(ctx))
	Must(conn.Exec(ctx, string(schema)))
	conn.Release()

	//Setup routing
	router := gin.Default()

	// Phase 0: Room creation
	router.POST("/rendezvous", state.createRendezvous)

	// Phase 1: Pairing — pubkey exchange
	router.POST("/rendezvous/:id", state.postRendezvousPubkey)
	router.GET("/rendezvous/:id", state.getRendezvousPubkeys)

	// Phase 1: Pairing — encrypted bundle exchange
	router.POST("/rendezvous/:id/bundle", state.postRendezvousBundle)
	router.GET("/rendezvous/:id/bundle", state.getRendezvousBundles)

	// Phase 2 & 3: Inbox
	router.POST("/inbox/:user_id", state.postInbox)
	router.GET("/inbox/:user_id", state.getInbox)

	s := &http.Server{
		Addr:    ":9191",
		Handler: router,
	}
	s.ListenAndServe()
}

// ============================================================================
// PHASE 0 — ROOM CREATION
// ============================================================================

func (s *State) createRendezvous(c *gin.Context) {
	// Generate 128-bit cryptographically random ID (32 hex chars)
	idBytes := make([]byte, 16)
	if _, err := rand.Read(idBytes); err != nil {
		c.JSON(http.StatusInternalServerError, gin.H{"error": "failed to generate room id"})
		return
	}
	id := hex.EncodeToString(idBytes)

	expire := pgtype.Timestamp{
		Time:  time.Now().Add(24 * time.Hour).UTC(),
		Valid: true,
	}

	room, err := s.queries.InsertRoom(s.ctx, db.InsertRoomParams{
		ID:        id,
		ExpiresAt: expire,
	})
	if err != nil {
		c.JSON(http.StatusInternalServerError, gin.H{"error": "failed to create room"})
		return
	}

	c.JSON(http.StatusCreated, gin.H{
		"rendezvous_id": room.ID,
		"expires_at":    room.ExpiresAt.Time.Format(time.RFC3339),
	})
}

// ============================================================================
// PHASE 1 — PAIRING: PUBKEY EXCHANGE
// ============================================================================

type PubkeyUpload struct {
	Role   string `json:"role" binding:"required,oneof=initiator responder"`
	Pubkey []byte `json:"pubkey" binding:"required"`
}

// POST /rendezvous/:id — both sides upload their ephemeral X25519 pubkey.
func (s *State) postRendezvousPubkey(c *gin.Context) {
	roomID := c.Param("id")

	// Verify room exists and isn't expired
	if _, err := s.queries.GetRoom(s.ctx, roomID); err != nil {
		c.JSON(http.StatusNotFound, gin.H{"error": "room not found or expired"})
		return
	}

	var req PubkeyUpload
	if err := c.ShouldBindJSON(&req); err != nil {
		c.JSON(http.StatusBadRequest, gin.H{"error": err.Error()})
		return
	}

	if err := s.queries.UpsertPubkey(s.ctx, db.UpsertPubkeyParams{
		RoomID: roomID,
		Role:   req.Role,
		Pubkey: req.Pubkey,
	}); err != nil {
		c.JSON(http.StatusInternalServerError, gin.H{"error": "failed to store pubkey"})
		return
	}

	c.JSON(http.StatusOK, gin.H{"status": "ok"})
}

// GET /rendezvous/:id — retrieve pubkeys so each side can see the other's key.
func (s *State) getRendezvousPubkeys(c *gin.Context) {
	roomID := c.Param("id")

	if _, err := s.queries.GetRoom(s.ctx, roomID); err != nil {
		c.JSON(http.StatusNotFound, gin.H{"error": "room not found or expired"})
		return
	}

	pubkeys, err := s.queries.GetPubkeys(s.ctx, roomID)
	if err != nil {
		c.JSON(http.StatusInternalServerError, gin.H{"error": "failed to fetch pubkeys"})
		return
	}

	result := make([]gin.H, len(pubkeys))
	for i, pk := range pubkeys {
		result[i] = gin.H{"role": pk.Role, "pubkey": pk.Pubkey}
	}

	c.JSON(http.StatusOK, result)
}

// ============================================================================
// PHASE 1 — PAIRING: ENCRYPTED BUNDLE EXCHANGE
// ============================================================================

type BundleUpload struct {
	Role   string `json:"role" binding:"required,oneof=initiator responder"`
	Bundle []byte `json:"bundle" binding:"required"`
}

// POST /rendezvous/:id/bundle — upload encrypted static bundle (opaque to server).
func (s *State) postRendezvousBundle(c *gin.Context) {
	roomID := c.Param("id")

	if _, err := s.queries.GetRoom(s.ctx, roomID); err != nil {
		c.JSON(http.StatusNotFound, gin.H{"error": "room not found or expired"})
		return
	}

	var req BundleUpload
	if err := c.ShouldBindJSON(&req); err != nil {
		c.JSON(http.StatusBadRequest, gin.H{"error": err.Error()})
		return
	}

	if err := s.queries.UpsertBundle(s.ctx, db.UpsertBundleParams{
		RoomID: roomID,
		Role:   req.Role,
		Bundle: req.Bundle,
	}); err != nil {
		c.JSON(http.StatusInternalServerError, gin.H{"error": "failed to store bundle"})
		return
	}

	c.JSON(http.StatusOK, gin.H{"status": "ok"})
}

// GET /rendezvous/:id/bundle — retrieve encrypted bundles.
func (s *State) getRendezvousBundles(c *gin.Context) {
	roomID := c.Param("id")

	if _, err := s.queries.GetRoom(s.ctx, roomID); err != nil {
		c.JSON(http.StatusNotFound, gin.H{"error": "room not found or expired"})
		return
	}

	bundles, err := s.queries.GetBundles(s.ctx, roomID)
	if err != nil {
		c.JSON(http.StatusInternalServerError, gin.H{"error": "failed to fetch bundles"})
		return
	}

	result := make([]gin.H, len(bundles))
	for i, b := range bundles {
		result[i] = gin.H{"role": b.Role, "bundle": b.Bundle}
	}

	c.JSON(http.StatusOK, result)
}

// ============================================================================
// PHASE 2 — ALICE SENDS (INBOX WRITE)
// ============================================================================

type InboxMessage struct {
	Payload []byte `json:"payload" binding:"required"`
}

// POST /inbox/:user_id — Alice uploads an encrypted location update package.
func (s *State) postInbox(c *gin.Context) {
	userID := c.Param("user_id")

	var req InboxMessage
	if err := c.ShouldBindJSON(&req); err != nil {
		c.JSON(http.StatusBadRequest, gin.H{"error": err.Error()})
		return
	}

	if err := s.queries.InsertInboxMessage(s.ctx, db.InsertInboxMessageParams{
		UserID:  userID,
		Payload: req.Payload,
	}); err != nil {
		c.JSON(http.StatusInternalServerError, gin.H{"error": "failed to store message"})
		return
	}

	c.JSON(http.StatusCreated, gin.H{"status": "ok"})
}

// ============================================================================
// PHASE 3 — BOB RECEIVES (INBOX READ + DELETE)
// ============================================================================

// GET /inbox/:user_id — Bob polls for new packages (delete-after-read).
func (s *State) getInbox(c *gin.Context) {
	userID := c.Param("user_id")

	messages, err := s.queries.GetAndDeleteInbox(s.ctx, userID)
	if err != nil {
		c.JSON(http.StatusInternalServerError, gin.H{"error": "failed to fetch inbox"})
		return
	}

	result := make([]gin.H, len(messages))
	for i, m := range messages {
		result[i] = gin.H{
			"id":         m.ID,
			"payload":    m.Payload,
			"created_at": m.CreatedAt.Time.Format(time.RFC3339),
		}
	}

	c.JSON(http.StatusOK, result)
}
