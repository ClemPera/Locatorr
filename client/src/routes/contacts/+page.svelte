<script lang="ts">
  import { onMount } from "svelte";
  import { getPairedContacts, deletePairedContact } from "$lib/api";
  import type { PairedContact } from "$lib/api";

  let contacts = $state<PairedContact[]>([]);
  let loading = $state(true);
  let error = $state("");

  async function loadContacts() {
    loading = true;
    error = "";
    try {
      contacts = await getPairedContacts();
    } catch (e) {
      error = String(e);
    } finally {
      loading = false;
    }
  }

  async function handleDelete(deviceId: string) {
    if (!confirm("Are you sure you want to remove this paired device?")) return;
    try {
      await deletePairedContact(deviceId);
      await loadContacts();
    } catch (e) {
      error = String(e);
    }
  }

  function formatDate(timestampSec: number): string {
    if (!timestampSec) return "Unknown";
    return new Date(timestampSec * 1000).toLocaleString();
  }

  onMount(() => {
    loadContacts();
  });
</script>

<div class="page-head">
  <h1>Paired Contacts</h1>
  <p>Devices that have been authenticated and paired with this instance.</p>
</div>

<div class="panel">
  <div class="panel-header">
    <span class="eyebrow">Contacts ({contacts.length})</span>
    <button onclick={loadContacts} disabled={loading} class="small-btn">Refresh</button>
  </div>
  <hr class="divider" />

  {#if loading}
    <p>Loading contacts...</p>
  {:else if error}
    <p class="error">{error}</p>
  {:else if contacts.length === 0}
    <p class="empty-text">No paired contacts found. Go to <strong>03 Pair</strong> to pair a device.</p>
  {:else}
    <div class="contacts-list">
      {#each contacts as contact (contact.deviceId)}
        <div class="contact-card">
          <div class="contact-main">
            <div class="contact-title">
              <span class="device-name">{contact.deviceName}</span>
              <span class="device-id mono">ID: {contact.deviceId}</span>
            </div>
            <div class="contact-details">
              <span class="detail-label">Fingerprint:</span>
              <span class="mono fingerprint">{contact.fingerprint}</span>
            </div>
            <div class="contact-details">
              <span class="detail-label">Paired On:</span>
              <span class="date">{formatDate(contact.pairedAt)}</span>
            </div>
          </div>
          <button class="danger-btn" onclick={() => handleDelete(contact.deviceId)}>Remove</button>
        </div>
      {/each}
    </div>
  {/if}
</div>

<style>
  .panel-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
  }

  .small-btn {
    padding: var(--space-1) var(--space-3);
    font-size: var(--text-xs);
  }

  .empty-text {
    color: var(--text-muted);
    font-size: var(--text-sm);
    margin: var(--space-4) 0;
  }

  .contacts-list {
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
    margin-top: var(--space-3);
  }

  .contact-card {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: var(--space-3) var(--space-4);
    background: rgba(255, 255, 255, 0.02);
    border: 1px solid var(--border-subtle);
    border-radius: 4px;
  }

  .contact-main {
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
  }

  .contact-title {
    display: flex;
    align-items: center;
    gap: var(--space-3);
  }

  .device-name {
    font-weight: 600;
    font-size: var(--text-base);
    color: var(--text-heading);
  }

  .device-id {
    font-size: var(--text-xs);
    color: var(--text-muted);
  }

  .contact-details {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    font-size: var(--text-xs);
  }

  .detail-label {
    color: var(--text-muted);
  }

  .fingerprint {
    color: var(--signal);
  }

  .date {
    color: var(--text-body);
  }

  .danger-btn {
    background: transparent;
    border: 1px solid var(--danger);
    color: var(--danger);
    padding: var(--space-1) var(--space-3);
    font-size: var(--text-xs);
    cursor: pointer;
    border-radius: 4px;
  }

  .danger-btn:hover {
    background: rgba(255, 0, 0, 0.1);
  }

  .error {
    color: var(--danger);
  }
</style>