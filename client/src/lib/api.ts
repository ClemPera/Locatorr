import { invoke } from "@tauri-apps/api/core";

export interface RendezvousInvitation {
  rendezvousId: string;
  xTempPub: Uint8Array
  token: Uint8Array
}
export function greet(name: string): Promise<string> {
  return invoke<string>("greet", { name })
}

export function genRdvInv(): Promise<RendezvousInvitation> {
  return invoke<RendezvousInvitation>("gen_rdv_inv")
}