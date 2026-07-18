<script lang="ts">
import { genRdvInv, acceptRdvInv, completeRdvPairing } from "$lib/api";
import type { RendezvousInvitation } from "$lib/api";
import { encodeInvitation, decodeInvitation, encodeResponse, decodeResponse } from "$lib/crypt";

// Inviting side: generate an invitation, wait for the peer's response code.
let invitation = $state() as RendezvousInvitation | undefined;
let hostResponseInput = $state("");
let hostFingerprint = $state("");
let hostError = $state("");
let hostBusy = $state(false);

// Joining side: paste an invitation code, get back a response code to send.
let joinInvitationInput = $state("");
let joinResponseCode = $state("");
let joinFingerprint = $state("");
let joinError = $state("");
let joinBusy = $state(false);

async function startInvite() {
  hostError = "";
  hostBusy = true;
  try {
    invitation = await genRdvInv();
    hostResponseInput = "";
    hostFingerprint = "";
  } catch (e) {
    hostError = String(e);
  } finally {
    hostBusy = false;
  }
}

async function completeInvite() {
  if (!invitation) return;
  hostError = "";
  hostBusy = true;
  try {
    const { theirPub } = decodeResponse(hostResponseInput);
    const result = await completeRdvPairing(invitation.rendezvousId, theirPub);
    hostFingerprint = result.fingerprint;
  } catch (e) {
    hostError = String(e);
  } finally {
    hostBusy = false;
  }
}

async function acceptInvite() {
  joinError = "";
  joinBusy = true;
  try {
    const peerInvitation = decodeInvitation(joinInvitationInput);
    const result = await acceptRdvInv(peerInvitation);
    joinResponseCode = encodeResponse(peerInvitation.rendezvousId, result.myPub);
    joinFingerprint = result.fingerprint;
  } catch (e) {
    joinError = String(e);
  } finally {
    joinBusy = false;
  }
}

function copy(text: string) {
  navigator.clipboard.writeText(text);
}
</script>

<div class="page-head">
  <h1>Pair a device</h1>
  <p>
    No relay yet in this build, so codes travel out of band: copy one from this device, paste it
    into the other. Compare the fingerprint on both screens before you trust the pairing.
  </p>
</div>

<div class="panel">
  <span class="eyebrow">invite a device</span>
  <hr class="divider" />

  {#if !invitation}
    <button class="primary" onclick={startInvite} disabled={hostBusy}>Generate invitation</button>
  {:else}
    <p>Send this code to the other device:</p>
    <div class="code-row">
      <input class="mono" readonly value={encodeInvitation(invitation)} />
      <button onclick={() => copy(encodeInvitation(invitation!))}>Copy</button>
    </div>

    <p>Then paste the response code it gives back:</p>
    <div class="code-row">
      <input class="mono" bind:value={hostResponseInput} placeholder="response code" />
      <button class="primary" onclick={completeInvite} disabled={hostBusy || !hostResponseInput}>
        Complete pairing
      </button>
    </div>
  {/if}

  {#if hostFingerprint}
    <p class="eyebrow">safety fingerprint</p>
    <p class="mono fingerprint">{hostFingerprint}</p>
    <p>Read this aloud with the other device's owner. It must match exactly.</p>
  {/if}

  {#if hostError}
    <p class="error">{hostError}</p>
  {/if}
</div>

<br />

<div class="panel">
  <span class="eyebrow">join with a code</span>
  <hr class="divider" />

  <p>Paste the invitation code from the inviting device:</p>
  <div class="code-row">
    <input class="mono" bind:value={joinInvitationInput} placeholder="invitation code" />
    <button class="primary" onclick={acceptInvite} disabled={joinBusy || !joinInvitationInput}>
      Accept invitation
    </button>
  </div>

  {#if joinResponseCode}
    <p>Send this response code back to the inviting device:</p>
    <div class="code-row">
      <input class="mono" readonly value={joinResponseCode} />
      <button onclick={() => copy(joinResponseCode)}>Copy</button>
    </div>
  {/if}

  {#if joinFingerprint}
    <p class="eyebrow">safety fingerprint</p>
    <p class="mono fingerprint">{joinFingerprint}</p>
    <p>Read this aloud with the other device's owner. It must match exactly.</p>
  {/if}

  {#if joinError}
    <p class="error">{joinError}</p>
  {/if}
</div>

<style>
  .code-row {
    display: flex;
    gap: var(--space-2);
    margin: var(--space-2) 0 var(--space-4);
  }

  .code-row input {
    flex: 1;
    min-width: 0;
  }

  .fingerprint {
    font-size: var(--text-lg);
    color: var(--signal);
    letter-spacing: 0.04em;
  }

  .error {
    color: var(--danger);
  }
</style>
