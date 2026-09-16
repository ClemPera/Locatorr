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