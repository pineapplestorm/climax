// Typed wrappers around the backend commands. These call `invoke`, which is the
// transport-agnostic `call` from transport.ts (Tauri IPC in the desktop shell,
// HTTP /rpc in a browser) - so every wrapper works under both transports with no
// per-call changes. See transport.ts + CLAUDE.md "client-server pivot".

import { call as invoke } from "./transport";

export type SessionStatus = "active" | "paused" | "ended" | "discarded";

export interface Session {
  id: number;
  started_at: number;
  ended_at: number | null;
  last_heartbeat: number | null;
  status: SessionStatus;
  notes: string | null;
  excluded: boolean;
  session_type: string | null;
  /** True for reconstructed / best-guess sessions (built from Stash history),
   *  false for live-tracked ones. Drives the "estimated" badge. */
  estimated: boolean;
  /** YYYY-MM-DD local. The day THIS session "belongs to" per user intent. */
  assigned_day: string;
  effective_duration_ms: number;
  scene_count: number;
  o_count: number;
}

/** Sort column for the dashboard Sessions table. Mirrors the backend
 *  `SessionSort` enum; parsed server-side, so an unknown value falls back to
 *  "start". */
export type SessionSort = "start" | "duration" | "scenes" | "cumshots";

/** One page of the Sessions table: the page rows + total matching count. */
export interface SessionPage {
  rows: Session[];
  total: number;
}

/** One unbroken stretch of playback inside a scene_play. Recorded from the same
 *  heartbeat that credits the watch time, so the runs always decompose the
 *  total rather than estimating it. */
export interface PlayRun {
  started_at: number;
  ended_at: number;
  seconds_tracked: number;
  /** Credited by the Stash play_duration poll rather than the bridge, so
   *  bounded by the poll interval rather than measured to the second. */
  off_bridge: boolean;
}

export interface PlayedScene {
  play_id: number;
  session_id: number;
  content_item_id: number;
  first_seen_at: number;
  last_seen_at: number;
  seconds_tracked: number;
  external_id: string | null;
  title: string | null;
  url: string | null;
  duration_seconds: number | null;
  source_key: string;
  /** Total cumshots logged on this scene within the current session. */
  cumshot_count: number;
  /** Subset of cumshot_count that originated in Climax (removable from Climax UI). */
  climax_cumshot_count: number;
  /** Stash thumbnail URL (populated from background metadata enrichment). */
  thumbnail_url: string | null;
  /** Raw JSON string from Stash enrichment: performers, studio, date, tags. */
  metadata_json: string | null;
  /** ms-since-epoch at which seconds_tracked first crossed the play
   *  threshold (Stash's "minimum play percent", mirrored). null = below
   *  threshold; tracker/overview hide the scene once it's no longer being
   *  actively watched UNLESS it has a cumshot, which forces it visible. */
  counted_at: number | null;
  /** ms-since-epoch of the last heartbeat that actually advanced
   *  seconds_tracked (i.e. the video was playing). The tracker + Overview show
   *  a scene while this is within their grace window (or it has a cumshot).
   *  Backend-authoritative, so visibility survives component remounts and
   *  window-hide timer throttling. null = never advanced live. */
  last_advance_at: number | null;
  /** True when tracked via the off-bridge poll path (a TV / phone app), never
   *  seen by the browser bridge. The wrap-up modal tags these rows. */
  off_bridge: boolean;
  /** The stretches of playback making up seconds_tracked, oldest first, so the
   *  session spine can draw when the video was actually moving instead of one
   *  block from first_seen_at. EMPTY for plays recorded before runs existed and
   *  for reconstructed / absorbed plays built from Stash history - treat that as
   *  "no detail available", NOT as "never watched", and fall back to the
   *  aggregate. */
  runs: PlayRun[];
  /** Stash's play timestamps for this scene inside this session's window, in
   *  order - one per player load that crossed Stash's minimum-play-percent. Two
   *  means the scene was genuinely opened again, as opposed to paused and
   *  resumed, which is what decides whether the spine draws it once or twice.
   *  Empty while a session is still running (they are imported when it ends),
   *  after a session that ended with Stash unreachable, and for a watch too
   *  brief for Stash to log a play at all. */
  stash_plays: number[];
}

export interface SceneMetadata {
  performers?: { id: string; name: string }[];
  studio?: { id: string; name: string } | null;
  date?: string | null;
  tags?: { id: string; name: string }[];
  /** Vertical resolution label (e.g. "2160p"), from Stash enrichment. Absent
   *  on scenes not yet re-enriched since the Phase 5 resolution add. */
  resolution?: string | null;
}

export function parseSceneMetadata(jsonStr: string | null): SceneMetadata | null {
  if (!jsonStr) return null;
  try { return JSON.parse(jsonStr) as SceneMetadata; } catch { return null; }
}

export interface OEvent {
  id: number;
  session_id: number | null;
  content_item_id: number | null;
  occurred_at: number;
  intensity: number | null;
  notes: string | null;
  stash_synced: boolean;
  /** 'climax' | 'stash' | 'manual'. Only 'climax' is deletable from inside Climax. */
  origin: string;
}

export interface OLogArgs {
  sessionId?: number | null;
  contentItemId?: number | null;
  occurredAt?: number | null;
  intensity?: number | null;
  notes?: string | null;
}

export interface IdleDetectionSetting {
  enabled: boolean;
  threshold_minutes: number;
}

/** Live remote-device tracking — detects scenes played on other devices (e.g.
 *  the Android TV app) by polling Stash's per-scene play_duration during a
 *  session, and credits them like local playback. */
export interface RemoteTrackingSetting {
  enabled: boolean;
  poll_seconds: number;
}

/** Passive-capture prompt: if you watch a scene without a session running,
 *  Climax nudges you to track it (backdated). `threshold_minutes` = watch time
 *  before it fires; `snooze_minutes` = suppression after you dismiss. */
export interface CapturePromptSetting {
  enabled: boolean;
  threshold_minutes: number;
  snooze_minutes: number;
}

/** The pending "track this?" prompt, surfaced as a bottom-right tracker toast. */
export interface CapturePayload {
  source: string;
  scene_id: string;
  content_item_id: number;
  /** When Climax first saw the watch begin — the backdated session start. */
  watch_started_at: number;
  title: string | null;
  thumbnail_url: string | null;
}

/** Play-counting threshold — mirrors Stash's "minimum play percent". A
 *  scene_play only "counts" once seconds_tracked >= duration × pct/100.
 *  null = never configured (uses 0% fallback until first-launch Stash sync
 *  populates it). */
export interface PlayCountingSetting {
  threshold_pct: number | null;
}

/** Stash connection — URL + optional API key, stored on the 'stash' source row. */
export interface StashConnectionSetting {
  /** Base URL, no trailing slash. e.g. "http://localhost:9999". */
  url: string;
  /** API key for Stash's `ApiKey` header. null when Stash has no auth on. */
  api_key: string | null;
}

/** Library-metadata sync (scene + entity details). `enabled` gates the periodic
 *  refresh; `cadence_minutes` is how often it runs and the per-sighting staleness. */
export interface MetadataRefreshSetting {
  enabled: boolean;
  cadence_minutes: number;
}

/** Stash-history mirror sync (Phase 7). `enabled` gates the lightweight periodic
 *  background reconcile of already-known scenes; the full-catalog import is
 *  on-demand regardless of this flag. */
