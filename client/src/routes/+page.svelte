<script lang="ts">
  import { onMount } from "svelte";
  import Map from "$lib/Map.svelte";
  import { getPairedContacts, pollLocationUpdates, sendLocationUpdate } from "$lib/api";
  import type { PairedContact, SentLocationUpdate } from "$lib/api";
  import { mergeReceivedUpdates, receivedUpdates, serverUrl } from "$lib/stores";

  // A lookup that never calls back would leave the button spinning forever.
  const GEO_TIMEOUT_MS = 15000;

  let contacts = $state<PairedContact[]>([]);
  let contactsLoading = $state(true);
  let contactsError = $state("");
  let selectedId = $state<string | null>(null);

  let latitude = $state("");
  let longitude = $state("");
  let accuracyM = $state<number | null>(null);
  let geoBusy = $state(false);
  let geoError = $state("");
  // Incremented on every lookup so stale callbacks are ignored.
  let geoRequest = 0;
  let geoTimer: ReturnType<typeof setTimeout> | undefined;

  let sendBusy = $state(false);
  let sendError = $state("");
  let sent = $state<SentLocationUpdate | null>(null);

  let receiveBusy = $state(false);
  let receiveError = $state("");
  let lastCheckedAt = $state<number | null>(null);

  const selected = $derived(contacts.find((contact) => contact.deviceId === selectedId) ?? null);
  const lat = $derived(parseCoordinate(latitude, 90));
  const lon = $derived(parseCoordinate(longitude, 180));
  const coordsReady = $derived(lat !== null && lon !== null);
  const canSend = $derived(selected !== null && coordsReady);

  const fixText = $derived(
    lat !== null && lon !== null
      ? `${formatCoordinate(lat)}, ${formatCoordinate(lon)}` +
          (accuracyM === null ? "" : ` ± ${Math.round(accuracyM)} m`)
      : "--"
  );
  const targetText = $derived(selected ? `→ ${selected.deviceName}` : "no contact selected");

  // The store is ordered oldest first, so the last entry is the newest fix.
  const newestFix = $derived(
    $receivedUpdates.length > 0 ? $receivedUpdates[$receivedUpdates.length - 1] : null
  );
  // One track at a time: joining points from different senders would draw lines
  // between unrelated people.
  const track = $derived(
    newestFix === null
      ? []
      : $receivedUpdates.filter((fix) => fix.senderId === newestFix.senderId)
  );
  const senderCount = $derived(new Set($receivedUpdates.map((fix) => fix.senderId)).size);
  const newestPosition = $derived(
    newestFix === null
      ? "--"
      : `${formatCoordinate(newestFix.latitude)}, ${formatCoordinate(newestFix.longitude)}`
  );
  const newestAccuracy = $derived(
    newestFix !== null && typeof newestFix.accuracyM === "number" && Number.isFinite(newestFix.accuracyM)
      ? `± ${Math.round(newestFix.accuracyM)} m`
      : "--"
  );
  const newestStamp = $derived(newestFix === null ? "--" : formatTimestamp(newestFix.timestamp));

  function parseCoordinate(raw: string, limit: number): number | null {
    const text = raw.trim();
    if (text === "") return null;
    const value = Number(text);
    if (!Number.isFinite(value) || Math.abs(value) > limit) return null;
    return value;
  }

  function formatCoordinate(value: number): string {
    const text = value.toFixed(5);
    return text.startsWith("-") ? text : `+${text}`;
  }

  function formatTimestamp(timestamp: number): string {
    if (!timestamp) return "unknown";
    // The command reports seconds; accept milliseconds too so the readout can
    // never land in 1970.
    const date = new Date(timestamp < 1e12 ? timestamp * 1000 : timestamp);
    const pad = (n: number) => String(n).padStart(2, "0");
    const day = `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`;
    const time = `${pad(date.getHours())}:${pad(date.getMinutes())}:${pad(date.getSeconds())}`;
    return `${day} ${time}`;
  }

  async function loadContacts() {
    contactsLoading = true;
    contactsError = "";
    try {
      contacts = await getPairedContacts();
      if (selectedId !== null && !contacts.some((c) => c.deviceId === selectedId)) {
        selectedId = null;
      }
      // With a single contact there is nothing to choose, so pick it outright.
      if (selectedId === null && contacts.length === 1) {
        selectedId = contacts[0].deviceId;
      }
    } catch (e) {
      contactsError = String(e);
    } finally {
      contactsLoading = false;
    }
  }

  function clearAccuracy() {
    accuracyM = null;
  }

  // Ends a lookup exactly once: clears the safety timer, reports a message
  // (empty on success) and invalidates any callback that arrives afterwards.
  function settleLookup(id: number, message: string) {
    if (id !== geoRequest) return;
    geoRequest++;
    clearTimeout(geoTimer);
    geoError = message;
    geoBusy = false;
  }

  function geolocationMessage(code: number): string {
    if (code === 1) return "Location permission was denied. Enter coordinates manually.";
    if (code === 2) return "The location is unavailable right now. Enter coordinates manually.";
    if (code === 3) return "The location lookup timed out. Enter coordinates manually.";
    return "Could not get a location. Enter coordinates manually.";
  }

  function requestPosition() {
    geoError = "";
    if (typeof navigator === "undefined" || !navigator.geolocation) {
      geoError = "This device does not report a location. Enter coordinates manually.";
      return;
    }

    const id = ++geoRequest;
    geoBusy = true;
    geoTimer = setTimeout(
      () => settleLookup(id, "The location lookup timed out. Enter coordinates manually."),
      GEO_TIMEOUT_MS
    );

    try {
      navigator.geolocation.getCurrentPosition(
        (position) => {
          if (id !== geoRequest) return;
          latitude = position.coords.latitude.toFixed(5);
          longitude = position.coords.longitude.toFixed(5);
          accuracyM = position.coords.accuracy > 0 ? position.coords.accuracy : null;
          settleLookup(id, "");
        },
        (error) => settleLookup(id, geolocationMessage(error.code)),
        { enableHighAccuracy: true, timeout: 12000, maximumAge: 0 }
      );
    } catch {
      settleLookup(id, "Could not get a location. Enter coordinates manually.");
    }
  }

  function handleCoordinateKey(event: KeyboardEvent) {
    if (event.key === "Enter" && canSend) handleSend();
  }

  async function checkForUpdates() {
    if (receiveBusy) return;
    receiveBusy = true;
    receiveError = "";
    try {
      const fetched = await pollLocationUpdates($serverUrl);
      // This screen is not the only writer of the store: an OS-level poller (the
      // Android background service) pushes into it as well, so merge into what is
      // already there instead of replacing it, and let the store apply the
      // "already plotted" rule in one place.
      receivedUpdates.update((current) => mergeReceivedUpdates(current, fetched));
      lastCheckedAt = Date.now();
    } catch (e) {
      receiveError = String(e);
    } finally {
      receiveBusy = false;
    }
  }

  async function handleSend() {
    if (sendBusy) return;
    if (selected === null) {
      sendError = "Select a contact first.";
      return;
    }
    if (lat === null || lon === null) {
      sendError = "Enter a valid latitude and longitude first.";
      return;
    }

    sendError = "";
    sent = null;
    sendBusy = true;
    try {
      sent = await sendLocationUpdate($serverUrl, selected.deviceId, lat, lon, accuracyM);
    } catch (e) {
      sendError = String(e);
    } finally {
      sendBusy = false;
    }
  }

  onMount(() => {
    loadContacts();
    return () => clearTimeout(geoTimer);
  });
