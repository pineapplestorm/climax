<!--
  Compact tracker widget. Loaded into the "tracker" window (380x280-ish).
  Toggl-style: just the live timer, session controls, current scenes with
  inline +Cumshot, and a link to the full dashboard for browsing.

  The dashboard window is a separate route (/dashboard) and lives in a
  separate Tauri window. They share the same backend state via Tauri commands.
-->
<script lang="ts">
  import { pulse } from "$lib/pulse";
  import { w, wording } from "$lib/wording.svelte";
  import { pauseAnimationsWhenHidden } from "$lib/pause-when-hidden";
  import { onMount, onDestroy } from "svelte";
  import { goto } from "$app/navigation";
  import { invoke } from "@tauri-apps/api/core";
  import { listen, emit, type UnlistenFn } from "@tauri-apps/api/event";
  import { isTauri } from "$lib/transport";
  import { getCurrentWindow, LogicalSize, PhysicalPosition, primaryMonitor, currentMonitor } from "@tauri-apps/api/window";
  import OPromptModal from "$lib/OPromptModal.svelte";
  import IdleReturnModal from "$lib/IdleReturnModal.svelte";
  import CapturePromptToast from "$lib/CapturePromptToast.svelte";
  import ConfirmDialog from "$lib/ConfirmDialog.svelte";
  import SceneCard from "$lib/SceneCard.svelte";
  import Icon from "$lib/Icon.svelte";
  import ProfileBadge from "$lib/ProfileBadge.svelte";
  import ConnectionBadge from "$lib/ConnectionBadge.svelte";
  import ServerPortBanner from "$lib/ServerPortBanner.svelte";
  import { startServerEvents } from "$lib/server-events";
  import {
    api,
    formatDuration,
    type Session,
    type PlayedScene,
    type OEvent,
    type IdlePayload,
    type CapturePayload,
    type CrashPayload,
  } from "$lib/api";

  // Tracker window auto-sizing. Width is fixed at NORMAL_WIDTH; height
  // grows with content — empty state → idle; +1 scene bumps the window
  // taller, etc. — and caps at MAX_TRACKER_HEIGHT (≈3 scenes), after
  // which the .main element scrolls internally.
  //
  // Exception: while the wrap-up modal is open, we widen the window to
  // WRAP_UP_WIDTH and fix the height to WRAP_UP_HEIGHT so the modal has
  // breathing room for the SceneCard + counter row. Dismissing the modal
  // returns to the content-driven tracker size.
  const NORMAL_WIDTH = 480;
  const WRAP_UP_WIDTH = 640;
  const WRAP_UP_HEIGHT = 560;
  const MIN_TRACKER_HEIGHT = 260;
  const MAX_TRACKER_HEIGHT = 580;
  // .main has padding-top: 14px, padding-bottom: 16px. The content host's
  // offsetHeight does NOT include this; we add it back below.
  const MAIN_PADDING_Y = 30;
  let trackerHeaderEl = $state<HTMLElement | undefined>(undefined);
  let trackerContentEl = $state<HTMLElement | undefined>(undefined);
  let trackerResizeObs: ResizeObserver | null = null;

  async function syncTrackerSize() {
    // Pure browser (Phase 5): no Tauri window to size. The route immediately
    // redirects to /dashboard in onMount, but $effects fire on the first render
    // BEFORE onMount, so this guard is what stops window ops running in a tab.
    if (!isTauri()) return;
    // Modal mode: fixed dimensions matched to the wrap-up modal layout.
    // The ResizeObserver below still fires when underlying tracker content
    // shifts; we ignore those while the modal is up.
    if (oPromptOpen) {
      try {
        await getCurrentWindow().setSize(new LogicalSize(WRAP_UP_WIDTH, WRAP_UP_HEIGHT));
      } catch (e) {
        console.error("tracker setSize (modal) failed", e);
      }
      return;
    }
    if (!trackerHeaderEl || !trackerContentEl) return;
    // Toast mode: fit the compact toast (no min-height floor) and pin it
    // bottom-right of the screen, like a notification.
    if (toastMode) {
      const th = trackerHeaderEl.offsetHeight + trackerContentEl.offsetHeight + MAIN_PADDING_Y;
      try {
        await getCurrentWindow().setSize(new LogicalSize(NORMAL_WIDTH, th));
        await positionToastBottomRight();
      } catch (e) {
        console.error("tracker toast setSize failed", e);
      }
      return;
    }
    const desired =
      trackerHeaderEl.offsetHeight +
      trackerContentEl.offsetHeight +
      MAIN_PADDING_Y;
    const clamped = Math.max(MIN_TRACKER_HEIGHT, Math.min(MAX_TRACKER_HEIGHT, desired));
    try {
      await getCurrentWindow().setSize(new LogicalSize(NORMAL_WIDTH, clamped));
    } catch (e) {
      console.error("tracker setSize failed", e);
    }
  }

  let active = $state<Session | null>(null);
  let scenes = $state<PlayedScene[]>([]);
  // Tracker shows newest scene at top — flipped vs the backend's ASC order.
  // The dashboard's SessionDetail keeps the chronological spine layout, so
  // we reverse only here and in Overview's active-session card.
  //
  // Visibility filter: a scene-play row appears here while it is actively being
  // played (its tracked seconds keep climbing), held for PLAYING_GRACE_MS after
  // its last credited second so a brief pause doesn't drop it, plus always if it
  // has a cumshot. Open-but-never-played tabs never appear.
  //
  // "Actively played" comes from the backend's `last_advance_at` (the last
  // heartbeat that advanced seconds_tracked). This is authoritative server
  // state, so the list is correct the instant we mount / the window is shown —
  // no client-side seconds-delta tracking to rebuild. (The old approach kept a
  // per-component baseline that reset on every navigation and froze while the
  // window was hidden, so a genuinely-playing scene took seconds to reappear,
  // or never.)
  const PLAYING_GRACE_MS = 60_000;
  // "Actively playing right now" = the backend stamped last_advance_at within
  // the grace window. Single source of truth for BOTH the card list and the
  // live "N scenes" count, so the two never disagree about what's playing.
  const isPlaying = (s: PlayedScene) =>
    s.last_advance_at != null && now - s.last_advance_at <= PLAYING_GRACE_MS;
  // Cards: scenes playing now, plus any scene with a cumshot (kept around so you
  // can still +/- its count after it's paused/ended). Newest first.
  const scenesVisible = $derived.by(() =>
    [...scenes].reverse().filter((s) => s.cumshot_count > 0 || isPlaying(s)),
  );
  // Three live stats below the timer. "playing" = scenes advancing this moment
  // (live, from last_advance_at; a paused scene drops out even with a cumshot,
  // resuming brings it back). "watched" is NOT shown here as a derived count —
  // it uses the backend's COUNTED footprint (active.scene_count: crossed the
  // play threshold OR has a cumshot), identical to SessionDetail / wrap-up /
  // Stash, so the surfaces never disagree and sub-threshold tab-opens (0s, never
  // really watched) don't inflate it. "playing" can briefly exceed "watched"
  // (a scene plays before it crosses the threshold).
  const playingCount = $derived(scenes.filter(isPlaying).length);
  let now = $state(Date.now());
  let lastRefreshAt = $state(Date.now());

  let oPromptOpen = $state(false);
  let oPromptSessionId = $state<number | null>(null);
  let oPromptScenes = $state<PlayedScene[]>([]);
  // True when the wrap-up modal was opened by the quit flow (Save & close / Go
  // back instead of Stop / Discard). Drives OPromptModal's quitMode + the
  // onClose/onCommitted branch (cancel the quit vs exit the app).
  let oPromptQuitMode = $state(false);

  // Quit guard: confirm shown when the user hits tray Quit with a session
  // running. "Wrap up & close" → the wrap-up modal in quit mode; "Go back" →
  // cancel the quit.
  let quitConfirmOpen = $state(false);
  let unlistenQuit: UnlistenFn | null = null;

  // Crash-recovery prompt: set on boot when a session was left running from a
  // previous run (beyond the recovery threshold). Reuses IdleReturnModal in
  // "crash" variant.
  let crashPayload = $state<CrashPayload | null>(null);

  // When the wrap-up modal opens/closes, the window needs to widen/restore.
  // ResizeObserver alone doesn't catch this because the modal is a fixed
  // overlay that doesn't change the tracker content-host's size — we have
  // to drive the size off the modal-open state directly.
  // Tracks whether the previous effect run was in toast mode, so we can detect
  // the toast → normal-tracker transition (capture accepted or snoozed) and
  // recentre. Plain let (not $state): only read/written inside the effect.
  let wasToast = false;
  $effect(() => {
    oPromptOpen;                                // track dependency
    const isToast = toastMode;                  // re-fit + reposition on toast in/out
    const leftToast = wasToast && !isToast;
    wasToast = isToast;
    requestAnimationFrame(async () => {
      await syncTrackerSize();
      // The capture toast pins the window bottom-right. When it resolves (Track
      // accepted → running session, or Snooze), return the window to centre so
      // the normal tracker doesn't stay stuck in the corner.
      if (leftToast) {
        try { await getCurrentWindow().center(); } catch { /* ignore */ }
      }
    });
  });

  // Idle prompt - shown when presence.rs fires `idle_detected`. Backend also
  // sets the tracker to alwaysOnTop=true so we float above other apps; when
  // the modal resolves it sets alwaysOnTop back to false.
  let idlePayload = $state<IdlePayload | null>(null);

  // Passive-capture prompt toast. Set when the backend detects you watching a
  // scene with no session running; rendered as a bottom-right toast. Track →
  // backdated session; Snooze → hide + grace. Lives here (not its own window)
  // for the same IPC reason as the idle prompt.
  let capturePayload = $state<CapturePayload | null>(null);
  let captureBusy = $state(false);
  let unlistenCapture: UnlistenFn | null = null;
  const toastMode = $derived(!active && !!capturePayload);

  let timer: ReturnType<typeof setInterval> | null = null;
  let poll: ReturnType<typeof setInterval> | null = null;
  let unlistenIdle: UnlistenFn | null = null;
  // Stash navbar indicator clicked while tracking → Climax emits
  // `tracker_stop_requested`; we run the same stop() as the Stop button.
  let unlistenStopReq: UnlistenFn | null = null;
  // Stop fn for the server->client event channel (client mode only; no-op otherwise).
  let stopServerEvents: (() => void) | null = null;

  const livenessElapsed = $derived.by(() => {
    if (!active) return 0;
    if (active.status !== "active") return active.effective_duration_ms;
    const sinceRefresh = now - lastRefreshAt;
    return active.effective_duration_ms + Math.max(0, sinceRefresh);
  });

  async function refresh() {
    try {
      const s = await api.sessionActive();
      // Anchor the live-timer offset to the moment we read the session, and
      // assign it TOGETHER with `active` (no await in between) so the derived
      // timer never sees a fresh effective_duration_ms paired with a stale
      // lastRefreshAt. That mismatch double-counted ~one poll interval for a
      // frame on every refresh — the timer briefly jumped forward then snapped
      // back, most visibly as the tens digit rolled over. Fetch scenes first,
      // then commit all three state values in one synchronous block.
      const at = Date.now();
      const sc = s ? await api.sessionScenes(s.id) : [];
      // Unlinked cumshots: in a session, the session's scene-less O's; idle,
      // today's session-less scene-less O's. Cheap local IPC, fetched each poll
      // so the +/- counter stays live and the - removes the right (newest) one.
      const su = s
        ? (await api.oListForSession(s.id)).filter((o) => o.content_item_id == null)
        : [];
      const slu = s ? [] : await api.oSessionlessUnlinkedToday();
      active = s;
      lastRefreshAt = at;
      scenes = sc;
      sessionUnlinked = su;
      sessionlessUnlinked = slu;
    } catch (e) {
      console.error("refresh failed", e);
    }
  }

  async function start() {
    await api.sessionStart();
    await refresh();
  }

  // The wrap-up opens whenever the session is going to be KEPT, or has rows to
  // review: any scene (counted or not), any cumshot (a scene-less one keeps a
  // session on its own), or "Keep empty sessions" being on. It is the only
  // place to add a note, adjust the count, pick the day after midnight, or
  // discard a misfire. A session with none of those is discarded on stop, so
  // there is nothing to wrap up. The setting is read fresh each time (and only
  // when the cheaper checks have not already decided) so a change made in
  // Settings, possibly from another window or device, applies to the next stop.
  async function needsWrapUp(): Promise<boolean> {
    if (!active) return false;
    if (scenes.length > 0 || active.o_count > 0) return true;
    return api.keepEmptySessionsGet().catch(() => false);
  }

  async function stop() {
    if (active && (await needsWrapUp())) {
      oPromptSessionId = active.id;
      oPromptScenes = scenes;
      oPromptOpen = true;
      return;
    }
    await api.sessionStop();
    await refresh();
  }

  async function pauseOrResume() {
    if (!active) return;
    if (active.status === "paused") await api.sessionResume();
    else await api.sessionPause();
    await refresh();
  }

  // Counts come straight from the backend after each mutation — no local
  // optimistic layer. The oLog/delete + refresh round trip is local IPC
  // (single-digit ms), so the badge updates effectively instantly while
  // staying authoritative. An earlier optimistic-guess approach could leave
  // a stale +1 sitting next to the now-updated backend count, showing one
  // tap as "2"; reading the backend directly removes that whole class of bug.
  async function logCumshot(scene: PlayedScene) {
    if (!active) return;
    try {
      await api.oLog({
        sessionId: active.id,
        contentItemId: scene.content_item_id,
      });
    } catch (e) {
      console.error("inline cumshot log failed", e);
    }
    await refresh();
  }

  async function removeCumshot(scene: PlayedScene) {
    if (!active) return;
    try {
      await api.oDeleteRecentForScene(active.id, scene.content_item_id, 1);
    } catch (e) {
      console.error("inline cumshot remove failed", e);
    }
    await refresh();
  }

  // Unlinked cumshot: an O not tied to any Stash scene ("you came, but not to
  // anything in Stash"). With a session running it attaches to that session
  // (bumping its o_count); with none it's logged session-less and counts toward
  // your daily / all-time / trends totals. Either way content_item_id is null, so
  // log_o has nothing to push to Stash. A +/- counter (like the scene rows): the
  // count is the session's unlinked O's while tracking, or TODAY's session-less
  // unlinked O's when idle, so you can add and remove as many as you want.
  let sessionUnlinked = $state<OEvent[]>([]);   // active session's unlinked O's
  let sessionlessUnlinked = $state<OEvent[]>([]); // today's session-less unlinked O's
  const unlinkedList = $derived(active ? sessionUnlinked : sessionlessUnlinked);
  const unlinkedCount = $derived(unlinkedList.length);
  async function addUnlinked() {
    try {
      await api.oLog({ sessionId: active?.id ?? null });
    } catch (e) {
      console.error("log unlinked cumshot failed", e);
    }
    await refresh();
  }
  async function removeUnlinked() {
    const last = unlinkedList[unlinkedList.length - 1];
    if (!last) return;
    try {
      await api.oDelete(last.id);
    } catch (e) {
      console.error("remove unlinked cumshot failed", e);
    }
    await refresh();
  }

  // The unlinked-cumshot control lives in a header popover (not inline) so it
  // never crowds the idle screen. Available in both states; the count is scoped
  // to the session while tracking, else today's session-less ones.
  let unlinkedPopoverOpen = $state(false);
  let quicklogWrapEl = $state<HTMLElement | undefined>(undefined);
  function toggleUnlinkedPopover() { unlinkedPopoverOpen = !unlinkedPopoverOpen; }
  $effect(() => {
    if (!unlinkedPopoverOpen) return;
    const onDown = (e: PointerEvent) => {
      if (quicklogWrapEl && !quicklogWrapEl.contains(e.target as Node)) unlinkedPopoverOpen = false;
    };
    const onKey = (e: KeyboardEvent) => { if (e.key === "Escape") unlinkedPopoverOpen = false; };
    document.addEventListener("pointerdown", onDown);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("pointerdown", onDown);
      document.removeEventListener("keydown", onKey);
    };
  });

  function cumshotModalClose() {
    oPromptOpen = false;
    if (oPromptQuitMode) {
      // "Go back" from the quit wrap-up: abandon the quit, keep the session.
      oPromptQuitMode = false;
      api.cancelQuit().catch((e) => console.error("cancel quit failed", e));
    }
  }
  async function cumshotModalCommitted() {
    oPromptOpen = false;
    if (oPromptQuitMode) {
      // "Save and close": commit() already stopped the session; now exit.
      oPromptQuitMode = false;
      try { await api.confirmQuit(); } catch (e) { console.error("confirm quit failed", e); }
      return;
    }
    await refresh();
  }

  async function idleResolved() {
    idlePayload = null;
    await refresh();
  }

  async function crashResolved() {
    crashPayload = null;
    await refresh();
  }

  // Quit guard. The tray Quit handler (with a session running) shows the tracker
  // + emits `quit_requested`; we confirm first. "Wrap up & close" runs the
  // wrap-up modal in quit mode (stops the session, then exits); "Go back"
  // abandons the quit and the session keeps running.
  function onQuitRequested() {
    quitConfirmOpen = true;
  }
  async function quitGoBack() {
    quitConfirmOpen = false;
    try { await api.cancelQuit(); } catch (e) { console.error("cancel quit failed", e); }
  }
  async function quitWrapUp() {
    quitConfirmOpen = false;
    if (active && (await needsWrapUp())) {
      oPromptSessionId = active.id;
      oPromptScenes = scenes;
      oPromptQuitMode = true;
      oPromptOpen = true;
    } else {
      // Nothing to wrap up - stop (auto-discards the empty session) and exit.
      try {
        await api.sessionStop();
        await api.confirmQuit();
      } catch (e) { console.error("quit stop/exit failed", e); }
    }
  }
  // Close the app WITHOUT ending the session. The session owner is the backend,
  // not this window: in client mode it keeps running on the server; on the
  // in-process default it's left active and crash recovery resumes / tidies it on
  // next launch. Either way the session isn't lost - confirmQuit just exits.
  async function quitCloseAnyway() {
    quitConfirmOpen = false;
    try { await api.confirmQuit(); } catch (e) { console.error("quit (keep session) failed", e); }
  }

  async function openDashboard() {
    try {
      await invoke("open_dashboard");
    } catch (e) {
      console.error("failed to open dashboard", e);
    }
  }

  // Open a scene's page in the dashboard (clicking a scene card's thumb/title).
  // The dashboard is a separate, persistent window, so emit a cross-window event
  // it listens for, then bring it to front. Same destination as SessionDetail's
  // scene cards — nav.goto("scene", id) over there.
  async function openSceneInDashboard(contentItemId: number) {
    try {
      await emit("open-scene-in-dashboard", { contentItemId });
      await invoke("open_dashboard");
    } catch (e) {
      console.error("open scene in dashboard failed", e);
    }
  }

  // Poll + 1s ticker are throttled while the tracker window is hidden (WebView2
  // backgrounds timers), so the "currently watching" list can read stale the
  // instant the window is shown. Refresh + re-anchor `now` on becoming visible.
  function onVisible() {
    if (document.visibilityState === "visible") {
      now = Date.now();
      refresh();
      // The backend SHOWS the tracker to surface a crash-recovery prompt (boot
      // detection) or a quit prompt (tray Quit). If the window was already
      // mounted when that happened, onMount's one-time check won't re-run — so
      // re-check on show. Both clear server-side once resolved, so this is
      // idempotent and cheap.
      api.crashPendingGet().then((p) => { if (p && !crashPayload) crashPayload = p; }).catch(() => {});
      api.quitPendingGet().then((q) => { if (q) quitConfirmOpen = true; }).catch(() => {});
    }
  }

  // Frameless window has no OS close button — hide via this (tray/hotkey reopen).
  async function hideTracker() {
    try {
      await getCurrentWindow().hide();
    } catch (e) {
      console.error("hide tracker failed", e);
    }
  }

  // Pin the window to the PRIMARY screen's bottom-right (toast placement).
  // Works in physical px against the monitor's own origin, so it's correct
  // across DPI scaling and multi-monitor. primaryMonitor() is more reliable
  // than currentMonitor() for a window that was just shown.
  async function positionToastBottomRight() {
    try {
      const win = getCurrentWindow();
      const mon = (await primaryMonitor()) ?? (await currentMonitor());
      if (!mon) return;
      const sf = mon.scaleFactor || 1;
      const outer = await win.outerSize(); // physical px, includes title bar
      const margin = Math.round(16 * sf);
      const taskbar = Math.round(48 * sf);
      const x = mon.position.x + mon.size.width - outer.width - margin;
      const y = mon.position.y + mon.size.height - outer.height - taskbar;
      await win.setPosition(
        new PhysicalPosition(Math.max(mon.position.x, x), Math.max(mon.position.y, y)),
      );
    } catch (e) {
      console.error("toast positioning failed", e);
    }
  }

  // Accept the capture prompt: backend starts a session backdated to when
  // watching began, then we refresh so the tracker shows the running session.
  async function trackFromPrompt() {
    if (captureBusy) return;
    captureBusy = true;
    try {
      await api.captureAccept();
      capturePayload = null;
      await refresh();
    } catch (e) {
      console.error("capture accept failed", e);
    } finally {
      captureBusy = false;
    }
  }

  // Snooze/dismiss: backend suppresses for the grace period; hide the toast.
  async function snoozePrompt() {
    try {
      await api.captureSnooze();
    } catch (e) {
      console.error("capture snooze failed", e);
    }
    capturePayload = null;
    try {
      await getCurrentWindow().hide();
    } catch {
      /* ignore */
    }
  }

  onMount(async () => {
    // Pure web client (Phase 5): the tracker is a native window widget - window
    // sizing, always-on-top, idle/crash/capture prompts - none of which exist in
    // a browser. The web access point is the dashboard; send the browser there
    // before any Tauri call fires. (Session control from a browser is the bridge
    // pill, per the deployment-tiers design.)
    if (!isTauri()) {
      await goto("/dashboard", { replaceState: true });
      return;
    }
    // Reveal the body (hidden in app.html until now) once fonts are ready and
    // after the next paint, so the cold-start window shows only the dark
    // background, never unstyled / wrong-font content.
    {
      const reveal = () => { document.body.style.visibility = "visible"; };
      (document.fonts?.ready ?? Promise.resolve()).then(() => requestAnimationFrame(reveal));
    }
    refresh();
    timer = setInterval(() => { now = Date.now(); }, 1000);
    poll = setInterval(refresh, 2000);
    document.addEventListener("visibilitychange", onVisible);
    // Pick up an idle prompt that fired before we mounted (e.g. presence
    // detected idle while the tracker window was closed).
    try {
      const pending = await api.idlePendingGet();
      if (pending) idlePayload = pending;
    } catch (e) {
      console.error("idlePendingGet failed", e);
    }
    try {
      unlistenIdle = await listen<IdlePayload>("idle_detected", (event) => {
        idlePayload = event.payload;
      });
    } catch (e) {
      console.error("failed to subscribe to idle_detected", e);
    }
    // Pick up a capture prompt that fired before we mounted, then subscribe.
    try {
      const pendingCap = await api.capturePendingGet();
      if (pendingCap) capturePayload = pendingCap;
    } catch (e) {
      console.error("capturePendingGet failed", e);
    }
    try {
      unlistenCapture = await listen<CapturePayload>("capture_prompt", (event) => {
        capturePayload = event.payload;
      });
    } catch (e) {
      console.error("failed to subscribe to capture_prompt", e);
    }
    // Stop request from the Stash navbar indicator (clicked while tracking).
    // Refresh first so stop() sees the current scenes, then run the same flow
    // as the Stop button (opens the wrap-up modal; "go back" keeps it running).
    try {
      unlistenStopReq = await listen("tracker_stop_requested", async () => {
        await refresh();
        stop();
      });
    } catch (e) {
      console.error("failed to subscribe to tracker_stop_requested", e);
    }
    // In client mode (connected to an external server) the server pushes its
    // UiSignals over a WebSocket instead of the Tauri event bus; this channel
    // re-emits them as the local events the listeners above already handle. A
    // no-op on the in-process default (the Rust reactor handles UiSignals there).
    stopServerEvents = startServerEvents();
    // Quit guard: pick up a quit requested before we mounted, then subscribe.
    try {
      if (await api.quitPendingGet()) quitConfirmOpen = true;
    } catch (e) {
      console.error("quitPendingGet failed", e);
    }
    try {
      unlistenQuit = await listen("quit_requested", () => { onQuitRequested(); });
    } catch (e) {
      console.error("failed to subscribe to quit_requested", e);
    }
    // Crash recovery: a session left running from a previous run, stashed on
    // boot. Shown via IdleReturnModal's "crash" variant.
    try {
      const pendingCrash = await api.crashPendingGet();
      if (pendingCrash) crashPayload = pendingCrash;
    } catch (e) {
      console.error("crashPendingGet failed", e);
    }
    // Frameless window: center it on first load for normal use (a capture toast
    // repositions itself bottom-right, so don't fight it).
    if (!capturePayload) {
      try { await getCurrentWindow().center(); } catch { /* ignore */ }
    }
    // Auto-fit the window to its content. Observing the content host
    // catches both scene-count changes (most common) and post-fetch
    // metadata layout shifts (performer/studio pills landing).
    if (trackerContentEl) {
      trackerResizeObs = new ResizeObserver(() => { syncTrackerSize(); });
      trackerResizeObs.observe(trackerContentEl);
      // First measurement after the initial render lands.
      requestAnimationFrame(() => syncTrackerSize());
    }
  });

  // The word used for an O (Settings -> General -> Wording). start() loads it
  // and keeps it in step with the dashboard window, where it is changed.
  onMount(() => wording.start());
  // Freeze animations while this window is minimised or hidden in the tray.
  onMount(() => pauseAnimationsWhenHidden());

  onDestroy(() => {
    if (timer) clearInterval(timer);
    if (poll) clearInterval(poll);
    document.removeEventListener("visibilitychange", onVisible);
    if (unlistenIdle) { unlistenIdle(); unlistenIdle = null; }
    if (unlistenCapture) { unlistenCapture(); unlistenCapture = null; }
    if (unlistenStopReq) { unlistenStopReq(); unlistenStopReq = null; }
    if (unlistenQuit) { unlistenQuit(); unlistenQuit = null; }
    if (stopServerEvents) { stopServerEvents(); stopServerEvents = null; }
    if (trackerResizeObs) { trackerResizeObs.disconnect(); trackerResizeObs = null; }
  });
