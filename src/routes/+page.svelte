<script lang="ts">
  import { onMount } from "svelte";
  import { listReceivedLocations, type ReceivedLocation } from "$lib/api";
  import { contacts, refreshContacts } from "$lib/stores";

  let locations = $state<ReceivedLocation[]>([]);
  let loading = $state(true);

  async function refresh() {
    loading = true;
    try {
      locations = await listReceivedLocations();
    } finally {
      loading = false;
    }
  }

  function nicknameFor(contactId: string): string {
    return $contacts.find((c) => c.id === contactId)?.nickname ?? contactId;
  }

  function ageLabel(updatedAt: number): string {
    const seconds = Math.floor(Date.now() / 1000) - updatedAt;
    if (seconds < 60) return `${seconds}s ago`;
    if (seconds < 3600) return `${Math.floor(seconds / 60)}m ago`;
    return `${Math.floor(seconds / 3600)}h ago`;
  }

  onMount(() => {
    refresh();
    refreshContacts();
  });
</script>

<header class="page-head">
  <p class="eyebrow">live</p>
  <h1>Who's sharing with you</h1>
</header>

{#if loading}
  <p>Reading the log…</p>
{:else if locations.length === 0}
  <div class="panel empty-state">
    <span class="live-dot" style="background: var(--fg-faint); box-shadow: none"></span>
    <h2>Nothing logged yet</h2>
    <p>
      Once a contact shares their location with you, their last known position shows up here.
      Pair with someone first, then ask them to turn sharing on for you.
    </p>
    <a href="/pair"><button class="primary">Pair with someone</button></a>
  </div>
{:else}
  <div class="panel">
    <table>
      <thead>
        <tr>
          <th>Contact</th>
          <th>Latitude</th>
          <th>Longitude</th>
          <th>Accuracy</th>
          <th>Last fix</th>
        </tr>
      </thead>
      <tbody>
        {#each locations as loc}
          <tr>
            <td>{nicknameFor(loc.contact_id)}</td>
            <td class="data">{loc.lat.toFixed(5)}</td>
            <td class="data">{loc.lon.toFixed(5)}</td>
            <td class="data">±{loc.accuracy.toFixed(0)}m</td>
            <td class="data">{ageLabel(loc.updated_at)}</td>
          </tr>
        {/each}
      </tbody>
    </table>
  </div>
{/if}

<style>
  .page-head {
    margin-bottom: var(--space-5);
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
  }

  table {
    width: 100%;
    border-collapse: collapse;
  }

  th {
    text-align: left;
    font-family: var(--font-data);
    font-size: var(--text-xs);
    letter-spacing: 0.06em;
    text-transform: uppercase;
    color: var(--fg-faint);
    font-weight: 500;
    padding: var(--space-2) var(--space-3);
    border-bottom: 1px solid var(--line);
  }

  td {
    padding: var(--space-3);
    border-bottom: 1px solid var(--line);
    font-size: var(--text-sm);
  }

  tr:last-child td {
    border-bottom: none;
  }

  .empty-state h2 {
    margin-top: var(--space-2);
  }

  .empty-state p {
    max-width: 32rem;
  }

  .empty-state button {
    margin-top: var(--space-2);
  }
</style>
