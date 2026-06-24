<script lang="ts">
  let { value }: { value: string } = $props();

  let groups = $derived(value.split(" ").filter(Boolean));
</script>

<div class="meter" role="group" aria-label={`Verification code ${value}`}>
  {#each groups as group}
    <span class="group">{group}</span>
  {/each}
</div>

<style>
  .meter {
    display: inline-flex;
    width: fit-content;
    background: var(--ink);
    border: 1px solid var(--line);
    border-radius: var(--radius);
    overflow: hidden;
  }

  .group {
    font-family: var(--font-data);
    font-size: 1.5rem;
    letter-spacing: 0.04em;
    color: var(--fg);
    padding: var(--space-3) var(--space-4);
    border-right: 1px solid var(--line);
  }

  .group:last-child {
    border-right: none;
  }
</style>
