<script lang="ts">
  import { onMount } from "svelte";
  import { get } from "svelte/store";
  import { contacts, refreshContacts } from "$lib/stores";
  import {
    verifyContact,
    setContactSharing,
    removeContact,
    checkContactFingerprint,
  } from "$lib/api";
  import FingerprintMeter from "$lib/components/FingerprintMeter.svelte";

  let expandedId = $state<string | null>(null);
  let busyId = $state<string | null>(null);
  let keyChanged = $state<Record<string, boolean>>({});

  onMount(async () => {
    await refreshContacts();
    const current = get(contacts);
    for (const contact of current) {
      if (contact.verified) {
        try {
          const match = await checkContactFingerprint(contact.id);
          if (!match) {
            keyChanged[contact.id] = true;
          }
        } catch {
          // If the check fails (e.g. missing contact), skip silently
        }
      }
    }
  });

  function toggleExpanded(id: string) {
    expandedId = expandedId === id ? null : id;
  }

  async function toggleSharing(id: string, current: boolean) {
    busyId = id;
    try {
      await setContactSharing(id, !current);
      await refreshContacts();
    } finally {
      busyId = null;
    }
  }

  async function confirmVerify(id: string) {
    busyId = id;
    try {
      await verifyContact(id);
      await refreshContacts();
    } finally {
      busyId = null;
    }
  }

  async function deleteContact(id: string) {
    busyId = id;
    try {
      await removeContact(id);
      await refreshContacts();
    } finally {
      busyId = null;
      expandedId = null;
    }
  }
</script>

<header class="page-head">
  <p class="eyebrow">contacts</p>
  <h1>People you've paired with</h1>
</header>

{#if $contacts.length === 0}
  <div class="panel empty-state">
    <h2>No contacts yet</h2>
    <p>Pair with someone first — they'll show up here once you've exchanged codes.</p>
    <a href="/pair"><button class="primary">Pair with someone</button></a>
  </div>
{:else}
  <ul class="list">
    {#each $contacts as contact}
      <li class="panel row">
        <button class="row-head" aria-expanded={expandedId === contact.id} onclick={() => toggleExpanded(contact.id)}>
          <div class="who">
            {#if contact.sharing}
              <span class="live-dot"></span>
            {:else}
              <span class="live-dot off"></span>
            {/if}
            <span class="nickname">{contact.nickname}</span>
            {#if !contact.verified}
              <span class="badge">unverified</span>
            {/if}
            {#if contact.verified && keyChanged[contact.id]}
              <span class="badge">key changed</span>
            {/if}
          </div>
          <div class="toggle" onclick={(e) => e.stopPropagation()} onkeydown={(e) => e.key === 'Enter' && e.stopPropagation()} role="presentation">
            <label>
              <input
                type="checkbox"
                checked={contact.sharing}
                disabled={busyId === contact.id}
                onchange={() => toggleSharing(contact.id, contact.sharing)}
              />
              <span>Share</span>
            </label>
          </div>
          <span class="chevron">{expandedId === contact.id ? "−" : "+"}</span>
        </button>

        {#if expandedId === contact.id}
          <div class="row-body">
            <div class="field-row">
              <span class="eyebrow">fingerprint</span>
              <FingerprintMeter value={contact.fingerprint} />
            </div>

            {#if contact.verified && keyChanged[contact.id]}
              <p class="hint">
                Key material has changed&mdash;{contact.nickname} may have reinstalled their app.
                Verify the new fingerprint before trusting this connection again.
              </p>
            {/if}

            {#if !contact.verified}
              <p class="hint">
                Not verified yet — compare the fingerprint above with {contact.nickname} over a
                separate channel before trusting this connection.
              </p>
              <button class="primary" disabled={busyId === contact.id} onclick={() => confirmVerify(contact.id)}>
                Mark verified
              </button>
            {/if}

            <hr class="divider" />

            <div class="actions">
              <button class="danger" disabled={busyId === contact.id} onclick={() => deleteContact(contact.id)}>
                Remove contact
              </button>
            </div>
          </div>
        {/if}
      </li>
    {/each}
  </ul>
{/if}

<style>
  .list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
  }

  .row {
    padding: 0;
    overflow: hidden;
  }

  .row-head {
    width: 100%;
    background: none;
    border: none;
    border-radius: 0;
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: var(--space-4) var(--space-5);
  }

  .row-head:hover {
    background: var(--panel-raised);
    border: none;
  }

  .who {
    display: flex;
    align-items: center;
    gap: var(--space-3);
  }

  .live-dot.off {
    background: var(--fg-faint);
    box-shadow: none;
  }

  .nickname {
    font-weight: 500;
  }

  .badge {
    font-family: var(--font-data);
    font-size: var(--text-xs);
    color: var(--amber);
    border: 1px solid var(--amber);
    border-radius: 999px;
    padding: 0.15rem var(--space-2);
  }

  .chevron {
    color: var(--fg-faint);
    font-family: var(--font-data);
  }

  .row-body {
    padding: 0 var(--space-5) var(--space-5);
  }

  .field-row {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    margin-bottom: var(--space-3);
  }

  .hint {
    color: var(--amber);
    margin-bottom: var(--space-3);
    max-width: 36rem;
  }

  .actions {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-4);
  }

  .toggle {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    font-size: var(--text-sm);
    color: var(--fg-muted);
  }


</style>
