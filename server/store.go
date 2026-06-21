package main

import (
	"sync"
	"time"
)

// Account holds only public key material. The server never sees a private key.
type Account struct {
	UserID    string
	MlDsaPub  []byte // ML-DSA-65 public key, used to verify the auth challenge signature
	KemPub    []byte // hybrid X25519 || ML-KEM-768 encapsulation public key material
	CreatedAt time.Time
}

// RelayEntry is the *latest* encrypted share from one user to another.
// Each PUT overwrites the previous entry for the same (From, To) pair: no history is kept.
type RelayEntry struct {
	From          string
	To            string
	Ciphertext    []byte // AES-256-GCM ciphertext of the location payload, under CK
	WrappedKey    []byte // CK wrapped (AES-256-GCM) under the hybrid wrap_key
	KemCiphertext []byte // ML-KEM encapsulation ciphertext needed by the recipient to recover wrap_key
	Nonce         []byte // AEAD nonce used for Ciphertext (the WrappedKey has its own nonce, see below)
	WrapNonce     []byte // AEAD nonce used for WrappedKey
	UpdatedAt     time.Time
}

// Challenge is a short-lived login nonce issued to a user_id, consumed on first use.
type Challenge struct {
	Nonce     []byte
	ExpiresAt time.Time
}

// Store is a simple in-memory, mutex-guarded store.
//
// This is intentionally minimal for the scaffold: every row here is either a public key
// (accounts) or an opaque encrypted blob (relay). Swap this for Postgres per the design doc
// once this needs to survive a restart; the schema is already in section 8 of the design doc
// and maps directly onto these structs.
type Store struct {
	mu         sync.RWMutex
	accounts   map[string]Account
	challenges map[string]Challenge
	relay      map[string]RelayEntry // key: from+"|"+to
	nextUserID int
}

func NewStore() *Store {
	return &Store{
		accounts:   make(map[string]Account),
		challenges: make(map[string]Challenge),
		relay:      make(map[string]RelayEntry),
	}
}

func relayKey(from, to string) string { return from + "|" + to }
