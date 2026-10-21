<!--
  Dashboard sidebar nav. Toggl-style: section groupings (ANALYZE, BROWSE)
  with Lucide-style icon + label entries. Active section gets the accent
  treatment (2px coral left border + coral-tinted bg + bright text).
  Bottom: tracker shortcut + settings + version (Geist Mono).

  Brand glyph in the top corner is the chart-line-plus-heart mark from
  the Climax logo.
-->
<script lang="ts">
  import { pulse } from "$lib/pulse";
  import type { ComponentProps } from "svelte";
  import Icon from "$lib/Icon.svelte";
  import ConnectionBadge from "$lib/ConnectionBadge.svelte";
  import { updateCheck } from "$lib/update-check.svelte";
  import type { Section } from "$lib/dashboard/nav.svelte";

  type Props = {
    activeSection: Section;
    onSectionChange: (s: Section) => void;
    /** Null in the browser web client - there's no tracker window to open, so
     *  the footer shortcut is hidden. */
    onOpenTracker: (() => void) | null;
    onOpenSettings: () => void;
    /** Open Settings on the version page. Omit and the version line stays a
     *  plain label even when an update is out. */
    onOpenVersion?: () => void;
    isSessionActive: boolean;
    isSessionPaused: boolean;
  };

  let { activeSection, onSectionChange, onOpenTracker, onOpenSettings, onOpenVersion, isSessionActive, isSessionPaused }: Props = $props();

  type IconName = ComponentProps<typeof Icon>["name"];
  type Item = { id: Section; label: string; icon: IconName };

  const analyze: Item[] = [
    { id: "overview",   label: "Overview", icon: "bar-chart-3" },
    { id: "trends",     label: "Trends",   icon: "trending-up" },
  ];
  const browse: Item[] = [
    { id: "sessions",   label: "Sessions",   icon: "list" },
    { id: "scenes",     label: "Scenes",     icon: "clapperboard" },
    { id: "performers", label: "Performers", icon: "user" },
    { id: "studios",    label: "Studios",    icon: "video" },
    { id: "tags",       label: "Tags",       icon: "tag" },
  ];

  const DONATE_URL = "https://buymeacoffee.com/pineapplestorm";
</script>

