import { describe, expect, it } from "vitest";
import type { RendezvousInvitation } from "../api";
import {
  bytesToHex,
  decodeInvitation,
  decodeResponse,
  encodeInvitation,
  encodeResponse,
  hexToBytes,
} from "../crypt";

/** 32 bytes, the size both the invitation and response keys must have. */
function bytes32(start = 0): number[] {
  return Array.from({ length: 32 }, (_, i) => (start + i) % 256);
}

describe("bytesToHex / hexToBytes", () => {
  it("round-trips every byte value", () => {
    const bytes = Array.from({ length: 256 }, (_, i) => i);
    expect(hexToBytes(bytesToHex(bytes))).toEqual(bytes);
  });

  it("formats each byte as two lowercase hex digits", () => {
    expect(bytesToHex([0, 15, 16, 255])).toBe("000f10ff");
  });

  it("rejects odd-length input", () => {
    expect(() => hexToBytes("abc")).toThrow(/invalid hex/);
  });

  it("rejects non-hex characters", () => {
    expect(() => hexToBytes("zz")).toThrow(/invalid hex/);
    expect(() => hexToBytes("0g")).toThrow(/invalid hex/);
  });
});

describe("encodeInvitation / decodeInvitation", () => {
  const invitation: RendezvousInvitation = {
    rendezvousId: "rdv-123",
    xTempPub: bytes32(1),
    token: bytes32(200),
  };

  it("round-trips a well-formed invitation", () => {
    expect(decodeInvitation(encodeInvitation(invitation))).toEqual(invitation);
  });

  it("ignores surrounding whitespace when decoding", () => {
    expect(decodeInvitation(`  ${encodeInvitation(invitation)}  `)).toEqual(invitation);
  });

  it("rejects a code with the wrong number of parts", () => {
    expect(() => decodeInvitation("only-one-part")).toThrow(/malformed invitation/);
    expect(() => decodeInvitation("rdv.deadbeef")).toThrow(/malformed invitation/);
  });

  it("rejects a code whose key or token is not 32 bytes", () => {
    const shortKey = ["rdv", bytesToHex([1, 2]), bytesToHex(bytes32())].join(".");
    const shortToken = ["rdv", bytesToHex(bytes32()), bytesToHex([1, 2])].join(".");
    expect(() => decodeInvitation(shortKey)).toThrow(/malformed invitation/);
    expect(() => decodeInvitation(shortToken)).toThrow(/malformed invitation/);
  });
});

describe("encodeResponse / decodeResponse", () => {
  const theirPub = bytes32(7);

  it("round-trips a well-formed response", () => {
    expect(decodeResponse(encodeResponse("rdv-123", theirPub))).toEqual({
      rendezvousId: "rdv-123",
      theirPub,
    });
  });

  it("rejects a code with the wrong number of parts", () => {
    expect(() => decodeResponse("only-one-part")).toThrow(/malformed response/);
    expect(() => decodeResponse("rdv.a.b")).toThrow(/malformed response/);
  });

  it("rejects a code whose key is not 32 bytes", () => {
    expect(() => decodeResponse(`rdv.${bytesToHex([1, 2])}`)).toThrow(/malformed response/);
  });
});
