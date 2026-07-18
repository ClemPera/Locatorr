import type { RendezvousInvitation } from "$lib/api";

export function bytesToHex(bytes: number[]): string {
  return bytes.map((b) => b.toString(16).padStart(2, "0")).join("");
}

export function hexToBytes(hex: string): number[] {
  const clean = hex.trim();
  if (clean.length % 2 !== 0 || !/^[0-9a-fA-F]*$/.test(clean)) {
    throw new Error("invalid hex string");
  }
  const bytes: number[] = [];
  for (let i = 0; i < clean.length; i += 2) {
    bytes.push(parseInt(clean.slice(i, i + 2), 16));
  }
  return bytes;
}

/**
 * Packs an invitation into a single pasteable code, since this POC has no
 * relay server yet: pairing happens by manually copying the code from the
 * inviting device to the joining one.
 */
export function encodeInvitation(invitation: RendezvousInvitation): string {
  return [invitation.rendezvousId, bytesToHex(invitation.xTempPub), bytesToHex(invitation.token)].join(".");
}

export function decodeInvitation(code: string): RendezvousInvitation {
  const parts = code.trim().split(".");
  if (parts.length !== 3) {
    throw new Error("malformed invitation code");
  }
  const [rendezvousId, xTempPubHex, tokenHex] = parts;
  const xTempPub = hexToBytes(xTempPubHex);
  const token = hexToBytes(tokenHex);
  if (xTempPub.length !== 32 || token.length !== 32) {
    throw new Error("malformed invitation code");
  }
  return { rendezvousId, xTempPub, token };
}

/** Packs a responder's public key into a pasteable code to send back to the inviter. */
export function encodeResponse(rendezvousId: string, myPub: number[]): string {
  return [rendezvousId, bytesToHex(myPub)].join(".");
}

export function decodeResponse(code: string): { rendezvousId: string; theirPub: number[] } {
  const parts = code.trim().split(".");
  if (parts.length !== 2) {
    throw new Error("malformed response code");
  }
  const [rendezvousId, pubHex] = parts;
  const theirPub = hexToBytes(pubHex);
  if (theirPub.length !== 32) {
    throw new Error("malformed response code");
  }
  return { rendezvousId, theirPub };
}
