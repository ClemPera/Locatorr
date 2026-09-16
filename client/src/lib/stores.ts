import { writable } from "svelte/store";
import type { PairedContact, ReceivedLocationUpdate } from "$lib/api";

export const serverUrl = writable<string>("http://localhost:9191");
export const deviceName = writable<string>("Device-" + Math.floor(1000 + Math.random() * 9000));
export const pairedContacts = writable<PairedContact[]>([]);

/**
 * The single sink for received fixes, ordered oldest first. It stays a plain store
 * on purpose: the UI is not the only writer, an OS-level background service is
 * meant to push into it too. Everything that adds to it should go through
 * mergeReceivedUpdates so one fix cannot be plotted twice.
 */
export const receivedUpdates = writable<ReceivedLocationUpdate[]>([]);

/**
 * Adds fixes that are not already in the list. The key is senderId + sequence, so
 * re-polling, or a fix an OS-level poller already delivered, cannot double-plot.
 */
export function mergeReceivedUpdates(
  current: ReceivedLocationUpdate[],
  incoming: ReceivedLocationUpdate[]
): ReceivedLocationUpdate[] {
  const seen = new Set(current.map((fix) => `${fix.senderId}:${fix.sequence}`));
  const merged = current.slice();
  for (const fix of incoming) {
    const key = `${fix.senderId}:${fix.sequence}`;
    if (seen.has(key)) continue;
    seen.add(key);
    merged.push(fix);
  }
  // Chronological, so the map can draw a track in the order the fixes were taken.
  merged.sort((a, b) => a.timestamp - b.timestamp || a.sequence - b.sequence);
  return merged;
}
