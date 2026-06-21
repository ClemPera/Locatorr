package main

import (
	"crypto/hmac"
	"crypto/sha256"
	"crypto/subtle"
	"encoding/base64"
	"errors"
	"fmt"
	"strconv"
	"strings"
	"time"
)

// --- Stateless session tokens ---
//
// The design doc calls these "JWT". Functionally a token here is the same thing a JWT would
// give us (signed, time-limited, no server-side session table needed), built with stdlib only
// so this scaffold has zero non-stdlib Go dependencies. Swap in a real JWT library
// (e.g. golang-jwt) if you want standard claims/tooling; the security property is identical
// either way: the server can't forge or accept a token without serverSecret.

type TokenIssuer struct {
	secret []byte
	ttl    time.Duration
}

func NewTokenIssuer(secret []byte, ttl time.Duration) *TokenIssuer {
	return &TokenIssuer{secret: secret, ttl: ttl}
}

func (t *TokenIssuer) Issue(userID string) (token string, expiresIn int64) {
	exp := time.Now().Add(t.ttl).Unix()
	payload := fmt.Sprintf("%s.%d", userID, exp)
	sig := t.sign(payload)
	return payload + "." + sig, int64(t.ttl.Seconds())
}

func (t *TokenIssuer) Verify(token string) (userID string, err error) {
	parts := strings.Split(token, ".")
	if len(parts) != 3 {
		return "", errors.New("malformed token")
	}
	payload := parts[0] + "." + parts[1]
	expectedSig := t.sign(payload)
	if subtle.ConstantTimeCompare([]byte(expectedSig), []byte(parts[2])) != 1 {
		return "", errors.New("bad signature")
	}
	exp, err := strconv.ParseInt(parts[1], 10, 64)
	if err != nil {
		return "", errors.New("bad expiry")
	}
	if time.Now().Unix() > exp {
		return "", errors.New("expired")
	}
	return parts[0], nil
}

func (t *TokenIssuer) sign(payload string) string {
	mac := hmac.New(sha256.New, t.secret)
	mac.Write([]byte(payload))
	return base64.RawURLEncoding.EncodeToString(mac.Sum(nil))
}

// --- PQ signature verification ---
//
// NOT IMPLEMENTED HERE ON PURPOSE. Verifying the ML-DSA-65 signature on the auth challenge
// needs a Go ML-DSA implementation (e.g. github.com/cloudflare/circl/sign/mldsa/mldsa65).
// That package couldn't be fetched in this sandbox: its transitive dependency
// golang.org/x/sys is hosted on a domain outside this environment's network allowlist.
// Wire up a real implementation before this leaves your machine; the default below fails
// closed (rejects everything) rather than silently accepting unverified signatures.
//
// Wiring it up with circl is three lines:
//
//	var pub mldsa65.PublicKey
//	pub.UnmarshalBinary(account.MlDsaPub)
//	ok := mldsa65.Verify(&pub, nonce, nil, signature)
type SignatureVerifier func(pub, msg, sig []byte) bool

func RejectAllVerifier(_, _, _ []byte) bool {
	return false
}
