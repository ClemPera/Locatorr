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

export function greet(name: string): Promise<string> {
  return invoke<string>("greet", { name })
}

export function genRdvInv(): Promise<RendezvousInvitation> {
  return invoke<RendezvousInvitation>("gen_rdv_inv")
}

export function acceptRdvInv(invitation: RendezvousInvitation): Promise<AcceptedRendezvous> {
  return invoke<AcceptedRendezvous>("accept_rdv_inv", { invitation })
}

export function completeRdvPairing(rendezvousId: string, theirPub: number[]): Promise<CompletedRendezvous> {
  return invoke<CompletedRendezvous>("complete_rdv_pairing", { rendezvousId, theirPub })
}