export interface MirrorSyncSetting {
  enabled: boolean;
  cadence_minutes: number;
}

/** Live mirror import / reconcile progress (poll `mirrorStatusGet`). */
export interface MirrorStatus {
  /** "idle" | "importing" | "reconciling" */
  phase: string;
  done: number;
  total: number;
  last_finished_at: number | null;
  last_error: string | null;
}

/** Single-scene mirror self-check: Climax's stash_synced O count vs Stash's live
 *  o_counter. `matches` is the mirror guarantee for that scene. */
export interface MirrorCheck {
  scene_id: string;
  content_id: number | null;
  climax_o_events: number;
  stash_o_counter: number;
  matches: boolean;
}

/** Result of pinging Stash GraphQL to validate URL + API key. */
export interface TestConnectionResult {
  ok: boolean;
  version: string | null;
  scene_count: number | null;
  error: string | null;
}

/** Climax bridge plugin state for the onboarding bridge step.
 *  `installed`/`enabled`/`version` come from Stash's plugin list; `connected`
 *  is true while a bridge is live on Climax's WebSocket (Stash tab open). */
export interface BridgeStatus {
  installed: boolean;
  enabled: boolean;
  version: string | null;
  connected: boolean;
}

/** Payload describing an in-progress idle stretch. Stored on the backend the
 *  moment the user crosses the idle threshold; the floating idle-prompt
 *  window reads it via `idle_pending_get` and live-updates the duration. */
export interface IdlePayload {
  session_id: number;
  /** Wall-clock ms of the user's last input before the AFK stretch. */
  idle_started_at: number;
}

/** Payload for the crash-recovery prompt: a session was left running from a
 *  previous run (crash / force-kill / power loss / OS shutdown) and the gap
 *  since its last heartbeat exceeded the recovery threshold. Set on boot, read
 *  by the tracker via `crash_pending_get`. `gap_started_at` is the last
 *  heartbeat = the natural session end time if the user picks Discard & End. */
export interface CrashPayload {
  session_id: number;
  gap_started_at: number;
}

