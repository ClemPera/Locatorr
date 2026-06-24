<script lang="ts">
  import { onMount } from "svelte";
  import { getSettings, updateSettings, registerWithRelay, authenticateWithRelay } from "$lib/api";

  let serverUrl = $state("");
  let pollIntervalSecs = $state(30);
  let saved = $state(false);
  let loading = $state(true);
  let relayStatus = $state("");

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

    // Register with relay and authenticate
    if (serverUrl.trim()) {
      relayStatus = "Registering with relay…";
      try {
        await registerWithRelay(serverUrl);
        relayStatus = "Authenticating…";
        await authenticateWithRelay(serverUrl);
        relayStatus = "Connected ✓";
      } catch (err) {
        relayStatus = `Relay error: ${err}`;
      }
    }
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
        The relay server that stores and forwards encrypted location shares. The app registers
        and authenticates with this server when you save settings.
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
    {#if relayStatus}
      <p class="relay-status">{relayStatus}</p>
    {/if}
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

  .relay-status {
    color: var(--fg-muted);
    font-size: var(--text-sm);
    margin-top: var(--space-3);
  }
</style>
