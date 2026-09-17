import { describe, expect, it } from "vitest";
import type { ReceivedLocationUpdate } from "../api";
import { mergeReceivedUpdates } from "../stores";

function fix(senderId: string, sequence: number, timestamp: number): ReceivedLocationUpdate {
  return {
    senderId,
    senderName: senderId,
    sequence,
    timestamp,
    latitude: 0,
    longitude: 0,
    accuracyM: null,
  };
}

describe("mergeReceivedUpdates", () => {
  it("merges fixes that are not already present", () => {
    const current = [fix("a", 1, 100)];
    const incoming = [fix("a", 2, 200), fix("b", 1, 150)];

    expect(mergeReceivedUpdates(current, incoming)).toEqual([
      fix("a", 1, 100),
      fix("b", 1, 150),
      fix("a", 2, 200),
    ]);
  });

  it("drops fixes already present with the same senderId and sequence", () => {
    const existing = fix("a", 1, 100);
    const current = [existing];

    const result = mergeReceivedUpdates(current, [fix("a", 1, 999)]);

    expect(result).toEqual([existing]);
    expect(result[0].timestamp).toBe(100);
  });

  it("keeps fixes from different senders with the same sequence", () => {
    const result = mergeReceivedUpdates([fix("a", 1, 100)], [fix("b", 1, 200)]);

    expect(result).toEqual([fix("a", 1, 100), fix("b", 1, 200)]);
  });

  it("drops duplicates within the incoming batch itself", () => {
    const incoming = [fix("a", 1, 100), fix("a", 1, 100), fix("a", 2, 200)];

    expect(mergeReceivedUpdates([], incoming)).toEqual([fix("a", 1, 100), fix("a", 2, 200)]);
  });

  it("sorts chronologically, using sequence as the tiebreaker", () => {
    const incoming = [
      fix("a", 1, 300),
      fix("b", 2, 100),
      fix("c", 3, 200),
      fix("d", 1, 200),
    ];

    expect(mergeReceivedUpdates([], incoming).map((f) => `${f.senderId}${f.sequence}`)).toEqual([
      "b2",
      "d1",
      "c3",
      "a1",
    ]);
  });

  it("returns the existing list unchanged for empty input", () => {
    const current = [fix("a", 1, 100), fix("b", 2, 200)];

    expect(mergeReceivedUpdates(current, [])).toEqual(current);
  });

  it("does not mutate the input arrays", () => {
    const current = [fix("a", 2, 200), fix("a", 1, 100)];
    const incoming = [fix("b", 1, 50), fix("b", 2, 25)];

    mergeReceivedUpdates(current, incoming);

    expect(current.map((f) => `${f.senderId}${f.sequence}`)).toEqual(["a2", "a1"]);
    expect(incoming.map((f) => `${f.senderId}${f.sequence}`)).toEqual(["b1", "b2"]);
  });
});
