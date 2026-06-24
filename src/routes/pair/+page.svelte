<script lang="ts">
  import { onMount } from "svelte";
  import { getPairingPayload, getSettings, setMyUsername, searchAndRequest, sendPairingRequest } from "$lib/api";
  import PairingQr from "$lib/components/PairingQr.svelte";
  import { scan } from "@tauri-apps/plugin-barcode-scanner";

  let myPayload = $state("");
  let copied = $state(false);
  let myUsername = $state("");
  let settingUsername = $state(false);
  let registered = $state(false);

  let theirInput = $state("");
  let requestStatus = $state("");
  let sending = $state(false);
  let scanning = $state(false);

  onMount(async () => {
    const settings = await getSettings();
    if (settings.relay_user_id) {
      registered = true;
      myUsername = settings.username || "";
    }
    myPayload = await getPairingPayload();
  });

  async function copyPayload() {
    await navigator.clipboard.writeText(myPayload);
    copied = true;
    setTimeout(() => (copied = false), 1500);
  }

  async function doSetUsername() {
    const settings = await getSettings();
    if (!settings.server_url || !myUsername.trim()) return;
    settingUsername = true;
    try {
      await setMyUsername(settings.server_url, myUsername.trim());
      myPayload = await getPairingPayload();
    } catch (err) {
      requestStatus = typeof err === "string" ? err : "Failed to set username.";
    } finally {
      settingUsername = false;
    }
  }

  async function sendRequest(event: Event) {
    event.preventDefault();
    requestStatus = "";
    const input = theirInput.trim();
    if (!input) return;

    const settings = await getSettings();
    if (!settings.server_url) {
      requestStatus = "No relay server configured. Go to Settings first.";
      return;
    }

    sending = true;
    try {
      // If it looks like a link, extract uid
      if (input.includes("://") || input.includes("uid=")) {
        const m = input.match(/[?&]uid=([^&]+)/);
        if (m) {
          await sendPairingRequest(settings.server_url, m[1]);
          requestStatus = "Pairing request sent!";
          theirInput = "";
        } else {
          requestStatus = "Invalid link format.";
        }
      } else {
        // Assume it's a username — do a relay lookup
        await searchAndRequest(settings.server_url, input);
        requestStatus = `Pairing request sent to ${input}!`;
        theirInput = "";
      }
    } catch (err) {
      requestStatus = typeof err === "string" ? err : "Failed to send request.";
    } finally {
      sending = false;
    }
  }

  async function scanQR() {
    scanning = true;
    try {
      const result = await scan({ windowed: false });
      if (result.content) theirInput = result.content;
    } catch {
      // user cancelled
    } finally {
      scanning = false;
    }
  }
</script>

<header class="page-head">
  <p class="eyebrow">pair</p>
  <h1>Connect with someone</h1>
</header>

<div class="grid">
  <section class="panel">
    <h2>Your identity</h2>
    {#if !registered}
      <p>You haven't registered with a relay yet. Go to <a href="/settings">Settings</a>, enter a relay URL, and save. Then come back here to set your username.</p>
    {:else}
      <label class="field">
        <span class="eyebrow">your username</span>
        <div class="username-row">
          <input bind:value={myUsername} placeholder="e.g. alice" />
          <button onclick={doSetUsername} disabled={settingUsername || !myUsername.trim()}>
            {settingUsername ? "Saving…" : "Set"}
          </button>
        </div>
      </label>

      <p>Share your link or QR to let someone pair with you. They'll send a request — you must approve it before the connection is made.</p>
      <div class="qr-wrap">
        {#if myPayload}
          <PairingQr value={myPayload} />
        {/if}
      </div>
      <textarea class="data" readonly rows="3" value={myPayload}></textarea>
      <button onclick={copyPayload}>{copied ? "Copied" : "Copy code"}</button>
    {/if}
  </section>

  <section class="panel">
    <h2>Add someone</h2>
    <p>Enter their username, paste their link, or scan their QR. A pairing request will be sent — they must approve first.</p>
    <form onsubmit={sendRequest}>
      <label class="field">
        <span class="eyebrow">username, link, or QR</span>
        <textarea class="data" rows="2" bind:value={theirInput} placeholder="username or locatorr://pair?uid=..."></textarea>
        <button type="button" onclick={scanQR} disabled={scanning}>
          {scanning ? "Scanning…" : "Scan QR"}
        </button>
      </label>
      {#if requestStatus}
        <p class="status">{requestStatus}</p>
      {/if}
      <button class="primary" type="submit" disabled={sending || !theirInput.trim()}>
        {sending ? "Sending…" : "Send pairing request"}
      </button>
    </form>
  </section>
</div>

<style>
  .page-head { margin-bottom: var(--space-5); display: flex; flex-direction: column; gap: var(--space-1); }
  .grid { display: grid; grid-template-columns: 1fr 1fr; gap: var(--space-5); }
  @media (max-width: 900px) { .grid { grid-template-columns: 1fr; } }
  h2 { margin-bottom: var(--space-2); }
  .panel p { margin-bottom: var(--space-4); }
  .qr-wrap { display: flex; justify-content: center; margin-bottom: var(--space-4); }
  textarea { width: 100%; font-family: var(--font-data); font-size: var(--text-xs); color: var(--fg-muted); background: var(--ink); border: 1px solid var(--line); border-radius: var(--radius); padding: var(--space-3); resize: none; margin-bottom: var(--space-3); }
  .field { display: flex; flex-direction: column; gap: var(--space-1); margin-bottom: var(--space-3); }
  .username-row { display: flex; gap: var(--space-2); }
  .username-row input { flex: 1; }
  .status { color: var(--signal); margin-bottom: var(--space-3); max-width: 36rem; }
</style>
