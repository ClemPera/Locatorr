> Acceptance criteria & progress tracking live in [`roadmap.md`](./roadmap.md).
> Companion diagram: [`protocol_diagram.html`](./protocol_diagram.html).

# Phase 1: Pairing & Bootstrapping (The Rendezvous Flow)

To work around the size limits of QR codes and deep links, the initial pairing
can use a two-step "rendezvous" exchange instead of transferring the entire
public key payload directly.

1. The Out-of-Band Invitation (QR or Link)

Instead of encoding the heavy PQ keys, the initiator (Alice) generates:

  - A temporary 32-byte rendezvous token (token).
  - An ephemeral X25519 key pair (x_{temp}, X_{temp}).

She creates a deep link or QR code containing:
yourapp://pair?id={rendezvous_id}&key={X_temp_base64}&token={token_base64}

This payload is small (~100 bytes), making the QR code simple to scan and the
deep link highly stable across various platforms.

2. The Key Exchange Channel

1.  Bob scans the QR code or opens the link.
2.  Bob's app generates its own ephemeral X25519 key pair (y_{temp}, Y_{temp}).
3.  Both apps connect to the server at a temporary endpoint coordinated by
    rendezvous_id.
4.  They perform an ephemeral Diffie-Hellman exchange (X_{temp} and Y_{temp})
    and derive a temporary transit key using the token as an authentication salt
    (e.g., via HKDF).
5.  Over this encrypted, authenticated temporary channel, Alice and Bob upload
    and exchange their actual static public key bundles:
    \text{Bundle} = \{ \text{User\_ID}, \text{ML-DSA-65\_pub}, \text{ML-KEM-768\_pub}, \text{X25519\_pub} \}
6.  Once the exchange is complete, the server discards the rendezvous room.

3. Verification

Both devices sort the exchanged bundles, hash them together, and display the
safety fingerprint:
\text{fingerprint} = \text{SHA256}(\text{sort}(\text{Bundle}_A, \text{Bundle}_B))[0:12]
Once the users verify this out-of-band, the static keys are stored locally and
marked as trusted.

# Phase 2: Sending a Location Update

When Alice wants to share her location with Bob, she uses an ephemeral-static
hybrid design. Because location sharing is asynchronous (Alice uploads, Bob
polls later), we cannot use a full double-ratchet without significant
complexity. Instead, we use an asymmetric key encapsulation mechanism combined
with a sender signature.

 Alice (Sender)                                             Bob (Recipient)
==============                                             =================
1. Gen Ephemeral x_eph, X_eph
2. ML-KEM.Encapsulate(Bob_KEM_pub) 
   --> ss_pq, ct_pq
3. DH(x_eph, Bob_X25519_pub) 
   --> ss_classic
4. KDF(ss_pq, ss_classic) --> wrap_key
5. Encrypt Payload with CK
6. Wrap CK with wrap_key
7. Sign (X_eph || ct_pq || wrapped_key) 
   with Alice_ML-DSA_priv
                                 [Upload to Server]
                               --------------------->
                                                       1. Verify ML-DSA signature
                                                       2. ML-KEM.Decapsulate(ct_pq) 
                                                          --> ss_pq
                                                       3. DH(Bob_X25519_priv, X_eph) 
                                                          --> ss_classic
                                                       4. KDF(ss_pq, ss_classic) 
                                                          --> wrap_key
                                                       5. Unwrap CK, Decrypt Payload

Step-by-Step Execution:

1.  Generate Ephemerals & Content Key:
      - Generate a random 256-bit content key (CK).
      - Generate an ephemeral X25519 key pair (x_{eph}, X_{eph}).
2.  Hybrid Key Encapsulation:
      - Compute the classical shared secret:
        ss_{classic} = \text{X25519}(x_{eph}, \text{Bob.x25519\_pub})
      - Compute the post-quantum shared secret and ciphertext:
        (ss_{pq}, ct_{pq}) = \text{ML-KEM-768.Encapsulate}(\text{Bob.kem\_pub})
      - Derive the wrapping key:
        wrap\_key = \text{HKDF}(ss_{classic} \mathbin{\Vert} ss_{pq} \mathbin{\Vert} X_{eph} \mathbin{\Vert} ct_{pq})
3.  Encryption:
      - Encrypt the location payload (coordinates, precision, timestamp) using
        AES-256-GCM with CK and a random nonce N_1.
      - Encrypt CK using AES-256-GCM with wrap\_key and a random nonce N_2 to
        produce wrapped\_key.
4.  Signature (Authenticity):
      - To prevent an adversary or a compromised server from tampering with the
        ephemeral key material or substituting ciphertexts, Alice signs the key
        handshake metadata and the wrapped key using her static ML-DSA private
        key:
        \text{sig} = \text{ML-DSA-65.Sign}(\text{Alice.mldsa\_priv}, X_{eph} \mathbin{\Vert} ct_{pq} \mathbin{\Vert} wrapped\_key \mathbin{\Vert} \text{timestamp})
5.  Upload:
      - Alice sends the following package to Bob's server inbox:
        \{ \text{sender\_id}, \text{ciphertext}, \text{wrapped\_key}, ct_{pq}, X_{eph}, \text{sig}, \text{nonces}, \text{timestamp} \}

# Phase 3: Receiving a Location Update

When Bob's app polls the server and retrieves Alice's latest payload, it
processes it as follows:

1.  Verify Signature:
      - Bob retrieves Alice's pinned public keys from local storage.
      - Bob verifies the signature:
        \text{Verify}(\text{Alice.mldsa\_pub}, \text{sig}, X_{eph} \mathbin{\Vert} ct_{pq} \mathbin{\Vert} wrapped\_key \mathbin{\Vert} \text{timestamp})
      - If verification fails, the payload is immediately discarded. This
        prevents unauthorized senders from injecting fake location coordinates
        or performing decryption-oracle attacks.
2.  Decapsulate:
      - Compute the classical shared secret:
        ss_{classic} = \text{X25519}(\text{Bob.x25519\_priv}, X_{eph})
      - Compute the post-quantum shared secret:
        ss_{pq} = \text{ML-KEM-768.Decapsulate}(ct_{pq}, \text{Bob.kem\_priv})
      - Reconstruct the wrapping key:
        wrap\_key = \text{HKDF}(ss_{classic} \mathbin{\Vert} ss_{pq} \mathbin{\Vert} X_{eph} \mathbin{\Vert} ct_{pq})
3.  Decrypt:
      - Unwrap the content key CK using wrap\_key.
      - Decrypt the location payload using CK and update the map UI.

Cryptographic Properties of this Design

  - Post-Quantum Confidentiality: Even if an attacker records the traffic today
    and attempts to decrypt it with a quantum computer in the future, they
    cannot break the ML-KEM-768 layer to recover ss_{pq}.
  - Post-Quantum Authenticity: By signing the ephemeral payload with ML-DSA-65,
    Alice ensures that even a quantum-capable attacker cannot spoof location
    updates or impersonate her.
  - Sender-Side Forward Secrecy: Because Alice uses an ephemeral X25519 key
    (x_{eph}), a future compromise of Alice's static private keys does not allow
    an attacker to decrypt historical location updates they may have
    intercepted.
  - Note on Recipient-Side Compromise: In an asynchronous store-and-forward
    system without prekeys (like Signal's OTR/X3DH), if Bob's static private
    keys (\text{Bob.kem\_priv} and \text{Bob.x25519\_priv}) are compromised, an
    attacker who has recorded historical traffic can decrypt past updates. For
    highly ephemeral data like location sharing, this is usually an acceptable
    trade-off compared to the complexity of managing prekey pools on the server.

