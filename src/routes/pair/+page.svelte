<script lang="ts">
  import { onMount } from "svelte";
  import { getPairingPayload, sendPairingRequest, getSettings } from "$lib/api";
  import PairingQr from "$lib/components/PairingQr.svelte";
  import { scan } from "@tauri-apps/plugin-barcode-scanner";

  let myPayload = $state("");
  let copied = $state(false);

  let theirPayload = $state("");
  let requestStatus = $state("");
  let sending = $state(false);
  let scanning = $state(false);

  onMount(async () => {
    myPayload = await getPairingPayload();
  });

  async function copyPayload() {
    await navigator.clipboard.writeText(myPayload);
    copied = true;
    setTimeout(() => (copied = false), 1500);
  }

  function extractUserId(payload: string): string | null {
    // locatorr://pair?uid=<user_id>
    const m = payload.match(/[?&]uid=([^&]+)/);
    return m ? m[1] : null;
  }

  async function sendRequest(event: Event) {
    event.preventDefault();
    requestStatus = "";
    const uid = extractUserId(theirPayload.trim());
    if (!uid) {
      requestStatus = "Invalid pairing code — could not find a user ID.";
      return;
    }
    const settings = await getSettings();
    if (!settings.server_url) {
      requestStatus = "No relay server configured. Go to Settings first.";
      return;
    }
    sending = true;
    try {
      await sendPairingRequest(settings.server_url, uid);
      requestStatus = "Pairing request sent! They'll need to approve it before the connection is established.";
      theirPayload = "";
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
      if (result.content) {
        theirPayload = result.content;
      }
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
    <h2>Your pairing code</h2>
    {#if myPayload.includes("local=1")}
      <p>You haven't registered with a relay yet. Go to <a href="/settings">Settings</a> and save a relay URL — your QR will link to your account.</p>
    {:else}
      <p>Show this QR to someone in person, or copy the link below to send remotely.</p>
    {/if}
    <div class="qr-wrap">
      {#if myPayload}
        <PairingQr value={myPayload} />
      {/if}
    </div>
    <textarea class="data" readonly rows="3" value={myPayload}></textarea>
    <button onclick={copyPayload}>{copied ? "Copied" : "Copy code"}</button>
  </section>

  <section class="panel">
    <h2>Add someone</h2>
    <p>Scan their QR or paste their link. This sends a pairing request — they must approve before the connection is made.</p>
    <form onsubmit={sendRequest}>
      <label class="field">
        <span class="eyebrow">their code</span>
        <textarea class="data" rows="3" bind:value={theirPayload} placeholder="Paste pairing link here"></textarea>
        <button type="button" onclick={scanQR} disabled={scanning}>
          {scanning ? "Scanning…" : "Scan QR code"}
        </button>
      </label>
      {#if requestStatus}
        <p class="status">{requestStatus}</p>
      {/if}
      <button class="primary" type="submit" disabled={sending || !theirPayload.trim()}>
        {sending ? "Sending…" : "Send pairing request"}
      </button>
    </form>
  </section>
</div>

<style>
  .page-head {
    margin-bottom: var(--space-5);
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
  }

  .grid {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: var(--space-5);
  }

  @media (max-width: 900px) {
    .grid {
      grid-template-columns: 1fr;
    }
  }

  h2 {
    margin-bottom: var(--space-2);
  }

  .panel p {
    margin-bottom: var(--space-4);
  }

  .qr-wrap {
    display: flex;
    justify-content: center;
    margin-bottom: var(--space-4);
  }

  textarea {
    width: 100%;
    font-family: var(--font-data);
    font-size: var(--text-xs);
    color: var(--fg-muted);
    background: var(--ink);
    border: 1px solid var(--line);
    border-radius: var(--radius);
    padding: var(--space-3);
    resize: none;
    margin-bottom: var(--space-3);
  }

  .field {
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
    margin-bottom: var(--space-3);
  }

  .status {
    color: var(--signal);
    margin-bottom: var(--space-3);
    max-width: 36rem;
  }
</style>