export const api = {
  sessionStart: () => invoke<Session>("session_start"),
  sessionStop: () => invoke<Session | null>("session_stop"),
  /** Discard the active session — status='discarded', rows kept in DB
   *  but filtered out of every dashboard query. Triggered from the
   *  wrap-up modal's two-tap confirmation. */
  sessionDiscard: () => invoke<Session | null>("session_discard"),
  sessionPause: () => invoke<Session | null>("session_pause"),
  sessionResume: () => invoke<Session | null>("session_resume"),
  sessionActive: () => invoke<Session | null>("session_active"),
  sessionGet: (id: number) => invoke<Session | null>("session_get", { id }),
  sessionsList: (limit = 50, offset = 0) =>
    invoke<Session[]>("sessions_list", { limit, offset }),
  /** One page of the dashboard Sessions table: filtered by assigned_day range,
   *  sorted by `sort` ("start"|"duration"|"scenes"|"cumshots") in `desc` order,
   *  paginated. Returns the page rows + total matching count (sort holds across
   *  pages — SQLite does the ORDER BY/LIMIT). */
  sessionsPage: (args: {
    startDay: string;
    endDay: string;
    sort?: SessionSort;
    desc?: boolean;
    limit?: number;
    offset?: number;
  }) =>
    invoke<SessionPage>("sessions_page", {
      startDay: args.startDay,
      endDay: args.endDay,
      sort: args.sort ?? "start",
      desc: args.desc ?? true,
      limit: args.limit ?? 50,
      offset: args.offset ?? 0,
    }),
  sessionsInRange: (startMs: number, endMs: number) =>
    invoke<Session[]>("sessions_in_range", { startMs, endMs }),
  sessionsForDay: (day: string) =>
    invoke<Session[]>("sessions_for_day", { day }),
  sessionSetAssignedDay: (sessionId: number, day: string) =>
    invoke<Session>("session_set_assigned_day", { sessionId, day }),
  sessionScenes: (sessionId: number) =>
    invoke<PlayedScene[]>("session_scenes", { sessionId }),
  oLog: (args: OLogArgs) =>
    invoke<OEvent>("o_log", {
      sessionId: args.sessionId ?? null,
      contentItemId: args.contentItemId ?? null,
      occurredAt: args.occurredAt ?? null,
      intensity: args.intensity ?? null,
      notes: args.notes ?? null,
    }),
  oListForSession: (sessionId: number) =>
    invoke<OEvent[]>("o_list_for_session", { sessionId }),
  oDelete: (oId: number) => invoke<void>("o_delete", { oId }),
  oDeleteRecentForScene: (sessionId: number, contentItemId: number, count: number) =>
    invoke<number>("o_delete_recent_for_scene", { sessionId, contentItemId, count }),
  /** Today's session-less, scene-less ("unlinked") cumshots — logged with no
   *  session and no scene. Powers the empty-tracker +/- counter. */
  oSessionlessUnlinkedToday: () =>
    invoke<OEvent[]>("o_sessionless_unlinked_today", {}),
  oEventUpdate: (oId: number, patch: {
    occurredAt?: number;
    contentItemId?: number;
    notes?: string;
    intensity?: number;
  }) =>
    invoke<OEvent>("o_event_update", {
      oId,
      occurredAt: patch.occurredAt ?? null,
      contentItemId: patch.contentItemId ?? null,
      notes: patch.notes ?? null,
      intensity: patch.intensity ?? null,
    }),
  sessionDelete: (sessionId: number) => invoke<void>("session_delete", { sessionId }),
  sessionDeleteMany: (sessionIds: number[]) =>
    invoke<number>("session_delete_many", { sessionIds }),
  /** Remove one scene from a session without deleting the session. Pulls the
   *  scene's watch time + counted play out of the session and rolls the same
   *  back out of Stash; any cumshots on that scene in the session go too. */
  sceneDeleteFromSession: (sessionId: number, contentItemId: number) =>
    invoke<void>("scene_delete_from_session", { sessionId, contentItemId }),
  /** Non-destructive remove: detach a scene from the session WITHOUT rolling it
   *  back out of Stash, so it stays recoverable / re-homeable elsewhere. */
  sceneDetachFromSession: (sessionId: number, contentItemId: number) =>
    invoke<void>("scene_detach_from_session", { sessionId, contentItemId }),
  sessionUpdate: (sessionId: number, patch: {
    startedAt?: number;
    endedAt?: number;
    notes?: string;
    excluded?: boolean;
    sessionType?: string;
  }) =>
    invoke<Session>("session_update", {
      sessionId,
      startedAt: patch.startedAt ?? null,
      endedAt: patch.endedAt ?? null,
      notes: patch.notes ?? null,
      excluded: patch.excluded ?? null,
      sessionType: patch.sessionType ?? null,
    }),

  // ---------- Idle detection ----------
  idleDetectionGet: () =>
    invoke<IdleDetectionSetting>("idle_detection_get"),
  idleDetectionSet: (enabled: boolean, thresholdMinutes: number) =>
    invoke<IdleDetectionSetting>("idle_detection_set", {
      enabled,
      thresholdMinutes,
    }),

  // ---------- Remote-device tracking ----------
  remoteTrackingGet: () =>
    invoke<RemoteTrackingSetting>("remote_tracking_get"),
  remoteTrackingSet: (enabled: boolean, pollSeconds: number) =>
    invoke<RemoteTrackingSetting>("remote_tracking_set", {
      enabled,
      pollSeconds,
    }),

  // ---------- Wording (cumshot / orgasm) ----------
  // Display only. Read and written through src/lib/wording.svelte.ts, which
  // owns the reactive copy every component reads - call that, not these.
  wordingGet: () => invoke<string>("wording_get"),
  wordingSet: (value: string) => invoke<string>("wording_set", { value }),

  // ---------- Keep empty sessions ----------
  // Off (the default): stopping a session with no counted scene and no cumshot
  // discards it. On: it is kept, so Climax works as a plain timer.
  keepEmptySessionsGet: () => invoke<boolean>("keep_empty_sessions_get"),
  keepEmptySessionsSet: (enabled: boolean) =>
    invoke<boolean>("keep_empty_sessions_set", { enabled }),

  // ---------- Passive-capture prompt ----------
  capturePromptGet: () =>
    invoke<CapturePromptSetting>("capture_prompt_get"),
  capturePromptSet: (enabled: boolean, thresholdMinutes: number, snoozeMinutes: number) =>
    invoke<CapturePromptSetting>("capture_prompt_set", {
      enabled,
      thresholdMinutes,
      snoozeMinutes,
    }),
  /** Pending capture prompt, if one fired before the tracker started listening. */
  capturePendingGet: () =>
    invoke<CapturePayload | null>("capture_pending_get"),
  /** Accept: start a session backdated to when watching began. Returns it. */
  captureAccept: () =>
    invoke<Session | null>("capture_accept"),
  /** Snooze/dismiss the prompt for the configured grace. */
  captureSnooze: () =>
    invoke<void>("capture_snooze"),
  /** Read the local play-count threshold. */
  playCountingGet: () =>
    invoke<PlayCountingSetting>("play_counting_get"),
  /** Save the local threshold. Pass null to clear (the next launch's
   *  Stash-sync will then repopulate it). */
  playCountingSet: (thresholdPct: number | null) =>
    invoke<PlayCountingSetting>("play_counting_set", { thresholdPct }),
  /** Force a re-fetch of Stash's minimumPlayPercent and overwrite the
   *  local value. For the Settings "Sync from Stash" button. */
  playCountingSyncFromStash: () =>
    invoke<PlayCountingSetting>("play_counting_sync_from_stash"),
  /** Returns the currently-active idle prompt's payload, or null if there
   *  isn't one. Idle-prompt window calls this on mount. */
  idlePendingGet: () =>
    invoke<IdlePayload | null>("idle_pending_get"),
  /** "Keep" — do nothing, idle time counts. Clears the prompt. */
  idleKeep: () =>
    invoke<void>("idle_keep"),
  /** "Discard & Continue" — insert an idle pause for [started, now], session
   *  keeps running. */
  idleDiscardContinue: (sessionId: number, idleStartedAt: number) =>
    invoke<void>("idle_discard_continue", { sessionId, idleStartedAt }),
  /** "Discard & End" — end the session at `idleStartedAt`. */
  idleDiscardEnd: (sessionId: number, idleStartedAt: number) =>
    invoke<Session | null>("idle_discard_end", { sessionId, idleStartedAt }),

  // ---------- Quit guard ----------
  /** Whether a Quit was requested with a session running (tracker reads on mount). */
  quitPendingGet: () => invoke<boolean>("quit_pending_get"),
  /** Actually exit the app (after the wrap-up "Save & close" stopped the session). */
  confirmQuit: () => invoke<void>("confirm_quit"),
  /** "Go back" — abandon the quit, session keeps running. */
  cancelQuit: () => invoke<void>("cancel_quit"),

  // ---------- Crash recovery ----------
  /** The pending crash-recovery prompt, or null. Tracker reads on mount. */
  crashPendingGet: () => invoke<CrashPayload | null>("crash_pending_get"),
  /** "Keep" — the downtime counts as session time; session continues. */
  crashKeep: () => invoke<void>("crash_keep"),
  /** "Discard & Continue" — exclude the downtime (crash pause), keep running. */
  crashDiscardContinue: (sessionId: number, gapStartedAt: number) =>
    invoke<void>("crash_discard_continue", { sessionId, gapStartedAt }),
  /** "Discard & End" — end the session at `gapStartedAt` (last heartbeat). */
  crashDiscardEnd: (sessionId: number, gapStartedAt: number) =>
    invoke<Session | null>("crash_discard_end", { sessionId, gapStartedAt }),

  // ---------- General settings (startup / shortcut) ----------
  /** What Climax shows on launch: "silent" | "tracker" | "dashboard". */
  launchBehaviorGet: () => invoke<string>("launch_behavior_get"),
  launchBehaviorSet: (mode: string) => invoke<void>("launch_behavior_set", { mode }),
  /** Global Ctrl+Shift+L shortcut on/off (applies immediately). */
  hotkeyEnabledGet: () => invoke<boolean>("hotkey_enabled_get"),
  hotkeyEnabledSet: (enabled: boolean) => invoke<void>("hotkey_enabled_set", { enabled }),
  /** Launch Climax at Windows login (autostart). */
  autostartGet: () => invoke<boolean>("autostart_get"),
  autostartSet: (enabled: boolean) => invoke<void>("autostart_set", { enabled }),

  // ---------- Dashboard / Overview ----------
  dashboardHeroStats: () =>
    invoke<HeroStats>("dashboard_hero_stats"),
  dashboardMonthlyBuckets: (months?: number) =>
    invoke<MonthlyBucket[]>("dashboard_monthly_buckets", { months: months ?? null }),
  dashboardDailyBuckets: (year: number, month: number) =>
    invoke<DailyBucket[]>("dashboard_daily_buckets", { year, month }),

  // ---------- Stash connection ----------
  stashConnectionGet: () =>
    invoke<StashConnectionSetting>("stash_connection_get"),
  stashConnectionSet: (url: string, apiKey: string | null) =>
    invoke<StashConnectionSetting>("stash_connection_set", { url, apiKey }),
  /** Ping Stash GraphQL with the currently-saved URL + key. Persist before testing. */
  stashConnectionTest: () =>
    invoke<TestConnectionResult>("stash_connection_test"),

  // ---------- Bridge plugin (onboarding) ----------
  /** Is the Climax bridge installed in Stash, and is one connected right now? */
  bridgeStatus: () => invoke<BridgeStatus>("bridge_status"),
  /** Install the bridge from the public plugin index; resolves with refreshed
   *  status. Blocks up to ~30s while Stash runs the install job + reloads. */
  bridgeInstall: () => invoke<BridgeStatus>("bridge_install"),

  // ---------- Filter-aware dashboard queries (Phase 3) ----------
  dashboardRangeStats: (args: FilterArgs) =>
    invoke<RangeStats>("dashboard_range_stats", {
      startDay: args.startDay,
      endDay: args.endDay,
      sceneContentItemIds: args.sceneContentItemIds ?? null,
      performerIds: args.performerIds ?? null,
      studioIds: args.studioIds ?? null,
      tagIds: args.tagIds ?? null,
    }),
  dashboardFilteredBuckets: (args: FilterArgs & { granularity: "day" | "week" | "month" | "year" }) =>
    invoke<RangeBucket[]>("dashboard_filtered_buckets", {
      startDay: args.startDay,
      endDay: args.endDay,
      granularity: args.granularity,
      sceneContentItemIds: args.sceneContentItemIds ?? null,
      performerIds: args.performerIds ?? null,
      studioIds: args.studioIds ?? null,
      tagIds: args.tagIds ?? null,
    }),
  dashboardFilteredSessionCount: (args: FilterArgs) =>
    invoke<number>("dashboard_filtered_session_count", {
      startDay: args.startDay,
      endDay: args.endDay,
      sceneContentItemIds: args.sceneContentItemIds ?? null,
      performerIds: args.performerIds ?? null,
      studioIds: args.studioIds ?? null,
      tagIds: args.tagIds ?? null,
    }),

  // ---------- Catalog list / search (filter pickers) ----------
  catalogListPerformers: () =>
    invoke<NamedEntity[]>("catalog_list_performers"),
  catalogListStudios: () =>
    invoke<NamedEntity[]>("catalog_list_studios"),
  catalogListTags: () =>
    invoke<NamedEntity[]>("catalog_list_tags"),
  catalogSearchScenes: (query: string, limit?: number) =>
    invoke<SceneEntity[]>("catalog_search_scenes", { query, limit: limit ?? null }),

  // ---------- Catalog browse (Phase 5: Scenes / Performers / Studios / Tags) ----------
  /** Every watched scene with aggregated activity (watch time, cumshots,
   *  sessions, last-watched) + parsed metadata. Frontend sorts/filters/searches
   *  in-memory. */
  catalogBrowseScenes: () => invoke<BrowseScene[]>("catalog_browse_scenes"),
  catalogBrowsePerformers: () => invoke<BrowsePerformer[]>("catalog_browse_performers"),
  catalogBrowseStudios: () => invoke<BrowseStudio[]>("catalog_browse_studios"),
  catalogBrowseTags: () => invoke<BrowseTag[]>("catalog_browse_tags"),
  /** Fire-and-forget: background-enrich performers/studios/tags from Stash
   *  (images, favorite, demographics, parent studio). kind ∈ performer|studio|tag. */
  catalogEnrichEntities: (kind: "performer" | "studio" | "tag") =>
    invoke<void>("catalog_enrich_entities", { kind }),
  /** Earliest assigned_day in the catalog (YYYY-MM-DD), or null if no
   *  sessions yet. Used by the filter store to dynamically clip "All time". */
  dashboardFirstSessionDay: () =>
    invoke<string | null>("dashboard_first_session_day"),

  // ---------- Default date-range preset (per dashboard section / scope) ----------
  /** Returns the saved default date preset for a section ("overview",
   *  "sessions", ...), or "last_12_months" if unset / malformed. */
  defaultDatePresetGet: (scope: string) =>
    invoke<string>("default_date_preset_get", { scope }),
  defaultDatePresetSet: (scope: string, preset: string) =>
    invoke<string>("default_date_preset_set", { scope, preset }),

  /** Per-chart default metric ("cumshots" | "watch_time" | "sessions") — the
   *  metric that chart's toggle starts on. `scope` is the chart id (e.g.
   *  "overview", "trends_area", "trends_rankings"); each is independent. */
  defaultChartMetricGet: (scope: string) =>
    invoke<string>("default_chart_metric_get", { scope }),
  defaultChartMetricSet: (scope: string, metric: string) =>
    invoke<string>("default_chart_metric_set", { scope, metric }),

  /** Default side of the Trends Top-performers female/male toggle
   *  ("female" | "male"); set via that toggle's Default pill. */
  defaultPerformerGenderGet: () =>
    invoke<string>("default_performer_gender_get"),
  defaultPerformerGenderSet: (gender: "female" | "male") =>
    invoke<string>("default_performer_gender_set", { gender }),

  /** Per-page saved default view (search/filters/sort/rows JSON) for a browse
   *  page; null if unset. `kind` ∈ scene | performer | studio | tag. */
  browseDefaultGet: (kind: "scene" | "performer" | "studio" | "tag") =>
    invoke<string | null>("browse_default_get", { kind }),
  browseDefaultSet: (kind: "scene" | "performer" | "studio" | "tag", view: string) =>
    invoke<void>("browse_default_set", { kind, view }),

  // ---------- Library metadata sync ----------
  /** Metadata sync setting (enabled + cadence in minutes). */
  metadataRefreshGet: () => invoke<MetadataRefreshSetting>("metadata_refresh_get"),
  metadataRefreshSet: (enabled: boolean, cadenceMinutes: number) =>
    invoke<MetadataRefreshSetting>("metadata_refresh_set", { enabled, cadenceMinutes }),
  /** Live metadata-sync progress; poll while a run is in flight. */
  metadataStatusGet: () => invoke<MirrorStatus>("metadata_status_get"),
  /** Refresh library metadata now (background): scene details for known scenes
   *  plus performer / studio / tag details and images. Poll metadataStatusGet. */
  metadataSyncNow: () => invoke<void>("metadata_sync_now"),
  /** Force-refresh one content_item's metadata from Stash, ignoring TTL. Used by
   *  the per-scene refresh button in SessionDetail. */
  refreshSceneMetadata: (contentItemId: number) =>
    invoke<void>("refresh_scene_metadata", { contentItemId }),

  // ---------- Stash-history mirror (Phase 7) ----------
  /** Background-reconcile setting (enabled + cadence in minutes). */
  mirrorSyncGet: () => invoke<MirrorSyncSetting>("mirror_sync_get"),
  mirrorSyncSet: (enabled: boolean, cadenceMinutes: number) =>
    invoke<MirrorSyncSetting>("mirror_sync_set", { enabled, cadenceMinutes }),
  /** Live sync progress; poll while a run is in flight. */
  mirrorStatusGet: () => invoke<MirrorStatus>("mirror_status_get"),
  /** Run the mirror sync now (background): pulls every active Stash scene,
   *  discovers ones watched while Climax was closed, refreshes cumshots / play
   *  count / watch duration. Also runs automatically on launch + on cadence. */
  mirrorSyncNow: () => invoke<void>("mirror_sync_now"),
  /** Mirror self-check for one Stash scene id (verification aid): compares
   *  Climax's stash_synced O count to Stash's live o_counter. */
  mirrorCheckScene: (sceneId: string) =>
    invoke<MirrorCheck>("mirror_check_scene", { sceneId }),

  // ---------- Trends (Phase 6) ----------
  /** Contiguous, zero-filled per-day activity over [startDay, endDay]. The
   *  Trends page's backbone — sliced client-side for summary cards, area chart,
   *  busiest days, heatmap, and time-grouped report rows. */
  trendsDailySeries: (startDay: string, endDay: string) =>
    invoke<DailyActivity[]>("trends_daily_series", { startDay, endDay }),
  /** Per-local-hour {cumshots, sessions, active_days} (24 buckets) for the
   *  range. Powers Peak hours + the report's Time-of-day grouping. */
  trendsHourHistogram: (startDay: string, endDay: string) =>
    invoke<HourHistogram>("trends_hour_histogram", { startDay, endDay }),
  /** Range-scoped per-entity totals for the leaderboards + report entity dims.
   *  `dimension` ∈ scene | performer | studio | tag (scene ids are stringified
   *  content_items.id). */
  trendsEntityBreakdown: (dimension: "scene" | "performer" | "studio" | "tag", startDay: string, endDay: string) =>
    invoke<EntityBreakdownRow[]>("trends_entity_breakdown", { dimension, startDay, endDay }),
  /** Export CSV via a native Save dialog. Returns the chosen path, or null if
   *  the user cancelled. */
  trendsExportCsv: (contents: string, defaultName: string) =>
    invoke<string | null>("trends_export_csv", { contents, defaultName }),

  // ---------- Retroactive session reconstruction (Phase 7, component 2) ----------
  /** Cluster the session-less Stash-history imports in [startDay, endDay] into
   *  candidate sessions. `gapMinutes` overrides the saved setting; omit to use
   *  it. `respectDismissed` (default true) hides previously-dismissed candidates
   *  — pass false for the "import older history" full scan. */
  reconstructCandidates: (
    startDay: string,
    endDay: string,
    opts?: { gapMinutes?: number; respectDismissed?: boolean },
  ) =>
    invoke<CandidateSession[]>("reconstruct_candidates", {
      startDay,
      endDay,
      gapMinutes: opts?.gapMinutes ?? null,
      respectDismissed: opts?.respectDismissed ?? null,
    }),
  /** Accept a (possibly edited) candidate: create an `estimated` session, adopt
   *  its loose cumshots, write per-scene watch rows. Returns the new session id. */
  reconstructAccept: (args: {
    startMs: number;
    endMs: number;
    assignedDay: string;
    oEventIds: number[];
    scenes: AcceptScene[];
  }) =>
    invoke<number>("reconstruct_accept", {
      startMs: args.startMs,
      endMs: args.endMs,
      assignedDay: args.assignedDay,
      oEventIds: args.oEventIds,
      scenes: args.scenes,
    }),
  /** Dismiss candidates so the prompt won't re-offer them (still reachable via
   *  the full scan). Returns rows recorded. */
  reconstructDismiss: (playImportIds: number[], oEventIds: number[]) =>
    invoke<number>("reconstruct_dismiss", { playImportIds, oEventIds }),
  /** The "while you were away" launch scan: reconstruction candidates AFTER the
   *  most recent logged session (the recency floor), dismissed-respecting. Uses the
   *  saved session-gap. `since_ms` is null on a fresh DB (no prior session). */
  reconstructAwayCandidates: () => invoke<AwayResult>("reconstruct_away_candidates"),
  /** Session-gap reconstruction threshold (minutes). */
  sessionGapGet: () => invoke<SessionGapSetting>("session_gap_get"),
  sessionGapSet: (gapMinutes: number) =>
    invoke<SessionGapSetting>("session_gap_set", { gapMinutes }),
  // ---- First-launch onboarding ----
  /** Whether the first-launch setup wizard should run (fresh install, Stash on). */
  onboardingNeeded: () => invoke<boolean>("onboarding_needed"),
  /** Mark the setup wizard finished. */
  onboardingComplete: () => invoke<void>("onboarding_complete"),
  /** Reset the first-launch flag so the wizard runs again (re-run setup / test). */
  onboardingReset: () => invoke<void>("onboarding_reset"),
  /** Onboarding "estimate past sessions": bulk-accept all reconstruction
   *  candidates across history. Returns the number of estimated sessions made. */
  onboardingEstimateSessions: () => invoke<number>("onboarding_estimate_sessions"),
  /** Stash play/o history inside a completed session's window that the session
   *  doesn't track yet (the "Stash logged activity here" review). */
  sessionPendingHistory: (sessionId: number, includeActive = false, includeDismissed = false) =>
    invoke<SessionPendingHistory>("session_pending_history", { sessionId, includeActive, includeDismissed }),
  /** Fold selected Stash scenes/cumshots into the session (review-first add). */
  sessionAbsorbHistory: (sessionId: number, contentItemIds: number[], oEventIds: number[]) =>
    invoke<void>("session_absorb_history", { sessionId, contentItemIds, oEventIds }),
  /** Skip selected Stash history so the session review stops offering it. Pass
   *  `mirrorToReconstruction` true from the WRAP-UP skip only (its session can
   *  auto-discard, so the skip must also suppress the events from the "while you
   *  were away" / Untracked reconstruction prompt). The SessionDetail fold review
   *  leaves it false (ended session; mirroring would over-hide). */
  sessionDismissHistory: (
    sessionId: number,
    contentItemIds: number[],
    oEventIds: number[],
    mirrorToReconstruction = false,
  ) =>
    invoke<void>("session_dismiss_history", {
      sessionId,
      contentItemIds,
      oEventIds,
      mirrorToReconstruction,
    }),
  /** Preview merging two sessions (whether it's allowed + the time gap). */
  sessionMergeCheck: (aId: number, bId: number) =>
    invoke<MergeCheck>("session_merge_check", { aId, bId }),
  /** Merge two consecutive sessions into one spanning both. `gapAsPause` drops a
   *  pause for the gap so the duration stays accurate. Returns the survivor id. */
  sessionMerge: (aId: number, bId: number, gapAsPause: boolean) =>
    invoke<number>("session_merge", { aId, bId, gapAsPause }),
  /** Preview reopening a session: can it be made live again, and how long ago
   *  did it end. */
  sessionReopenCheck: (sessionId: number) =>
    invoke<ReopenCheck>("session_reopen_check", { sessionId }),
  /** Reopen an ended session so it tracks live again (stopped by accident).
   *  `gapAsPause` makes the time since it ended a pause rather than counted
   *  session time. Returns null if it can't be reopened. */
  sessionReopen: (sessionId: number, gapAsPause: boolean) =>
    invoke<Session | null>("session_reopen", { sessionId, gapAsPause }),
  /** Preview continuing a session: is there untracked activity right after it to
   *  fold in, plus the gap + counts. */
  sessionExtendCheck: (sessionId: number) =>
    invoke<ExtendSessionCheck>("session_extend_check", { sessionId }),
  /** Continue a session: fold the untracked activity that follows it into the
   *  session (extending its end). `gapAsPause` makes the gap a pause rather than
   *  counted session time. Returns false if there was nothing to continue. */
  sessionExtend: (sessionId: number, gapAsPause: boolean) =>
    invoke<boolean>("session_extend", { sessionId, gapAsPause }),

  // ---------- Backup and restore (Phase 7) ----------
  /** Saved backup settings: last destination folder + last-backup time. */
  backupSettingsGet: () => invoke<BackupSetting>("backup_settings_get"),
  /** The folder holding the live DB - and where each restore drops a
   *  *-pre-restore-*.sqlite safety copy. Shown in the Restore section so the user
   *  knows where their rollback file is. */
  backupDataFolder: () => invoke<string>("backup_data_folder"),
  /** Back up the whole library to a folder you pick (timestamped, never
   *  overwriting). Returns where it landed + the updated setting, or null if the
   *  folder picker was cancelled. */
  backupNow: () => invoke<BackupResult | null>("backup_now"),
  /** Pick a backup file and validate it. Returns the candidate to confirm, null
   *  if cancelled; throws if the file isn't a usable Climax backup. */
  backupRestorePick: () => invoke<RestoreCandidate | null>("backup_restore_pick"),
  /** Apply a restore: snapshots the current DB as a safety net, stages the file,
   *  and RESTARTS the app to swap it in. The promise will not resolve on success
   *  (the app relaunches). */
  backupRestoreApply: (path: string) =>
    invoke<void>("backup_restore_apply", { path }),
  /** Full reset: wipe the ENTIRE database (incl. settings) and RESTART, recreating
   *  a pristine install (onboarding re-arms). The promise won't resolve on success
   *  (the app relaunches). Guard behind a strong confirmation in the UI. */
  backupReset: () => invoke<void>("backup_reset"),

  // ---------- Profile (real vs scratch) ----------
  /** The active launch profile + whether Stash is on. Scratch = disposable test
   *  sandbox with Stash off; the UI shows a SCRATCH badge for it. */
  profileGet: () => invoke<ProfileInfo>("profile_get"),

  // ---------- Server connection (Phase 2 app-as-client) ----------
  /** Probe whether `base` (a server origin like "http://localhost:9998") is a
   *  reachable, healthy Climax server. Runs from Rust (CORS-free) - the webview
   *  can't reach the server cross-origin. Drives the connection indicator's poll
   *  + the Settings test button. Never throws for an unreachable host: returns
   *  false. NOTE: a NATIVE command (always local IPC, never routed to the
   *  server), so it works regardless of client mode. */
  serverPing: (base: string) => invoke<boolean>("server_ping", { base }),

  /** Rust-side client-mode boot config (which external server, if any). Mirrors
   *  the frontend's localStorage; the shell reads it at boot to decide whether to
   *  spawn its in-process backend. Setting it takes effect on the next launch.
   *  NATIVE (always local IPC). */
  clientConfigGet: () => invoke<string | null>("client_config_get"),
  clientConfigSet: (serverUrl: string | null, token: string | null = null) =>
    invoke<void>("client_config_set", { serverUrl, token }),

  /** The port THIS app's own backend serves on - what the bridge connects to -
   *  plus whether it actually bound it at boot. `ok: false` means nothing can
   *  reach Climax on this PC, so the tracker and dashboard say so. NATIVE: routed
   *  to a server it would answer about the server's port instead. */
  serverPortGet: () => invoke<ServerPortInfo>("server_port_get"),
  /** Persist the port. Applies on the next launch (the server binds at boot), so
   *  the caller restarts the app. NATIVE. */
  serverPortSet: (port: number) => invoke<void>("server_port_set", { port }),

  // ---------- Updates ----------
  /** The newest published release, from the backend's cache. Never touches the
   *  network, so it's safe to call from any view on mount. Note
   *  `backend_version` is the version of whatever ANSWERED - in client mode
   *  that's the server, not this app. */
  updateCheck: () => invoke<UpdateInfo>("update_check"),
  /** Ask GitHub now, ignoring the once-a-day cache. Awaited, so it can take a
   *  few seconds - show a busy label. */
  updateCheckNow: () => invoke<UpdateInfo>("update_check_now"),
  /** Stop showing the update notice until something newer than this exists. */
  updateDismiss: (version: string) => invoke<void>("update_dismiss", { version }),
  /** THIS app's version (from tauri.conf.json). NATIVE - over /rpc it would
   *  report the server's version instead, and in client mode they differ. */
  appVersionGet: () => invoke<string>("app_version_get"),
  /** Relaunch the app (so the Rust boot re-reads the client config). Returns true
   *  if it relaunched (release build; the promise won't really resolve), false in
   *  dev where auto-restart would orphan Vite (the caller asks for a manual
   *  restart instead). NATIVE. */
  restartApp: () => invoke<boolean>("restart_app"),
};

