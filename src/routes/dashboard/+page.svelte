<!--
  Dashboard chassis. Holds the sidebar nav + the filter bar (visual shell)
  + the section content area + the global Settings modal.

  The actual content of each section lives in its own component under
  $lib/dashboard/sections/. Section switching is local state — no URL
  routing yet, no separate Tauri windows, just the active section pointer
  driving which component renders.
-->
<script lang="ts">
  import { wording } from "$lib/wording.svelte";
  import { pauseAnimationsWhenHidden } from "$lib/pause-when-hidden";
  import { onMount, onDestroy } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { listen, type UnlistenFn } from "@tauri-apps/api/event";
  import Sidebar from "$lib/dashboard/Sidebar.svelte";
  import Overview from "$lib/dashboard/sections/Overview.svelte";
  import Trends from "$lib/dashboard/sections/Trends.svelte";
  import Sessions from "$lib/dashboard/sections/Sessions.svelte";
  import Scenes from "$lib/dashboard/sections/Scenes.svelte";
  import Performers from "$lib/dashboard/sections/Performers.svelte";
  import Studios from "$lib/dashboard/sections/Studios.svelte";
  import Tags from "$lib/dashboard/sections/Tags.svelte";
  import SettingsModal from "$lib/SettingsModal.svelte";
  import ServerPortBanner from "$lib/ServerPortBanner.svelte";
  import UpdateBanner from "$lib/UpdateBanner.svelte";
  import { updateCheck } from "$lib/update-check.svelte";
  import { openUrl } from "@tauri-apps/plugin-opener";
  import ProfileBadge from "$lib/ProfileBadge.svelte";
  import WebTokenGate from "$lib/WebTokenGate.svelte";
  import Onboarding from "$lib/Onboarding.svelte";
  import AwayPrompt from "$lib/AwayPrompt.svelte";
  import { api } from "$lib/api";
  import { activeSession } from "$lib/active-session.svelte";
  import { nav } from "$lib/dashboard/nav.svelte";
  import { isTauri } from "$lib/transport";

  // Section pointer lives in the shared nav store so cross-section jumps (a
  // performer/studio pill, a related-scene row) can drive it from anywhere.
  const activeSection = $derived(nav.activeSection);
  let settingsOpen = $state(false);
  /** Which Settings page to land on. The update notice deep-links to its own.
   *  Reset on EVERY exit, including the re-run-onboarding one - the modal is
   *  recreated per open and reads this seed fresh, so a stale value would land
   *  the next gear click on the wrong page. */
  let settingsPage = $state<"general" | "version" | "server">("general");
  let showOnboarding = $state(false);
  /** True only when the user opened the wizard themselves from Settings, which
   *  makes it closeable. A genuine first run is not cancellable - there is
   *  nothing to return to, and the per-step skips are the way past. */
  let onboardingCancellable = $state(false);

  /** Open a release page: the OS browser on desktop, a new tab in the web
   *  client (the opener plugin needs the Tauri runtime). */
  function openRelease(url: string) {
    if (isTauri()) openUrl(url);
    else window.open(url, "_blank", "noopener");
  }
  // "While you were away" launch prompt: untracked activity since the last session.
  // Checked once per dashboard load (after onboarding is ruled out).
  let showAway = $state(false);
  let awaySince = $state<number | null>(null);

  // The sidebar brand dot reflects whether a session is running, regardless of
  // which section is open. Reads the shared active-session store (one poll for
  // the whole dashboard) instead of polling session_active itself.
  let unsub: (() => void) | null = null;
  let unlistenOpenScene: UnlistenFn | null = null;
  let contentEl = $state<HTMLDivElement | undefined>();

  // The away prompt reads play_imports / o_events, which the boot mirror sync
  // fills in the BACKGROUND - so WAIT for that sync to settle before asking, and
  // ask exactly once.
  //
  // It used to poll and show the moment it found ANY candidates. That is a
  // half-finished sync: the mirror PRUNES stale imports at the END of its run
  // (sweep_stale_imports), so scanning mid-walk sees rows that are about to be
  // deleted - and the prompt offered to recreate sessions the user had just
  // deleted. Seen for real. Only a settled mirror can be trusted here.
  //
  // The `i >= 3` beat is still needed because the sync reports "idle" for the
  // moment before it starts. If it never settles inside the cap (a big library, a
  // slow Stash) the prompt is SKIPPED rather than shown on unreconciled data - it
  // will appear next launch, and Settings -> Untracked sessions is always there.
  async function awayCheck() {
    const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
    let settled = false;
    for (let i = 0; i < 90; i++) {
      const st = await api.mirrorStatusGet().catch(() => null);
      if (st && st.phase === "idle" && i >= 3) {
        settled = true;
        break;
      }
      await sleep(700);
    }
    if (!settled) return;
    const r = await api.reconstructAwayCandidates().catch(() => null);
    if (r && r.candidates.length > 0) {
      awaySince = r.since_ms;
      showAway = true;
    }
  }

  onMount(() => {
    // macOS runs this window with titleBarStyle Overlay, so there is no title
    // bar and the traffic lights float over our own top-left. Flag it on <html>
    // so both this shell and Sidebar's scoped CSS can leave room for them.
    if (navigator.userAgent.includes("Macintosh")) {
      document.documentElement.classList.add("mac-overlay-titlebar");
    }
    // Cached read, no network - the backend refreshed it at boot if it was stale.
    void updateCheck.load();
    // Reveal the body (hidden in app.html until now) once fonts are ready and
    // after the next paint, so the cold-start window shows only the dark
    // background, never unstyled / wrong-font content.
    {
      const reveal = () => { document.body.style.visibility = "visible"; };
      (document.fonts?.ready ?? Promise.resolve()).then(() => requestAnimationFrame(reveal));
    }
    unsub = activeSession.subscribe();
    // Let nav.goto() capture the departure scroll position centrally.
    nav.registerScroller(contentEl ?? null);
    // The tracker (a separate window) opens a scene's page over here by emitting
    // this event then focusing the dashboard. This window is persistent, so the
    // listener is alive even while the dashboard is hidden. Tauri-only: a plain
    // browser has no tracker window and no event bus.
    if (isTauri()) {
      listen<{ contentItemId: number }>("open-scene-in-dashboard", (e) => {
        nav.goto("scene", e.payload.contentItemId);
      }).then((un) => { unlistenOpenScene = un; });
    }
    // First-launch setup wizard (fresh install, real profile). Boot forces the
    // dashboard window so this is seen. If it's NOT needed, fall through to the
    // "while you were away" prompt - untracked activity since your last session.
    // The two are mutually exclusive (onboarding's Estimate step handles the
    // cold backlog), so away only fires once onboarding is ruled out.
    api.onboardingNeeded()
      .then((needed) => {
        if (needed) { showOnboarding = true; return; }
        awayCheck();
      })
      .catch(() => {});
  });
  onDestroy(() => { unsub?.(); unlistenOpenScene?.(); });

  // The word used for an O (Settings -> General -> Wording). start() loads it
  // and keeps it in step with the tracker window and other devices.
  onMount(() => wording.start());
  // Freeze animations while this window is minimised (or the tab is hidden).
  onMount(() => pauseAnimationsWhenHidden());

  // Re-establish .content's scroll position after cross-section navigation:
  // Back restores the origin view's position, a sidebar switch starts at the
  // top. The destination's content usually renders ASYNC (data fetches), so we
  // EAGERLY hold the closest achievable position on every frame while the
  // content grows underneath — landing approximately right immediately and
  // refining as data renders. Never a deferred surprise jump (an earlier
  // wait-until-tall version detonated the scroll seconds late), and ANY user
  // input (wheel / touch / click / key) cancels the hold instantly.
  $effect(() => {
    const pending = nav.pendingScroll;
    const el = contentEl;
    if (pending === null || !el) return;
    const want: number = pending;
    let done = false;
    let raf = 0;
    const deadline = performance.now() + 1200;
    function finish() {
      if (done) return;
      done = true;
      cancelAnimationFrame(raf);
      el!.removeEventListener("wheel", finish);
      el!.removeEventListener("touchstart", finish);
      el!.removeEventListener("mousedown", finish);
      window.removeEventListener("keydown", finish);
      if (nav.pendingScroll === want) nav.pendingScroll = null;
    }
    el.addEventListener("wheel", finish, { passive: true });
    el.addEventListener("touchstart", finish, { passive: true });
    el.addEventListener("mousedown", finish);
    window.addEventListener("keydown", finish);
    function attempt() {
      if (done || nav.pendingScroll !== want) return finish();
      const maxTop = el!.scrollHeight - el!.clientHeight;
      el!.scrollTop = Math.min(want, Math.max(0, maxTop));
      if (maxTop >= want || performance.now() > deadline) return finish();
      raf = requestAnimationFrame(attempt);
    }
    attempt();
    return finish;
  });

  const sectionTitle = $derived.by(() => {
    switch (activeSection) {
      case "overview":   return "Overview";
      case "trends":     return "Trends";
      case "sessions":   return "Sessions";
      case "scenes":     return "Scenes";
      case "performers": return "Performers";
      case "studios":    return "Studios";
      case "tags":       return "Tags";
    }
  });

  function openTracker() {
    invoke("open_tracker").catch(console.error);
  }
