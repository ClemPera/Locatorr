<script lang="ts">
  import { onMount } from "svelte";
  import { getSettings, updateSettings } from "$lib/api";

  let serverUrl = $state("");
  let pollIntervalSecs = $state(30);
  let saved = $state(false);
  let loading = $state(true);

  onMount(async () => {
    const settings = await getSettings();
    serverUrl = settings.server_url;
    pollIntervalSecs = settings.poll_interval_secs;
    loading = false;
  });

  async function save(event: Event) {
    event.preventDefault();
    await updateSettings({ server_url: serverUrl, poll_interval_secs: pollIntervalSecs });
    saved = true;
    setTimeout(() => (saved = false), 1500);
  }
</script>

<header class="page-head">
  <p class="eyebrow">settings</p>
  <h1>Configuration</h1>
</header>

{#if loading}
  <p>Loading settings…</p>
{:else}
  <form class="panel" onsubmit={save}>
    <label class="field">
      <span class="eyebrow">relay server url</span>
      <input bind:value={serverUrl} placeholder="https://relay.example.com" />
      <span class="help">
        Not wired up to any network calls yet in this build — sharing and polling land in the
        next pass. Saved here so the setting exists when they do.
      </span>
    </label>

    <hr class="divider" />

    <label class="field">
      <span class="eyebrow">poll interval (seconds)</span>
      <input type="number" min="5" bind:value={pollIntervalSecs} />
      <span class="help">
        How often the app checks the relay for new locations once that's connected. Lower is
        fresher, higher is easier on battery.
      </span>
    </label>

    <button class="primary" type="submit">{saved ? "Saved" : "Save"}</button>
  </form>
{/if}

<style>
  .field {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    margin-bottom: var(--space-4);
  }

  .field input {
    font-family: var(--font-data);
    max-width: 24rem;
  }

  .help {
    color: var(--fg-faint);
    font-size: var(--text-sm);
    max-width: 32rem;
  }
</style>
