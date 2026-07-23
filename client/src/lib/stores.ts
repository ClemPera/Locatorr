import { writable } from "svelte/store";
import type { PairedContact } from "$lib/api";

export const serverUrl = writable<string>("http://localhost:9191");
export const deviceName = writable<string>("Device-" + Math.floor(1000 + Math.random() * 9000));
export const pairedContacts = writable<PairedContact[]>([]);