</script>

<svelte:head>
  <title>Climax</title>
</svelte:head>

<div class="tracker">
  <ProfileBadge />
  <header bind:this={trackerHeaderEl} data-tauri-drag-region>
    <div class="brand" data-tauri-drag-region>
      <img class="brand-mark" src="/climax-icon.png" alt="Climax" draggable="false" width="22" height="22" />
      <span class="title">Climax</span>
      <div class="dot" class:active={active && active.status === "active"} use:pulse={active?.status === "active"} class:paused={active && active.status === "paused"}></div>
      <ConnectionBadge />
    </div>
    {#if !toastMode}
      <div class="header-actions">
        <div class="quicklog-wrap" bind:this={quicklogWrapEl}>
          <button
            class="win-close quicklog-btn"
            class:on={unlinkedPopoverOpen}
            onclick={toggleUnlinkedPopover}
            title={w("Log a cumshot with no scene")}
            aria-label={w("Log a cumshot with no scene")}
          >
            <Icon name="cumshot" size={15} filled color="var(--accent)" />
            {#if unlinkedCount > 0}<span class="ql-badge">{unlinkedCount}</span>{/if}
          </button>
          {#if unlinkedPopoverOpen}
            <!-- svelte-ignore a11y_no_static_element_interactions -->
            <div class="quicklog-popover" onmousedown={(e) => e.stopPropagation()}>
              <div class="ql-head">
                <span class="ql-title">{w("Cumshots with no scene")}</span>
                <span class="ql-scope">{active ? "this session" : "today"}</span>
              </div>
              <div class="ql-stepper">
                <button
                  class="cumshot-mini minus"
                  disabled={unlinkedCount === 0}
                  onclick={removeUnlinked}
                  title="Remove the last one"
                  aria-label="Remove the last one"
                >
                  <Icon name="minus" size={14} />
                </button>
                <span class="unlinked-count">{unlinkedCount}</span>
                <button
                  class="cumshot-mini"
                  onclick={addUnlinked}
                  title={w("Log a cumshot not tied to any scene")}
                  aria-label={w("Log a cumshot not tied to any scene")}
                >
                  <Icon name="cumshot" size={16} color="var(--accent)" filled />
                </button>
              </div>
              <div class="ql-note">These have no scene attached, so they are never sent to Stash.</div>
            </div>
          {/if}
        </div>
        <button class="dashboard-link" onclick={openDashboard} title="Open the dashboard">
          Dashboard →
        </button>
        <button class="win-close" onclick={hideTracker} title="Close this window. Climax stays open in your tray." aria-label="Close this window. Climax stays open in your tray.">
          <Icon name="x" size={14} />
        </button>
      </div>
    {/if}
  </header>

  <main class:idle={!active && !toastMode}>
    <!-- Content host: its offsetHeight drives the window auto-resize via
         the ResizeObserver in onMount. Don't add padding here — that
         lives on .main and is added back in JS as MAIN_PADDING_Y. -->
    <div class="content-host" bind:this={trackerContentEl}>
    <!-- The bridge can't reach a backend that never bound its port, so the
         tracker would otherwise just sit here looking idle forever. Settings
         live in the dashboard, hence the handoff. Never in toast mode - that's
         the small capture window. -->
    {#if !toastMode}
      <ServerPortBanner onFix={openDashboard} fixLabel="Open the dashboard" />
    {/if}
    {#if toastMode && capturePayload}
      <CapturePromptToast
        payload={capturePayload}
        busy={captureBusy}
        onTrack={trackFromPrompt}
        onSnooze={snoozePrompt}
      />
    {:else if !active}
      <div class="empty-state">
        <div class="start-wrap">
          <div class="empty-hint">no session running</div>
          <button class="primary big" onclick={start}>Start session</button>
        </div>
      </div>
    {:else}
      <div class="active-state">
        <div class="timer-block">
          <div class="timer">{formatDuration(livenessElapsed)}</div>
          <div class="status status-{active.status}">
            {active.status === "paused" ? "Paused" : "Tracking"}
          </div>
        </div>

        <div class="controls">
          <button class="danger" onclick={stop}>Stop</button>
          <button class="secondary" onclick={pauseOrResume}>
            {active.status === "paused" ? "Resume" : "Pause"}
          </button>
        </div>

        <div class="quick-stats">
          <span class="qs"><span class="qs-num qs-live">{playingCount}</span> scene{playingCount === 1 ? "" : "s"} playing</span>
          <span class="qs"><span class="qs-num">{active.scene_count}</span> scene{active.scene_count === 1 ? "" : "s"} watched</span>
          <span class="qs"><span class="qs-num qs-accent">{active.o_count}</span> {w(active.o_count === 1 ? "cumshot" : "cumshots")}</span>
        </div>

        {#if scenesVisible.length > 0}
          <ul class="scenes">
            {#each scenesVisible as scene (scene.play_id)}
              {@const removable = scene.cumshot_count}
              <li class="scene-row">
                <div class="scene-card-wrap">
                  <SceneCard scene={scene} compact={true} onSelect={() => openSceneInDashboard(scene.content_item_id)} />
                </div>
                <div class="scene-actions">
                  {#if removable > 0}
                    <button class="cumshot-mini minus" onclick={() => removeCumshot(scene)} title={w("Remove the most recent cumshot on this scene")} aria-label={w("Remove cumshot")}>
                      <Icon name="minus" size={14} />
                    </button>
                  {/if}
                  <button class="cumshot-mini" onclick={() => logCumshot(scene)} title={w("Log a cumshot on this scene")} aria-label={w("Log cumshot")}>
                    <Icon name="cumshot" size={16} color="var(--accent)" filled />
                  </button>
                </div>
              </li>
            {/each}
          </ul>
        {/if}
      </div>
    {/if}
    </div>
  </main>
</div>

{#if oPromptOpen && oPromptSessionId !== null && active}
  <OPromptModal
    sessionId={oPromptSessionId}
    scenes={oPromptScenes}
    session={active!}
    quitMode={oPromptQuitMode}
    onClose={cumshotModalClose}
    onCommitted={cumshotModalCommitted}
  />
{/if}

{#if idlePayload}
  <IdleReturnModal
    sessionId={idlePayload.session_id}
    gapStartedAt={idlePayload.idle_started_at}
    variant="idle"
    onResolved={idleResolved}
  />
{/if}

{#if crashPayload}
  <IdleReturnModal
    sessionId={crashPayload.session_id}
    gapStartedAt={crashPayload.gap_started_at}
    variant="crash"
    onResolved={crashResolved}
  />
{/if}

{#if quitConfirmOpen}
  <ConfirmDialog
    title="Session running"
    message="A session is running. Wrap it up before closing, or close Climax without ending it - you can pick it up next time."
    confirmLabel="Wrap up and close"
    cancelLabel="Go back"
    extraLabel="Close without ending"
    onConfirm={quitWrapUp}
    onCancel={quitGoBack}
    onExtra={quitCloseAnyway}
  />
{/if}

<style>
  /* Geist + Geist Mono are bundled locally (see app.html /fonts.css). */

  :global(:root) {
    --ink-950: #0A0B0F;
    --ink-900: #101218;
    --ink-850: #15171E;
    --ink-800: #1B1E26;
    --ink-700: #252934;
    --ink-600: #353A47;
    --ink-500: #5C6273;
    --ink-400: #8A909E;
    --ink-100: #ECEEF3;
    --ink-050: #F7F8FA;

    --coral-500: #EF6B7A;
    --coral-400: #F58895;
    --coral-600: #D9485A;
    --bone-400:  #F2E8D4;

    --green-500: #4ADE80;
    --amber-500: #FBB454;
    --red-500:   #F26B6B;
    --red-400:   #F58D8D;

    --coral-a-08: rgba(239, 107, 122, 0.08);
    --coral-a-16: rgba(239, 107, 122, 0.16);
    --coral-a-24: rgba(239, 107, 122, 0.24);

    --bg:             var(--ink-950);
    --bg-elevated:    var(--ink-900);
    --bg-card:        var(--ink-850);
    --bg-card-hover:  var(--ink-800);
    --bg-sunken:      #07080B;

    --fg:             var(--ink-100);
    --fg-strong:      var(--ink-050);
    --fg-muted:       var(--ink-400);
    --fg-subtle:      var(--ink-500);

    --border:         var(--ink-700);
    --border-strong:  var(--ink-600);
    --border-subtle:  #1C1F26;

    --accent:         var(--coral-500);
    --accent-hover:   var(--coral-400);
    --accent-soft:    var(--coral-a-16);
    --accent-fg:      #FFFFFF;
    --highlight:      var(--bone-400);
    --live:           var(--green-500);
    --warn:           var(--amber-500);
    --danger:         var(--red-500);

    --font-sans:    "Geist", "Inter", system-ui, -apple-system, sans-serif;
    --font-display: "Geist", "Inter", system-ui, sans-serif;
    --font-mono:    "Geist Mono", "JetBrains Mono", "SF Mono", "Consolas", monospace;

    --ease-out: cubic-bezier(0.16, 1, 0.3, 1);
    --dur-1: 80ms;
    --dur-2: 120ms;
    --dur-3: 200ms;

    /* Legacy aliases (transition) */
    --text: var(--fg);
    --text-bright: var(--fg-strong);
    --text-muted: var(--fg-muted);
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
    overflow: hidden;
  }

  .tracker {
    display: flex;
    flex-direction: column;
    height: 100vh;
    box-sizing: border-box;
  }

  header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 10px 14px;
    border-bottom: 1px solid var(--border);
    flex-shrink: 0;
  }
  .brand {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .brand-mark {
    width: 22px;
    height: 22px;
    border-radius: 6px;
    flex-shrink: 0;
    display: block;
    object-fit: cover;
  }
  .title {
    font-size: 13px;
    font-weight: 600;
    letter-spacing: 0.4px;
  }
  .dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--text-muted);
    transition: background 200ms;
  }
  .dot.active {
    background: var(--green);
    position: relative;
  }
  /* The pulse is a copy of the dot that grows and fades. Its size and fade
     come from --cx-pulse, which src/lib/pulse.ts sets about 10 times a second
     (not 60 - see that file for the measurements), and only transform and
     opacity change, so nothing is repainted. Same look as the old box-shadow
     pulse: 7px dot + 6px halo = scale 19/7. */
  .dot.active::after {
    content: "";
    position: absolute;
    inset: 0;
    border-radius: 50%;
    background: inherit;
    pointer-events: none;
    will-change: transform, opacity;
    transform: scale(calc(1 + 1.714 * var(--cx-pulse, 0)));
    opacity: calc(0.33 * (1 - var(--cx-pulse, 0)));
  }
  .dot.paused { background: var(--amber); }
  .dashboard-link {
    background: transparent;
    border: 1px solid var(--border);
    color: var(--text-muted);
    padding: 4px 10px;
    border-radius: 6px;
    font-size: 11px;
    font-weight: 500;
    cursor: pointer;
    font-family: inherit;
    transition: background 120ms, border-color 120ms, color 120ms;
  }
  .dashboard-link:hover {
    background: var(--bg-card-hover);
    border-color: #3a3e4a;
    color: var(--text);
  }
  .header-actions {
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .win-close {
    width: 24px;
    height: 24px;
    padding: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    background: transparent;
    border: 1px solid var(--border);
    border-radius: 6px;
    color: var(--text-muted);
    cursor: pointer;
    font-family: inherit;
    transition: background 120ms, border-color 120ms, color 120ms;
  }
  .win-close:hover {
    background: var(--bg-card-hover);
    border-color: #3a3e4a;
    color: var(--text);
  }

  main {
    flex: 1;
    overflow-y: auto;
    padding: 14px 16px 16px;
    display: flex;
    flex-direction: column;
  }
  /* Spaced from here rather than inside a wrapper, so the healthy case (the
     banner renders nothing) adds no height to the auto-resized window. */
  .content-host > :global(.port-banner) {
    margin-bottom: 10px;
  }
  /* Idle only: vertically centre the sparse "no active session" content in the
     window. NOT applied while tracking - that content can exceed the window and
     must stay top-aligned + scrollable (centering would clip the top). */
  main.idle {
    justify-content: center;
  }

  .empty-state {
    display: flex;
    justify-content: center;
  }
  /* The button is the centred element (main.idle centres this box vertically,
     and content-host's height == the button since the hint is absolute). The
     hint floats just above the button, so it naturally sits above centre. */
  .start-wrap {
    position: relative;
    display: flex;
  }
  .empty-hint {
    position: absolute;
    bottom: calc(100% + 14px);
    left: 50%;
    transform: translateX(-50%);
    white-space: nowrap;
    font-size: 10px;
    color: var(--text-muted);
    text-transform: uppercase;
    letter-spacing: 0.15em;
  }

  .active-state {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  .timer-block {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 2px;
  }
  .timer {
    font-family: "JetBrains Mono", "SF Mono", "Consolas", monospace;
    font-size: 44px;
    font-weight: 600;
    line-height: 1;
    letter-spacing: -0.02em;
    font-variant-numeric: tabular-nums;
  }
  .status {
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.15em;
    color: var(--text-muted);
  }
  .status-paused { color: var(--amber); }
  .status-active { color: var(--green); }

  .controls {
    display: flex;
    gap: 6px;
    justify-content: center;
  }

  button {
    padding: 6px 16px;
    border-radius: 7px;
    font-size: 12px;
    font-weight: 500;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--text);
    cursor: pointer;
    transition: background 120ms, border-color 120ms;
    font-family: inherit;
  }
  button:hover:not(:disabled) { border-color: #3a3e4a; background: var(--bg-card-hover); }

  button.primary {
    background: var(--accent);
    border-color: var(--accent);
    color: #fff;
  }
  button.primary:hover { background: #ff7280; border-color: #ff7280; }
  button.primary.big {
    padding: 11.5px 25px;
    font-size: 15px;
  }
  button.danger {
    background: var(--red);
    border-color: var(--red);
    color: #fff;
  }
  button.danger:hover { background: #fa8585; border-color: #fa8585; }
  button.secondary {
    background: transparent;
    color: var(--text);
  }

  .quick-stats {
    display: flex;
    justify-content: center;
    gap: 16px;
    padding: 6px 0;
    border-top: 1px solid var(--border);
    border-bottom: 1px solid var(--border);
    font-size: 11px;
    color: var(--text-muted);
  }
  .qs {
    display: flex;
    align-items: baseline;
    gap: 4px;
  }
  .qs-num {
    font-weight: 600;
    color: var(--text);
    font-variant-numeric: tabular-nums;
    font-size: 13px;
  }
  .qs-num.qs-live { color: var(--live); }
  .qs-num.qs-accent { color: var(--accent); }

  /* Unlinked-cumshot quick-log: a header icon button + popover, reachable in both
     states without crowding the idle screen. Stepper buttons reuse .cumshot-mini. */
  .quicklog-wrap { position: relative; display: flex; }
  .quicklog-btn { position: relative; }
  .quicklog-btn.on { background: var(--accent-soft); border-color: var(--accent); }
  .ql-badge {
    position: absolute;
    top: -5px;
    right: -5px;
    min-width: 15px;
    height: 15px;
    padding: 0 3px;
    box-sizing: border-box;
    border-radius: 8px;
    background: var(--accent);
    color: #fff;
    font-size: 9px;
    font-weight: 700;
    line-height: 15px;
    text-align: center;
    font-variant-numeric: tabular-nums;
  }
  .quicklog-popover {
    position: absolute;
    top: calc(100% + 8px);
    right: 0;
    z-index: 50;
    display: flex;
    flex-direction: column;
    gap: 10px;
    width: max-content;
    padding: 12px 14px;
    background: var(--bg-card);
    border: 1px solid var(--border-strong);
    border-radius: 10px;
    box-shadow: 0 14px 36px rgba(0, 0, 0, 0.55);
    animation: ql-pop 140ms var(--ease-out);
  }
  @keyframes ql-pop {
    from { opacity: 0; transform: translateY(-4px); }
    to   { opacity: 1; transform: none; }
  }
  .ql-head {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 16px;
  }
  .ql-title { font-size: 12px; font-weight: 600; color: var(--text); }
  .ql-scope {
    font-size: 9px;
    color: var(--text-muted);
    text-transform: uppercase;
    letter-spacing: 0.12em;
  }
  .ql-stepper {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 10px;
  }
  .ql-note { font-size: 10px; color: var(--text-muted); text-align: center; }
  .unlinked-count {
    min-width: 18px;
    text-align: center;
    font-weight: 600;
    font-size: 14px;
    color: var(--accent);
    font-variant-numeric: tabular-nums;
  }

  .scenes {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
  }
  .scene-row {
    display: flex;
    align-items: stretch;
    justify-content: space-between;
    padding: 4px 0;
    gap: 6px;
    border-bottom: 1px solid var(--border);
  }
  .scene-row:last-child { border-bottom: none; }
  .scene-card-wrap {
    flex: 1;
    min-width: 0;
  }
  .scene-actions {
    display: flex;
    align-items: center;
    gap: 4px;
    flex-shrink: 0;
    padding-right: 2px;
  }
  .cumshot-mini {
    width: 32px;
    height: 32px;
    padding: 0;
    background: var(--bg-card-hover);
    border: 1px solid var(--border);
    border-radius: 8px;
    font-size: 18px;
    cursor: pointer;
    line-height: 1;
    transition: background 120ms, border-color 120ms, transform 80ms, color 120ms;
    font-family: inherit;
    flex-shrink: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--text);
  }
  .cumshot-mini:hover {
    background: var(--accent-soft);
    border-color: var(--accent);
  }
  .cumshot-mini:active {
    transform: scale(0.9);
  }
  .cumshot-mini.minus {
    font-size: 18px;
    color: var(--text-muted);
    font-weight: 600;
  }
  .cumshot-mini.minus:hover {
    background: #f8717118;
    border-color: var(--red);
    color: var(--red);
  }
</style>
