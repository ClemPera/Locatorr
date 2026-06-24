<script lang="ts">
  import QRCode from "qrcode";

  let { value }: { value: string } = $props();

  let dataUrl = $state("");
  let qrError = $state("");

  $effect(() => {
    qrError = "";
    if (!value) {
      dataUrl = "";
      return;
    }
    // QRCode is a CJS module; try the default export pattern first.
    // If it fails, the placeholder stays and we log the error.
    const qr = (QRCode as any).default || QRCode;
    if (typeof qr?.toDataURL !== "function") {
      qrError = "QR library failed to load";
      console.error("QRCode.toDataURL is not a function. Import result:", QRCode);
      return;
    }
    qr.toDataURL(value, {
      width: 240,
      margin: 1,
      color: { dark: "#e7ecf0", light: "#1a232d" },
    })
      .then((url: string) => {
        dataUrl = url;
        qrError = "";
      })
      .catch((err: unknown) => {
        dataUrl = "";
        qrError = "QR too large or generation failed";
        console.error("QR generation failed:", err, "value length:", value.length);
      });
  });
</script>

{#if dataUrl}
  <img class="qr" src={dataUrl} alt="Pairing QR code" width="240" height="240" />
{:else if qrError}
  <div class="qr placeholder error">{qrError}</div>
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

  .placeholder.error {
    color: var(--danger);
    border: 1px solid var(--danger);
  }
</style>