// ---------- Dashboard types ----------

export interface HeroStats {
  cumshots_today: number;
  record_day_count: number;
  record_day_date: string | null;
  current_streak_days: number;
  streak_at_risk: boolean;
  /** Longest run of consecutive days with a logged session, ever, and its
   *  inclusive bounds. Same measure as `current_streak_days` but not anchored
   *  to today, so a run still going can BE the record. 0 = nothing logged. */
  record_streak_days: number;
  record_streak_start: string | null;
  record_streak_end: string | null;
  days_since_last_cumshot: number | null;
  longest_session_ms: number;
  longest_session_date: string | null;
  longest_session_id: number | null;
  month_watch_time_ms: number;
  month_cumshots: number;
  month_sessions: number;
  month_active_days: number;
}

export interface MonthlyBucket {
  /** YYYY-MM. */
  month: string;
  watch_time_ms: number;
  cumshots: number;
  sessions: number;
}

export interface DailyBucket {
  /** YYYY-MM-DD. */
  day: string;
  watch_time_ms: number;
  cumshots: number;
  sessions: number;
}

/** Filter-aware bucket for the dashboard bar chart. `key` is YYYY-MM-DD
 *  (granularity="day"/"week" — week keys are the Monday) or YYYY-MM
 *  ("month") or YYYY ("year"). Mirrors the `Granularity` union in
 *  filter-store; kept inline here to avoid a circular import. */
