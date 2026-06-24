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

	"github.com/cloudflare/circl/sign/mldsa/mldsa65"
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
// Uses CIRCL (github.com/cloudflare/circl) for ML-DSA-65 verification against the
// account's stored public key. This was stubbed to fail-closed in the initial scaffold
// because circl's transitive dep golang.org/x/sys couldn't be fetched in the dev sandbox;
// that network constraint is no longer present.
type SignatureVerifier func(pub, msg, sig []byte) bool

func circlMldsa65Verifier(pub, msg, sig []byte) bool {
	var pk mldsa65.PublicKey
	if err := pk.UnmarshalBinary(pub); err != nil {
		return false
	}
	return mldsa65.Verify(&pk, msg, nil, sig)
}
