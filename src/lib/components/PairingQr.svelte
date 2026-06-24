<script lang="ts">
  import * as QRCode from "qrcode";

  let { value }: { value: string } = $props();

  let dataUrl = $state("");

  // PQ keys (ML-DSA-65 + ML-KEM-768) are too large for a single QR code.
  // The QR shows a short fingerprint instead — the full pairing code is in
  // the textarea below for copy/paste.
  $effect(() => {
    if (!value) {
      dataUrl = "";
      return;
    }
    // Show only the first 60 chars as a visual identifier
    const short = value.length > 60 ? value.slice(0, 60) : value;
    QRCode.toDataURL(short, {
      width: 240,
      margin: 1,
      color: { dark: "#e7ecf0", light: "#1a232d" },
    })
      .then((url: string) => {
        dataUrl = url;
      })
      .catch(() => {
        dataUrl = "";
      });
  });
</script>

{#if dataUrl}
  <img class="qr" src={dataUrl} alt="Pairing code" width="240" height="240" />
{:else}
  <div class="qr placeholder" aria-hidden="true"></div>
{/if}

<style>
  .qr {
    border-radius: var(--radius);
    border: 1px solid var(--line);
  }

  .placeholder {
    width: 240px;
    height: 240px;
    background: var(--panel-raised);
  }
</style>