export interface RangeBucket {
  key: string;
  granularity: "day" | "week" | "month" | "year";
  watch_time_ms: number;
  cumshots: number;
  sessions: number;
}

/** Hero second-row stats scoped to the filter range. Replaces the old
 *  "this month" fields on `HeroStats` when a non-trivial filter is set. */
export interface RangeStats {
  watch_time_ms: number;
  cumshots: number;
  sessions: number;
  active_days: number;
}

/** {id, name} pair for performer / studio / tag pickers. IDs are
 *  Stash-side strings. */
export interface NamedEntity {
  id: string;
  name: string;
}

// ---------- Catalog browse rows (Phase 5) ----------

/** A watched scene with aggregated activity, for the Scenes grid + detail. */
export interface BrowseScene {
  id: number; // content_items.id
  external_id: string | null;
  title: string | null;
  url: string | null;
  thumbnail_url: string | null;
  duration_seconds: number | null;
  resolution: string | null;
  studio: NamedEntity | null;
  performers: NamedEntity[];
  tags: NamedEntity[];
  watch_time_ms: number;
  /** Mirror total: all O's incl. session-less Stash-history imports (Phase 7). */
  cumshots: number;
  /** Session-attributed subset of `cumshots` (logged inside Climax sessions). */
  cumshots_tracked: number;
  sessions: number;
  /** Stash's play_count for the scene (or summed across an entity's scenes). */
  play_count: number;
  last_watched_at: number | null;
}

