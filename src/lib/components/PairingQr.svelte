<script lang="ts">
  import * as QRCode from "qrcode";

  let { value }: { value: string } = $props();

  let dataUrl = $state("");

  $effect(() => {
    if (!value) {
      dataUrl = "";
      return;
    }
    QRCode.toDataURL(value, {
      width: 240,
      margin: 1,
      color: { dark: "#e7ecf0", light: "#1a232d" },
    })
      .then((url: string) => (dataUrl = url))
      .catch(() => (dataUrl = ""));
  });
</script>

{#if dataUrl}
  <img class="qr" src={dataUrl} alt="Pairing QR code" width="240" height="240" />
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