</script>

<svelte:head><title>Climax - Dashboard</title></svelte:head>

<div class="layout">
  <!-- macOS only: with no title bar there is nothing left to grab, so this
       invisible strip across the top restores click-and-drag to move the
       window. It sits ABOVE the sidebar's and header's top padding, which is
       empty by design, so it never covers a control. The traffic lights are
       drawn by the window itself, above the webview, so they still take
       clicks straight through this. -->
  <div class="mac-drag" data-tauri-drag-region></div>
  {#if !isTauri()}
    <!-- Token prompt for a token-protected server (web only; dormant until a
         data command 401s). Desktop client-mode auth rides the rpc_http proxy. -->
    <WebTokenGate />
  {/if}
  <ProfileBadge />
  <Sidebar
    activeSection={activeSection}
    onSectionChange={(s) => nav.setSection(s)}
    onOpenTracker={isTauri() ? openTracker : null}
    onOpenSettings={() => settingsOpen = true}
    onOpenVersion={() => { settingsPage = 'version'; settingsOpen = true; }}
    isSessionActive={activeSession.current?.status === "active"}
    isSessionPaused={activeSession.current?.status === "paused"}
  />

  <div class="main">
    <header class="section-header">
      <h1>{sectionTitle}</h1>
    </header>

    <!-- Outside .content so it can't scroll out of view: if this shows, nothing
         is being tracked at all, and Settings (right here) is where it's fixed.
         Spaced from here rather than wrapped in a padded div, so the healthy case
         (the component renders nothing) leaves no gap behind. -->
    <ServerPortBanner onFix={() => settingsOpen = true} />
    <UpdateBanner
      onView={openRelease}
      onDetails={() => { settingsPage = 'version'; settingsOpen = true; }}
    />

    <div class="content" bind:this={contentEl}>
      {#if activeSection === "overview"}
        <Overview />
      {:else if activeSection === "trends"}
        <Trends />
      {:else if activeSection === "sessions"}
        <Sessions />
      {:else if activeSection === "scenes"}
        <Scenes />
      {:else if activeSection === "performers"}
        <Performers />
      {:else if activeSection === "studios"}
        <Studios />
      {:else if activeSection === "tags"}
        <Tags />
      {/if}
    </div>
  </div>
</div>

{#if settingsOpen}
  <SettingsModal
    initialPage={settingsPage}
    onClose={() => { settingsOpen = false; settingsPage = 'general'; }}
    onRerunOnboarding={() => { settingsOpen = false; settingsPage = 'general'; onboardingCancellable = true; showOnboarding = true; }}
  />
{/if}

{#if showOnboarding}
  <Onboarding
    canCancel={onboardingCancellable}
    onClose={() => { showOnboarding = false; onboardingCancellable = false; }}
    onConnectToServer={() => {
      showOnboarding = false;
      onboardingCancellable = false;
      settingsPage = 'server';
      settingsOpen = true;
    }}
  />
{/if}

{#if showAway}
  <AwayPrompt sinceMs={awaySince} onClose={() => showAway = false} />
{/if}

<style>
  /* Geist + Geist Mono are bundled locally (static/fonts/, loaded via
     /fonts.css in app.html) - no network, no FOUT, works offline. */

  /* ----------------------------------------------------------------
     Climax design tokens — sourced from the climax-design skill.
     Two layers: primitives (raw values) and semantic tokens (what UI
     code references). Never hand-roll new hex values; extend here.
     ---------------------------------------------------------------- */
  :global(:root) {
    /* INK SCALE — warm-cool neutrals biased a hair warm so coral reads
       as the same family. */
    --ink-950: #0A0B0F;
    --ink-900: #101218;
    --ink-850: #15171E;
    --ink-800: #1B1E26;
    --ink-700: #252934;
    --ink-600: #353A47;
    --ink-500: #5C6273;
    --ink-400: #8A909E;
    --ink-300: #B6BBC6;
    --ink-200: #D5D9E0;
    --ink-100: #ECEEF3;
    --ink-050: #F7F8FA;

    /* CORAL — pulled from the logo's chart line. The brand. */
    --coral-700: #B83A4A;
    --coral-600: #D9485A;
    --coral-500: #EF6B7A;
    --coral-400: #F58895;
    --coral-300: #FAA8B1;

    /* BONE — pulled from the logo's bar fills. Reserved for milestone
       moments (record day, longest session). */
    --bone-700: #B8AC92;
    --bone-500: #EBDFC2;
    --bone-400: #F2E8D4;
    --bone-300: #F7F0E0;

    /* FUNCTIONAL — kept distinct from coral so destruction never blurs
       into brand. */
    --green-500: #4ADE80;
    --amber-500: #FBB454;
    --red-500:   #F26B6B;
    --red-400:   #F58D8D;
    --blue-500:  #6FA5FF;

    /* ALPHA OVERLAYS */
    --coral-a-08: rgba(239, 107, 122, 0.08);
    --coral-a-16: rgba(239, 107, 122, 0.16);
    --coral-a-24: rgba(239, 107, 122, 0.24);
    --bone-a-12:  rgba(242, 232, 212, 0.12);
    --white-a-04: rgba(255, 255, 255, 0.04);
    --white-a-08: rgba(255, 255, 255, 0.08);

    /* SEMANTIC TOKENS */
    --bg:             var(--ink-950);
    --bg-elevated:    var(--ink-900);
    --bg-card:        var(--ink-850);
    --bg-card-hover:  var(--ink-800);
    --bg-input:       var(--ink-900);
    --bg-sunken:      #07080B;

    --fg:             var(--ink-100);
    --fg-strong:      var(--ink-050);
    --fg-muted:       var(--ink-400);
    --fg-subtle:      var(--ink-500);
    --fg-disabled:    var(--ink-600);

    --border:         var(--ink-700);
    --border-strong:  var(--ink-600);
    --border-subtle:  #1C1F26;

    --accent:         var(--coral-500);
    --accent-hover:   var(--coral-400);
    --accent-press:   var(--coral-600);
    --accent-soft:    var(--coral-a-16);
    --accent-glow:    var(--coral-a-24);
    --accent-fg:      #FFFFFF;

    --highlight:      var(--bone-400);
    --highlight-soft: var(--bone-a-12);

    --live:           var(--green-500);
    --warn:           var(--amber-500);
    --danger:         var(--red-500);
    --link:           var(--blue-500);

    --chart-grid: #1A1D25;

    /* TYPE */
    --font-sans:    "Geist", "Inter", system-ui, -apple-system, sans-serif;
    --font-display: "Geist", "Inter", system-ui, sans-serif;
    --font-mono:    "Geist Mono", "JetBrains Mono", "SF Mono", "Consolas", monospace;

    --ls-eyebrow: 0.14em;

    /* SHAPE */
    --radius-sm:   6px;
    --radius-md:   8px;
    --radius-lg:   10px;
    --radius-xl:   14px;
    --radius-pill: 999px;
    --radius: var(--radius-lg);

    /* MOTION */
    --ease-out:    cubic-bezier(0.16, 1, 0.3, 1);
    --ease-in-out: cubic-bezier(0.65, 0, 0.35, 1);
    --dur-1: 80ms;
    --dur-2: 120ms;
    --dur-3: 200ms;
    --dur-4: 320ms;

    /* ELEVATION */
    --shadow-sm:   0 1px 2px rgba(0, 0, 0, 0.30);
    --shadow-md:   0 4px 12px rgba(0, 0, 0, 0.35);
    --shadow-lg:   0 12px 32px rgba(0, 0, 0, 0.45);
    --shadow-glow: 0 0 0 3px var(--coral-a-24);

    /* LEGACY ALIASES — keep old var names working during the transition.
       Components that still reference these get the new colour through
       the alias. Safe to remove once everything has been migrated. */
    --bg-card-hover: var(--ink-800);
    --text: var(--fg);
    --text-bright: var(--fg-strong);
    --text-muted: var(--fg-muted);
    --accent-soft: var(--coral-a-16);
    --green: var(--live);
    --amber: var(--warn);
    --red: var(--danger);
    --cumshot: var(--accent);
  }

  :global(html), :global(body) {
    background: var(--bg);
    color: var(--fg);
    font-family: var(--font-sans);
    font-size: 14px;
    line-height: 20px;
    font-weight: 400;
    letter-spacing: -0.005em;
    margin: 0;
    padding: 0;
    -webkit-font-smoothing: antialiased;
    text-rendering: optimizeLegibility;
    font-feature-settings: "ss01", "cv11";
    overflow-x: hidden;
  }

  /* Fixed app-shell: the viewport never scrolls as a whole. The left rail and
     the section header stay put; only .content scrolls internally. Keeps the
     Tracker/Settings rail buttons always visible no matter how long the
     right-hand content is. */
  .layout {
    display: flex;
    height: 100vh;
    overflow: hidden;
  }

  .main {
    flex: 1;
    min-width: 0;
    height: 100vh;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .section-header {
    flex-shrink: 0;
    padding: 16px 24px 14px;
    border-bottom: 1px solid var(--border-subtle);
    background: var(--bg);
  }

  /* macOS: no title bar, so the window's own chrome no longer pushes content
     down. Drop the header to the same baseline the sidebar uses, otherwise the
     title sits higher than the brand beside it. */
  :global(html.mac-overlay-titlebar) .section-header { padding-top: 38px; }

  .mac-drag { display: none; }
  :global(html.mac-overlay-titlebar) .mac-drag {
    display: block;
    position: fixed;
    top: 0;
    left: 0;
    right: 0;
    /* Shorter than the 38px of padding it sits in, so it can never swallow a
       click meant for the brand or the section title. */
    height: 30px;
    z-index: 5;
  }
  .section-header h1 {
    margin: 0;
    font-family: var(--font-display);
    font-size: 22px;
    font-weight: 600;
    color: var(--fg-strong);
    letter-spacing: -0.01em;
  }

  /* Lines the banner up with the header + content padding without a wrapper
     element, so nothing is left behind when it renders nothing. */
  .main > :global(.port-banner),
  .main > :global(.update-banner) {
    flex-shrink: 0;
    margin: 14px 24px 0;
  }

  .content {
    flex: 1;
    min-height: 0;
    /* Vertical scroll only. overflow-y:auto alone makes overflow-x compute to
       auto too, so a sub-pixel-wide child surfaces a stray horizontal scrollbar
       once "Fit" removes the vertical one — pin the cross-axis to hidden. */
    overflow: hidden auto;
    /* Top padding is 12px, NOT 18, and FilterBar's ::before strip must match it
       exactly - see the comment there. The strip is painted above the sticky bar
       to cover this band once pinned, so any padding LARGER than the gap the
       section leaves above the bar means the strip covers the bottom of whatever
       sits there (it was eating 6px off the hero stats card). 12px also matches
       the Overview's own inter-section gap, so the rhythm is consistent. */
    padding: 12px 24px 24px;
    display: flex;
    flex-direction: column;
    gap: 16px;
  }
</style>