export interface BrowsePerformer {
  id: string;
  name: string;
  scene_count: number;
  watch_time_ms: number;
  /** Mirror total: all O's incl. session-less Stash-history imports (Phase 7). */
  cumshots: number;
  /** Session-attributed subset of `cumshots` (logged inside Climax sessions). */
  cumshots_tracked: number;
  sessions: number;
  /** Stash's play_count for the scene (or summed across an entity's scenes). */
  play_count: number;
  last_watched_at: number | null;
  /** Stash enrichment (background-filled; null/empty until enriched). */
  image_url: string | null;
  favorite: boolean;
  /** Stash GenderEnum (FEMALE / MALE / TRANSGENDER_FEMALE / ...); null until
   *  enriched or unset in Stash. */
  gender: string | null;
  ethnicity: string | null;
  country: string | null;
  height_cm: number | null;
  hair_color: string | null;
  aliases: string[];
}

export interface BrowseStudio {
  id: string;
  name: string;
  parent_studio: string | null;
  scene_count: number;
  watch_time_ms: number;
  /** Mirror total: all O's incl. session-less Stash-history imports (Phase 7). */
  cumshots: number;
  /** Session-attributed subset of `cumshots` (logged inside Climax sessions). */
  cumshots_tracked: number;
  sessions: number;
  /** Stash's play_count for the scene (or summed across an entity's scenes). */
  play_count: number;
  last_watched_at: number | null;
  image_url: string | null;
}

