import { invoke } from "@tauri-apps/api/core";

export interface RendezvousInvitation {
  rendezvousId: string;
  xTempPub: number[];
  token: number[];
}

export interface AcceptedRendezvous {
  myPub: number[];
  fingerprint: string;
}

export interface CompletedRendezvous {
  fingerprint: string;
}

export interface IdentityBundle {
  deviceId: string;
  deviceName: string;
  x25519Pub: number[];
  mlKemPub: number[];
  mlDsaPub: number[];
}

export interface PairedContact {
  deviceId: string;
  deviceName: string;
  fingerprint: string;
  bundle: IdentityBundle;
  pairedAt: number;
}

export interface CompletedRendezvousWithContact {
  fingerprint: string;
  peerDeviceId: string;
  peerDeviceName: string;
}

export interface SentLocationUpdate {
  contactDeviceId: string;
  contactDeviceName: string;
  sequence: number;
  timestamp: number;
}

export interface ReceivedLocationUpdate {
  senderId: string;
  senderName: string;
  sequence: number;
  timestamp: number;
  latitude: number;
  longitude: number;
  accuracyM: number | null;
}

export interface PositionFix {
  latitude: number;
  longitude: number;
  accuracyM: number | null;
  timestamp: number;
}

/** Payload of the "tracking://status" event. */
export interface TrackingStatus {
  running: boolean;
  targetDeviceId: string | null;
  targetName: string | null;
  intervalMs: number;
  sentCount: number;
  receivedCount: number;
  lastError: string | null;
  lastFixAt: number | null;
}

export interface LocationPermissionStatus {
  /** True when Android reports the location permission as granted. */
  granted: boolean;
  /** Alias to raw state: "granted", "denied", "prompt" or "prompt-with-rationale". */
  states: Record<string, string>;
}

export function greet(name: string): Promise<string> {
  return invoke<string>("greet", { name });
}

export function startPairingSession(serverUrl: string): Promise<RendezvousInvitation> {
  return invoke<RendezvousInvitation>("start_pairing_session", { serverUrl });
}

export function pollAndCompletePairing(
  serverUrl: string,
  rendezvousId: string,
  deviceName: string
): Promise<CompletedRendezvousWithContact> {
  return invoke<CompletedRendezvousWithContact>("poll_and_complete_pairing", {
    serverUrl,
    rendezvousId,
    deviceName,
  });
}

export function acceptPairingSession(
  serverUrl: string,
  invitation: RendezvousInvitation,
  deviceName: string
): Promise<CompletedRendezvousWithContact> {
  return invoke<CompletedRendezvousWithContact>("accept_pairing_session", {
    serverUrl,
    invitation,
    deviceName,
  });
}

export function getPairedContacts(): Promise<PairedContact[]> {
  return invoke<PairedContact[]>("get_paired_contacts");
}

export function deletePairedContact(deviceId: string): Promise<void> {
  return invoke<void>("delete_paired_contact", { deviceId });
}

export function sendLocationUpdate(
  serverUrl: string,
  contactDeviceId: string,
  latitude: number,
  longitude: number,
  accuracyM?: number | null
): Promise<SentLocationUpdate> {
  return invoke<SentLocationUpdate>("send_location_update", {
    serverUrl,
    contactDeviceId,
    latitude,
    longitude,
    accuracyM: accuracyM ?? null,
  });
}

export function pollLocationUpdates(serverUrl: string): Promise<ReceivedLocationUpdate[]> {
  return invoke<ReceivedLocationUpdate[]>("poll_location_updates", { serverUrl });
}

/**
 * Everything this process has decrypted so far, oldest first, and not cleared when
 * tracking stops. The "tracking://received" event is fire-and-forget, so this is
 * how a fix that arrived while the screen was closed gets restored.
 */
export function getReceivedUpdates(): Promise<ReceivedLocationUpdate[]> {
  return invoke<ReceivedLocationUpdate[]>("get_received_updates");
}

export function startTracking(
  serverUrl: string,
  contactDeviceId: string,
  intervalMs: number
): Promise<void> {
  return invoke<void>("start_tracking", { serverUrl, contactDeviceId, intervalMs });
}

export function stopTracking(): Promise<void> {
  return invoke<void>("stop_tracking");
}

/**
 * One position from the device. On Android this is the only working source:
 * navigator.geolocation is not implemented in the webview.
 */
export function getCurrentPosition(): Promise<PositionFix> {
  return invoke<PositionFix>("get_current_position");
}

/**
 * Opens the real system permission dialog and resolves with the state after the
 * user has answered, so it can take seconds. It rejects when there is no live
 * Android activity to host the dialog, such as desktop dev.
 */
export function requestLocationPermissions(): Promise<LocationPermissionStatus> {
  return invoke<LocationPermissionStatus>("request_location_permissions");
}

export function genRdvInv(): Promise<RendezvousInvitation> {
  return invoke<RendezvousInvitation>("gen_rdv_inv");
}

export function acceptRdvInv(invitation: RendezvousInvitation): Promise<AcceptedRendezvous> {
  return invoke<AcceptedRendezvous>("accept_rdv_inv", { invitation });
}

export function completeRdvPairing(
  rendezvousId: string,
  theirPub: number[]
): Promise<CompletedRendezvous> {
  return invoke<CompletedRendezvous>("complete_rdv_pairing", { rendezvousId, theirPub });
}