<aside class="sidebar">
  <div class="brand">
    <img class="brand-mark" src="/climax-icon.png" alt="Climax" width="24" height="24" />
    <span class="brand-name">Climax</span>
    <div class="dot" class:active={isSessionActive} use:pulse={isSessionActive} class:paused={isSessionPaused} aria-label={isSessionActive ? "session running" : isSessionPaused ? "session paused" : "idle"}></div>
    <ConnectionBadge />
  </div>

  <nav class="nav">
    <div class="group">
      <div class="group-label">Analyse</div>
      {#each analyze as item (item.id)}
        <button
          class="nav-item"
          class:active={activeSection === item.id}
          onclick={() => onSectionChange(item.id)}
        >
          <Icon name={item.icon} size={16} />
          <span class="nav-label">{item.label}</span>
        </button>
      {/each}
    </div>

    <div class="group">
      <div class="group-label">Browse</div>
      {#each browse as item (item.id)}
        <button
          class="nav-item"
          class:active={activeSection === item.id}
          onclick={() => onSectionChange(item.id)}
        >
          <Icon name={item.icon} size={16} />
          <span class="nav-label">{item.label}</span>
        </button>
      {/each}
    </div>
  </nav>

  <div class="footer">
    {#if onOpenTracker}
      <button class="nav-item" onclick={onOpenTracker} title="Open the tracker window">
        <Icon name="arrow-left-to-line" size={16} />
        <span class="nav-label">Tracker</span>
      </button>
    {/if}
    <button class="nav-item" onclick={onOpenSettings} title="Settings">
      <Icon name="settings" size={16} />
      <span class="nav-label">Settings</span>
    </button>
    <!-- Donate: the one intentional break from the monochrome-icon rail — a coral
         heart, since it's a call-to-action and coral is the brand colour. -->
    <a class="nav-item donate" href={DONATE_URL} target="_blank" rel="noopener" title="Support Climax by buying me a coffee">
      <Icon name="heart" size={16} filled color="var(--accent)" />
      <span class="nav-label">Donate</span>
    </a>
    <!-- Real, not a literal: this used to be a hardcoded "v0.1.0" that would
         quietly lie the moment a release bumped it. When a newer release exists
         it becomes the quiet, permanent cue - the dashboard notice above can be
         dismissed, this cannot, and it sits where someone looks for a version
         anyway. -->
    {#if updateCheck.available && onOpenVersion}
      <button class="version update" onclick={onOpenVersion} title="Open Version and updates to see the new release.">
        v{updateCheck.myVersion} · update
      </button>
    {:else}
      <div class="version">{updateCheck.myVersion ? `v${updateCheck.myVersion}` : ""}</div>
    {/if}
  </div>
</aside>

<style>
  .sidebar {
    width: 208px;
    flex-shrink: 0;
    background: var(--bg-sunken);
    border-right: 1px solid var(--border-subtle);
    display: flex;
    flex-direction: column;
    height: 100vh;
    box-sizing: border-box;
    padding: 16px 0 10px;
  }

  /* macOS runs the dashboard with titleBarStyle Overlay: no title bar, and the
     traffic lights float over this sidebar's top-left. Push our content clear
     of them (they finish around y=26) while the sidebar's background still
     runs to the very top, which is what makes it look integrated rather than
     like a fake title bar. */
  :global(html.mac-overlay-titlebar) .sidebar { padding-top: 38px; }

  .brand {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 2px 18px 18px;
    border-bottom: 1px solid var(--border-subtle);
  }
  .brand-mark {
    width: 24px;
    height: 24px;
    border-radius: 6px;
    flex-shrink: 0;
    display: block;
    object-fit: cover;
  }
  .brand-name {
    font-size: 15px;
    font-weight: 600;
    color: var(--fg-strong);
    letter-spacing: 0.1px;
    flex: 1;
  }
  .dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--fg-muted);
    transition: background var(--dur-3);
    flex-shrink: 0;
  }
  .dot.active {
    background: var(--live);
    position: relative;
  }
  /* Low-rate pulse driven by --cx-pulse from src/lib/pulse.ts (about 10 frames a
     second, not 60), changing only transform + opacity, so a running session does not
     repaint the window every frame. 8px dot + 6px halo = scale 20/8. */
  .dot.active::after {
    content: "";
    position: absolute;
    inset: 0;
    border-radius: 50%;
    background: inherit;
    pointer-events: none;
    will-change: transform, opacity;
    transform: scale(calc(1 + 1.5 * var(--cx-pulse, 0)));
    opacity: calc(0.45 * (1 - var(--cx-pulse, 0)));
  }
  .dot.paused { background: var(--warn); }

  .nav {
    flex: 1;
    overflow-y: auto;
    padding: 14px 0;
    display: flex;
    flex-direction: column;
    gap: 16px;
  }
  .group {
    display: flex;
    flex-direction: column;
    gap: 1px;
  }
  .group-label {
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: var(--ls-eyebrow, 0.14em);
    color: var(--fg-subtle);
    font-weight: 600;
    padding: 0 18px;
    margin-bottom: 6px;
  }

  .nav-item {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 7px 18px 7px 16px;
    background: transparent;
    border: none;
    border-left: 2px solid transparent;
    color: var(--fg-muted);
    font-size: 13px;
    font-family: inherit;
    text-align: left;
    cursor: pointer;
    transition: background var(--dur-2), color var(--dur-2), border-color var(--dur-2);
  }
  .nav-item:hover {
    background: var(--bg-elevated);
    color: var(--fg);
  }
  .nav-item.active {
    background: var(--coral-a-08);
    border-left-color: var(--accent);
    color: var(--fg-strong);
  }
  .nav-label { flex: 1; }
  /* The lone coral icon in the rail. Label still behaves like the others on
     hover; the heart stays coral (its colour is set inline on the glyph). */
  .nav-item.donate { text-decoration: none; }

  .footer {
    border-top: 1px solid var(--border-subtle);
    padding-top: 10px;
    display: flex;
    flex-direction: column;
    gap: 1px;
  }
  .version {
    font-size: 11px;
    color: var(--fg-subtle);
    padding: 8px 18px 0;
    font-variant-numeric: tabular-nums;
    font-family: var(--font-mono);
    /* Holds its line while the version loads. It used to be a literal, so it
       had nothing to wait for; without this the footer twitches on every
       dashboard mount as the line collapses and then fills. */
    min-height: 1.2em;
    box-sizing: content-box;
  }
  /* Same line, same place, same size - only the colour changes, so it reads as
     the version having something to say rather than as a new control. */
  .version.update {
    display: block;
    width: 100%;
    text-align: left;
    border: none;
    background: none;
    color: var(--accent);
    cursor: pointer;
  }
  .version.update:hover { color: var(--accent-hover, #f58895); }
</style>
