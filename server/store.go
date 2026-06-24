package main

import (
	"context"
	"sync"
	"time"

	"github.com/jackc/pgx/v5/pgxpool"
)

// Account holds only public key material. The server never sees a private key.
type Account struct {
	UserID    string
	MlDsaPub  []byte
	KemPub    []byte
	CreatedAt time.Time
}

// RelayEntry is the *latest* encrypted share from one user to another.
type RelayEntry struct {
	From          string
	To            string
	Ciphertext    []byte
	WrappedKey    []byte
	KemCiphertext []byte
	Nonce         []byte
	WrapNonce     []byte
	UpdatedAt     time.Time
}

// Challenge is a short-lived login nonce issued to a user_id, consumed on first use.
type Challenge struct {
	Nonce     []byte
	ExpiresAt time.Time
}

// Store is a mutex-guarded cache backed by either an in-memory map (tests)
// or PostgreSQL (production). The maps serve as a fast read cache; all writes
// go through to the backing store first.
type Store struct {
	mu         sync.RWMutex
	accounts   map[string]Account
	challenges map[string]Challenge
	relay      map[string]RelayEntry

	pool *pgxpool.Pool // nil means in-memory-only
}

const pgSchema = `
CREATE TABLE IF NOT EXISTS accounts (
	user_id    TEXT PRIMARY KEY,
	ml_dsa_pub BYTEA NOT NULL,
	kem_pub    BYTEA NOT NULL,
	created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE TABLE IF NOT EXISTS challenges (
	user_id   TEXT PRIMARY KEY,
	nonce     BYTEA NOT NULL,
	expires_at TIMESTAMPTZ NOT NULL
);
CREATE TABLE IF NOT EXISTS relay (
	from_user      TEXT NOT NULL,
	to_user        TEXT NOT NULL,
	ciphertext     BYTEA NOT NULL,
	wrapped_key    BYTEA NOT NULL,
	kem_ciphertext BYTEA NOT NULL,
	nonce          BYTEA NOT NULL,
	wrap_nonce     BYTEA NOT NULL,
	updated_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
	PRIMARY KEY (from_user, to_user)
);
`

// NewStore creates an in-memory store with no external database.
// Suitable for tests and single-use runs.
func NewStore() *Store {
	return &Store{
		accounts:   make(map[string]Account),
		challenges: make(map[string]Challenge),
		relay:      make(map[string]RelayEntry),
	}
}

// OpenStorePG opens a PostgreSQL-backed store. The connection string is passed
// directly to pgxpool (e.g. "postgres://user:pass@localhost/locatorr").
// It runs the schema and preloads existing data into the read cache.
func OpenStorePG(ctx context.Context, connStr string) (*Store, error) {
	pool, err := pgxpool.New(ctx, connStr)
	if err != nil {
		return nil, err
	}
	if _, err := pool.Exec(ctx, pgSchema); err != nil {
		pool.Close()
		return nil, err
	}
	s := &Store{
		pool:       pool,
		accounts:   make(map[string]Account),
		challenges: make(map[string]Challenge),
		relay:      make(map[string]RelayEntry),
	}
	if err := s.loadAll(ctx); err != nil {
		pool.Close()
		return nil, err
	}
	return s, nil
}

func relayKey(from, to string) string { return from + "|" + to }

// --- public read/write API ---

func (s *Store) GetAccount(userID string) (Account, bool) {
	s.mu.RLock()
	defer s.mu.RUnlock()
	a, ok := s.accounts[userID]
	return a, ok
}

func (s *Store) PutAccount(a Account) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.accounts[a.UserID] = a
	if s.pool != nil {
		_, _ = s.pool.Exec(context.Background(),
			`INSERT INTO accounts (user_id, ml_dsa_pub, kem_pub, created_at)
			 VALUES ($1, $2, $3, $4)
			 ON CONFLICT (user_id) DO UPDATE SET ml_dsa_pub = $2, kem_pub = $3`,
			a.UserID, a.MlDsaPub, a.KemPub, a.CreatedAt,
		)
	}
}

