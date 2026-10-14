<!--
  App settings — a paged window (left rail + content pane). Opened from the gear
  button on the dashboard.

  Pages (rail order, user-chosen):
   - General — startup / launch behavior / global hotkey (applied immediately)
   - Untracked sessions — review/import sessions reconstructed from Stash history
     (embeds UntrackedSessions.svelte; rail shows a count badge)
   - Sync — Library metadata · Play history · Play-count threshold
   - Smart tracking — Idle detection (+ future passive-capture prompt)
   - Stash configuration — URL · API key · Test
   - Backup and restore — back up / restore the Climax DB (embeds BackupRestore.svelte)

  The config pages share one Save (persists every section together). The Untracked
  page has its own Apply (immediate), so the Save footer is hidden there.
-->
<script lang="ts">
  import { w, wording, type Wording } from "$lib/wording.svelte";
  import { onMount, onDestroy } from "svelte";
  import {
    api,
    formatAgo,
    dayKey,
    today,
    type IdleDetectionSetting,
    type RemoteTrackingSetting,
    type CapturePromptSetting,
    type StashConnectionSetting,
    type PlayCountingSetting,
    type MirrorStatus,
    type TestConnectionResult,
    type ServerPortInfo,
  } from "$lib/api";
  import UntrackedSessions from "./UntrackedSessions.svelte";
  import BackupRestore from "./BackupRestore.svelte";
  import Icon from "./Icon.svelte";
  import { serverConfig } from "$lib/server-config.svelte";
  import { updateCheck } from "$lib/update-check.svelte";
  import { openUrl } from "@tauri-apps/plugin-opener";
  import { isTauri } from "$lib/transport";
  import Select from "$lib/Select.svelte";

  type Page = "untracked" | "sync" | "smart" | "stash" | "general" | "server" | "backup" | "version" | "donate";
  const DONATE_URL = "https://buymeacoffee.com/pineapplestorm";
  // Public release home. The DEV repo is pineapplestorm/climax-dev; this name is
  // reserved for the public release, so the link resolves once that repo exists.
  const APP_DOWNLOAD_URL = "https://github.com/pineapplestorm/climax/releases";
  // The project's home page - the README, not the release list. Both 404 until
  // the public repo exists, which is expected before the first release.
  const APP_REPO_URL = "https://github.com/pineapplestorm/climax";
  // Pure browser web client (Phase 5): no tray/hotkey/autostart, no separate
  // windows, and no client-mode switching (the browser is already talking to
  // whichever server served it) - those controls hide below.
  const webClient = !isTauri();
  // The accelerator is registered as "CommandOrControl+Shift+L", so the key you
  // actually press differs by platform. Two visible strings named Ctrl outright,
  // which is simply wrong on a Mac - confirmed by testing that Cmd+Shift+L is
  // what fires. Name the real key rather than hardcoding either one.
  const shortcutLabel =
    typeof navigator !== "undefined" && navigator.userAgent.includes("Macintosh")
      ? "Cmd+Shift+L"
      : "Ctrl+Shift+L";
  // Open an external link: the OS browser via the opener plugin on desktop, a
  // plain new tab in the web client (the plugin needs the Tauri runtime).
  function openExternal(url: string) {
    if (webClient) window.open(url, "_blank", "noopener");
    else openUrl(url);
  }
  type Props = {
    onClose: () => void;
    /** Re-run the first-launch setup wizard (the dashboard hosts it). */
    onRerunOnboarding?: () => void;
    /** Page to open on. Omit for the usual landing page. */
    initialPage?: Page;
  };
  let { onClose, onRerunOnboarding, initialPage }: Props = $props();

  const RANGE_START = "2000-01-01";
  const PAGE_TITLES: Record<Page, string> = {
    untracked: "Untracked sessions",
    sync: "Sync",
    smart: "Smart tracking",
    stash: "Stash configuration",
    general: "General",
    server: "Server / connection",
    backup: "Backup and restore",
    version: "Version and updates",
    donate: "Donate",
  };
  // `initialPage` only seeds the landing page - the rail owns it from then on,
  // so a caller deep-linking here can't fight the user's own navigation. Reading
  // it once is the point, hence the ignore: the modal is recreated per open
  // (`{#if settingsOpen}`), so a later prop change has no one to surprise.
  // svelte-ignore state_referenced_locally
  let activePage = $state<Page>(initialPage ?? "general");
  let untrackedCount = $state(0);

  // ---------- Load state ----------
  let loaded = $state(false);
  let saving = $state(false);
  let saved = $state(false);
  let errorMsg = $state<string | null>(null);

  // In client mode every data command travels to the external Climax server,
  // so a rejected command here is usually that server being unreachable -
  // point the user there. In-process mode gets no suffix.
  const serverHint = () =>
    serverConfig.url ? " Check the connection to your Climax server." : "";

  // "Re-run setup" (General page): reopen the wizard, which the dashboard hosts.
  //
  // Deliberately does NOT clear `onboarding_completed` first. It used to, which
  // meant backing out of a re-run left setup marked incomplete and the wizard
  // fired on every launch until you walked it to the end. The wizard renders
  // from a flag here, not from that setting, so clearing it bought nothing. The
  // setting only ever needs to go true, which `finish()` does; abandoning a
  // re-run now leaves it exactly as it was, and so does a crash mid-wizard.
  function rerunSetup() {
    onRerunOnboarding?.();
  }

  // Idle detection form.
  let idleEnabled = $state(true);
  let idleThresholdMinutes = $state(10);

  // Remote-device tracking form (live Stash play_duration poll).
  let remoteEnabled = $state(true);
  let remotePollSeconds = $state(15);

  // Passive-capture prompt form (remind me to track a session).
  let captureEnabled = $state(false);
  let captureThresholdMinutes = $state(3);
  let captureSnoozeMinutes = $state(30);

  // Wording (General page). Applies the moment it is picked, like the rest of
  // that page; the store updates every open view and the backend.
  let wordingErr = $state<string | null>(null);
  async function pickWording(v: Wording) {
    if (wording.value === v) return;
    wordingErr = null;
    try {
      await wording.set(v);
    } catch (e) {
      console.error("set wording failed", e);
      wordingErr = "Couldn't save that." + serverHint();
    }
  }

  // Keep empty sessions (stop keeps a session with nothing logged in it).
  // `keepEmptyLoaded` is what the backend last reported, so Save only writes it
  // when it actually changed - a desktop client pointed at an older server that
  // has never heard of the setting can still save everything else.
  let keepEmptySessions = $state(false);
  let keepEmptyLoaded = false;

  // Stash connection form.
  let stashUrl = $state("http://localhost:9999");
  let stashApiKey = $state("");
  let showApiKey = $state(false);

  // Play counting threshold. "" = clear the local override (re-sync from Stash).
  let playThresholdInput = $state<string | number>("");
  let playThresholdServerNull = $state(true);
  let syncingPlayThreshold = $state(false);
  let playThresholdSyncError = $state<string | null>(null);
  let playThreshSyncedMsg = $state<string | null>(null);

  // Test connection state.
  let testing = $state(false);
  let testResult = $state<TestConnectionResult | null>(null);

  // Play-history mirror. Cadence shown in HOURS; backend stores minutes.
  let mirrorEnabled = $state(true);
  let mirrorCadenceHours = $state(24);
  let mirrorBusy = $state(false);
  let mirrorStatus = $state<MirrorStatus | null>(null);
  let mirrorPoll: ReturnType<typeof setInterval> | null = null;
  const mirrorActive = (p: string | undefined) => p === "syncing";

  // Library-metadata sync. Cadence in DAYS.
  let metadataEnabled = $state(true);
  let metadataCadenceDays = $state(7);
  let metadataBusy = $state(false);
  let metadataStatus = $state<MirrorStatus | null>(null);
  let metadataPoll: ReturnType<typeof setInterval> | null = null;
  const metadataActive = (p: string | undefined) => p === "syncing";

  // ---- General page (startup / shortcut). Applied immediately on change. ----
  let openAtLogin = $state(false);
  let launchBehavior = $state<"silent" | "tracker" | "dashboard">("tracker");
  let hotkeyEnabled = $state(true);

  async function setOpenAtLogin(v: boolean) {
    openAtLogin = v;
    try { await api.autostartSet(v); } catch (e) { console.error("autostart set failed", e); openAtLogin = !v; }
  }
  async function setLaunchBehavior(v: "silent" | "tracker" | "dashboard") {
    launchBehavior = v;
    try { await api.launchBehaviorSet(v); } catch (e) { console.error("launch behavior set failed", e); }
  }
  async function setHotkeyEnabled(v: boolean) {
    hotkeyEnabled = v;
    try { await api.hotkeyEnabledSet(v); } catch (e) { console.error("hotkey toggle failed", e); hotkeyEnabled = !v; }
  }

  // ---- Server / connection page (app-as-client). Client-LOCAL (localStorage),
  // applied on demand + a reload so every view re-fetches from the chosen backend.
  let serverEnabled = $state(serverConfig.enabled);
  let serverUrlInput = $state(serverConfig.url ?? "http://localhost:9998");
  let serverTokenInput = $state(serverConfig.token ?? "");
  let serverTest = $state<"idle" | "testing" | "ok" | "fail">("idle");
  let serverApplying = $state(false);
  // Dev only: auto-restart would orphan Vite, so we ask for a manual restart.
  let serverDevRestartHint = $state(false);
  // Normalised target vs the active config -> whether Apply does anything.
  const serverTarget = $derived(
    serverEnabled && serverUrlInput.trim()
      ? serverUrlInput.trim().replace(/\/+$/, "")
      : null,
  );
  const serverTokenTarget = $derived(
    serverTarget && serverTokenInput.trim() ? serverTokenInput.trim() : null,
  );
  const serverDirty = $derived(
    serverTarget !== serverConfig.url || serverTokenTarget !== serverConfig.token,
  );

  async function testServer() {
    serverTest = "testing";
    try {
      serverTest = (await api.serverPing(serverUrlInput.trim())) ? "ok" : "fail";
    } catch {
      serverTest = "fail";
    }
  }
  async function applyServer() {
    serverApplying = true;
    serverDevRestartHint = false;
    // localStorage + transport + mirror to client.json (incl. the token)
    serverConfig.set(serverTarget, serverTokenTarget);
    // RESTART (not just reload) so the Rust boot re-reads client.json: switching
    // between the in-process backend and thin-client mode is a boot-path decision a
    // webview reload can't make. Ensure the config is on disk first.
    try {
      await api.clientConfigSet(serverTarget, serverTokenTarget);
      const restarted = await api.restartApp();
      if (!restarted) {
        // Dev build: auto-restart would orphan Vite. The config is written; a full
        // manual restart (re-run the dev app) applies the thin-client boot.
        serverApplying = false;
        serverDevRestartHint = true;
      }
    } catch (e) {
      console.error("apply server config failed; falling back to reload", e);
      location.reload();
    }
  }

  // ---- Version and updates. State lives in the shared store, so dismissing the
  // dashboard notice and checking from here stay in agreement without a reload.
  let updateErr = $state<string | null>(null);
  /** A check that FAILED, from this session's button or from whatever the
   *  backend recorded last. Without the second half a failed boot check reads as
   *  "None published yet" - an assertion about the world that is false and that
   *  this screen gives no way to doubt. */
  const updateProblem = $derived(updateErr ?? updateCheck.info?.last_error ?? null);
  /** Short-month + day + two-digit year, the app's date convention. */
  const releaseDate = (ms: number | null): string | null =>
    ms === null
      ? null
      : new Date(ms).toLocaleDateString([], { day: "numeric", month: "short", year: "2-digit" });

  /** How long ago the check ran. `formatAgo` would say "0m ago" for one that
   *  just finished, which reads as broken rather than fresh - and "just now" is
   *  what makes this line double as the button's confirmation. */
  const checkedAgo = (ms: number): string =>
    Date.now() - ms < 60_000 ? "just now" : formatAgo(ms);
  /** Null only before the first check has ever completed. */
  const lastChecked = $derived(
    updateCheck.info?.checked_at ? checkedAgo(updateCheck.info.checked_at) : null,
  );

  async function checkForUpdates() {
    updateErr = await updateCheck.checkNow();
  }

  // ---- Port for the built-in backend. Only meaningful in HOST mode: in client
  // mode an external server owns the port and this app binds nothing at all.
  // Loaded on its own rather than in the main settings fetch below, because this
  // page deliberately sits OUTSIDE the `loaded` gate - it has to stay usable when
  // a configured server is unreachable, and `server_port_get` is a native command
  // that answers either way.
  const hostMode = $derived(!webClient && serverConfig.url === null);
  let portInfo = $state<ServerPortInfo | null>(null);
  let portInput = $state("");
  let portErr = $state<string | null>(null);
  let portApplying = $state(false);
  let portDevRestartHint = $state(false);
  const portDirty = $derived(
    portInfo !== null &&
      // Capped at five digits so an absurd number reaches the backend as a
      // number rather than as 1e+21, which would bypass its bounds message.
      /^\d{1,5}$/.test(portInput.trim()) &&
      Number(portInput.trim()) !== portInfo.port,
  );

  async function applyPort() {
    portApplying = true;
    portErr = null;
    portDevRestartHint = false;
    try {
      // Persist first: the port is read at boot, so it has to be on disk before
      // the restart that picks it up.
      await api.serverPortSet(Number(portInput.trim()));
      const restarted = await api.restartApp();
      if (!restarted) {
        // Dev build: auto-restart would orphan Vite, same as applying a server.
        portApplying = false;
        portDevRestartHint = true;
      }
    } catch (e) {
      console.error("set port failed", e);
      const s = String(e).trim();
      const looksDesigned =
        /^[A-Z]/.test(s) && s.endsWith(".") && !s.includes(": ") && s.length <= 200;
      portErr = looksDesigned ? s : "Couldn't save the port. Try again.";
      portApplying = false;
    }
  }

  onMount(async () => {
    if (!webClient) {
      try {
        portInfo = await api.serverPortGet();
        portInput = String(portInfo.port);
      } catch (e) {
        console.error("read port failed", e);
      }
    }
    // Cached read, no network. Also refreshes what the dashboard notice loaded
    // earlier, so a dismissal made there shows correctly here.
    void updateCheck.load();
    try {
      const [idle, remote, capture, stash, play, mirror, mstatus, meta, metaStatus, autostart, launchBeh, hotkeyEn, keepEmpty] = await Promise.all([
        api.idleDetectionGet(),
        api.remoteTrackingGet(),
        api.capturePromptGet(),
        api.stashConnectionGet(),
        api.playCountingGet(),
        api.mirrorSyncGet(),
        api.mirrorStatusGet(),
        api.metadataRefreshGet(),
        api.metadataStatusGet(),
        api.autostartGet().catch(() => false),
        api.launchBehaviorGet().catch(() => "tracker"),
        api.hotkeyEnabledGet().catch(() => true),
        api.keepEmptySessionsGet().catch(() => false),
      ]);
      keepEmptySessions = keepEmpty;
      keepEmptyLoaded = keepEmpty;
      idleEnabled = idle.enabled;
      idleThresholdMinutes = idle.threshold_minutes;
      remoteEnabled = remote.enabled;
      remotePollSeconds = remote.poll_seconds;
      captureEnabled = capture.enabled;
      captureThresholdMinutes = capture.threshold_minutes;
      captureSnoozeMinutes = capture.snooze_minutes;
      stashUrl = stash.url || "http://localhost:9999";
      stashApiKey = stash.api_key ?? "";
      openAtLogin = autostart;
      launchBehavior = launchBeh as "silent" | "tracker" | "dashboard";
      hotkeyEnabled = hotkeyEn;
      playThresholdServerNull = play.threshold_pct === null;
      playThresholdInput = play.threshold_pct === null ? "" : String(play.threshold_pct);
      mirrorEnabled = mirror.enabled;
      mirrorCadenceHours = Math.max(1, Math.round(mirror.cadence_minutes / 60));
      mirrorStatus = mstatus;
      if (mirrorActive(mstatus.phase)) {
        mirrorBusy = true;
        startMirrorPoll();
      }
      metadataEnabled = meta.enabled;
      metadataCadenceDays = Math.max(1, Math.round(meta.cadence_minutes / (60 * 24)));
      metadataStatus = metaStatus;
      if (metadataActive(metaStatus.phase)) {
        metadataBusy = true;
        startMetadataPoll();
      }
    } catch (e) {
      console.error("load settings failed", e);
      errorMsg = "Couldn't load settings." + serverHint();
    } finally {
      loaded = true;
    }
    void refreshCount();
  });

  onDestroy(() => {
    stopMirrorPoll();
    stopMetadataPoll();
  });

  // Untracked-sessions count for the rail badge (not-yet-removed candidates).
  async function refreshCount() {
    try {
      const c = await api.reconstructCandidates(RANGE_START, dayKey(today()), {
        respectDismissed: true,
      });
      untrackedCount = c.length;
    } catch {
      untrackedCount = 0;
    }
  }

  function startMirrorPoll() {
    if (mirrorPoll) return;
    mirrorPoll = setInterval(async () => {
      try {
        mirrorStatus = await api.mirrorStatusGet();
        if (!mirrorActive(mirrorStatus.phase)) {
          stopMirrorPoll();
          mirrorBusy = false;
        }
      } catch {
        /* transient; keep polling */
      }
    }, 1000);
  }
  function stopMirrorPoll() {
    if (mirrorPoll) {
      clearInterval(mirrorPoll);
      mirrorPoll = null;
    }
  }

  async function syncMirrorNow() {
    if (mirrorBusy) return;
    mirrorBusy = true;
    try {
      await persistStashConnection();
      await api.mirrorSyncNow();
      mirrorStatus = await api.mirrorStatusGet();
      startMirrorPoll();
    } catch (e) {
      console.error("play history sync failed", e);
      errorMsg = "Couldn't start the sync." + serverHint();
      mirrorBusy = false;
    }
  }

  function startMetadataPoll() {
    if (metadataPoll) return;
    metadataPoll = setInterval(async () => {
      try {
        metadataStatus = await api.metadataStatusGet();
        if (!metadataActive(metadataStatus.phase)) {
          stopMetadataPoll();
          metadataBusy = false;
        }
      } catch {
        /* transient; keep polling */
      }
    }, 1000);
  }
  function stopMetadataPoll() {
    if (metadataPoll) {
      clearInterval(metadataPoll);
      metadataPoll = null;
    }
  }

  async function syncMetadataNow() {
    if (metadataBusy) return;
    metadataBusy = true;
    try {
      await persistStashConnection();
      await api.metadataSyncNow();
      metadataStatus = await api.metadataStatusGet();
      startMetadataPoll();
    } catch (e) {
      console.error("metadata refresh failed", e);
      errorMsg = "Couldn't start the refresh." + serverHint();
      metadataBusy = false;
    }
  }

  async function persistStashConnection(): Promise<StashConnectionSetting> {
    const key = stashApiKey.trim();
    return await api.stashConnectionSet(stashUrl.trim(), key.length > 0 ? key : null);
  }

  async function testConnection() {
    if (testing) return;
    testing = true;
    testResult = null;
    try {
      await persistStashConnection();
      testResult = await api.stashConnectionTest();
    } catch (e) {
      console.error("stash connection test failed", e);
      testResult = { ok: false, version: null, scene_count: null, error: "Couldn't run the test." + serverHint() };
    } finally {
      testing = false;
    }
  }

  async function syncPlayThresholdFromStash() {
    if (syncingPlayThreshold) return;
    syncingPlayThreshold = true;
    playThresholdSyncError = null;
    playThreshSyncedMsg = null;
    try {
      await persistStashConnection();
      const synced = await api.playCountingSyncFromStash();
      playThresholdServerNull = synced.threshold_pct === null;
      playThresholdInput = synced.threshold_pct === null ? "" : String(synced.threshold_pct);
      if (synced.threshold_pct === null) {
        playThresholdSyncError =
          "Stash didn't return a value (history may be disabled). Using the 0% fallback.";
      } else {
        playThreshSyncedMsg = `Synced - Stash uses ${synced.threshold_pct}%`;
      }
    } catch (e) {
      console.error("play threshold sync failed", e);
      // The backend classifies Stash failures into plain sentences ("Stash
      // rejected the request (HTTP 401). ...", "Stash returned an error: ...");
      // show those as-is. Anything raw (a local DB failure, say) gets a
      // cause-neutral line - this catch funnels more than reachability.
      const s = String(e).trim();
      const looksDesigned =
        /^[A-Z]/.test(s) && s.endsWith(".") && !s.includes(": ") && s.length <= 200;
      playThresholdSyncError =
        looksDesigned || s.startsWith("Stash returned an error")
          ? s
          : "Couldn't sync the threshold from Stash. Try again.";
    } finally {
      syncingPlayThreshold = false;
    }
  }

  async function save() {
    if (saving) return;
    saving = true;
    saved = false;
    errorMsg = null;
    try {
      const minutes = Math.max(1, Math.floor(idleThresholdMinutes || 1));
      const _idle: IdleDetectionSetting = await api.idleDetectionSet(idleEnabled, minutes);

      const pollSecs = Math.max(5, Math.min(60, Math.floor(remotePollSeconds || 15)));
      const _remote: RemoteTrackingSetting = await api.remoteTrackingSet(remoteEnabled, pollSecs);

      const capThresh = Math.max(1, Math.floor(captureThresholdMinutes || 3));
      const capSnooze = Math.max(1, Math.floor(captureSnoozeMinutes || 30));
      const _capture: CapturePromptSetting = await api.capturePromptSet(captureEnabled, capThresh, capSnooze);

      if (keepEmptySessions !== keepEmptyLoaded) {
        keepEmptyLoaded = await api.keepEmptySessionsSet(keepEmptySessions);
      }

      await persistStashConnection();

      const raw = String(playThresholdInput ?? "").trim();
      const pct = raw === "" ? null : Number.parseFloat(raw);
      const cleanedPct =
        pct !== null && Number.isFinite(pct) ? Math.max(0, Math.min(100, pct)) : null;
      const _play: PlayCountingSetting = await api.playCountingSet(cleanedPct);
      playThresholdServerNull = _play.threshold_pct === null;
      playThresholdInput = _play.threshold_pct === null ? "" : String(_play.threshold_pct);

      const cadenceMinutes = Math.max(60, Math.round((mirrorCadenceHours || 24) * 60));
      await api.mirrorSyncSet(mirrorEnabled, cadenceMinutes);

      const metaCadenceMinutes = Math.max(1440, Math.round((metadataCadenceDays || 7) * 24 * 60));
      await api.metadataRefreshSet(metadataEnabled, metaCadenceMinutes);

      saving = false;
      saved = true;
    } catch (e) {
      console.error("save settings failed", e);
      errorMsg = "Couldn't save settings." + serverHint();
      saving = false;
    }
  }
