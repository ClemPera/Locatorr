<script lang="ts">
  import "@fontsource/space-grotesk/400.css";
  import "@fontsource/space-grotesk/500.css";
  import "@fontsource/space-grotesk/600.css";
  import "@fontsource/ibm-plex-mono/400.css";
  import "@fontsource/ibm-plex-mono/500.css";
  import "../lib/styles/app.css";
  import { page } from "$app/stores";

  let { children } = $props();

  const nav = [
    { href: "/", label: "Live", hint: "01" },
    { href: "/contacts", label: "Contacts", hint: "02" },
    { href: "/pair", label: "Pair", hint: "03" },
    { href: "/settings", label: "Settings", hint: "04" },
  ];
</script>

<div class="shell">
  <nav class="rail">
    <div class="brand">
      <span class="brand-mark">◍</span>
      <span class="brand-name">Locatorr</span>
    </div>
    <ul>
      {#each nav as item}
        <li>
          <a href={item.href} class:active={$page.url.pathname === item.href}>
            <span class="hint mono">{item.hint}</span>
            <span>{item.label}</span>
          </a>
        </li>
      {/each}
    </ul>
    <p class="rail-foot eyebrow">end-to-end encrypted</p>
  </nav>
  <main class="content">
    <div class="content-inner">
      {@render children()}
    </div>
  </main>
</div>

<style>
  .shell {
    display: flex;
    min-height: 100vh;
  }

  .rail {
    width: var(--nav-w);
    flex-shrink: 0;
    background: var(--panel);
    border-right: 1px solid var(--line);
    padding: var(--space-5) var(--space-4);
    display: flex;
    flex-direction: column;
  }

  .brand {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    margin-bottom: var(--space-6);
    padding: 0 var(--space-2);
  }

  .brand-mark {
    color: var(--signal);
    font-size: 1.1rem;
  }

  .brand-name {
    font-weight: 600;
    letter-spacing: -0.01em;
  }

  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
  }

  a {
    display: flex;
    align-items: center;
    gap: var(--space-3);
    padding: var(--space-3) var(--space-2);
    border-radius: var(--radius);
    color: var(--fg-muted);
    text-decoration: none;
    font-size: var(--text-sm);
    font-weight: 500;
    transition: background 0.15s ease, color 0.15s ease;
  }

  a:hover {
    background: var(--panel-raised);
    color: var(--fg);
  }

  a.active {
    background: var(--signal-dim);
    color: var(--fg);
  }

  .hint {
    color: var(--fg-faint);
    font-size: var(--text-xs);
  }

  a.active .hint {
    color: var(--signal);
  }

  .rail-foot {
    margin-top: auto;
    padding: 0 var(--space-2);
  }

  .content {
    flex: 1;
    display: flex;
    justify-content: center;
    padding: var(--space-6) var(--space-8);
  }

  .content-inner {
    width: 100%;
    max-width: 60rem;
  }

  @media (max-width: 720px) {
    .shell {
      flex-direction: column;
    }

    .rail {
      width: 100%;
      flex-direction: row;
      align-items: center;
      border-right: none;
      border-bottom: 1px solid var(--line);
      padding: var(--space-3) var(--space-4);
    }

    .brand {
      margin-bottom: 0;
    }

    ul {
      flex-direction: row;
      margin-left: auto;
    }

    .rail-foot {
      display: none;
    }

    .content {
      padding: var(--space-5);
    }
  }
</style>
