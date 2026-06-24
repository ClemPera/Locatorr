<script lang="ts">
  import { onMount } from "svelte";
  import { listReceivedLocations, sendLocationUpdate, pollInboxForLocations, getSettings, setContactSharing, type ReceivedLocation } from "$lib/api";
  import { contacts, refreshContacts } from "$lib/stores";
  import { getCurrentPosition } from "@tauri-apps/plugin-geolocation";

  let locations = $state<ReceivedLocation[]>([]);
  let loading = $state(true);
  let myLat = $state<number | null>(null);
  let myLon = $state<number | null>(null);
  let myAccuracy = $state<number | null>(null);
  let sharing = $state(false);

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

  async function captureAndShare() {
    try {
      const pos = await getCurrentPosition();
      myLat = pos.coords.latitude;
      myLon = pos.coords.longitude;
      myAccuracy = pos.coords.accuracy;
      const settings = await getSettings();
      if (settings.server_url) {
        sharing = true;
        await sendLocationUpdate(settings.server_url, myLat, myLon, myAccuracy);
        await pollInboxForLocations(settings.server_url);
        await refresh();
      }
    } catch (err) {
      console.error("location error:", err);
    } finally {
      sharing = false;
    }
  }

  async function toggleShare(contactId: string, current: boolean) {
    await setContactSharing(contactId, !current);
    await refreshContacts();
  }
</script>

<header class="page-head">
  <p class="eyebrow">live</p>
  <h1>Who's sharing with you</h1>
  <div class="my-location">
    <button onclick={captureAndShare} disabled={sharing}>
      {sharing ? "Sharing…" : "Capture & share my location"}
    </button>
    {#if myLat !== null}
      <span class="data">
        {myLat.toFixed(5)}, {myLon?.toFixed(5)} ±{myAccuracy?.toFixed(0)}m
      </span>
    {/if}
  </div>
</header>

{#if loading}
  <p>Reading the log…</p>
{:else}
  <!-- Who can see you -->
  <section class="panel sharing-panel">
    <h2>Who can see you</h2>
    {#if $contacts.filter(c => c.sharing).length === 0}
      <p class="muted">No one has access to your location right now. Enable sharing for a contact to let them see where you are.</p>
    {:else}
      <ul class="share-list">
        {#each $contacts.filter(c => c.sharing) as contact}
          <li>
            <span class="live-dot"></span>
            <span>{contact.nickname}</span>
            <button class="small danger" onclick={() => toggleShare(contact.id, true)}>Stop sharing</button>
          </li>
        {/each}
      </ul>
    {/if}
    <details class="more-contacts">
      <summary>Manage all contacts</summary>
      <ul class="share-list">
        {#each $contacts.filter(c => !c.sharing) as contact}
          <li>
            <span class="live-dot off"></span>
            <span>{contact.nickname}</span>
            <button class="small" onclick={() => toggleShare(contact.id, false)}>Share</button>
          </li>
        {/each}
      </ul>
    </details>
  </section>

  {#if locations.length === 0}
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
{/if}

<style>
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

  .my-location {
    display: flex;
    align-items: center;
    gap: var(--space-4);
    margin-top: var(--space-3);
  }

  .sharing-panel {
    margin-bottom: var(--space-5);
  }

  .sharing-panel h2 {
    margin-bottom: var(--space-3);
  }

  .share-list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
  }

  .share-list li {
    display: flex;
    align-items: center;
    gap: var(--space-3);
  }

  .share-list .live-dot.off {
    background: var(--fg-faint);
    box-shadow: none;
  }

  .more-contacts {
    margin-top: var(--space-4);
  }

  .more-contacts summary {
    cursor: pointer;
    color: var(--fg-muted);
    font-size: var(--text-sm);
  }

  button.small {
    font-size: var(--text-xs);
    padding: var(--space-1) var(--space-3);
    margin-left: auto;
  }

  button.small.danger {
    border-color: var(--danger);
    color: var(--danger);
  }

  button.small.danger:hover {
    background: var(--danger);
    color: var(--fg);
  }

  .muted {
    color: var(--fg-muted);
    font-size: var(--text-sm);
  }
</style>