// PopChallenge atomically fetches and deletes a challenge (one-time use).
func (s *Store) PopChallenge(userID string) (Challenge, bool) {
	s.mu.Lock()
	defer s.mu.Unlock()
	ch, ok := s.challenges[userID]
	if ok {
		delete(s.challenges, userID)
		if s.pool != nil {
			_, _ = s.pool.Exec(context.Background(),
				"DELETE FROM challenges WHERE user_id = $1", userID,
			)
		}
	}
	return ch, ok
}

func (s *Store) PutChallenge(userID string, ch Challenge) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.challenges[userID] = ch
	if s.pool != nil {
		_, _ = s.pool.Exec(context.Background(),
			`INSERT INTO challenges (user_id, nonce, expires_at) VALUES ($1, $2, $3)
			 ON CONFLICT (user_id) DO UPDATE SET nonce = $2, expires_at = $3`,
			userID, ch.Nonce, ch.ExpiresAt,
		)
	}
}

func (s *Store) PutRelay(e RelayEntry) {
	s.mu.Lock()
	defer s.mu.Unlock()
	key := relayKey(e.From, e.To)
	s.relay[key] = e
	if s.pool != nil {
		_, _ = s.pool.Exec(context.Background(),
			`INSERT INTO relay (from_user, to_user, ciphertext, wrapped_key, kem_ciphertext, nonce, wrap_nonce, updated_at)
			 VALUES ($1,$2,$3,$4,$5,$6,$7,$8)
			 ON CONFLICT (from_user, to_user) DO UPDATE SET
			   ciphertext=$3, wrapped_key=$4, kem_ciphertext=$5, nonce=$6, wrap_nonce=$7, updated_at=$8`,
			e.From, e.To, e.Ciphertext, e.WrappedKey, e.KemCiphertext, e.Nonce, e.WrapNonce, e.UpdatedAt,
		)
	}
}

func (s *Store) DeleteRelay(from, to string) {
	s.mu.Lock()
	defer s.mu.Unlock()
	key := relayKey(from, to)
	delete(s.relay, key)
	if s.pool != nil {
		_, _ = s.pool.Exec(context.Background(),
			"DELETE FROM relay WHERE from_user = $1 AND to_user = $2", from, to,
		)
	}
}

func (s *Store) InboxFor(userID string) []RelayEntry {
	s.mu.RLock()
	defer s.mu.RUnlock()
	var out []RelayEntry
	for _, e := range s.relay {
		if e.To == userID {
			out = append(out, e)
		}
	}
	if out == nil {
		out = []RelayEntry{}
	}
	return out
}

// --- initial load from PG ---

func (s *Store) loadAll(ctx context.Context) error {
	s.mu.Lock()
	defer s.mu.Unlock()

	rows, err := s.pool.Query(ctx, "SELECT user_id, ml_dsa_pub, kem_pub, created_at FROM accounts")
	if err != nil {
		return err
	}
	defer rows.Close()
	for rows.Next() {
		var a Account
		if err := rows.Scan(&a.UserID, &a.MlDsaPub, &a.KemPub, &a.CreatedAt); err != nil {
			return err
		}
		s.accounts[a.UserID] = a
	}

	cRows, err := s.pool.Query(ctx, "SELECT user_id, nonce, expires_at FROM challenges")
	if err != nil {
		return err
	}
	defer cRows.Close()
	for cRows.Next() {
		var userID string
		var ch Challenge
		if err := cRows.Scan(&userID, &ch.Nonce, &ch.ExpiresAt); err != nil {
			return err
		}
		s.challenges[userID] = ch
	}

	rRows, err := s.pool.Query(ctx, "SELECT from_user, to_user, ciphertext, wrapped_key, kem_ciphertext, nonce, wrap_nonce, updated_at FROM relay")
	if err != nil {
		return err
	}
	defer rRows.Close()
	for rRows.Next() {
		var e RelayEntry
		if err := rRows.Scan(&e.From, &e.To, &e.Ciphertext, &e.WrappedKey, &e.KemCiphertext, &e.Nonce, &e.WrapNonce, &e.UpdatedAt); err != nil {
			return err
		}
		s.relay[relayKey(e.From, e.To)] = e
	}
	return nil
}
