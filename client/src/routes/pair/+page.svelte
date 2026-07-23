<script lang="ts">
  import {
    startPairingSession,
    pollAndCompletePairing,
    acceptPairingSession,
  } from "$lib/api";
  import type { RendezvousInvitation, CompletedRendezvousWithContact } from "$lib/api";
  import { encodeInvitation, decodeInvitation } from "$lib/crypt";
  import { serverUrl, deviceName } from "$lib/stores";

  // Host (Inviting) Side
  let invitation = $state<RendezvousInvitation | undefined>();
  let hostStatus = $state("");
  let hostResult = $state<CompletedRendezvousWithContact | undefined>();
  let hostError = $state("");
  let hostBusy = $state(false);

  // Guest (Joining) Side
  let joinInvitationInput = $state("");
  let joinStatus = $state("");
  let joinResult = $state<CompletedRendezvousWithContact | undefined>();
  let joinError = $state("");
  let joinBusy = $state(false);

  async function handleStartInvite() {
    hostError = "";
    hostResult = undefined;
    hostStatus = "Creating room on server...";
    hostBusy = true;
    try {
      invitation = await startPairingSession($serverUrl);
      hostStatus = "Waiting for guest device to join session...";
      
      // Automatically poll server in background until guest connects & completes pairing
      const result = await pollAndCompletePairing(
        $serverUrl,
        invitation  .rendezvousId,
        $deviceName
      );
      hostResult = result;
      hostStatus = "Pairing complete!";
    } catch (e) {
      hostError = String(e);
      hostStatus = "";
    } finally {
      hostBusy = false;
    }
  }

  async function handleAcceptInvite() {
    joinError = "";
    joinResult = undefined;
    joinStatus = "Connecting to rendezvous room...";
    joinBusy = true;
    try {
      const peerInvitation = decodeInvitation(joinInvitationInput);
      const result = await acceptPairingSession(
        $serverUrl,
        peerInvitation,
        $deviceName
      );
      joinResult = result;
      joinStatus = "Pairing complete!";
    } catch (e) {
      joinError = String(e);
      joinStatus = "";
    } finally {
      joinBusy = false;
    }
  }

  function copyText(text: string) {
    navigator.clipboard.writeText(text);
  }
</script>

<div class="page-head">
  <h1>Pair a device</h1>
  <p>
    Connect two devices via the rendezvous server. Generate an invitation code on one device and enter it on the other to securely exchange public keys.
  </p>
</div>

<!-- Server & Device Config Bar -->
<div class="config-bar panel">
  <div class="field">
    <label for="server-url">Rendezvous Server URL</label>
    <input id="server-url" class="mono" bind:value={$serverUrl} placeholder="http://localhost:9191" />
  </div>
  <div class="field">
    <label for="device-name">My Device Name</label>
    <input id="device-name" bind:value={$deviceName} placeholder="Device Name" />
  </div>
</div>

<br />

<!-- Inviter Panel -->
<div class="panel">
  <span class="eyebrow">Option A: Invite a device</span>
  <hr class="divider" />

  {#if !invitation}
    <button class="primary" onclick={handleStartInvite} disabled={hostBusy}>
      {hostBusy ? "Creating session..." : "Generate invitation"}
    </button>
  {:else}
    <p>Share this invitation code with the joining device:</p>
    <div class="code-row">
      <input class="mono" readonly value={encodeInvitation(invitation)} />
      <button onclick={() => copyText(encodeInvitation(invitation!))}>Copy</button>
    </div>

    {#if hostBusy}
      <div class="status-indicator">
        <span class="spinner"></span>
        <span>{hostStatus}</span>
      </div>
    {/if}
  {/if}

  {#if hostResult}
    <div class="success-box">
      <p class="success-title">Device Paired Successfully!</p>
      <p>Paired with: <strong>{hostResult.peerDeviceName}</strong> ({hostResult.peerDeviceId})</p>
      <p class="eyebrow">Safety Fingerprint</p>
      <p class="mono fingerprint">{hostResult.fingerprint}</p>
      <p class="hint">Verify this fingerprint matches the fingerprint shown on the other device screen.</p>
    </div>
  {/if}

  {#if hostError}
    <p class="error">{hostError}</p>
  {/if}
</div>

<br />

<!-- Joiner Panel -->
<div class="panel">
  <span class="eyebrow">Option B: Join with an invitation code</span>
  <hr class="divider" />

  <p>Paste the invitation code generated on the inviting device:</p>
  <div class="code-row">
    <input class="mono" bind:value={joinInvitationInput} placeholder="rendezvousId.xTempPub.token" />
    <button class="primary" onclick={handleAcceptInvite} disabled={joinBusy || !joinInvitationInput.trim()}>
      {joinBusy ? "Joining..." : "Accept invitation"}
    </button>
  </div>

  {#if joinBusy}
    <div class="status-indicator">
      <span class="spinner"></span>
      <span>{joinStatus}</span>
    </div>
  {/if}

  {#if joinResult}
    <div class="success-box">
      <p class="success-title">Device Paired Successfully!</p>
      <p>Paired with: <strong>{joinResult.peerDeviceName}</strong> ({joinResult.peerDeviceId})</p>
      <p class="eyebrow">Safety Fingerprint</p>
      <p class="mono fingerprint">{joinResult.fingerprint}</p>
      <p class="hint">Verify this fingerprint matches the fingerprint shown on the other device screen.</p>
    </div>
  {/if}

  {#if joinError}
    <p class="error">{joinError}</p>
  {/if}
</div>

<style>
  .config-bar {
    display: flex;
    gap: var(--space-4);
    margin-bottom: var(--space-4);
  }

  .config-bar .field {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
  }

  .config-bar label {
    font-size: var(--text-xs);
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--text-muted);
  }

  .code-row {
    display: flex;
    gap: var(--space-2);
    margin: var(--space-2) 0 var(--space-4);
  }

  .code-row input {
    flex: 1;
    min-width: 0;
  }

  .status-indicator {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    margin: var(--space-3) 0;
    color: var(--signal);
    font-size: var(--text-sm);
  }

  .spinner {
    width: 14px;
    height: 14px;
    border: 2px solid var(--signal);
    border-top-color: transparent;
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
  }

  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }

  .success-box {
    margin-top: var(--space-4);
    padding: var(--space-4);
    background: rgba(0, 255, 128, 0.05);
    border: 1px solid var(--signal);
    border-radius: 4px;
  }

  .success-title {
    font-weight: 600;
    color: var(--signal);
    margin-bottom: var(--space-2);
  }

  .fingerprint {
    font-size: var(--text-lg);
    color: var(--signal);
    letter-spacing: 0.04em;
    margin: var(--space-1) 0;
  }

  .hint {
    font-size: var(--text-xs);
    color: var(--text-muted);
  }

  .error {
    color: var(--danger);
    margin-top: var(--space-2);
  }
</style>
