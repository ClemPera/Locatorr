<script lang="ts">
  import * as QRCode from "qrcode";

  let { value }: { value: string } = $props();

  let dataUrl = $state("");
  let error = $state("");

  $effect(() => {
    if (!value) {
      dataUrl = "";
      error = "";
      return;
    }
    QRCode.toDataURL(value, {
      width: 240,
      margin: 1,
      color: { dark: "#e7ecf0", light: "#1a232d" },
      errorCorrectionLevel: "L",
    })
      .then((url: string) => {
        dataUrl = url;
        error = "";
      })
      .catch((err: unknown) => {
        dataUrl = "";
        error = `QR too large — copy the code below instead`;
        console.warn("QR generation failed:", err);
      });
  });
</script>

{#if dataUrl}
  <img class="qr" src={dataUrl} alt="Pairing QR code" width="240" height="240" />
{:else if error}
  <div class="qr placeholder error-state" aria-label={error}>{error}</div>
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
    display: flex;
    align-items: center;
    justify-content: center;
    text-align: center;
    font-size: var(--text-sm);
    color: var(--fg-muted);
    padding: var(--space-4);
  }
</style>
