import { invoke } from "@tauri-apps/api/core";

export interface Contact {
  id: string;
  nickname: string;
  relay_user_id: string;
  fingerprint: string;
  verified: boolean;
  sharing: boolean;
  created_at: number;
}

export interface Settings {
  server_url: string;
  poll_interval_secs: number;
  relay_user_id?: string;
}

export interface ReceivedLocation {
  contact_id: string;
  lat: number;
  lon: number;
  accuracy: number;
  updated_at: number;
}

export async function getPairingPayload(): Promise<string> {
  const res = await invoke<{ pairing_payload: string }>("get_pairing_payload");
  return res.pairing_payload;
}

export interface Preview {
  fingerprint: string;
  relay_user_id: string;
}

export async function previewPairing(payload: string): Promise<Preview> {
  return invoke<Preview>("preview_pairing", { payload });
}

export function addContact(payload: string, nickname: string): Promise<Contact> {
  return invoke<Contact>("add_contact", { payload, nickname });
}

export function listContacts(): Promise<Contact[]> {
  return invoke<Contact[]>("list_contacts");
}

export function checkContactFingerprint(contactId: string): Promise<boolean> {
  return invoke<boolean>("check_contact_fingerprint", { contactId });
}

export function verifyContact(contactId: string): Promise<void> {
  return invoke<void>("verify_contact", { contactId });
}

export function setContactSharing(contactId: string, sharing: boolean): Promise<void> {
  return invoke<void>("set_contact_sharing", { contactId, sharing });
}

export function removeContact(contactId: string): Promise<void> {
  return invoke<void>("remove_contact", { contactId });
}

export function getSettings(): Promise<Settings> {
  return invoke<Settings>("get_settings");
}

export function updateSettings(settings: Settings): Promise<void> {
  return invoke<void>("update_settings", { settings });
}

export function listReceivedLocations(): Promise<ReceivedLocation[]> {
  return invoke<ReceivedLocation[]>("list_received_locations");
}

// ── relay integration ────────────────────────────────────────────────────

export function registerWithRelay(serverUrl: string): Promise<string> {
  return invoke<string>("register_with_relay", { serverUrl });
}

export function authenticateWithRelay(serverUrl: string): Promise<string> {
  return invoke<string>("authenticate_with_relay", { serverUrl });
}

export function sendLocationUpdate(
  serverUrl: string,
  lat: number,
  lon: number,
  accuracy: number,
): Promise<void> {
  return invoke<void>("send_location_update", { serverUrl, lat, lon, accuracy });
}

export function pollInboxForLocations(serverUrl: string): Promise<ReceivedLocation[]> {
  return invoke<ReceivedLocation[]>("poll_inbox_for_locations", { serverUrl });
}