export interface BrowseTag {
  id: string;
  name: string;
  scene_count: number;
  watch_time_ms: number;
  /** Mirror total: all O's incl. session-less Stash-history imports (Phase 7). */
  cumshots: number;
  /** Session-attributed subset of `cumshots` (logged inside Climax sessions). */
  cumshots_tracked: number;
  sessions: number;
  /** Stash's play_count for the scene (or summed across an entity's scenes). */
  play_count: number;
  last_watched_at: number | null;
  image_url: string | null;
}

/** Compact scene row for the scene-picker typeahead. id is
 *  `content_items.id`, NOT the Stash external_id. */
export interface SceneEntity {
  id: number;
  title: string | null;
  external_id: string | null;
  thumbnail_url: string | null;
}

// ---------- Trends rows (Phase 6) ----------

/** One day of activity. `active` is derived client-side as sessions > 0;
 *  the backend omits it (and dow/ts, also derived client-side). */
export interface DailyActivity {
  /** YYYY-MM-DD (assigned_day). */
  date: string;
  sessions: number;
  cumshots: number;
  watch_time_ms: number;
}

/** Per-local-hour distribution; each array is 24 entries indexed by hour. */
export interface HourHistogram {
  cumshots: number[];
  sessions: number[];
  active_days: number[];
}

/** Range-scoped per-entity totals (performer / studio / tag). */
export interface EntityBreakdownRow {
  id: string;
  name: string;
  watch_time_ms: number;
  cumshots: number;
  sessions: number;
  scene_count: number;
  /** Performer rows only: Stash GenderEnum; null until enriched / unset. */
  gender: string | null;
}

// ---------- Retroactive session reconstruction (Phase 7, component 2) ----------

/** A scene within a candidate session, with its in-cluster timestamps + the
 *  underlying import row ids (so accept/dismiss can act on exact rows). */
export interface CandidateScene {
  content_item_id: number;
  external_id: string | null;
  title: string | null;
  thumbnail_url: string | null;
  play_times: number[];
  cumshot_times: number[];
  play_import_ids: number[];
  o_event_ids: number[];
  /** Estimated seconds watched in this session for this scene (approximate). */
  est_seconds: number;
  /** True when watched below the live play threshold with no cumshot. The review
   *  collapses these behind a toggle and excludes them from a default accept. */
  below_threshold: boolean;
}

/** A proposed (not-yet-created) session inferred from Stash history. */
export interface CandidateSession {
  start_ms: number;
  end_ms: number;
  /** "boundable" | "lone_boundable" | "lone_fuzzy" — confidence tier. */
  tier: string;
  event_count: number;
  play_count: number;
  cumshot_count: number;
  assigned_day: string;
  scenes: CandidateScene[];
}

/** Result of the "while you were away" launch scan. */
export interface AwayResult {
  /** Most recent logged session's end (epoch ms), or null on a fresh DB. */
  since_ms: number | null;
  candidates: CandidateSession[];
}

/** One scene to record on an accepted candidate's session. */
export interface AcceptScene {
  content_item_id: number;
  est_seconds: number;
  first_seen_ms: number;
  last_seen_ms: number;
  /** Extra cumshots to log on this scene that Stash doesn't have yet (created as
   *  origin='climax' o_events and pushed to Stash on accept). */
  extra_cumshots: number;
}

/** A scene Stash logged plays for inside a session's window that the session
 *  doesn't track yet (surfaced for the per-session "Stash activity" review). */
export interface PendingHistoryScene {
  content_item_id: number;
  external_id: string | null;
  title: string | null;
  thumbnail_url: string | null;
  play_times: number[];
  est_seconds: number;
}
export interface PendingHistoryO {
  o_event_id: number;
  content_item_id: number | null;
  occurred_at: number;
}
export interface SessionPendingHistory {
  scenes: PendingHistoryScene[];
  cumshots: PendingHistoryO[];
}

/** Preview of a session merge: whether it's allowed + the span/gap. */
export interface MergeCheck {
  ok: boolean;
  reason: string | null;
  gap_ms: number;
  merged_start: number;
  merged_end: number;
}

/** Preview of reopening a session (making an ended one live again). */
export interface ReopenCheck {
  can_reopen: boolean;
  gap_ms: number;
}

/** Preview of continuing a session (folding the untracked activity after it). */
export interface ExtendSessionCheck {
  can_extend: boolean;
  gap_ms: number;
  scene_count: number;
  cumshot_count: number;
}

export interface SessionGapSetting {
  gap_minutes: number;
}

export interface BackupSetting {
  dest_folder: string | null;
  last_backup_at: number | null;
}

export interface BackupResult {
  path: string;
  settings: BackupSetting;
}

export interface RestoreCandidate {
  path: string;
  file_name: string;
}

export interface ProfileInfo {
  profile: string; // "real" | "scratch"
  stash_enabled: boolean;
  db_filename: string;
}

export interface ReleaseInfo {
  /** The tag with any leading "v" stripped, e.g. "0.2.0". */
  version: string;
  published_at: number | null;
  /** The release page - what the Download button opens. */
  url: string;
}

export interface UpdateInfo {
  /** The version of the backend that answered. In client mode this is the
   *  SERVER, not this app - compare against your own version, not this one. */
  backend_version: string;
  /** Unix ms of the last completed check. Null = never checked, which reads
   *  differently from "checked, and you are up to date". */
  checked_at: number | null;
  /** Null also covers "no release has been published yet". */
  latest: ReleaseInfo | null;
  last_error: string | null;
  dismissed_version: string | null;
}

/** Is `latest` newer than `current`? Mirrors `update::is_newer` in the backend:
 *  numeric compare of the leading dot-separated parts (so 0.10.0 beats 0.9.0,
 *  which a string compare gets wrong), a leading "v" ignored, missing parts
 *  treated as 0, and a prerelease ranking below the release it precedes.
 *  Anything unparseable answers false - if we cannot tell, we do not nag. */
export function isNewerVersion(latest: string, current: string): boolean {
  const split = (raw: string): [number[], boolean] => {
    const trimmed = (raw ?? "").trim().replace(/^v/, "");
    const core = trimmed.split(/[-+]/)[0] ?? "";
    const nums: number[] = [];
    for (const part of core.split(".")) {
      if (!/^\d+$/.test(part)) break;
      nums.push(Number(part));
    }
    return [nums, trimmed.length !== core.length];
  };
  const [l, lPre] = split(latest);
  const [c, cPre] = split(current);
  if (l.length === 0 || c.length === 0) return false;
  for (let i = 0; i < Math.max(l.length, c.length); i++) {
    const a = l[i] ?? 0;
    const b = c[i] ?? 0;
    if (a !== b) return a > b;
  }
  return cPre && !lPre;
}

