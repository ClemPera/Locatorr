<script lang="ts">
  import { onMount } from "svelte";
  import { getPairingPayload, addContact, verifyContact, type Contact } from "$lib/api";
  import { refreshContacts } from "$lib/stores";
  import PairingQr from "$lib/components/PairingQr.svelte";
  import FingerprintMeter from "$lib/components/FingerprintMeter.svelte";
  import { scan } from "@tauri-apps/plugin-barcode-scanner";

  let myPayload = $state("");
  let copied = $state(false);

  let theirPayload = $state("");
  let nickname = $state("");
  let adding = $state(false);
  let addError = $state("");
  let newContact = $state<Contact | null>(null);
  let scanning = $state(false);

  onMount(async () => {
    myPayload = await getPairingPayload();
  });

  async function copyPayload() {
    await navigator.clipboard.writeText(myPayload);
    copied = true;
    setTimeout(() => (copied = false), 1500);
  }

  async function submitAddContact(event: Event) {
    event.preventDefault();
    addError = "";
    adding = true;
    try {
      newContact = await addContact(theirPayload.trim(), nickname.trim() || "Unnamed contact");
      await refreshContacts();
      theirPayload = "";
      nickname = "";
    } catch (err) {
      addError = typeof err === "string" ? err : "Couldn't add that contact. Check the payload and try again.";
    } finally {
      adding = false;
    }
  }

  async function markVerified() {
    if (!newContact) return;
    await verifyContact(newContact.id);
    newContact = { ...newContact, verified: true };
    await refreshContacts();
  }

  async function scanQR() {
    scanning = true;
    try {
      const result = await scan({ windowed: false });
      if (result.content) {
        theirPayload = result.content;
      }
    } catch {
      // user cancelled or no camera
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
    <p>Show this QR to someone in person, or copy the link below to send remotely. They'll scan it and your keys will be fetched securely from the relay.</p>
    <div class="qr-wrap">
      {#if myPayload}
        <PairingQr value={myPayload} />
      {/if}
    </div>
    <textarea class="data" readonly rows="3" value={myPayload}></textarea>
    <button onclick={copyPayload}>{copied ? "Copied" : "Copy code"}</button>
  </section>

  <section class="panel">
    <h2>Add a contact</h2>
    <p>Paste the code they shared with you.</p>
    <form onsubmit={submitAddContact}>
      <label class="field">
        <span class="eyebrow">nickname</span>
        <input bind:value={nickname} placeholder="What do you call them?" />
      </label>
      <label class="field">
        <span class="eyebrow">their code</span>
        <textarea class="data" rows="3" bind:value={theirPayload} placeholder="Paste pairing code here"
        ></textarea>
        <button type="button" onclick={scanQR} disabled={scanning}>
          {scanning ? "Scanning…" : "Scan QR code"}
        </button>
      </label>
      {#if addError}
        <p class="error">{addError}</p>
      {/if}
      <button class="primary" type="submit" disabled={adding || !theirPayload.trim()}>
        {adding ? "Adding…" : "Add contact"}
      </button>
    </form>
  </section>
</div>

{#if newContact}
  <section class="panel verify-panel">
    <p class="eyebrow">verify {newContact.nickname}</p>
    <h2>Read this aloud to each other</h2>
    <p>
      Compare this code over a call, in person, or any channel other than the one you used to
      pair. If it matches on both screens, the connection wasn't tampered with in transit.
    </p>
    <FingerprintMeter value={newContact.fingerprint} />
    {#if newContact.verified}
      <p class="confirmed">✓ Marked verified.</p>
    {:else}
      <button class="primary" onclick={markVerified}>It matches — mark verified</button>
    {/if}
  </section>
{/if}

<style>
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

  .field input {
    font-family: var(--font-ui);
  }

  .error {
    color: var(--danger);
    margin-bottom: var(--space-3);
  }

  .verify-panel {
    margin-top: var(--space-5);
  }

  .verify-panel p {
    max-width: 36rem;
  }

  .confirmed {
    color: var(--signal);
    margin-top: var(--space-3);
  }

  .verify-panel button {
    margin-top: var(--space-3);
  }
</style>