</script>

<!-- svelte-ignore a11y_click_events_have_key_events a11y_no_noninteractive_element_interactions -->
<div class="overlay" role="dialog" aria-modal="true" tabindex="-1"
  onkeydown={(e) => { if (e.key === 'Escape') onClose(); }}
  onclick={onClose}>
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <div class="window" role="document" onclick={(e) => e.stopPropagation()}>
    <nav class="rail">
      <div class="rail-brand">
        <img class="rail-logo" src="/climax-icon.png" alt="Climax" width="22" height="22" />
        <span class="rail-brand-name">Climax Settings</span>
      </div>
      <button class="rail-item" class:active={activePage === 'general'} onclick={() => activePage = 'general'}>
        <Icon name="settings" size={16} />
        <span class="rail-label">General</span>
      </button>
      <button class="rail-item" class:active={activePage === 'untracked'} onclick={() => activePage = 'untracked'}>
        <Icon name="inbox" size={16} />
        <span class="rail-label">Untracked sessions</span>
        {#if untrackedCount > 0}
          <span class="badge">{untrackedCount}</span>
        {:else}
          <span class="badge caught"><Icon name="check" size={13} /></span>
        {/if}
      </button>
      <button class="rail-item" class:active={activePage === 'sync'} onclick={() => activePage = 'sync'}>
        <Icon name="refresh-cw" size={16} />
        <span class="rail-label">Sync</span>
      </button>
      <button class="rail-item" class:active={activePage === 'smart'} onclick={() => activePage = 'smart'}>
        <Icon name="brain" size={16} />
        <span class="rail-label">Smart tracking</span>
      </button>
      <button class="rail-item" class:active={activePage === 'stash'} onclick={() => activePage = 'stash'}>
        <Icon name="plug" size={16} />
        <span class="rail-label">Stash configuration</span>
      </button>
      {#if !webClient}
        <button class="rail-item" class:active={activePage === 'server'} onclick={() => activePage = 'server'}>
          <Icon name="server" size={16} />
          <span class="rail-label">Server / connection</span>
        </button>
      {/if}
      <button class="rail-item" class:active={activePage === 'backup'} onclick={() => activePage = 'backup'}>
        <Icon name="database-backup" size={16} />
        <span class="rail-label">Backup and restore</span>
      </button>
      <button class="rail-item" class:active={activePage === 'version'} onclick={() => activePage = 'version'}>
        <Icon name="download" size={16} />
        <span class="rail-label">Version and updates</span>
      </button>
      <button class="rail-item" class:active={activePage === 'donate'} onclick={() => activePage = 'donate'}>
        <Icon name="heart" size={16} />
        <span class="rail-label">Donate</span>
      </button>
    </nav>

    <div class="main">
      <header><h2>{PAGE_TITLES[activePage]}</h2></header>

      <div class="content">
        {#if activePage === 'untracked'}
          <UntrackedSessions embedded onApplied={refreshCount} />
        {:else if activePage === 'backup'}
          <BackupRestore />
        {:else if activePage === 'donate'}
          <div class="config-scroll">
            <div class="donate-page">
              <p class="donate-lead">Climax is open source, and free forever.</p>
              <p class="donate-body">
                I build this app in my spare time and give it away for nothing. If it's
                been valuable to you and you'd like to support the work (and you can spare
                it), you can buy me a coffee. Completely optional, and thank you either way.
              </p>
              <button class="donate-btn" onclick={() => openExternal(DONATE_URL)}>
                <Icon name="heart" size={16} filled color="#fff" />
                <span>Buy me a coffee</span>
              </button>
            </div>
          </div>
        {:else if activePage === 'server'}
          <!-- Client-LOCAL config (localStorage), NOT a backend setting - so this
               page stays reachable even when the configured server is unreachable
               and `loaded` never flips (the escape hatch back to the in-process
               backend). Kept OUTSIDE the `loaded` gate for exactly that reason. -->
          <div class="config-scroll">
            <section class="setting-group">
              <div class="group-header">
                <h3>Where Climax runs</h3>
                <p class="group-help">
                  By default the Climax app runs its own backend locally on this PC.
                  Depending on your needs, you can instead run a separate Climax
                  server - for example, on a NAS or always-on box - and use this app
                  purely as a thin client. On-this-PC features (system tray, global
                  shortcut, idle detection, reminders) always stay local. Applying a
                  change restarts Climax.
                </p>
              </div>
              <label class="row toggle">
                <span>Connect to an external server</span>
                <input class="switch-input" type="checkbox" checked={serverEnabled}
                  onchange={(e) => { serverEnabled = e.currentTarget.checked; serverTest = 'idle'; }} />
                <span class="switch"></span>
              </label>
              {#if serverEnabled}
                <label class="row">
                  <span class="label">Server address</span>
                  <span class="control">
                    <input class="text-input" type="text" placeholder="http://localhost:9998"
                      value={serverUrlInput}
                      oninput={(e) => { serverUrlInput = e.currentTarget.value; serverTest = 'idle'; }} />
                  </span>
                </label>
                <label class="row">
                  <span class="label">Token (if the server has one)</span>
                  <span class="control">
                    <input class="text-input" type="password" placeholder="Leave empty for none"
                      value={serverTokenInput}
                      oninput={(e) => { serverTokenInput = e.currentTarget.value; }} />
                  </span>
                </label>
                <div class="row inline">
                  <button class="action" disabled={serverTest === 'testing' || !serverUrlInput.trim()} onclick={testServer}>
                    {serverTest === 'testing' ? 'Testing...' : 'Test connection'}
                  </button>
                  {#if serverTest === 'ok'}
                    <span class="status ok">Reachable</span>
                  {:else if serverTest === 'fail'}
                    <span class="status err">No Climax server found at that address</span>
                  {/if}
                </div>
              {/if}
              <div class="row inline standalone">
                <button class="action primary" disabled={!serverDirty || serverApplying} onclick={applyServer}>
                  {serverApplying ? 'Applying...' : 'Apply and restart'}
                </button>
                <span class="status"
                  class:ok={serverConfig.url !== null && serverConfig.state === 'connected'}
                  class:err={serverConfig.url !== null && serverConfig.state === 'unreachable'}>
                  Current: {serverConfig.url === null
                    ? 'using the built-in backend on this PC'
                    : serverConfig.state === 'connected'
                      ? `connected to ${serverConfig.url}`
                      : serverConfig.state === 'unreachable'
                        ? `can't reach ${serverConfig.url}`
                        : `connecting to ${serverConfig.url}...`}
                </span>
              </div>
              {#if serverDevRestartHint}
                <p class="status ok">Saved. Fully restart Climax (re-run the dev app) to apply - dev builds can't auto-restart.</p>
              {/if}
            </section>

            <!-- Host mode only: in client mode an external server owns the port
                 and this app binds nothing, so there's nothing here to set. -->
            {#if hostMode}
              <section class="setting-group">
                <div class="group-header">
                  <h3>Port on this PC</h3>
                  <p class="group-help">
                    The Stash bridge reaches Climax on this port. If another program
                    has taken it, or your system has reserved it, Climax can't start
                    and nothing gets tracked - move it here, then set the bridge's
                    Climax URL in Stash to match. Applying a change restarts Climax.
                  </p>
                </div>
                {#if portInfo && !portInfo.ok}
                  <p class="status err">{portInfo.error}</p>
                {/if}
                <label class="row">
                  <span class="label">Serve on port</span>
                  <span class="control">
                    <input type="number" min="1024" max="65535" step="1"
                      value={portInput}
                      oninput={(e) => { portInput = e.currentTarget.value; portErr = null; }} />
                  </span>
                </label>
                <div class="row inline">
                  <button class="action primary" disabled={!portDirty || portApplying} onclick={applyPort}>
                    {portApplying ? 'Applying...' : 'Apply and restart'}
                  </button>
                  {#if portErr}
                    <span class="status err">{portErr}</span>
                  {:else if portInfo?.ok}
                    <span class="status ok">Serving on {portInfo.port}</span>
                  {/if}
                </div>
                {#if portDevRestartHint}
                  <p class="status ok">Saved. Fully restart Climax (re-run the dev app) to apply - dev builds can't auto-restart.</p>
                {/if}
              </section>
            {/if}
          </div>
        {:else if !loaded}
          <div class="config-scroll"><div class="loading">Loading...</div></div>
        {:else}
          <div class="config-scroll" oninput={() => { saved = false; }} onchange={() => { saved = false; }}>
            {#if activePage === 'sync'}
              <!-- Library metadata -->
              <section class="setting-group">
                <div class="group-header">
                  <h3>Library metadata</h3>
                  <p class="group-help">
                    Pulls scene and performer details from Stash: titles,
                    thumbnails, studios, and tags. Runs when Climax launches and
                    again on the interval below.
                  </p>
                </div>
                <label class="row toggle">
                  <span>Refresh library metadata automatically in the background</span>
                  <input class="switch-input" type="checkbox" bind:checked={metadataEnabled} />
                  <span class="switch"></span>
                </label>
                <label class="row" class:disabled={!metadataEnabled}>
                  <span class="label">Refresh every</span>
                  <span class="control">
                    <input type="number" min="1" step="1" bind:value={metadataCadenceDays} disabled={!metadataEnabled} />
                    <span class="unit">days</span>
                  </span>
                </label>
                <div class="row inline">
                  <button class="action" disabled={metadataBusy} onclick={syncMetadataNow}>
                    {metadataBusy ? "Refreshing..." : "Refresh now"}
                  </button>
                </div>
                {#if metadataBusy && metadataStatus && metadataStatus.total > 0}
                  <div class="row inline"><span class="status">Refreshing {metadataStatus.done.toLocaleString()} / {metadataStatus.total.toLocaleString()} scenes...</span></div>
                {:else if metadataStatus?.last_error}
                  <div class="row inline"><span class="status err">{metadataStatus.last_error}</span></div>
                {:else if metadataStatus?.last_finished_at}
                  <div class="row inline"><span class="status ok">Last refreshed {formatAgo(metadataStatus.last_finished_at)}</span></div>
                {/if}
              </section>

              <!-- Play history mirror -->
              <section class="setting-group">
                <div class="group-header">
                  <h3>Play history</h3>
                  <p class="group-help">
                    Keeps {w("cumshots")}, play count, and watch time matched to Stash,
                    including history from before you installed Climax. Runs when
                    Climax launches and again on the interval below.
                  </p>
                </div>
                <label class="row toggle">
                  <span>Sync history automatically in the background</span>
                  <input class="switch-input" type="checkbox" bind:checked={mirrorEnabled} />
                  <span class="switch"></span>
                </label>
                <label class="row" class:disabled={!mirrorEnabled}>
                  <span class="label">Sync every</span>
                  <span class="control">
                    <input type="number" min="1" step="1" bind:value={mirrorCadenceHours} disabled={!mirrorEnabled} />
                    <span class="unit">hours</span>
                  </span>
                </label>
                <div class="row inline">
                  <button class="action" disabled={mirrorBusy} onclick={syncMirrorNow}>
                    {mirrorBusy ? "Syncing..." : "Sync now"}
                  </button>
                </div>
                {#if mirrorBusy && mirrorStatus && mirrorStatus.total > 0}
                  <div class="row inline"><span class="status">Syncing {mirrorStatus.done.toLocaleString()} / {mirrorStatus.total.toLocaleString()} scenes...</span></div>
                {:else if mirrorStatus?.last_error}
                  <div class="row inline"><span class="status err">{mirrorStatus.last_error}</span></div>
                {:else if mirrorStatus?.last_finished_at}
                  <div class="row inline"><span class="status ok">Last synced {formatAgo(mirrorStatus.last_finished_at)}</span></div>
                {/if}
              </section>

              <!-- Play counting threshold -->
              <section class="setting-group">
                <div class="group-header">
                  <h3>Play count threshold</h3>
                  <p class="group-help">
                    How much of a scene you must watch before it counts as a play.
                    The default (and recommended) behaviour is to mirror Stash, but
                    you can override it here if you need to.
                  </p>
                </div>
                <label class="row">
                  <span class="label">Minimum play percent</span>
                  <span class="control">
                    <input type="number" min="0" max="100" step="0.5" bind:value={playThresholdInput} placeholder={playThresholdServerNull ? "0" : ""} />
                    <span class="unit">%</span>
                  </span>
                </label>
                <div class="row inline">
                  <button type="button" class="action" onclick={syncPlayThresholdFromStash} disabled={syncingPlayThreshold}>
                    {syncingPlayThreshold ? "Syncing..." : "Sync from Stash"}
                  </button>
                  {#if playThresholdSyncError}
                    <span class="status err">✗ {playThresholdSyncError}</span>
                  {:else if playThreshSyncedMsg}
                    <span class="status ok">✓ {playThreshSyncedMsg}</span>
                  {:else if playThresholdServerNull && !syncingPlayThreshold}
                    <span class="status">Not synced yet. Using 0% as fallback for now.</span>
                  {/if}
                </div>
              </section>
            {/if}

            {#if activePage === 'smart'}
              <!-- Idle detection. Desktop only: presence.rs reads OS-level input
                   (GetLastInputInfo), which a browser tab has no access to. -->
              <section class="setting-group">
                <div class="group-header">
                  <h3>Idle detection</h3>
                  {#if webClient}
                    <p class="group-help">
                      Climax can't see your mouse or keyboard from a browser, so idle
                      detection only runs in the desktop app.
                    </p>
                  {:else}
                    <p class="group-help">
                      While a session is running, Climax watches for mouse and
                      keyboard activity. If you go idle and come back, it asks how to
                      handle the gap.
                    </p>
                  {/if}
                </div>
                {#if webClient}
                  <div class="row inline standalone">
                    <button class="action" onclick={() => openExternal(APP_DOWNLOAD_URL)}>
                      Download the desktop app
                    </button>
                  </div>
                {:else}
                  <label class="row toggle">
                    <span>Enabled</span>
                    <input class="switch-input" type="checkbox" bind:checked={idleEnabled} />
                    <span class="switch"></span>
                  </label>
                  <label class="row" class:disabled={!idleEnabled}>
                    <span class="label">Ask me after</span>
                    <span class="control">
                      <input type="number" min="1" step="1" bind:value={idleThresholdMinutes} disabled={!idleEnabled} />
                      <span class="unit">minutes</span>
                    </span>
                  </label>
                {/if}
              </section>

              <!-- Off-bridge tracking (poll-based; catches non-bridge playback) -->
              <section class="setting-group">
                <div class="group-header">
                  <h3>Off-bridge tracking</h3>
                  <p class="group-help">
                    Counts playback the bridge can't see - your TV, phone apps, or
                    other browsers - by watching Stash in the background. Anything
                    the bridge already tracked is skipped, so nothing counts twice.
                  </p>
                </div>
                <label class="row toggle">
                  <span>Enabled</span>
                  <input class="switch-input" type="checkbox" bind:checked={remoteEnabled} />
                  <span class="switch"></span>
                </label>
                <label class="row" class:disabled={!remoteEnabled}>
                  <span class="label">Check for off-bridge playback every</span>
                  <span class="control">
                    <input type="number" min="5" max="60" step="1" bind:value={remotePollSeconds} disabled={!remoteEnabled} />
                    <span class="unit">seconds</span>
                  </span>
                </label>
              </section>

              <!-- Remind me to track a session (passive capture). Desktop only:
                   the reminder is an always-on-top toast window, which a browser
                   tab can't put over whatever else you're doing. -->
              <section class="setting-group">
                <div class="group-header">
                  <h3>Remind me to track a session</h3>
                  {#if webClient}
                    <p class="group-help">
                      The reminder has to appear over whatever else you're doing, which
                      a browser tab can't do. This one only runs in the desktop app.
                    </p>
                  {:else}
                    <p class="group-help">
                      When a scene is playing but no session is running, Climax will
                      send you a reminder to start tracking. The session will be
                      backdated to when watching began, so the time you already put
                      in still counts.
                    </p>
                  {/if}
                </div>
                {#if webClient}
                  <div class="row inline standalone">
                    <button class="action" onclick={() => openExternal(APP_DOWNLOAD_URL)}>
                      Download the desktop app
                    </button>
                  </div>
                {:else}
                  <label class="row toggle">
                    <span>Enabled</span>
                    <input class="switch-input" type="checkbox" bind:checked={captureEnabled} />
                    <span class="switch"></span>
                  </label>
                  <label class="row" class:disabled={!captureEnabled}>
                    <span class="label">Remind me after watching for</span>
                    <span class="control">
                      <input type="number" min="1" step="1" bind:value={captureThresholdMinutes} disabled={!captureEnabled} />
                      <span class="unit">minutes</span>
                    </span>
                  </label>
                  <label class="row" class:disabled={!captureEnabled}>
                    <span class="label">After dismissing, snooze for</span>
                    <span class="control">
                      <input type="number" min="1" step="1" bind:value={captureSnoozeMinutes} disabled={!captureEnabled} />
                      <span class="unit">minutes</span>
                    </span>
                  </label>
                {/if}
              </section>

              <!-- Keep empty sessions. Server-side rule (the backend decides what
                   a stopped session becomes), so it is NOT desktop-gated. -->
              <section class="setting-group">
                <div class="group-header">
                  <h3>Empty sessions</h3>
                  <p class="group-help">
                    A session with no scenes and no {w("cumshots")} is normally discarded
                    when you stop it. Turn this on to keep it, so Climax can work as
                    a plain timer without Stash.
                  </p>
                </div>
                <label class="row toggle">
                  <span>Keep empty sessions</span>
                  <input class="switch-input" type="checkbox" bind:checked={keepEmptySessions} />
                  <span class="switch"></span>
                </label>
              </section>
            {/if}

            {#if activePage === 'stash'}
              <!-- Stash connection -->
              <section class="setting-group">
                <div class="group-header">
                  <h3>Connection</h3>
                  <p class="group-help">
                    Climax reads scene metadata straight from Stash over its API.
                    (The bridge plugin is separate and only reports what's playing
                    right now.)
                  </p>
                </div>
                <label class="row stacked">
                  <span class="label">Stash URL</span>
                  <input type="text" class="text-input" bind:value={stashUrl} placeholder="http://localhost:9999" spellcheck="false" autocomplete="off" />
                </label>
                <label class="row stacked">
                  <span class="label">API key <span class="hint">(optional)</span></span>
                  <span class="key-input">
                    <input type={showApiKey ? "text" : "password"} class="text-input" bind:value={stashApiKey} placeholder="Leave blank if Stash has no auth" spellcheck="false" autocomplete="off" />
                    <button type="button" class="key-toggle" onclick={() => (showApiKey = !showApiKey)} title={showApiKey ? "Hide" : "Show"}>{showApiKey ? "Hide" : "Show"}</button>
                  </span>
                </label>
                <div class="row inline">
                  <button class="action" disabled={testing} onclick={testConnection}>
                    {testing ? "Testing..." : "Test connection"}
                  </button>
                  {#if testResult}
                    {#if testResult.ok}
                      <span class="status ok">✓ Connected{testResult.version ? ` to Stash ${testResult.version}` : ""}{testResult.scene_count != null ? ` - ${testResult.scene_count.toLocaleString()} scenes` : ""}</span>
                    {:else}
                      <span class="status err">✗ {testResult.error ?? "Connection failed"}</span>
                    {/if}
                  {/if}
                </div>
              </section>
            {/if}

            {#if activePage === 'general'}
              {#if !webClient}
                <!-- Startup (desktop only: autostart + windows are app-shell things) -->
                <section class="setting-group">
                  <div class="group-header">
                    <h3>Startup</h3>
                    <p class="group-help">
                      Choose whether Climax opens when your computer starts, and
                      what it shows when it does. Changes take effect right away.
                    </p>
                  </div>
                  <label class="row toggle">
                    <span>Open at login</span>
                    <input class="switch-input" type="checkbox" checked={openAtLogin}
                      onchange={(e) => setOpenAtLogin(e.currentTarget.checked)} />
                    <span class="switch"></span>
                  </label>
                  <label class="row">
                    <span class="label">When Climax opens, show</span>
                    <span class="control">
                      <Select
                        value={launchBehavior}
                        options={[
                          { value: "silent", label: "Silent (tray only)" },
                          { value: "tracker", label: "Open tracker" },
                          { value: "dashboard", label: "Open dashboard" },
                        ]}
                        onChange={(v) => setLaunchBehavior(v as 'silent' | 'tracker' | 'dashboard')}
                        fullWidth
                        ariaLabel="When Climax opens, show"
                      />
                    </span>
                  </label>
                </section>

                <!-- Global shortcut (desktop only: needs the OS-level hotkey) -->
                <section class="setting-group">
                  <div class="group-header">
                    <h3>Global shortcut</h3>
                    <p class="group-help">
                      Press {shortcutLabel} anywhere to open the Tracker and start a
                      session (if one isn't already running). This can only start a
                      session, never stop one.
                    </p>
                  </div>
                  <label class="row toggle">
                    <span>Enable {shortcutLabel}</span>
                    <input class="switch-input" type="checkbox" checked={hotkeyEnabled}
                      onchange={(e) => setHotkeyEnabled(e.currentTarget.checked)} />
                    <span class="switch"></span>
                  </label>
                </section>
              {:else}
                <!-- Web client: the desktop-only startup/shortcut/tray features
                     live in the app, not the browser. Say so once, plainly. -->
                <section class="setting-group">
                  <div class="group-header">
                    <h3>This browser</h3>
                    <p class="group-help">
                      You're using Climax in a browser. Startup options, the global
                      shortcut, and the system tray live in the desktop app.
                    </p>
                  </div>
                </section>
              {/if}

              <!-- Wording. The two button labels are the literal choices, so
                   they are deliberately NOT passed through w(). -->
              <section class="setting-group">
                <div class="group-header">
                  <h3>Wording</h3>
                  <p class="group-help">
                    Choose the word Climax uses throughout the app. This only
                    changes what you see.
                  </p>
                </div>
                <div class="row">
                  <span class="label">Use the word</span>
                  <span class="control">
                    <div class="cx-toggle" role="group" aria-label="Word used throughout the app">
                      <button class:active={wording.value === "cumshot"} aria-pressed={wording.value === "cumshot"} onclick={() => pickWording("cumshot")}>Cumshot</button>
                      <button class:active={wording.value === "orgasm"} aria-pressed={wording.value === "orgasm"} onclick={() => pickWording("orgasm")}>Orgasm</button>
                    </div>
                    {#if wordingErr}<span class="status err">{wordingErr}</span>{/if}
                  </span>
                </div>
              </section>

              <!-- First-launch setup -->
              <section class="setting-group">
                <div class="group-header">
                  <h3>First-launch setup</h3>
                  <p class="group-help">
                    Run the setup wizard again to import your Stash history or
                    estimate past sessions. It is recommended to back up your
                    library before running this.
                  </p>
                </div>
                <div class="row inline standalone">
                  <!-- No busy state: this just opens the wizard, it no longer
                       writes anything first. -->
                  <button class="action" onclick={rerunSetup}>Re-run setup</button>
                </div>
              </section>
            {/if}

            {#if activePage === 'version'}
              <section class="setting-group">
                <div class="group-header">
                  <h3>Updates</h3>
                  <p class="group-help">
                    Check here for the latest version of Climax, published on GitHub.
                  </p>
                </div>
                <div class="row">
                  <span class="label">Your version</span>
                  <span class="control">
                    <!-- The stored version has its "v" stripped so it compares
                         directly against a tag; display puts it back, matching the
                         sidebar. -->
                    <span class="value">{updateCheck.myVersion ? `v${updateCheck.myVersion}` : "-"}</span>
                  </span>
                </div>
                <!-- Only in desktop client mode, where the app and the server it
                     talks to are separately released and can genuinely differ. -->
                {#if !webClient && !hostMode}
                  <div class="row">
                    <span class="label">Server version</span>
                    <span class="control">
                      <span class="value">{updateCheck.info?.backend_version ? `v${updateCheck.info.backend_version}` : "-"}</span>
                    </span>
                  </div>
                {/if}
                <div class="row">
                  <span class="label">Latest release</span>
                  <span class="control">
                    <!-- A version and a date are numbers, so they stay mono. The
                         fallbacks are prose and must not be: mono is reserved for
                         numbers app-wide, and a mono word reads as machine output. -->
                    {#if updateCheck.latest}
                      {@const release = updateCheck.latest}
                      <span class="value">
                        v{release.version}{releaseDate(release.published_at)
                          ? ` · ${releaseDate(release.published_at)}`
                          : ""}
                      </span>
                    {:else if updateCheck.info?.checked_at && !updateProblem}
                      <span class="value text">None published yet</span>
                    {:else}
                      <span class="value text">-</span>
                    {/if}
                  </span>
                </div>
                <div class="row inline">
                  <button class="action" disabled={updateCheck.checking} onclick={checkForUpdates}>
                    {updateCheck.checking ? 'Checking...' : 'Check for updates'}
                  </button>
                  {#if updateCheck.available && updateCheck.latest}
                    {@const release = updateCheck.latest}
                    <!-- The tray warning lives here rather than in the intro: it
                         only matters at the moment you install, and this button
                         only exists when there is something to install. -->
                    <button
                      class="action primary"
                      title={webClient
                        ? "Open the release page on GitHub."
                        : "Quit Climax from the tray first, then run the installer."}
                      onclick={() => openExternal(release.url)}
                    >
                      Download {release.version}
                    </button>
                  {/if}
                  <!-- Doubles as the button's confirmation: pressing it turns the
                       time into "just now", so the check never looks like it did
                       nothing. That is what lets the "Last checked" row go. -->
                  {#if updateProblem}
                    <span class="status err">{updateProblem}</span>
                  {:else if updateCheck.available}
                    <!-- Two tones on purpose. The verdict stays neutral (the coral
                         Download button beside it carries the urgency, and red is
                         reserved for a real fault like a failed port bind), while
                         green stays on the half it is actually true of: the check
                         ran. -->
                    <!-- Nested, not two sibling spans: the row's 16px flex gap
                         would push them apart and they would read as two separate
                         items rather than one line.
                         The divider is an EXPRESSION and sits outside the coloured
                         span on purpose - Svelte trims whitespace at an element's
                         edges, so a leading space inside the span is dropped and
                         the text collides with the dot. -->
                    <span class="status">
                      Out of date{#if lastChecked}{" · "}<span class="fresh">checked {lastChecked}</span>{/if}
                    </span>
                  {:else if updateCheck.latest}
                    <span class="status ok">Up to date{lastChecked ? ` · checked ${lastChecked}` : ""}</span>
                  {:else if lastChecked}
                    <!-- No verdict is possible with nothing published - there is
                         no version to be up to date with. Green because the check
                         itself succeeded, which is what this line confirms. -->
                    <span class="status ok">Checked {lastChecked}</span>
                  {/if}
                </div>
                <!-- Its own row, below. Sharing the row above put it between
                     "Check for updates" and the status that button produces,
                     which broke the association between an action and its
                     result. This is a separate errand, so it reads as one. -->
                <div class="row inline">
                  <button class="action with-icon" onclick={() => openExternal(APP_REPO_URL)}>
                    <Icon name="external-link" size={14} /> View on GitHub
                  </button>
                </div>
              </section>
            {/if}

            {#if errorMsg}
              <div class="error">{errorMsg}</div>
            {/if}
          </div>
        {/if}
      </div>

      {#if activePage !== 'untracked' && activePage !== 'general' && activePage !== 'server' && activePage !== 'backup' && activePage !== 'version' && activePage !== 'donate'}
        <footer>
          <button class="cancel" disabled={saving} onclick={onClose}>Cancel</button>
          <button class="save" disabled={saving || !loaded} onclick={save}>
            {saving ? "Saving..." : saved ? "Saved ✓" : "Save"}
          </button>
        </footer>
      {/if}
    </div>
  </div>
</div>

<style>
  /* Settings modal, Climax design system v2 (visual rework). Same class
     names as the markup; every value resolves to a design token with the
     exact token hex as a fallback, so it renders right even if a token is
     missing. Adds the rail brand lockup, rail-item icons, and the toggle
     switch that replaces the native checkbox. */

  .overlay {
    position: fixed; inset: 0;
    background: rgba(7, 8, 11, 0.72);
    display: flex; align-items: center; justify-content: center;
    z-index: 1200;
    animation: fade-in 140ms cubic-bezier(0.16, 1, 0.3, 1);
  }
  @keyframes fade-in { from { opacity: 0 } to { opacity: 1 } }

  .window {
    background: var(--bg-card, #15171E);
    border: 1px solid var(--border, #252934);
    border-radius: var(--radius-xl, 14px);
    width: min(1000px, 94vw);
    height: min(88vh, 760px);
    display: flex;
    box-shadow: var(--shadow-lg, 0 12px 32px rgba(0,0,0,0.45));
    overflow: hidden;
    animation: pop-in 220ms cubic-bezier(0.16, 1, 0.3, 1);
  }
  @keyframes pop-in {
    from { transform: translateY(8px) scale(0.98); opacity: 0 }
    to   { transform: none; opacity: 1 }
  }

  /* ---------- left rail ---------- */
  .rail {
    width: 224px; flex-shrink: 0;
    background: var(--bg-sunken, #07080B);
    border-right: 1px solid var(--border-subtle, #1C1F26);
    display: flex; flex-direction: column; gap: 1px;
    padding: 18px 0 12px;
  }

  .rail-brand {
    display: flex; align-items: center; gap: 9px;
    padding: 2px 20px 16px 18px;
    margin-bottom: 6px;
    border-bottom: 1px solid var(--border-subtle, #1C1F26);
  }
  .rail-logo {
    width: 22px; height: 22px;
    border-radius: 6px;
    object-fit: cover; display: block; flex-shrink: 0;
  }
  .rail-brand-name {
    font-size: 13px; font-weight: 600;
    color: var(--fg-strong, #F7F8FA);
    letter-spacing: 0.1px;
    white-space: nowrap;
  }

  .rail-item {
    display: flex; align-items: center; gap: 10px;
    width: 100%; text-align: left;
    padding: 9px 20px 9px 18px;
    border: none; border-left: 2px solid transparent;
    background: transparent; color: var(--fg-muted, #8A909E);
    font-family: inherit; font-size: 13px; cursor: pointer;
    transition: background 120ms, color 120ms, border-color 120ms;
  }
  .rail-item:hover { background: var(--bg-elevated, #101218); color: var(--fg, #ECEEF3); }
  .rail-item.active {
    background: var(--coral-a-08, rgba(239,107,122,0.08));
    border-left-color: var(--accent, #EF6B7A);
    color: var(--fg-strong, #F7F8FA);
  }
  .rail-item .rail-label { flex: 1; }

  /* ---------- donate page ---------- */
  .donate-page {
    max-width: 64ch;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  /* Match the other pages: heading == .group-header h3, body == .group-help. */
  .donate-lead {
    margin: 0;
    font-size: 18px;
    font-weight: 600;
    letter-spacing: -0.01em;
    color: var(--fg-strong, #F7F8FA);
  }
  .donate-body {
    margin: 0;
    font-size: 14px;
    line-height: 1.55;
    color: var(--fg-muted, #8A909E);
    text-wrap: pretty;
  }
  .donate-btn {
    align-self: flex-start;
    margin-top: 14px;
    display: inline-flex;
    align-items: center;
    gap: 8px;
    padding: 10px 18px;
    background: var(--accent, #EF6B7A);
    border: 1px solid var(--accent, #EF6B7A);
    border-radius: 8px;
    color: #fff;
    font-family: inherit;
    font-size: 13px;
    font-weight: 600;
    text-decoration: none;
    cursor: pointer;
    transition: background 120ms, border-color 120ms;
  }
  .donate-btn:hover {
    background: var(--accent-hover, #F58895);
    border-color: var(--accent-hover, #F58895);
  }

  .badge {
    font-size: 11px; font-weight: 600; min-width: 20px; height: 18px; padding: 0 6px;
    display: inline-flex; align-items: center; justify-content: center; border-radius: 999px;
    background: var(--accent, #EF6B7A); color: var(--accent-fg, #fff);
    font-variant-numeric: tabular-nums; font-family: var(--font-mono, monospace);
  }
  .badge.caught { background: transparent; color: var(--live, #4ADE80); font-weight: 400; }

  /* ---------- main pane ---------- */
  .main { flex: 1; min-width: 0; display: flex; flex-direction: column; }
  header {
    padding: 18px 28px 16px;
    border-bottom: 1px solid var(--border-subtle, #1C1F26);
    flex-shrink: 0;
  }
  header h2 {
    margin: 0; font-family: var(--font-display, system-ui);
    font-size: 22px; font-weight: 600; letter-spacing: -0.01em;
    color: var(--fg-strong, #F7F8FA);
  }

  .content { flex: 1; min-height: 0; display: flex; flex-direction: column; }
  .config-scroll {
    /* hidden auto (not overflow-y:auto): overflow-y:auto forces overflow-x to
       compute to auto too, so any over-wide child (e.g. a long unwrapped status
       line) surfaces a stray horizontal scrollbar. Pin the cross-axis. */
    overflow: hidden auto; flex: 1; min-height: 0;
    padding: 24px 28px 28px;
    /* Generous gap between whole setting groups (heading + description + its
       indented rows = one unit) so adjacent units read as clearly separate. */
    display: flex; flex-direction: column; gap: 44px;
  }
  .loading { color: var(--fg-muted, #8A909E); font-size: 13px; text-align: center; padding: 20px; }

  /* ---------- setting group ---------- */
  .setting-group { display: flex; flex-direction: column; gap: 4px; }
  /* Section heading reads as the PARENT: bright, bold, and clearly larger than
     the grey explanation beneath it + the indented setting rows under it. */
  .group-header h3 {
    margin: 0 0 5px; font-size: 18px; font-weight: 600; letter-spacing: -0.01em;
    color: var(--fg-strong, #F7F8FA);
  }
  .group-help {
    margin: 0; font-size: 14px; line-height: 1.55; color: var(--fg-muted, #8A909E);
    max-width: 64ch; text-wrap: pretty;
  }

  /* ---------- rows ---------- */
  .row {
    display: flex; align-items: center; justify-content: space-between; gap: 16px;
    padding: 14px 0; margin-top: 6px;
    /* Indent the actual settings under their section heading (which stays at
       the group's left edge) so the parent/child grouping reads clearly. */
    margin-left: 26px;
    border-top: 1px solid var(--border-subtle, #1C1F26);
    transition: opacity 200ms;
  }
  .row:first-of-type { margin-top: 10px; }
  .row.stacked { flex-direction: column; align-items: stretch; gap: 8px; }
  .row.inline { justify-content: flex-start; flex-wrap: wrap; border-top: none; padding-top: 4px; margin-top: 0; }
  /* Optical alignment, not geometric: the row already starts at the same 26px as
     the labels above, but a 1px border under a 6px corner radius reads as inset
     next to a text glyph on the same line. Pulling the leading button back by its
     border width makes the edge look flush with the labels it sits under. */
  .row.inline > .action:first-child { margin-left: -1px; }
  /* A standalone action button that is its section's first/only control (e.g.
     "Re-run setup") - restore the normal first-row separator + spacing so it
     doesn't read as a tight sub-action hugging rows that aren't there. */
  .row.inline.standalone { border-top: 1px solid var(--border-subtle, #1C1F26); padding-top: 14px; margin-top: 6px; }
  .row.disabled { opacity: 0.45; }
  .row.toggle { cursor: pointer; }
  .row .label, .row.toggle > span { font-size: 14px; color: var(--fg, #ECEEF3); font-weight: 500; white-space: nowrap; }
  .row .label .hint { font-size: 12px; color: var(--fg-subtle, #5C6273); font-weight: 400; margin-left: 5px; }

  /* ---------- toggle switch (replaces the native checkbox).
     The hidden input keeps bind:checked working; .switch is the visual
     track that picks up :checked via the sibling combinator. ---------- */
  .switch-input { position: absolute; opacity: 0; width: 0; height: 0; pointer-events: none; }
  .switch {
    flex-shrink: 0; width: 40px; height: 23px; border-radius: 999px;
    background: var(--ink-700, #252934); border: 1px solid var(--border-strong, #353A47);
    position: relative; transition: background 200ms cubic-bezier(0.16,1,0.3,1), border-color 200ms;
  }
  .switch::after {
    content: ""; position: absolute; top: 2px; left: 2px;
    width: 17px; height: 17px; border-radius: 50%; background: #fff;
    box-shadow: 0 1px 2px rgba(0,0,0,0.3);
    transition: transform 200ms cubic-bezier(0.16,1,0.3,1);
  }
  .switch-input:checked + .switch { background: var(--accent, #EF6B7A); border-color: var(--accent, #EF6B7A); }
  .switch-input:checked + .switch::after { transform: translateX(17px); }
  .switch-input:focus-visible + .switch { box-shadow: 0 0 0 3px var(--coral-a-24, rgba(239,107,122,0.24)); }

  /* ---------- inputs ---------- */
  .control { display: flex; align-items: center; gap: 8px; }
  /* A read-only value in a row's control slot. Matches input TEXT (mono, 13px,
     --fg) so a fact and an editable field read as the same kind of thing, but
     with no box - there is nothing to type into. */
  .control .value {
    font-size: 13px; font-family: var(--font-mono, monospace);
    color: var(--fg, #ECEEF3); font-variant-numeric: tabular-nums;
  }
  /* Prose in the value slot: an absence or a state, not a number. Mono is
     reserved for numbers app-wide - a mono word reads as machine output, which
     is the same reason the connection badge was moved off it. */
  .control .value.text {
    font-family: inherit; color: var(--fg-muted, #8A909E);
  }
  input[type="number"], .text-input {
    background: var(--bg-input, #101218); border: 1px solid var(--border, #252934);
    border-radius: var(--radius-sm, 6px); color: var(--fg, #ECEEF3);
    font-size: 13px; font-family: var(--font-mono, monospace);
    padding: 8px 11px; transition: border-color 120ms, box-shadow 120ms;
  }
  input[type="number"] { width: 92px; text-align: left; font-variant-numeric: tabular-nums; }
  .text-input { width: 100%; }
  input[type="number"]:focus, .text-input:focus {
    outline: none; border-color: var(--accent, #EF6B7A);
    box-shadow: 0 0 0 3px var(--coral-a-24, rgba(239,107,122,0.24));
  }
  input[type="number"]:disabled { opacity: 0.5; cursor: default; }
  .unit { font-size: 12px; color: var(--fg-muted, #8A909E); min-width: 46px; text-align: left; }

  .key-input { display: flex; gap: 6px; width: 100%; }
  .key-input .text-input { flex: 1; }
  .key-toggle {
    padding: 0 14px; background: transparent; color: var(--fg-muted, #8A909E);
    border: 1px solid var(--border, #252934); border-radius: var(--radius-sm, 6px);
    font-family: inherit; font-size: 12px; font-weight: 500; cursor: pointer;
    transition: background 120ms, color 120ms, border-color 120ms;
  }
  .key-toggle:hover { background: var(--bg-card-hover, #1B1E26); border-color: var(--border-strong, #353A47); color: var(--fg, #ECEEF3); }

  /* ---------- secondary action button ---------- */
  /* Match the Backup/restore action buttons (the reference): 8x16 padding, 13px. */
  .action {
    padding: 8px 16px; background: var(--bg-card-hover, #1B1E26); color: var(--fg, #ECEEF3);
    border: 1px solid var(--border, #252934); border-radius: var(--radius-sm, 6px);
    font-family: inherit; font-size: 13px; font-weight: 500; cursor: pointer; white-space: nowrap;
    transition: background 120ms, border-color 120ms, transform 80ms;
  }
  .action:hover:not(:disabled) { border-color: var(--border-strong, #353A47); background: var(--bg-elevated, #101218); }
  /* An action carrying a leading icon. Without the flex the inline SVG sits on
     the text baseline and reads a couple of pixels low. */
  .action.with-icon { display: inline-flex; align-items: center; gap: 6px; }
  .action:active:not(:disabled) { transform: translateY(1px); }
  .action:disabled { opacity: 0.5; cursor: default; }
  .action.primary { background: var(--accent, #EF6B7A); border-color: var(--accent, #EF6B7A); color: var(--accent-fg, #fff); }
  .action.primary:hover:not(:disabled) { background: var(--accent-hover, #F58895); border-color: var(--accent-hover, #F58895); }

  /* ---------- status text ---------- */
  .status { font-size: 12px; line-height: 1.4; font-variant-numeric: tabular-nums; color: var(--fg-muted, #8A909E); white-space: nowrap; }
  .status.ok { color: var(--live, #4ADE80); }
  /* The green half of a two-tone status: the verdict stays neutral, this marks
     the part that genuinely succeeded (the check ran). */
  .status .fresh { color: var(--live, #4ADE80); }
  /* Errors can be long (a reqwest/GraphQL failure carries the full Stash URL).
     The base .status is white-space: nowrap, which would defeat word-break and
     push the line wider than the pane (a stray horizontal scrollbar, via the
     overflow-y:auto -> overflow-x:auto quirk below). Let errors WRAP. */
  .status.err { color: var(--danger, #F26B6B); white-space: normal; overflow-wrap: anywhere; }

  .error {
    padding: 8px 12px; border: 1px solid rgba(242,107,107,0.25);
    background: rgba(242,107,107,0.07); color: var(--danger, #F26B6B);
    border-radius: var(--radius-md, 8px); font-size: 12px;
  }

  /* ---------- footer (non-untracked pages only) ---------- */
  footer {
    padding: 16px 28px; border-top: 1px solid var(--border-subtle, #1C1F26);
    display: flex; justify-content: flex-end; gap: 10px;
    background: var(--bg-sunken, #07080B); flex-shrink: 0;
  }
  footer button {
    padding: 9px 18px; border-radius: var(--radius-md, 8px); font-size: 13px; font-weight: 500;
    border: 1px solid var(--border, #252934); background: transparent; color: var(--fg, #ECEEF3);
    cursor: pointer; font-family: inherit; white-space: nowrap;
    transition: background 120ms, border-color 120ms, transform 80ms;
  }
  footer button:active { transform: translateY(1px); }
  footer .cancel:hover:not(:disabled) { background: var(--bg-card-hover, #1B1E26); border-color: var(--border-strong, #353A47); }
  footer .save { background: var(--accent, #EF6B7A); border-color: var(--accent, #EF6B7A); color: var(--accent-fg, #fff); }
  footer .save:hover:not(:disabled) { background: var(--accent-hover, #F58895); border-color: var(--accent-hover, #F58895); }
  footer .save:focus-visible { outline: none; box-shadow: 0 0 0 3px var(--coral-a-24, rgba(239,107,122,0.24)); }
  footer button:disabled { opacity: 0.5; cursor: default; }
</style>