</script>

<div class="live">
  <div class="page-head">
    <h1>Live</h1>
    <p>Send your position to a paired contact, and follow the positions they send back.</p>
  </div>

  <div class="panel config-bar">
    <div class="field">
      <label class="eyebrow" for="server-url">Rendezvous server</label>
      <input
        id="server-url"
        class="mono"
        type="text"
        autocomplete="off"
        spellcheck="false"
        placeholder="http://localhost:9191"
        bind:value={$serverUrl}
      />
    </div>
  </div>

  <div class="panel">
    <div class="panel-head">
      <span class="eyebrow">Received ({$receivedUpdates.length})</span>
      <button
        type="button"
        class="small-btn"
        onclick={checkForUpdates}
        disabled={receiveBusy}
      >
        {receiveBusy ? "Checking" : "Check for updates"}
      </button>
    </div>
    <hr class="divider" />

    <Map points={track} height={300} />

    <div class="received-foot">
      {#if receiveBusy}
        <p class="status-line">
          <span class="spinner" aria-hidden="true"></span>
          Checking the server...
        </p>
      {/if}

      {#if receiveError}
        <p class="error">{receiveError}</p>
      {/if}

      {#if newestFix}
        <dl class="readout">
          <dt class="eyebrow">Sender</dt>
          <dd>
            {newestFix.senderName}
            <span class="mono id-tag">{newestFix.senderId}</span>
          </dd>
          <dt class="eyebrow">Position</dt>
          <dd class="mono">{newestPosition}</dd>
          <dt class="eyebrow">Accuracy</dt>
          <dd class="mono">{newestAccuracy}</dd>
          <dt class="eyebrow">Timestamp</dt>
          <dd class="mono">{newestStamp}</dd>
          <dt class="eyebrow">Sequence</dt>
          <dd class="mono">{newestFix.sequence}</dd>
        </dl>
        {#if senderCount > 1}
          <p class="hint">
            The track follows {newestFix.senderName}. {senderCount} senders have sent fixes.
          </p>
        {/if}
        {#if lastCheckedAt !== null}
          <p class="hint">Last check {formatTimestamp(lastCheckedAt)}.</p>
        {/if}
      {:else if !receiveBusy && receiveError === ""}
        <p class="hint">
          {#if lastCheckedAt !== null}
            No updates waiting. Checked {formatTimestamp(lastCheckedAt)}.
          {:else}
            Nothing received yet. Check for updates to fetch what the server holds for this device.
          {/if}
        </p>
      {/if}
    </div>
  </div>

  <div class="panel">
    <div class="panel-head">
      <span class="eyebrow">Target ({contacts.length})</span>
      <button
        type="button"
        class="small-btn"
        onclick={loadContacts}
        disabled={contactsLoading}
      >
        {contactsLoading ? "Loading" : "Refresh"}
      </button>
    </div>
    <hr class="divider" />

    {#if contactsLoading && contacts.length === 0}
      <p class="muted">Loading contacts...</p>
    {:else if contactsError}
      <p class="error">{contactsError}</p>
    {:else if contacts.length === 0}
      <div class="empty-state">
        <p class="eyebrow">No paired contacts</p>
        <p>Pair a device before sending a location update.</p>
        <a class="go-pair" href="/pair">Pair a device</a>
      </div>
    {:else}
      <div class="contact-list">
        {#each contacts as contact (contact.deviceId)}
          <label class="contact-row" class:selected={contact.deviceId === selectedId}>
            <input
              type="radio"
              name="contact"
              value={contact.deviceId}
              aria-label="{contact.deviceName}, {contact.deviceId}"
              bind:group={selectedId}
            />
            <span class="mark" aria-hidden="true"></span>
            <span class="contact-body">
              <span class="contact-name">{contact.deviceName}</span>
              <span class="contact-id">
                <span class="k">ID</span>
                <span class="mono v">{contact.deviceId}</span>
              </span>
              <span class="eyebrow contact-fp-label">Fingerprint</span>
              <span class="mono contact-fp">{contact.fingerprint}</span>
            </span>
          </label>
        {/each}
      </div>
    {/if}
  </div>

  <div class="panel">
    <span class="eyebrow">Position</span>
    <hr class="divider" />

    <div class="coord-grid">
      <label class="field">
        <span class="eyebrow">Latitude</span>
        <input
          class="mono"
          type="text"
          inputmode="decimal"
          autocomplete="off"
          spellcheck="false"
          placeholder="48.85660"
          bind:value={latitude}
          oninput={clearAccuracy}
          onkeydown={handleCoordinateKey}
        />
        {#if latitude.trim() !== "" && lat === null}
          <span class="field-error">Number between -90 and 90.</span>
        {/if}
      </label>
      <label class="field">
        <span class="eyebrow">Longitude</span>
        <input
          class="mono"
          type="text"
          inputmode="decimal"
          autocomplete="off"
          spellcheck="false"
          placeholder="2.35220"
          bind:value={longitude}
          oninput={clearAccuracy}
          onkeydown={handleCoordinateKey}
        />
        {#if longitude.trim() !== "" && lon === null}
          <span class="field-error">Number between -180 and 180.</span>
        {/if}
      </label>
    </div>

    <div class="field-actions">
      <button type="button" onclick={requestPosition} disabled={geoBusy}>
        {geoBusy ? "Locating..." : "Use current position"}
      </button>
      {#if geoBusy}
        <span class="spinner" aria-hidden="true"></span>
      {/if}
      {#if accuracyM !== null}
        <span class="accuracy">
          <span class="eyebrow">Accuracy</span>
          <span class="mono">± {Math.round(accuracyM)} m</span>
        </span>
      {/if}
    </div>

    {#if geoError}
      <p class="notice">{geoError}</p>
    {/if}
  </div>

  <div class="panel">
    <span class="eyebrow">Transmit</span>
    <hr class="divider" />

    <div class="status">
      <span class="live-dot" class:standby={!canSend}></span>
      <span class="mono status-fix">{fixText}</span>
      <span class="status-target">{targetText}</span>
    </div>

    <button
      type="button"
      class="primary send-btn"
      onclick={handleSend}
      disabled={!canSend || sendBusy}
    >
      {sendBusy ? "Sending..." : "Send update"}
    </button>

    {#if sendError}
      <p class="error">{sendError}</p>
    {/if}

    {#if sent}
      <div class="sent-box">
        <p class="sent-head">
          <span class="live-dot"></span>
          <span>Update handed to the server</span>
        </p>
        <dl class="readout">
          <dt class="eyebrow">Contact</dt>
          <dd>
            {sent.contactDeviceName}
            <span class="mono id-tag">{sent.contactDeviceId}</span>
          </dd>
          <dt class="eyebrow">Sequence</dt>
          <dd class="mono">{sent.sequence}</dd>
          <dt class="eyebrow">Timestamp</dt>
          <dd class="mono">{formatTimestamp(sent.timestamp)}</dd>
        </dl>
      </div>
    {/if}
  </div>
</div>

<style>
  .live {
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
  }

  .live .page-head {
    margin-bottom: var(--space-1);
  }

  .panel-head {
    display: flex;
    flex-wrap: wrap;
    justify-content: space-between;
    align-items: center;
    gap: var(--space-2) var(--space-4);
  }

  .small-btn {
    padding: var(--space-1) var(--space-3);
    font-size: var(--text-xs);
  }

  .muted {
    font-size: var(--text-sm);
  }

  .hint {
    margin-top: var(--space-2);
    font-size: var(--text-xs);
    color: var(--fg-faint);
  }

  .config-bar {
    display: flex;
    flex-wrap: wrap;
    align-items: flex-end;
    gap: var(--space-2) var(--space-4);
  }

  .config-bar .field {
    flex: 1 1 16rem;
  }

  /* Received */

  .received-foot {
    margin-top: var(--space-4);
  }

  .status-line {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    font-size: var(--text-sm);
    color: var(--signal);
  }

  /* Contact list */

  .contact-list {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    margin-top: var(--space-3);
  }

  /* Sit the empty state on the same text edge as the eyebrow above it. */
  .empty-state {
    padding: var(--space-6) 0;
  }

  .contact-row {
    position: relative;
    display: flex;
    align-items: flex-start;
    gap: var(--space-3);
    padding: var(--space-3) var(--space-4);
    background: var(--ink);
    border: 1px solid var(--line);
    border-radius: var(--radius);
    cursor: pointer;
    transition: border-color 0.15s ease, background 0.15s ease;
  }

  .contact-row:hover {
    border-color: var(--fg-faint);
  }

  .contact-row.selected {
    background: var(--panel-raised);
    border-color: var(--signal);
  }

  .contact-row input {
    position: absolute;
    width: 1px;
    height: 1px;
    margin: 0;
    opacity: 0;
    pointer-events: none;
  }

  .contact-row:has(input:focus-visible) {
    outline: 2px solid var(--signal);
    outline-offset: 2px;
  }

  .mark {
    flex-shrink: 0;
    width: 0.6rem;
    height: 0.6rem;
    margin-top: 0.35rem;
    border: 1px solid var(--fg-faint);
    border-radius: 50%;
    transition: background 0.15s ease, border-color 0.15s ease, box-shadow 0.15s ease;
  }

  .selected .mark {
    background: var(--signal);
    border-color: var(--signal);
    box-shadow: 0 0 0 3px var(--signal-dim);
  }

  .contact-body {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }

  .contact-name {
    font-weight: 600;
  }

  .contact-id {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-2);
    margin-top: 0.15rem;
    font-size: var(--text-xs);
  }

  .contact-fp-label {
    margin-top: var(--space-2);
  }

  .contact-fp {
    font-size: var(--text-sm);
    color: var(--signal);
    overflow-wrap: anywhere;
  }

  .k {
    color: var(--fg-faint);
  }

  .v {
    color: var(--fg-muted);
  }

  /* Position */

  .field {
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
    min-width: 0;
  }

  .field input {
    width: 100%;
    min-width: 0;
  }

  .coord-grid {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: var(--space-4);
  }

  .field-error {
    font-size: var(--text-xs);
    color: var(--danger);
  }

  .field-actions {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: var(--space-2);
    margin-top: var(--space-4);
  }

  .accuracy {
    display: flex;
    align-items: baseline;
    gap: var(--space-2);
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

  .notice {
    margin-top: var(--space-3);
    font-size: var(--text-sm);
    color: var(--amber);
  }

  /* Transmit */

  .status {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: var(--space-3);
    margin: var(--space-4) 0;
  }

  .status-fix {
    font-size: var(--text-sm);
    color: var(--fg);
  }

  .status-target {
    font-size: var(--text-sm);
    color: var(--fg-muted);
  }

  .live-dot.standby {
    background: var(--fg-faint);
    box-shadow: none;
  }

  .send-btn {
    width: 100%;
    padding: var(--space-3) var(--space-4);
  }

  .error {
    margin-top: var(--space-3);
    font-size: var(--text-sm);
    color: var(--danger);
  }

  .sent-box {
    margin-top: var(--space-4);
    padding: var(--space-4);
    background: var(--panel-raised);
    border-left: 2px solid var(--signal);
    border-radius: var(--radius);
    animation: reveal 0.25s ease-out;
  }

  @keyframes reveal {
    from {
      opacity: 0;
      transform: translateY(4px);
    }
  }

  .sent-head {
    display: flex;
    align-items: center;
    gap: var(--space-3);
    margin-bottom: var(--space-3);
    font-size: var(--text-sm);
    color: var(--fg);
  }

  .readout {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr);
    gap: var(--space-1) var(--space-5);
    margin: 0;
  }

  .readout dt,
  .readout dd {
    margin: 0;
  }

  .readout dt {
    font-weight: 400;
  }

  .readout dd {
    font-size: var(--text-sm);
    color: var(--fg);
  }

  .id-tag {
    margin-left: var(--space-2);
    font-size: var(--text-xs);
    color: var(--fg-faint);
  }

  .go-pair {
    display: inline-block;
    margin-top: var(--space-2);
    padding: var(--space-2) var(--space-4);
    background: var(--signal-dim);
    border: 1px solid var(--signal);
    border-radius: var(--radius);
    color: var(--fg);
    font-size: var(--text-sm);
    font-weight: 500;
    text-decoration: none;
    transition: background 0.15s ease, color 0.15s ease;
  }

  .go-pair:hover {
    background: var(--signal);
    color: var(--ink);
  }
</style>