export interface ServerPortInfo {
  /** The configured port. In client mode nothing is bound locally, so this is
   *  what the built-in backend would use if it were switched back on. */
  port: number;
  /** False only when the in-process server failed to bind - the bridge can't
   *  reach this app, so nothing is being tracked. True in client mode. */
  ok: boolean;
  /** The failure as a sentence fit to show, when `ok` is false. */
  error: string | null;
}

/** Args object accepted by every filter-aware command. start_day /
 *  end_day are YYYY-MM-DD inclusive. Empty / undefined arrays mean
 *  "no filter on this dimension". */
export interface FilterArgs {
  startDay: string;
  endDay: string;
  sceneContentItemIds?: number[];
  performerIds?: string[];
  studioIds?: string[];
  tagIds?: string[];
}

/** Epoch ms -> "YYYY-MM-DDTHH:MM" (local timezone) for <input type="datetime-local">. */
export function epochToLocalInput(ms: number): string {
  const d = new Date(ms);
  const pad = (n: number) => n.toString().padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

/** Inverse of epochToLocalInput. Returns ms. */
export function localInputToEpoch(str: string): number {
  return new Date(str).getTime();
}

export function formatDuration(ms: number): string {
  const totalSecs = Math.max(0, Math.floor(ms / 1000));
  const h = Math.floor(totalSecs / 3600);
  const m = Math.floor((totalSecs % 3600) / 60);
  const s = totalSecs % 60;
  const pad = (n: number) => n.toString().padStart(2, "0");
  return h > 0 ? `${pad(h)}:${pad(m)}:${pad(s)}` : `${pad(m)}:${pad(s)}`;
}

export function formatTimeAgo(ms: number): string {
  const diff = Date.now() - ms;
  if (diff < 60_000) return "just now";
  if (diff < 3_600_000) return `${Math.floor(diff / 60_000)}m ago`;
  if (diff < 86_400_000) return `${Math.floor(diff / 3_600_000)}h ago`;
  return `${Math.floor(diff / 86_400_000)}d ago`;
}

/** Scene-length clock from seconds: "1:41:38" or "41:28" (no leading-zero
 *  hour). Used for the duration label on scene tiles + detail headers. */
export function formatDurationShort(seconds: number | null): string {
  if (!seconds || seconds <= 0) return "-";
  const total = Math.floor(seconds);
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  const pad = (n: number) => n.toString().padStart(2, "0");
  return h > 0 ? `${h}:${pad(m)}:${pad(s)}` : `${m}:${pad(s)}`;
}

/** Compact watch-time for browse tiles/stats: "44h 29m", "12m", "—". */
export function formatWatchShort(ms: number | null): string {
  if (!ms || ms < 60_000) return "-";
  const totalMin = Math.floor(ms / 60_000);
  const h = Math.floor(totalMin / 60);
  const m = totalMin % 60;
  if (h === 0) return `${m}m`;
  if (m === 0) return `${h}h`;
  return `${h}h ${m}m`;
}

/** Relative "last watched" for browse tiles: "5m ago", "3d ago", "4mo ago",
 *  "never" when null. Coarser than formatTimeAgo (adds days/months/years). */
export function formatAgo(ms: number | null): string {
  if (!ms) return "never";
  const diff = Date.now() - ms;
  if (diff < 0) return "just now";
  const min = Math.floor(diff / 60_000);
  if (min < 60) return `${min}m ago`;
  const h = Math.floor(min / 60);
  if (h < 24) return `${h}h ago`;
  const d = Math.floor(h / 24);
  if (d < 30) return `${d}d ago`;
  const mo = Math.floor(d / 30);
  if (mo < 12) return `${mo}mo ago`;
  return `${Math.floor(mo / 12)}y ago`;
}

/** Competition ranking ("1224"): tied values SHARE the best rank, and the next
 *  distinct value skips the places they took - two tied at the top are BOTH #1
 *  and the next is #3, never #2. So a rank always answers "how many are at
 *  least this good", which keeps it honest against the "#N of M" denominator
 *  every rank display pairs it with (#2 while two entities sit above you would
 *  contradict the count). Same scheme everywhere: browse rank pills, the grid
 *  trophies, and the Trends leaderboards.
 *
 *  This form takes the peer values UNSORTED and counts how many beat `value`. */
export function competitionRank(values: number[], value: number): number {
  let ahead = 0;
  for (const v of values) if (v > value) ahead++;
  return ahead + 1;
}

/** `competitionRank` for a list ALREADY sorted descending: one rank per entry,
 *  positionally, in a single pass. Use this for a whole leaderboard; use
 *  `competitionRank` for a single entity against its peers. */
export function competitionRanks(sortedDesc: number[]): number[] {
  const out: number[] = [];
  for (let i = 0; i < sortedDesc.length; i++) {
    out.push(i > 0 && sortedDesc[i] === sortedDesc[i - 1] ? out[i - 1] : i + 1);
  }
  return out;
}

export function formatDate(ms: number): string {
  return new Date(ms).toLocaleString();
}

export function formatTimeOnly(ms: number): string {
  return new Date(ms).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
}

export function formatDateOnly(ms: number): string {
  return new Date(ms).toLocaleDateString([], { weekday: "short", month: "short", day: "numeric" });
}

/** Local midnight in epoch ms for the given date. */
export function startOfDayMs(d: Date): number {
  const n = new Date(d);
  n.setHours(0, 0, 0, 0);
  return n.getTime();
}

/** Local midnight of the day AFTER `d`. (Exclusive end of day.) */
export function endOfDayMs(d: Date): number {
  const n = new Date(d);
  n.setHours(0, 0, 0, 0);
  n.setDate(n.getDate() + 1);
  return n.getTime();
}

/** Monday-start week for the given date (returns the date of Monday at 00:00). */
export function startOfWeek(d: Date): Date {
  const n = new Date(d);
  n.setHours(0, 0, 0, 0);
  // getDay(): 0=Sun..6=Sat. Convert to Mon=0..Sun=6.
  const offset = (n.getDay() + 6) % 7;
  n.setDate(n.getDate() - offset);
  return n;
}

/** Today's local date with the time portion zeroed. */
export function today(): Date {
  const n = new Date();
  n.setHours(0, 0, 0, 0);
  return n;
}

export function isSameDay(a: Date, b: Date): boolean {
  return a.getFullYear() === b.getFullYear()
    && a.getMonth() === b.getMonth()
    && a.getDate() === b.getDate();
}

/** YYYY-MM-DD in local time for a Date. Used to identify a session's "day". */
export function dayKey(d: Date): string {
  const pad = (n: number) => n.toString().padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
}

/** Parse a YYYY-MM-DD string into a local Date (midnight). */
export function parseDayKey(key: string): Date {
  const [y, m, d] = key.split("-").map((s) => parseInt(s, 10));
  return new Date(y, m - 1, d);
}

/** Friendly label for a day key: "Today", "Yesterday", or "Wed, May 22". */
export function dayKeyLabel(key: string): string {
  const d = parseDayKey(key);
  const todayD = today();
  if (isSameDay(d, todayD)) return "Today";
  const y = new Date(todayD);
  y.setDate(y.getDate() - 1);
  if (isSameDay(d, y)) return "Yesterday";
  return d.toLocaleDateString([], { weekday: "short", month: "short", day: "numeric" });
}
