import { writable } from "svelte/store";
import { listContacts, type Contact } from "$lib/api";

export const contacts = writable<Contact[]>([]);

export async function refreshContacts(): Promise<void> {
  contacts.set(await listContacts());
}
