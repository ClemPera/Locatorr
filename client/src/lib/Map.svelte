<script lang="ts">
  import { onMount } from "svelte";

  /**
   * Plain points, not wire messages: anything with a latitude and longitude can be
   * plotted, plus a timestamp and an accuracy when the caller has them.
   */
  interface MapPoint {
    latitude: number;
    longitude: number;
    /** Unix seconds or milliseconds. A point without one is still plotted. */
    timestamp?: number;
    /** Horizontal uncertainty in metres. Drawn only when it is visible at this scale. */
    accuracyM?: number | null;
  }

  interface Props {
    /** Oldest first. The last entry is the newest fix and carries the marker. */
    points: MapPoint[];
    /** Height of the frame in CSS pixels. */
    height?: number;
  }

  interface Colors {
    ink: string;
    line: string;
    signal: string;
    signalDim: string;
    faint: string;
    muted: string;
    font: string;
  }

  let { points, height = 300 }: Props = $props();

  // Mercator runs to infinity at the poles, so it is clamped at this latitude.
  const MAX_LAT = 85.05112878;
  const METRES_PER_UNIT = 40075016.686;
  // Room for the edge labels, the marker ring and the scale bar.
  const PAD = 26;
  // A single fix, or a track that never moved, has zero span in one or both axes
  // and would scale to infinity. Widening the degenerate axis to this much
  // longitude (about 4 km at the equator) keeps the fit finite and centred.
  const MIN_SPAN_DEG = 0.04;
  const TAU = Math.PI * 2;
  const IDLE_PITCH = 48;

  let host = $state<HTMLDivElement | undefined>();
  let canvas = $state<HTMLCanvasElement | undefined>();
  let size = $state({ w: 0, h: 0 });

  const summary = $derived(describe(points));

  $effect(() => {
    const el = canvas;
    if (el === undefined || size.w <= 0 || size.h <= 0) return;
    draw(el, size.w, size.h, points);
  });

  onMount(() => {
    const el = host;
    if (el === undefined) return;
    const observer = new ResizeObserver((entries) => {
      for (const entry of entries) {
        const { width, height: boxHeight } = entry.contentRect;
        if (Math.abs(width - size.w) > 0.5 || Math.abs(boxHeight - size.h) > 0.5) {
          size = { w: width, h: boxHeight };
        }
      }
    });
    observer.observe(el);
    return () => observer.disconnect();
  });

  /** Web Mercator, normalised to 0..1 across the world. */
  function project(latitude: number, longitude: number): { x: number; y: number } {
    const phi = (clampLat(latitude) * Math.PI) / 180;
    return {
      x: (longitude + 180) / 360,
      y: (1 - Math.log(Math.tan(phi) + 1 / Math.cos(phi)) / Math.PI) / 2,
    };
  }

  /** The inverse of the y component, so the view can be named in degrees. */
  function unproject(y: number): number {
    return (Math.atan(Math.sinh(Math.PI * (1 - 2 * y))) * 180) / Math.PI;
  }

  function clampLat(latitude: number): number {
    return Math.min(MAX_LAT, Math.max(-MAX_LAT, latitude));
  }

  /** Picks a 1, 2 or 5 times a power of ten step for a span of about six lines. */
  function niceStep(span: number): number {
    const raw = span / 6;
    if (!Number.isFinite(raw) || raw <= 0) return 1;
    const magnitude = 10 ** Math.floor(Math.log10(raw));
    const normalised = raw / magnitude;
    // Rounded to the nearest 1, 2 or 5 (midpoints at 1.5 and 3.5), so the line
    // count stays near six instead of collapsing to three.
    const step = normalised < 1.5 ? 1 : normalised < 3.5 ? 2 : normalised < 7.5 ? 5 : 10;
    return step * magnitude;
  }

  /** Wraps a longitude back into -180..180 for display. */
  function normaliseLon(longitude: number): number {
    return ((((longitude + 180) % 360) + 360) % 360) - 180;
  }

  /** The largest 1, 2 or 5 times a power of ten that still fits the target. */
  function niceDistanceBelow(target: number): number {
    if (!Number.isFinite(target) || target <= 0) return 1;
    const magnitude = 10 ** Math.floor(Math.log10(target));
    for (const factor of [10, 5, 2, 1]) {
      if (factor * magnitude <= target) return factor * magnitude;
    }
    return magnitude;
  }

  function decimalsFor(step: number): number {
    return Math.max(0, -Math.floor(Math.log10(step)));
  }

  function formatDegrees(value: number, decimals: number): string {
    const rounded = Number(value.toFixed(decimals));
    const shown = Object.is(rounded, -0) ? 0 : rounded;
    return `${shown < 0 ? "" : "+"}${shown.toFixed(decimals)}°`;
  }

  function formatMetres(metres: number): string {
    return metres >= 1000 ? `${metres / 1000} km` : `${metres} m`;
  }

  /** Aligns a hairline with the device pixel grid so it stays crisp. */
  function snap(value: number, dpr: number): number {
    return (Math.round(value * dpr - dpr / 2) + dpr / 2) / dpr;
  }

  // Canvas cannot use CSS classes, so the tokens are read straight off the element.
  function readColors(el: Element): Colors {
    const styles = getComputedStyle(el);
    const token = (name: string, fallback: string) =>
      styles.getPropertyValue(name).trim() || fallback;
    return {
      ink: token("--ink", "#10161d"),
      line: token("--line", "#2c3a47"),
      signal: token("--signal", "#4fb3a9"),
      signalDim: token("--signal-dim", "#335c57"),
      faint: token("--fg-faint", "#6f7c88"),
      muted: token("--fg-muted", "#8a98a6"),
      font: token("--font-data", "ui-monospace, monospace"),
    };
  }

  function describe(pts: MapPoint[]): string {
    if (pts.length === 0) return "Map with no received position";
    const newest = pts[pts.length - 1];
    const count = pts.length === 1 ? "1 received position" : `${pts.length} received positions`;
    const lat = formatDegrees(newest.latitude, 5);
    const lon = formatDegrees(newest.longitude, 5);
    return `Map with ${count}, newest at ${lat}, ${lon}`;
  }

  function draw(el: HTMLCanvasElement, w: number, h: number, pts: MapPoint[]) {
    const ctx = el.getContext("2d");
    if (ctx === null) return;

    // Match the backing store to the device so labels and hairlines stay sharp.
    const dpr = Math.min(window.devicePixelRatio || 1, 3);
    const backingW = Math.max(1, Math.round(w * dpr));
    const backingH = Math.max(1, Math.round(h * dpr));
    if (el.width !== backingW || el.height !== backingH) {
      el.width = backingW;
      el.height = backingH;
    }
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);

    const colors = readColors(el);
    ctx.fillStyle = colors.ink;
    ctx.fillRect(0, 0, w, h);

    if (pts.length === 0) {
      drawIdle(ctx, w, h, dpr, colors);
      return;
    }
    drawTrack(ctx, w, h, dpr, pts, colors);
  }

  // Nothing received yet: a blank chart, not a broken widget.
  function drawIdle(
    ctx: CanvasRenderingContext2D,
    w: number,
    h: number,
    dpr: number,
    colors: Colors
  ) {
    // A screen texture rather than a graticule: with no fix there is no place to
    // plot, so labelling a grid with coordinates would be inventing one.
    ctx.strokeStyle = colors.line;
    ctx.lineWidth = 1;
    ctx.beginPath();
    for (let x = IDLE_PITCH; x < w; x += IDLE_PITCH) {
      ctx.moveTo(snap(x, dpr), 0);
      ctx.lineTo(snap(x, dpr), h);
    }
    for (let y = IDLE_PITCH; y < h; y += IDLE_PITCH) {
      ctx.moveTo(0, snap(y, dpr));
      ctx.lineTo(w, snap(y, dpr));
    }
    ctx.stroke();

    const cx = w / 2;
    const cy = h / 2;
    ctx.strokeStyle = colors.faint;
    ctx.globalAlpha = 0.7;
    ctx.setLineDash([3, 4]);
    ctx.beginPath();
    ctx.arc(cx, cy - 26, 18, 0, TAU);
    ctx.stroke();
    ctx.setLineDash([]);
    ctx.beginPath();
    ctx.moveTo(snap(cx, dpr), cy - 54);
    ctx.lineTo(snap(cx, dpr), cy - 44);
    ctx.moveTo(snap(cx, dpr), cy - 8);
    ctx.lineTo(snap(cx, dpr), cy + 2);
    ctx.moveTo(cx - 28, cy - 26);
    ctx.lineTo(cx - 18, cy - 26);
    ctx.moveTo(cx + 18, cy - 26);
    ctx.lineTo(cx + 28, cy - 26);
    ctx.stroke();

    ctx.textAlign = "center";
    ctx.textBaseline = "middle";
    ctx.fillStyle = colors.faint;
    ctx.font = `11px ${colors.font}`;
    ctx.fillText("NO FIX", cx, cy + 14);
    ctx.globalAlpha = 0.75;
    ctx.font = `10px ${colors.font}`;
    ctx.fillText("no updates received", cx, cy + 34);
    ctx.globalAlpha = 1;
  }

  function drawTrack(
    ctx: CanvasRenderingContext2D,
    w: number,
    h: number,
    dpr: number,
    pts: MapPoint[],
    colors: Colors
  ) {
    // Mercator is conformal, so one scale serves both axes and nothing stretches.
    // Longitude wraps at the antimeridian, so the track is unwrapped first: a hop
    // from +179.9 to -179.9 is 0.2° of movement, not a whole world of it.
    const xs: number[] = [];
    let frameShift = 0;
    for (let i = 0; i < pts.length; i++) {
      const raw = (pts[i].longitude + 180) / 360;
      // Keep every fix within half a world of the one before it.
      if (i > 0) frameShift = Math.round(xs[i - 1] - raw);
      xs.push(raw + frameShift);
    }
    const ys = pts.map((point) => project(point.latitude, 0).y);

    let minX = Infinity;
    let maxX = -Infinity;
    let minY = Infinity;
    let maxY = -Infinity;
    for (let i = 0; i < xs.length; i++) {
      minX = Math.min(minX, xs[i]);
      maxX = Math.max(maxX, xs[i]);
      minY = Math.min(minY, ys[i]);
      maxY = Math.max(maxY, ys[i]);
    }

    const innerW = Math.max(1, w - PAD * 2);
    const innerH = Math.max(1, h - PAD * 2);
    const minSpan = MIN_SPAN_DEG / 360;
    const scale = Math.min(
      innerW / Math.max(maxX - minX, minSpan),
      innerH / Math.max(maxY - minY, minSpan)
    );
    // Centred on the fix itself, so a widened degenerate axis still renders it centred.
    const cx = (minX + maxX) / 2;
    const cy = (minY + maxY) / 2;

    const toX = (x: number) => (x - cx) * scale + w / 2;
    const toY = (y: number) => (y - cy) * scale + h / 2;
    // The graticule sits in the same unwrapped frame as the track.
    const toXOfLon = (longitude: number) => toX((longitude + 180) / 360 + frameShift);

    // Graticule, plus the labels that give the view its coordinates.
    const lonLeft = (cx - w / 2 / scale - frameShift) * 360 - 180;
    const lonRight = (cx + w / 2 / scale - frameShift) * 360 - 180;
    const latTop = unproject(cy - h / 2 / scale);
    const latBottom = unproject(cy + h / 2 / scale);
    const lonStep = niceStep(lonRight - lonLeft);
    const latStep = niceStep(Math.abs(latTop - latBottom));

    ctx.strokeStyle = colors.line;
    ctx.lineWidth = 1;
    ctx.beginPath();
    for (let k = Math.ceil(lonLeft / lonStep); k * lonStep <= lonRight; k++) {
      const x = snap(toXOfLon(k * lonStep), dpr);
      ctx.moveTo(x, 0);
      ctx.lineTo(x, h);
    }
    for (let k = Math.ceil(latBottom / latStep); k * latStep <= latTop; k++) {
      const y = snap(toY(project(k * latStep, 0).y), dpr);
      ctx.moveTo(0, y);
      ctx.lineTo(w, y);
    }
    ctx.stroke();

    const lonDecimals = decimalsFor(lonStep);
    const latDecimals = decimalsFor(latStep);
    ctx.fillStyle = colors.faint;
    ctx.font = `10px ${colors.font}`;
    ctx.textAlign = "left";
    ctx.textBaseline = "top";
    for (let k = Math.ceil(lonLeft / lonStep); k * lonStep <= lonRight; k++) {
      const label = formatDegrees(normaliseLon(k * lonStep), lonDecimals);
      const x = toXOfLon(k * lonStep) + 4;
      // A label that would run off the right edge is dropped rather than clipped.
      if (x + ctx.measureText(label).width > w - 2) continue;
      ctx.fillText(label, x, 6);
    }
    ctx.textBaseline = "bottom";
    for (let k = Math.ceil(latBottom / latStep); k * latStep <= latTop; k++) {
      const y = toY(project(k * latStep, 0).y);
      // The top band belongs to the meridian labels.
      if (y < PAD - 4) continue;
      ctx.fillText(formatDegrees(k * latStep, latDecimals), 5, y - 4);
    }

    const px = xs.map((x, i) => ({ x: toX(x), y: toY(ys[i]) }));

    // Older fixes fade into the newest one so movement reads at a glance.
    const last = px.length - 1;
    if (last > 0) {
      ctx.strokeStyle = colors.signal;
      ctx.lineWidth = 2;
      ctx.lineJoin = "round";
      ctx.lineCap = "round";
      for (let i = 1; i <= last; i++) {
        ctx.globalAlpha = 0.12 + 0.6 * (i / last);
        ctx.beginPath();
        ctx.moveTo(px[i - 1].x, px[i - 1].y);
        ctx.lineTo(px[i].x, px[i].y);
        ctx.stroke();
      }
      // A short track gets a dot per fix so two or three points still read as one.
      if (px.length <= 24) {
        ctx.fillStyle = colors.signal;
        for (let i = 0; i < last; i++) {
          ctx.globalAlpha = 0.25 + 0.5 * (i / last);
          ctx.beginPath();
          ctx.arc(px[i].x, px[i].y, 2, 0, TAU);
          ctx.fill();
        }
      }
      ctx.globalAlpha = 1;
    }

    const fix = px[last];
    const accuracy = pts[last].accuracyM;
    if (typeof accuracy === "number" && Number.isFinite(accuracy) && accuracy > 0) {
      const lat = clampLat(pts[last].latitude);
      const radius = (accuracy / (METRES_PER_UNIT * Math.cos((lat * Math.PI) / 180))) * scale;
      // Below a few pixels the ring says nothing, so it is left out.
      if (radius >= 8 && radius < Math.max(w, h) * 2) {
        ctx.strokeStyle = colors.signalDim;
        ctx.lineWidth = 1;
        ctx.globalAlpha = 0.9;
        ctx.beginPath();
        ctx.arc(fix.x, fix.y, radius, 0, TAU);
        ctx.stroke();
      }
    }

    // Reticle ticks, then the marker: same shape as the live dot in the panel headers.
    ctx.globalAlpha = 0.55;
    ctx.strokeStyle = colors.signal;
    ctx.beginPath();
    ctx.moveTo(fix.x - 15, fix.y);
    ctx.lineTo(fix.x - 8, fix.y);
    ctx.moveTo(fix.x + 8, fix.y);
    ctx.lineTo(fix.x + 15, fix.y);
    ctx.moveTo(fix.x, fix.y - 15);
    ctx.lineTo(fix.x, fix.y - 8);
    ctx.moveTo(fix.x, fix.y + 8);
    ctx.lineTo(fix.x, fix.y + 15);
    ctx.stroke();

    ctx.globalAlpha = 1;
    ctx.strokeStyle = colors.signalDim;
    ctx.lineWidth = 3;
    ctx.beginPath();
    ctx.arc(fix.x, fix.y, 6.5, 0, TAU);
    ctx.stroke();
    ctx.fillStyle = colors.signal;
    ctx.beginPath();
    ctx.arc(fix.x, fix.y, 5, 0, TAU);
    ctx.fill();

    // Without a basemap the scale bar is the only thing that says how big this is.
    const latCentre = clampLat(unproject(cy));
    const metresPerPx = (METRES_PER_UNIT * Math.cos((latCentre * Math.PI) / 180)) / scale;
    const metres = niceDistanceBelow(metresPerPx * Math.min(w * 0.3, 170));
    const barPx = metres / metresPerPx;
    const barRight = w - PAD + 4;
    const barLeft = barRight - barPx;
    const barY = h - 10;
    ctx.strokeStyle = colors.muted;
    ctx.lineWidth = 1;
    ctx.beginPath();
    ctx.moveTo(snap(barLeft, dpr), barY);
    ctx.lineTo(snap(barRight, dpr), barY);
    ctx.moveTo(snap(barLeft, dpr), barY - 5);
    ctx.lineTo(snap(barLeft, dpr), barY);
    ctx.moveTo(snap(barRight, dpr), barY - 5);
    ctx.lineTo(snap(barRight, dpr), barY);
    ctx.stroke();
    ctx.fillStyle = colors.muted;
    ctx.font = `10px ${colors.font}`;
    ctx.textAlign = "left";
    ctx.textBaseline = "bottom";
    ctx.fillText(formatMetres(metres), barLeft, barY - 7);
  }
</script>

<div class="map" bind:this={host} style="height: {height}px">
  <canvas bind:this={canvas} aria-hidden="true"></canvas>
  <p class="map-summary">{summary}</p>
</div>

<style>
  .map {
    position: relative;
    background: var(--ink);
    border: 1px solid var(--line);
    border-radius: var(--radius);
    overflow: hidden;
  }

  canvas {
    display: block;
    width: 100%;
    height: 100%;
  }

  /* The drawing itself is decorative; this carries the same reading to a screen reader. */
  .map-summary {
    position: absolute;
    width: 1px;
    height: 1px;
    margin: -1px;
    padding: 0;
    overflow: hidden;
    clip-path: inset(50%);
    white-space: nowrap;
    border: 0;
  }
</style>
