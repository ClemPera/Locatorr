<script lang="ts">
  import { onMount } from "svelte";
  import { listen } from "@tauri-apps/api/event";
  import Map from "$lib/Map.svelte";
  import {
    getCurrentPosition,
    getPairedContacts,
    getReceivedUpdates,
    pollLocationUpdates,
    requestLocationPermissions,
    sendLocationUpdate,
    startTracking,
    stopTracking,
  } from "$lib/api";
  import type {
    PairedContact,
    ReceivedLocationUpdate,
    SentLocationUpdate,
    TrackingStatus,
  } from "$lib/api";
  import {
    mergeReceivedUpdates,
    receivedUpdates,
    serverUrl,
    trackingStatus,
  } from "$lib/stores";

  // A lookup that never calls back would leave the button spinning forever.
  const GEO_TIMEOUT_MS = 15000;
  // Offered intervals for background sharing, in milliseconds.
  const TRACKING_INTERVALS = [15000, 30000, 60000];
  const PERMISSION_SETTINGS_HINT = "If the prompt did not appear, grant it in the app's settings.";

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
  // Set only when the best-effort restore on mount could not reach the service, and
  // cleared again once an explicit check has been made.
  let restoreFailed = $state(false);

  // Background sharing keeps its own choice of contact. It is deliberately never
  // filled in for you, not even when only one contact exists: sharing this device's
  // position with the wrong person is the failure worth designing against.
  let trackingTargetId = $state<string | null>(null);
  let trackingIntervalMs = $state(30000);
  let trackingBusy = $state(false);
  let trackingCallError = $state("");
  // Set when a permission outcome stopped background sharing from starting.
  let permissionNotice = $state("");
  let permissionDetail = $state("");
  // Notification state from the last permission request, when there was one.
  let notificationsGranted = $state<boolean | null>(null);

  // True once a start has returned, or once an event has reported a running state.
  let trackingStarted = $state(false);
  // Event listeners held per component instance, all removed on destroy.
  const unsubscribes: Array<() => void> = [];

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

  const trackingTarget = $derived(
    contacts.find((contact) => contact.deviceId === trackingTargetId) ?? null
  );
  const trackingRunning = $derived($trackingStatus?.running === true);
  // The event is the source of truth once it arrives, but a start that returned
  // counts too, so the control flips without waiting for the first report.
  const sharingActive = $derived(trackingRunning || trackingStarted);
  const canToggleSharing = $derived(
    !trackingBusy && (sharingActive || trackingTarget !== null)
  );
  const sharingLabel = $derived(sharingButtonLabel(trackingBusy, sharingActive));

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

  function formatMaybeTimestamp(timestamp: number | null | undefined): string {
    return typeof timestamp === "number" && timestamp > 0 ? formatTimestamp(timestamp) : "none yet";
  }

  function formatInterval(intervalMs: number | null | undefined): string {
    return typeof intervalMs === "number" && Number.isFinite(intervalMs) && intervalMs > 0
      ? `${Math.round(intervalMs / 1000)} s`
      : "--";
  }

  function sharingButtonLabel(busy: boolean, active: boolean): string {
    if (busy) return active ? "Stopping..." : "Starting...";
    return active ? "Stop sharing" : "Start sharing";
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
      // The sharing target is only dropped when it disappears. It is never filled
      // in, however few contacts there are.
      if (trackingTargetId !== null && !contacts.some((c) => c.deviceId === trackingTargetId)) {
        trackingTargetId = null;
      }
    } catch (e) {
      contactsError = String(e);
    } finally {
      contactsLoading = false;
    }
  }

  // Typing a coordinate abandons any lookup that is still out, so a fix that lands
  // late cannot overwrite what was typed by hand.
  function handleManualInput() {
    geoRequest++;
    clearTimeout(geoTimer);
    geoBusy = false;
    geoError = "";
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

  // Asks only when the user does something that needs it, never on its own. Android
  // answers immediately without a dialog when the permission is already granted.
  async function ensureLocationPermission(): Promise<{ granted: boolean; error: string }> {
    try {
      const status = await requestLocationPermissions();
      const notifications: string | undefined = status.states?.["notifications"];
      notificationsGranted = notifications === undefined ? null : notifications === "granted";
      return { granted: status.granted === true, error: "" };
    } catch (e) {
      return { granted: false, error: String(e) };
    }
  }

  function applyFix(fixLat: number, fixLon: number, fixAccuracy: number | null) {
    latitude = fixLat.toFixed(5);
    longitude = fixLon.toFixed(5);
    accuracyM = typeof fixAccuracy === "number" && fixAccuracy > 0 ? fixAccuracy : null;
  }

  async function requestPosition() {
    geoError = "";
    const id = ++geoRequest;
    geoBusy = true;

    // Ask before reading: the runtime dialog has to be opened once, or the first
    // read on Android fails on permission. It is user paced, so the lookup deadline
    // only starts once it has been answered.
    const permission = await ensureLocationPermission();
    if (id !== geoRequest) return;
    if (!permission.granted) {
      geoBusy = false;
      geoError =
        permission.error === ""
          ? `Location permission is needed for this device to read a position. ${PERMISSION_SETTINGS_HINT} You can still enter coordinates manually.`
          : `Location permission could not be requested: ${permission.error} You can still enter coordinates manually.`;
      return;
    }

    geoTimer = setTimeout(() => {
      // Releases the button without invalidating the request: a fix that arrives
      // after this is still applied, it just no longer blocks the screen.
      if (id !== geoRequest) return;
      geoBusy = false;
      geoError = "The position lookup is taking too long. Enter coordinates manually.";
    }, GEO_TIMEOUT_MS);

    // The device command is the real path: the Android webview has no
    // navigator.geolocation. The browser API is only a fallback for desktop dev,
    // so the button is never dead.
    let commandError = "";
    try {
      const fix = await getCurrentPosition();
      if (id !== geoRequest) return;
      applyFix(fix.latitude, fix.longitude, fix.accuracyM);
      settleLookup(id, "");
      return;
    } catch (e) {
      commandError = String(e);
    }
    if (id !== geoRequest) return;

    if (typeof navigator === "undefined" || !navigator.geolocation) {
      settleLookup(id, commandError);
      return;
    }

    try {
      navigator.geolocation.getCurrentPosition(
        (position) => {
          if (id !== geoRequest) return;
          applyFix(
            position.coords.latitude,
            position.coords.longitude,
            position.coords.accuracy > 0 ? position.coords.accuracy : null
          );
          settleLookup(id, "");
        },
        (error) => {
          const fallback = geolocationMessage(error.code);
          settleLookup(id, `${commandError} The browser fallback failed too: ${fallback}`);
        },
        { enableHighAccuracy: true, timeout: 12000, maximumAge: 0 }
      );
    } catch {
      settleLookup(id, `${commandError} The browser fallback failed as well.`);
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
      // The restore note has served its purpose once the server has been asked.
      restoreFailed = false;
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

  async function toggleBackgroundSharing() {
    if (trackingBusy) return;
    if (sharingActive) {
      await stopBackgroundSharing();
      return;
    }
    await startBackgroundSharing();
  }

  async function startBackgroundSharing() {
    if (trackingTarget === null || trackingBusy) return;
    // Stays true across both awaits, so the dialog cannot be answered with a second
    // tap and start_tracking can never be called twice.
    trackingBusy = true;
    trackingCallError = "";
    permissionNotice = "";
    permissionDetail = "";
    try {
      const permission = await ensureLocationPermission();
      if (!permission.granted) {
        // Nothing was started, and the message says so rather than implying the
        // service is running without permission.
        permissionNotice =
          "Location permission is needed to share this device's position, so sharing did not start.";
        permissionDetail =
          permission.error === ""
            ? PERMISSION_SETTINGS_HINT
            : `The permission request failed: ${permission.error}`;
        return;
      }
      await startTracking($serverUrl, trackingTarget.deviceId, trackingIntervalMs);
      // The command returned, so the service is up. It keeps sending this device's
      // position to this contact until it is stopped.
      trackingStarted = true;
    } catch (e) {
      trackingCallError = String(e);
    } finally {
      trackingBusy = false;
    }
  }

  async function stopBackgroundSharing() {
    trackingBusy = true;
    trackingCallError = "";
    try {
      await stopTracking();
      trackingStarted = false;
      // Do not sit on a stale "running" if the status event does not follow.
      trackingStatus.update((current) =>
        current === null ? null : { ...current, running: false }
      );
    } catch (e) {
      trackingCallError = String(e);
    } finally {
      trackingBusy = false;
    }
  }

  onMount(() => {
    loadContacts();

    // Listening lives here, in the page, so it dies with the page: every listener is
    // removed on destroy, and one that lands after destroy is removed on arrival.
    // That is what keeps a hot reload or a navigation from stacking duplicates on
    // the same event.
    let disposed = false;
    const removeSubscriptions = () => {
      for (const off of unsubscribes.splice(0)) off();
    };

    void (async () => {
      try {
        unsubscribes.push(
          await listen<TrackingStatus>("tracking://status", (event) => {
            trackingStatus.set(event.payload);
            trackingStarted = event.payload.running;
          }),
          await listen<ReceivedLocationUpdate[]>("tracking://received", (event) => {
            // A fix the background service picked up lands in the same store the map
            // reads, through the same de-duplication rule as an explicit check.
            receivedUpdates.update((current) => mergeReceivedUpdates(current, event.payload));
          })
        );
      } catch {
        // Events only exist inside the Tauri shell. Outside it the panel still works,
        // it just has no status to report.
      }
      if (disposed) removeSubscriptions();

      // The catch-up runs only after both listeners are attached, so a fix that
      // arrives during the call is either inside the result or delivered to the
      // listener, and the merge de-duplicates the overlap. It never reads the store
      // before awaiting: the update callback sees whatever is in the store at the
      // moment it runs, so a concurrent event can only ever be added to, never
      // overwritten. Best effort, so a failure is a quiet note rather than an error.
      try {
        const stored = await getReceivedUpdates();
        receivedUpdates.update((current) => mergeReceivedUpdates(current, stored));
      } catch {
        restoreFailed = true;
      }
    })();

    return () => {
      disposed = true;
      removeSubscriptions();
      clearTimeout(geoTimer);
    };
  });
</script>

{#snippet contactChoice(contact: PairedContact)}
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
{/snippet}

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

      {#if restoreFailed}
        <p class="hint">
          Earlier updates could not be restored. Check for updates to fetch anything the server
          still holds.
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
            {@render contactChoice(contact)}
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
          oninput={handleManualInput}
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
          oninput={handleManualInput}
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

  <div class="panel">
    <span class="eyebrow">Background sharing</span>
    <hr class="divider" />

    <p class="muted">
      Keeps sending this device's position to the contact chosen here, on a fixed interval, while
      the app is in the background. Pick the contact explicitly: this list never fills itself in.
    </p>

    {#if contactsLoading && contacts.length === 0}
      <p class="muted">Loading contacts...</p>
    {:else if contacts.length === 0}
      <p class="hint">
        {#if contactsError}
          Contacts could not be listed, so there is nothing to choose from. The panel above has
          the error.
        {:else}
          No paired contacts yet. Pair a device before starting background sharing.
        {/if}
      </p>
    {:else}
      <div class="contact-list">
        {#each contacts as contact (contact.deviceId)}
          <!-- Its own radio group: choosing a contact here must not disturb the send
               target above, and neither choice is ever made for the user. -->
          <label class="contact-row" class:selected={contact.deviceId === trackingTargetId}>
            <input
              type="radio"
              name="tracking-target"
              value={contact.deviceId}
              aria-label="{contact.deviceName}, {contact.deviceId}"
              bind:group={trackingTargetId}
            />
            {@render contactChoice(contact)}
          </label>
        {/each}
      </div>
    {/if}

    <label class="field interval">
      <span class="eyebrow">Interval</span>
      <select class="mono" bind:value={trackingIntervalMs}>
        {#each TRACKING_INTERVALS as intervalMs (intervalMs)}
          <option value={intervalMs}>{formatInterval(intervalMs)}</option>
        {/each}
      </select>
    </label>
    <p class="hint">
      15 seconds is the floor, so this device is not woken for a fix more often than that.
    </p>

    <div class="tracking-actions">
      <button
        type="button"
        class="tracking-btn"
        class:primary={!sharingActive}
        class:danger={sharingActive}
        onclick={toggleBackgroundSharing}
        disabled={!canToggleSharing}
      >
        {sharingLabel}
      </button>
      {#if !sharingActive && trackingTarget === null && contacts.length > 0}
        <span class="hint">Choose a contact before starting.</span>
      {/if}
    </div>

    {#if permissionNotice}
      <p class="notice">{permissionNotice}</p>
      {#if permissionDetail}
        <p class="hint">{permissionDetail}</p>
      {/if}
    {/if}

    {#if trackingCallError}
      <p class="error">{trackingCallError}</p>
    {/if}

    <div class="tracking-status">
      {#if $trackingStatus}
        <p class="status-head">
          <span class="live-dot" class:standby={!$trackingStatus.running}></span>
          <span>{$trackingStatus.running ? "Running" : "Stopped"}</span>
        </p>
        <dl class="readout">
          <dt class="eyebrow">Target</dt>
          <dd>
            {$trackingStatus.targetName || "none"}
            {#if $trackingStatus.targetDeviceId}
              <span class="mono id-tag">{$trackingStatus.targetDeviceId}</span>
            {/if}
          </dd>
          <dt class="eyebrow">Interval in use</dt>
          <dd class="mono">{formatInterval($trackingStatus.intervalMs)}</dd>
          <dt class="eyebrow">Sent</dt>
          <dd class="mono">{$trackingStatus.sentCount ?? 0}</dd>
          <dt class="eyebrow">Received</dt>
          <dd class="mono">{$trackingStatus.receivedCount ?? 0}</dd>
          <dt class="eyebrow">Last fix</dt>
          <dd class="mono">{formatMaybeTimestamp($trackingStatus.lastFixAt)}</dd>
        </dl>
        {#if $trackingStatus.lastError}
          <p class="error">{$trackingStatus.lastError}</p>
        {/if}
        {#if $trackingStatus.running}
          <p class="hint">
            Android keeps a notification in the drawer while this service runs.
            {#if notificationsGranted === true}
              Notification permission was granted when sharing started, so it should be there.
            {:else if notificationsGranted === false}
              Notification permission was not granted when sharing started, so it may be absent.
              Sharing keeps running either way.
            {:else}
              On Android 13 and later the system can deny notification permission: sharing keeps
              running and the notification is simply absent. This screen cannot tell which case
              applies.
            {/if}
          </p>
        {/if}
      {:else if trackingStarted}
        <p class="hint">
          Sharing was started, but no status report has arrived from the service yet.
        </p>
      {:else}
        <p class="hint">Not started. Background sharing only sends after you start it here.</p>
      {/if}
    </div>
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

  /* Background sharing */

  .interval {
    max-width: 14rem;
    margin-top: var(--space-4);
  }

  .tracking-actions {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-3);
    margin-top: var(--space-4);
  }

  .tracking-actions .hint {
    margin-top: 0;
  }

  .tracking-btn {
    min-width: 12rem;
    padding: var(--space-3) var(--space-4);
  }

  .tracking-status {
    margin-top: var(--space-4);
  }

  .status-head {
    display: flex;
    align-items: center;
    gap: var(--space-3);
    margin-bottom: var(--space-3);
    font-size: var(--text-sm);
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
