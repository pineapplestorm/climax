// Tauri commands - the bridge between the Svelte frontend and the Rust backend.
// All commands are async and return Result<T, String> so errors surface in JS as strings.

use tauri::{AppHandle, Manager, Runtime, State};
use tauri_plugin_autostart::ManagerExt;

use crate::catalog::{self, BrowsePerformer, BrowseScene, BrowseStudio, BrowseTag};
use crate::dashboard::{
    self, DailyBucket, HeroStats, MonthlyBucket, NamedEntity, RangeBucket, RangeStats,
    SceneEntity,
};
use crate::db::{self, now_ms};
use crate::mirror::{self, MirrorCheck, MirrorStatus};
use crate::models::{CapturePayload, CrashPayload, IdlePayload, OEvent, Session, SessionPage, SessionSort};
use crate::session::PlayedScene;
use crate::settings::{self, IdleDetectionSetting, MetadataRefreshSetting, PlayCountingSetting, RemoteTrackingSetting, StashConnectionSetting};
use crate::stash::{self, TestConnectionResult};
use crate::state::AppState;
// Relocated to climax-core::rpc in Phase 2b (the /rpc command surface);
// re-imported here for the Tauri command wrappers that still build these shapes.
use crate::rpc::{bridge_status_of, filter_from_args, BridgeStatus, ProfileInfo};
use crate::server;

/// Window the idle prompt is hosted in (mirrors presence.rs).
const PROMPT_WINDOW_LABEL: &str = "tracker";

/// How long to wait for the HTTP port to become rebindable before a restart.
/// Generous because failing to release it leaves the relaunched app with no
/// backend at all (the bridge can't reach it), whereas a slow restart is merely
/// slow. Only ever spent on the restart paths, never on a normal launch.
const RESTART_PORT_RELEASE: std::time::Duration = std::time::Duration::from_secs(5);

/// Clear the pending-idle slot and restore the prompt host window's
/// alwaysOnTop to false. Called by every resolve command
/// (keep / discard_continue / discard_end). The window itself stays visible
/// so the user keeps their tracker; only the always-on-top is undone.
async fn finish_idle_prompt<R: Runtime>(app: &AppHandle<R>, state: &AppState) {
    let mut slot = state.pending_idle.write().await;
    *slot = None;
    if let Some(win) = app.get_webview_window(PROMPT_WINDOW_LABEL) {
        let _ = win.set_always_on_top(false);
    }
}

type CmdResult<T> = Result<T, String>;

fn err<E: std::fmt::Display>(e: E) -> String {
    format!("{:#}", e)
}

#[tauri::command]
pub async fn session_start(state: State<'_, AppState>) -> CmdResult<Session> {
    state.session.start().await.map_err(err)
}

#[tauri::command]
pub async fn session_stop(state: State<'_, AppState>) -> CmdResult<Option<Session>> {
    state.session.stop().await.map_err(err)
}

/// Discard the active session. Status flips to 'discarded'; the dashboard
/// filters those out everywhere. Reachable from the wrap-up modal only,
/// behind a two-tap confirmation. See session.rs::discard for details.
#[tauri::command]
pub async fn session_discard(state: State<'_, AppState>) -> CmdResult<Option<Session>> {
    state.session.discard().await.map_err(err)
}

#[tauri::command]
pub async fn session_pause(state: State<'_, AppState>) -> CmdResult<Option<Session>> {
    state.session.pause("manual").await.map_err(err)
}

#[tauri::command]
pub async fn session_resume(state: State<'_, AppState>) -> CmdResult<Option<Session>> {
    state.session.resume().await.map_err(err)
}

#[tauri::command]
pub async fn session_active(state: State<'_, AppState>) -> CmdResult<Option<Session>> {
    state.session.active().await.map_err(err)
}

/// Fetch one session by id (any status), or null if it doesn't exist. Used by
/// SessionDetail for row-click expansion instead of scanning the session list.
#[tauri::command]
pub async fn session_get(state: State<'_, AppState>, id: i64) -> CmdResult<Option<Session>> {
    state.session.get(id).await.map_err(err)
}

#[tauri::command]
pub async fn sessions_list(
    state: State<'_, AppState>,
    limit: Option<i64>,
    offset: Option<i64>,
) -> CmdResult<Vec<Session>> {
    state
        .session
        .list(limit.unwrap_or(50), offset.unwrap_or(0))
        .await
        .map_err(err)
}

#[tauri::command]
pub async fn session_scenes(
    state: State<'_, AppState>,
    session_id: i64,
) -> CmdResult<Vec<PlayedScene>> {
    state.session.scenes_in_session(session_id).await.map_err(err)
}

/// One page of the dashboard Sessions table: sessions whose assigned_day falls
/// in [start_day, end_day], sorted by `sort` ("start"|"duration"|"scenes"|
/// "cumshots", default "start") in `desc` order, paginated by limit/offset.
/// Returns the page rows + total matching count (so sort holds across pages).
#[tauri::command]
pub async fn sessions_page(
    state: State<'_, AppState>,
    start_day: String,
    end_day: String,
    sort: Option<String>,
    desc: Option<bool>,
    limit: Option<i64>,
    offset: Option<i64>,
) -> CmdResult<SessionPage> {
    let sort = SessionSort::parse(sort.as_deref().unwrap_or("start"));
    let (rows, total) = state
        .session
        .list_page(
            &start_day,
            &end_day,
            sort,
            desc.unwrap_or(true),
            limit.unwrap_or(50),
            offset.unwrap_or(0),
        )
        .await
        .map_err(err)?;
    Ok(SessionPage { rows, total })
}

#[tauri::command]
pub async fn sessions_in_range(
    state: State<'_, AppState>,
    start_ms: i64,
    end_ms: i64,
) -> CmdResult<Vec<Session>> {
    state.session.list_in_range(start_ms, end_ms).await.map_err(err)
}

#[tauri::command]
pub async fn sessions_for_day(
    state: State<'_, AppState>,
    day: String,
) -> CmdResult<Vec<Session>> {
    state.session.list_for_day(&day).await.map_err(err)
}

#[tauri::command]
pub async fn session_set_assigned_day(
    state: State<'_, AppState>,
    session_id: i64,
    day: String,
) -> CmdResult<Session> {
    state.session.set_assigned_day(session_id, &day).await.map_err(err)
}

#[tauri::command]
pub async fn o_log(
    state: State<'_, AppState>,
    session_id: Option<i64>,
    content_item_id: Option<i64>,
    occurred_at: Option<i64>,
    intensity: Option<i64>,
    notes: Option<String>,
) -> CmdResult<OEvent> {
    // From-frontend O events are always origin='climax' (so they sync to Stash
    // AND can be deleted from Climax later).
    state
        .session
        .log_o(session_id, content_item_id, occurred_at, intensity, notes, "climax")
        .await
        .map_err(err)
}

#[tauri::command]
pub async fn o_list_for_session(
    state: State<'_, AppState>,
    session_id: i64,
) -> CmdResult<Vec<OEvent>> {
    state.session.o_events_for_session(session_id).await.map_err(err)
}

#[tauri::command]
pub async fn o_delete(state: State<'_, AppState>, o_id: i64) -> CmdResult<()> {
    state.session.delete_o(o_id).await.map_err(err)
}

#[tauri::command]
pub async fn o_delete_recent_for_scene(
    state: State<'_, AppState>,
    session_id: i64,
    content_item_id: i64,
    count: i64,
) -> CmdResult<u64> {
    state
        .session
        .delete_recent_o_for_scene(session_id, content_item_id, count)
        .await
        .map_err(err)
}

#[tauri::command]
pub async fn session_delete(state: State<'_, AppState>, session_id: i64) -> CmdResult<()> {
    state.session.delete_session(session_id).await.map_err(err)
}

/// Remove a single scene from a session (without deleting the session). Pulls
/// the scene's watch time + counted play out of the session and rolls the same
/// back out of Stash; any cumshots on that scene in the session go too.
#[tauri::command]
pub async fn scene_delete_from_session(
    state: State<'_, AppState>,
    session_id: i64,
    content_item_id: i64,
) -> CmdResult<()> {
    state
        .session
        .delete_scene_from_session(session_id, content_item_id)
        .await
        .map_err(err)
}

/// Non-destructive remove: detach a scene from a session WITHOUT rolling it back
/// out of Stash, so it stays recoverable / re-homeable. (See
/// SessionManager::detach_scene_from_session.)
#[tauri::command]
pub async fn scene_detach_from_session(
    state: State<'_, AppState>,
    session_id: i64,
    content_item_id: i64,
) -> CmdResult<()> {
    state
        .session
        .detach_scene_from_session(session_id, content_item_id)
        .await
        .map_err(err)
}

#[tauri::command]
pub async fn session_delete_many(
    state: State<'_, AppState>,
    session_ids: Vec<i64>,
) -> CmdResult<u64> {
    state.session.delete_sessions_many(&session_ids).await.map_err(err)
}

#[tauri::command]
pub async fn session_update(
    state: State<'_, AppState>,
    session_id: i64,
    started_at: Option<i64>,
    ended_at: Option<i64>,
    notes: Option<String>,
    excluded: Option<bool>,
    session_type: Option<String>,
) -> CmdResult<Session> {
    state
        .session
        .update_session(session_id, started_at, ended_at, notes, excluded, session_type)
        .await
        .map_err(err)
}

#[tauri::command]
pub async fn o_event_update(
    state: State<'_, AppState>,
    o_id: i64,
    occurred_at: Option<i64>,
    content_item_id: Option<i64>,
    notes: Option<String>,
    intensity: Option<i64>,
) -> CmdResult<OEvent> {
    state
        .session
        .update_o_event(o_id, occurred_at, content_item_id, notes, intensity)
        .await
        .map_err(err)
}

#[tauri::command]
pub async fn o_sessionless_unlinked_today(state: State<'_, AppState>) -> CmdResult<Vec<OEvent>> {
    state.session.sessionless_unlinked_today().await.map_err(err)
}

// ---------- Idle detection ----------

#[tauri::command]
pub async fn idle_detection_get(state: State<'_, AppState>) -> CmdResult<IdleDetectionSetting> {
    settings::get_idle_detection(&state.pool).await.map_err(err)
}

#[tauri::command]
pub async fn idle_detection_set(
    state: State<'_, AppState>,
    enabled: bool,
    threshold_minutes: u32,
) -> CmdResult<IdleDetectionSetting> {
    // Hard floor at 1 minute. A threshold of 0 would fire constantly; sub-minute
    // thresholds aren't useful given our 30s poll cadence.
    let value = IdleDetectionSetting {
        enabled,
        threshold_minutes: threshold_minutes.max(1),
    };
    settings::set_idle_detection(&state.pool, &value).await.map_err(err)?;
    Ok(value)
}

// ---------- Remote-device tracking (live Stash play_duration poll) ----------

#[tauri::command]
pub async fn wording_get(state: State<'_, AppState>) -> CmdResult<String> {
    settings::get_wording(&state.pool).await.map_err(err)
}

#[tauri::command]
pub async fn wording_set(state: State<'_, AppState>, value: String) -> CmdResult<String> {
    settings::set_wording(&state.pool, &value).await.map_err(err)
}

#[tauri::command]
pub async fn keep_empty_sessions_get(state: State<'_, AppState>) -> CmdResult<bool> {
    settings::get_keep_empty_sessions(&state.pool).await.map_err(err)
}

#[tauri::command]
pub async fn keep_empty_sessions_set(state: State<'_, AppState>, enabled: bool) -> CmdResult<bool> {
    settings::set_keep_empty_sessions(&state.pool, enabled).await.map_err(err)?;
    Ok(enabled)
}

#[tauri::command]
pub async fn remote_tracking_get(state: State<'_, AppState>) -> CmdResult<RemoteTrackingSetting> {
    settings::get_remote_tracking(&state.pool).await.map_err(err)
}

#[tauri::command]
pub async fn remote_tracking_set(
    state: State<'_, AppState>,
    enabled: bool,
    poll_seconds: u32,
) -> CmdResult<RemoteTrackingSetting> {
    // Clamp to [5, 60]s: under record_heartbeat's 30s currentTime clamp and the
    // poller's own guard, so a bad value can't break crediting or hammer Stash.
    let value = RemoteTrackingSetting {
        enabled,
        poll_seconds: poll_seconds.clamp(5, 60),
    };
    settings::set_remote_tracking(&state.pool, &value).await.map_err(err)?;
    Ok(value)
}

// ---------- Passive-capture prompt (remind me to track a session) ----------

#[tauri::command]
pub async fn capture_prompt_get(
    state: State<'_, AppState>,
) -> CmdResult<settings::CapturePromptSetting> {
    settings::get_capture_prompt(&state.pool).await.map_err(err)
}

#[tauri::command]
pub async fn capture_prompt_set(
    state: State<'_, AppState>,
    enabled: bool,
    threshold_minutes: u32,
    snooze_minutes: u32,
) -> CmdResult<settings::CapturePromptSetting> {
    let value = settings::CapturePromptSetting {
        enabled,
        threshold_minutes: threshold_minutes.max(1),
        snooze_minutes: snooze_minutes.max(1),
    };
    settings::set_capture_prompt(&state.pool, &value).await.map_err(err)?;
    Ok(value)
}

/// The tracker reads this on mount so a prompt that fired before it was
/// listening still renders.
#[tauri::command]
pub async fn capture_pending_get(state: State<'_, AppState>) -> CmdResult<Option<CapturePayload>> {
    Ok(state.pending_capture.read().await.clone())
}

/// Accept the prompt: start a session BACKDATED to when watching began, seeded
/// with the scene. Clears the pending prompt + the always-on-top float; returns
/// the new session so the tracker can switch to showing it.
#[tauri::command]
pub async fn capture_accept<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, AppState>,
) -> CmdResult<Option<Session>> {
    // Drop the toast's always-on-top float (local window op), then do the session
    // work - on the SERVER in client mode (it holds pending_capture + the watch
    // state), against the in-process manager otherwise.
    if let Some(win) = app.get_webview_window(PROMPT_WINDOW_LABEL) {
        let _ = win.set_always_on_top(false);
    }
    crate::remote_session::capture_accept(&state).await.map_err(err)
}

/// Snooze/dismiss: suppress prompts for `snooze_minutes` and re-baseline the
/// no-session watch so the scene must be watched afresh before re-firing.
#[tauri::command]
pub async fn capture_snooze<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, AppState>,
) -> CmdResult<()> {
    // Snooze state lives wherever the capture loop runs - the SERVER in client
    // mode, the in-process state otherwise. Then drop the toast's float (local).
    crate::remote_session::capture_snooze(&state).await.map_err(err)?;
    if let Some(win) = app.get_webview_window(PROMPT_WINDOW_LABEL) {
        let _ = win.set_always_on_top(false);
    }
    Ok(())
}

// ---------- Play counting (Stash's "minimum play percent", mirrored) ----------

#[tauri::command]
pub async fn play_counting_get(state: State<'_, AppState>) -> CmdResult<PlayCountingSetting> {
    settings::get_play_counting(&state.pool).await.map_err(err)
}

/// Save the user's play-count threshold (percent of scene duration that must
/// be watched before a play "counts"). Pass null to clear it (which will let
/// the next launch's Stash-sync repopulate it).
#[tauri::command]
pub async fn play_counting_set(
    state: State<'_, AppState>,
    threshold_pct: Option<f32>,
) -> CmdResult<PlayCountingSetting> {
    let value = PlayCountingSetting {
        threshold_pct: threshold_pct.map(|p| p.clamp(0.0, 100.0)),
    };
    settings::set_play_counting(&state.pool, &value).await.map_err(err)?;
    Ok(value)
}

/// One-shot re-fetch of Stash's minimumPlayPercent. Used by the Settings UI
/// "Sync from Stash" button; overwrites the local value even if it was
/// previously customised. Returns the value that was written (or the
/// existing one untouched if Stash didn't return anything).
#[tauri::command]
pub async fn play_counting_sync_from_stash(
    state: State<'_, AppState>,
) -> CmdResult<PlayCountingSetting> {
    match crate::stash::fetch_minimum_play_percent(&state.pool).await {
        Ok(Some(pct)) => {
            let value = PlayCountingSetting { threshold_pct: Some(pct) };
            settings::set_play_counting(&state.pool, &value).await.map_err(err)?;
            Ok(value)
        }
        Ok(None) => {
            // Stash didn't expose it — leave the existing setting alone.
            settings::get_play_counting(&state.pool).await.map_err(err)
        }
        Err(e) => {
            // The Settings UI renders this string next to the sync button; the
            // top-level message is already user-shaped, the chain goes to the log.
            tracing::warn!("play threshold sync from Stash failed: {:#}", e);
            Err(format!("{}", e))
        }
    }
}

/// Idle-prompt window calls this on mount to fetch the payload it should
/// display. Returns None if there's no active idle prompt (e.g. user already
/// resolved it). The window should close itself in that case.
#[tauri::command]
pub async fn idle_pending_get(state: State<'_, AppState>) -> CmdResult<Option<IdlePayload>> {
    Ok(state.pending_idle.read().await.clone())
}

/// "Keep" path. Idle time stays counted as session time. Just clears the
/// pending-idle slot + hides the prompt window.
#[tauri::command]
pub async fn idle_keep<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, AppState>,
) -> CmdResult<()> {
    finish_idle_prompt(&app, &state).await;
    Ok(())
}

/// "Discard & Continue" path. Records the gap from `idle_started_at` to NOW
/// as a closed `session_pauses` row with reason='idle'; the session keeps
/// running. Then clears the prompt.
#[tauri::command]
pub async fn idle_discard_continue<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, AppState>,
    session_id: i64,
    idle_started_at: i64,
) -> CmdResult<()> {
    let resumed_at = now_ms();
    // In client mode the session lives on the server, so route the gap pause
    // there over /rpc; locally it hits the in-process manager unchanged.
    crate::remote_session::record_gap_pause(&state, session_id, idle_started_at, resumed_at, "idle")
        .await
        .map_err(err)?;
    finish_idle_prompt(&app, &state).await;
    Ok(())
}

/// "Discard & End" path. Ends the session at `idle_started_at` (AFK time
/// is not counted). Then clears the prompt.
#[tauri::command]
pub async fn idle_discard_end<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, AppState>,
    session_id: i64,
    idle_started_at: i64,
) -> CmdResult<Option<Session>> {
    let updated = crate::remote_session::end_session_at(&state, session_id, idle_started_at)
        .await
        .map_err(err)?;
    finish_idle_prompt(&app, &state).await;
    Ok(updated)
}

// ---------- Quit guard ----------
// Quitting (tray "Quit Climax") with a session running is intercepted: instead
// of exiting, the tray sets pending_quit + shows the tracker + emits
// `quit_requested`. The tracker shows a confirm; "Wrap up" runs the wrap-up
// modal (which stops the session) then calls confirm_quit to exit; "Go back"
// calls cancel_quit.

/// Tracker reads this on mount in case a quit was requested before it mounted.
#[tauri::command]
pub async fn quit_pending_get(state: State<'_, AppState>) -> CmdResult<bool> {
    Ok(*state.pending_quit.read().await)
}

/// Actually exit the app. Called after the wrap-up "Save & close" commit (which
/// has already stopped the session), or directly when there were no scenes to
/// wrap up. The process dies, so no further cleanup is needed.
#[tauri::command]
pub async fn confirm_quit<R: Runtime>(app: AppHandle<R>) -> CmdResult<()> {
    app.exit(0);
    Ok(())
}

/// "Go back" — abandon the quit. Clears pending_quit and drops the tracker's
/// always-on-top so it returns to normal.
#[tauri::command]
pub async fn cancel_quit<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, AppState>,
) -> CmdResult<()> {
    *state.pending_quit.write().await = false;
    if let Some(win) = app.get_webview_window(PROMPT_WINDOW_LABEL) {
        let _ = win.set_always_on_top(false);
    }
    Ok(())
}

// ---------- Crash recovery ----------
// On boot, lib.rs detects a session left running from a previous run (any
// unclean exit) and, if the gap since its last heartbeat exceeds the threshold,
// stashes a CrashPayload. The tracker reads it on mount and shows the recovery
// prompt — the same three options as idle return, with the "gap" being the
// downtime. The leftover is resumed to 'active' on boot so these reuse the same
// session machinery uniformly.

/// Clear the pending crash-recovery slot + drop the tracker's always-on-top.
async fn finish_crash_prompt<R: Runtime>(app: &AppHandle<R>, state: &AppState) {
    *state.pending_crash_recovery.write().await = None;
    if let Some(win) = app.get_webview_window(PROMPT_WINDOW_LABEL) {
        let _ = win.set_always_on_top(false);
    }
}

/// Tracker reads this on mount to show the crash-recovery prompt (or null).
#[tauri::command]
pub async fn crash_pending_get(state: State<'_, AppState>) -> CmdResult<Option<CrashPayload>> {
    Ok(state.pending_crash_recovery.read().await.clone())
}

/// "Keep" — the downtime counts as session time; the session continues. Just
/// clears the prompt (the session was already resumed to 'active' on boot).
#[tauri::command]
pub async fn crash_keep<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, AppState>,
) -> CmdResult<()> {
    finish_crash_prompt(&app, &state).await;
    Ok(())
}

/// "Discard & Continue" — record the downtime [gap_started_at, now] as a closed
/// 'crash' pause so it's excluded from the session duration; the session keeps
/// running. Then clears the prompt.
#[tauri::command]
pub async fn crash_discard_continue<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, AppState>,
    session_id: i64,
    gap_started_at: i64,
) -> CmdResult<()> {
    let resumed_at = now_ms();
    // Crash recovery only fires for a LOCAL backend (a client's empty cache shows
    // no leftover), but route through the helper for uniformity - the local
    // branch is identical to the previous direct call.
    crate::remote_session::record_gap_pause(&state, session_id, gap_started_at, resumed_at, "crash")
        .await
        .map_err(err)?;
    finish_crash_prompt(&app, &state).await;
    Ok(())
}

/// "Discard & End" — end the session at `gap_started_at` (the last heartbeat);
/// the downtime is not counted. Then clears the prompt.
#[tauri::command]
pub async fn crash_discard_end<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, AppState>,
    session_id: i64,
    gap_started_at: i64,
) -> CmdResult<Option<Session>> {
    let updated = crate::remote_session::end_session_at(&state, session_id, gap_started_at)
        .await
        .map_err(err)?;
    finish_crash_prompt(&app, &state).await;
    Ok(updated)
}

// ---------- General settings: launch behavior, hotkey, autostart ----------

/// What Climax shows on launch: "silent" (tray only) / "tracker" / "dashboard".
#[tauri::command]
pub async fn launch_behavior_get(state: State<'_, AppState>) -> CmdResult<String> {
    settings::get_launch_behavior(&state.pool).await.map_err(err)
}

#[tauri::command]
pub async fn launch_behavior_set(state: State<'_, AppState>, mode: String) -> CmdResult<()> {
    settings::set_launch_behavior(&state.pool, &mode).await.map_err(err)
}

/// Whether the global Ctrl+Shift+L shortcut is registered.
#[tauri::command]
pub async fn hotkey_enabled_get(state: State<'_, AppState>) -> CmdResult<bool> {
    settings::get_hotkey_enabled(&state.pool).await.map_err(err)
}

/// Persist the flag AND apply it live — register / unregister the shortcut now.
#[tauri::command]
pub async fn hotkey_enabled_set<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, AppState>,
    enabled: bool,
) -> CmdResult<()> {
    settings::set_hotkey_enabled(&state.pool, enabled)
        .await
        .map_err(err)?;
    let res = if enabled {
        crate::hotkey::register(&app, crate::DEFAULT_HOTKEY)
    } else {
        crate::hotkey::unregister(&app, crate::DEFAULT_HOTKEY)
    };
    if let Err(e) = res {
        tracing::warn!("apply hotkey_enabled={}: {:#}", enabled, e);
    }
    Ok(())
}

/// Whether Climax launches at Windows login (registry Run entry via the
/// autostart plugin; points at the current executable).
#[tauri::command]
pub async fn autostart_get<R: Runtime>(app: AppHandle<R>) -> CmdResult<bool> {
    app.autolaunch().is_enabled().map_err(err)
}

#[tauri::command]
pub async fn autostart_set<R: Runtime>(app: AppHandle<R>, enabled: bool) -> CmdResult<()> {
    let mgr = app.autolaunch();
    if enabled {
        mgr.enable().map_err(err)
    } else {
        mgr.disable().map_err(err)
    }
}

// ---------- Backup and restore ----------
//
// Snapshot the whole climax.sqlite to a user-chosen folder (timestamped, never
// overwriting) and restore one back (swapped in on relaunch). All file IO is
// Rust-side, mirroring trends_export_csv; the dialog plugin only picks paths.

/// Result of a successful backup: where it landed + the updated setting.
#[derive(serde::Serialize)]
pub struct BackupResult {
    pub path: String,
    pub settings: settings::BackupSetting,
}

/// A validated restore source the user picked (shown in the confirm step).
#[derive(serde::Serialize)]
pub struct RestoreCandidate {
    pub path: String,
    pub file_name: String,
}

/// True if `candidate` resolves to the live DB, its WAL/SHM sidecars, or the
/// staged-pending file. Restoring from the live DB would copy it WITHOUT its
/// (uncheckpointed) WAL, and the boot swap then deletes that WAL — silent loss
/// of exactly the most-recent writes. Reject it.
fn is_live_db_path(db_dir: &std::path::Path, db_name: &str, candidate: &str) -> bool {
    let cand = std::path::Path::new(candidate);
    let same = |a: &std::path::Path, b: &std::path::Path| {
        match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
            (Ok(x), Ok(y)) => x == y,
            _ => a == b,
        }
    };
    if same(cand, &db_dir.join(db_name)) || same(cand, &db::restore_pending_path(db_dir, db_name)) {
        return true;
    }
    let wal = format!("{db_name}-wal");
    let shm = format!("{db_name}-shm");
    match cand
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
    {
        Some(n) => n == wal || n == shm,
        None => false,
    }
}

/// Read the saved backup settings (last folder + last-backup time).
#[tauri::command]
pub async fn backup_settings_get(
    state: State<'_, AppState>,
) -> CmdResult<settings::BackupSetting> {
    settings::get_backup(&state.pool).await.map_err(err)
}

/// The folder holding Climax's live database - and where each restore drops a
/// `*-pre-restore-*.sqlite` safety copy of the current library before overwriting
/// it. Surfaced so the Restore UI can tell the user where their rollback file is.
#[tauri::command]
pub async fn backup_data_folder(state: State<'_, AppState>) -> CmdResult<String> {
    Ok(state.db_dir.to_string_lossy().to_string())
}

/// Reject a database-FILE operation (back up / restore / reset) when this app is a
/// thin CLIENT. The real database lives on the external server, not in this app's
/// throwaway local cache, so a `VACUUM INTO` / wipe / restart here would silently
/// operate on the wrong (empty) file. Real server-side backup-over-the-wire is a
/// later feature (2c-b's server tray). Until then, fail clearly.
fn ensure_local_backend(state: &AppState) -> CmdResult<()> {
    if state.client_server_url.is_some() {
        Err("This app is connected to an external Climax server, which owns the \
             database. Back up, restore, or reset from the server itself."
            .to_string())
    } else {
        Ok(())
    }
}

/// Back up now: pick a destination folder, then write a timestamped, consistent
/// snapshot of the live DB via SQLite `VACUUM INTO` (merges the WAL, so the file
/// is self-contained — the app keeps running). Returns null if cancelled.
#[tauri::command]
pub async fn backup_now<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, AppState>,
) -> CmdResult<Option<BackupResult>> {
    use tauri_plugin_dialog::DialogExt;
    ensure_local_backend(&state)?;

    let mut current = settings::get_backup(&state.pool).await.map_err(err)?;

    // Pick the destination folder, defaulting to the last one used.
    let mut builder = app.dialog().file();
    if let Some(dir) = current.dest_folder.as_deref() {
        builder = builder.set_directory(dir);
    }
    let Some(folder) = builder.blocking_pick_folder() else {
        return Ok(None);
    };
    let folder = folder.to_string();

    // Timestamped filename, profile-stemmed (climax-backup-... / climax-test-backup-...)
    // so a scratch backup is never mistaken for a real one; never overwrite (add
    // ms if the second already exists).
    let stem = state.db_filename.trim_end_matches(".sqlite");
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let mut target = std::path::Path::new(&folder).join(format!("{stem}-backup-{stamp}.sqlite"));
    if target.exists() {
        target = std::path::Path::new(&folder)
            .join(format!("{stem}-backup-{stamp}-{}.sqlite", now_ms()));
    }
    let target_str = target.to_string_lossy().to_string();

    // VACUUM INTO takes a string literal, not a bound param — escape quotes.
    let escaped = target_str.replace('\'', "''");
    sqlx::query(&format!("VACUUM INTO '{escaped}'"))
        .execute(&state.pool)
        .await
        .map_err(err)?;

    // VACUUM INTO faithfully copies the source's indexes - INCLUDING a corrupt
    // one. Rebuild them in the fresh backup so it's always self-consistent
    // (restore validation uses quick_check, which does NOT catch index-count
    // corruption). Best-effort: a REINDEX failure doesn't sink the backup.
    {
        use sqlx::Connection;
        let opts = sqlx::sqlite::SqliteConnectOptions::new()
            .filename(&target_str)
            .create_if_missing(false);
        match sqlx::SqliteConnection::connect_with(&opts).await {
            Ok(mut conn) => {
                if let Err(e) = sqlx::query("REINDEX").execute(&mut conn).await {
                    tracing::warn!("REINDEX of backup failed (kept as-is): {:#}", e);
                }
                let _ = conn.close().await;
            }
            Err(e) => tracing::warn!("could not open backup to REINDEX (kept as-is): {:#}", e),
        }
    }

    current.dest_folder = Some(folder);
    current.last_backup_at = Some(now_ms());
    settings::set_backup(&state.pool, &current)
        .await
        .map_err(err)?;

    Ok(Some(BackupResult {
        path: target_str,
        settings: current,
    }))
}

/// Pick a backup file and validate it (integrity + expected tables). Returns the
/// candidate for the confirm step, null if cancelled, or an error if the file
/// isn't a usable Climax database.
#[tauri::command]
pub async fn backup_restore_pick<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, AppState>,
) -> CmdResult<Option<RestoreCandidate>> {
    use tauri_plugin_dialog::DialogExt;
    ensure_local_backend(&state)?;
    let Some(picked) = app
        .dialog()
        .file()
        .add_filter("Climax backup", &["sqlite"])
        .blocking_pick_file()
    else {
        return Ok(None);
    };
    let path = picked.to_string();
    if is_live_db_path(&state.db_dir, &state.db_filename, &path) {
        return Err(
            "That's Climax's live database. Pick a backup file instead (use \"Back up now\" first)."
                .to_string(),
        );
    }
    db::validate_db_file(&path).await.map_err(err)?;
    let file_name = std::path::Path::new(&path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.clone());
    Ok(Some(RestoreCandidate { path, file_name }))
}

/// Apply a restore: re-validate, snapshot the CURRENT DB as a safety net, stage
/// the chosen file, and relaunch so the swap happens before the pool reopens.
/// On success the app restarts, so this never returns to the caller.
#[tauri::command]
pub async fn backup_restore_apply<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, AppState>,
    path: String,
) -> CmdResult<()> {
    ensure_local_backend(&state)?;
    let dir = &state.db_dir;
    if is_live_db_path(dir, &state.db_filename, &path) {
        return Err("That's Climax's live database; restore a saved backup file instead.".to_string());
    }
    db::validate_db_file(&path).await.map_err(err)?;

    // Safety snapshot of the current DB before we overwrite it. This is the
    // rollback the UI promises, so a failure CANCELS the restore (we don't stage
    // or restart) rather than silently leaving the user without one. Profile-
    // stemmed so a scratch pre-restore copy can't be confused with a real one.
    let stem = state.db_filename.trim_end_matches(".sqlite");
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let safety = dir.join(format!("{stem}-pre-restore-{stamp}.sqlite"));
    let escaped = safety.to_string_lossy().replace('\'', "''");
    sqlx::query(&format!("VACUUM INTO '{escaped}'"))
        .execute(&state.pool)
        .await
        .map_err(|e| {
            tracing::warn!("pre-restore safety copy failed: {:#}", e);
            "Couldn't write the pre-restore safety copy, so the restore was cancelled.".to_string()
        })?;

    // Stage the chosen file; the boot-time swap (db::apply_pending_restore)
    // re-validates and does the actual replace before the pool opens next launch.
    let pending = db::restore_pending_path(dir, &state.db_filename);
    std::fs::copy(&path, &pending).map_err(err)?;

    tracing::info!("restore staged from {}; restarting to apply", path);
    // Hand the port back BEFORE app.restart() spawns the replacement - see
    // server::release_port. Without this the child inherits the still-open
    // listener and cannot bind the port it is itself holding.
    server::release_port(RESTART_PORT_RELEASE).await;
    app.restart();
    #[allow(unreachable_code)]
    Ok(())
}

/// Full reset: wipe the ENTIRE database (sessions, history, AND settings like the
/// Stash connection) and relaunch. The boot swap deletes the DB before the pool
/// reopens, so a pristine, migrated database is recreated, exactly a fresh
/// install (onboarding re-arms; Stash defaults back to localhost:9999). Guarded
/// in the UI by a danger dialog + type-to-confirm. Restarts, so it never returns.
#[tauri::command]
pub async fn backup_reset<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, AppState>,
) -> CmdResult<()> {
    ensure_local_backend(&state)?;
    db::stage_wipe(&state.db_dir, &state.db_filename).map_err(err)?;
    tracing::warn!("full reset staged; restarting to apply");
    server::release_port(RESTART_PORT_RELEASE).await;
    app.restart();
    #[allow(unreachable_code)]
    Ok(())
}

/// Which profile this process launched in (real vs scratch) + whether Stash is
/// on. The frontend shows a SCRATCH badge when Stash is off so testing is never
/// mistaken for the real library.
#[tauri::command]
pub async fn profile_get(state: State<'_, AppState>) -> CmdResult<ProfileInfo> {
    Ok(ProfileInfo {
        profile: if state.stash_enabled { "real" } else { "scratch" }.to_string(),
        stash_enabled: state.stash_enabled,
        db_filename: state.db_filename.clone(),
    })
}

// ---------- Dashboard / Overview ----------

#[tauri::command]
pub async fn dashboard_hero_stats(state: State<'_, AppState>) -> CmdResult<HeroStats> {
    dashboard::hero_stats(&state.pool).await.map_err(err)
}

#[tauri::command]
pub async fn dashboard_monthly_buckets(
    state: State<'_, AppState>,
    months: Option<u32>,
) -> CmdResult<Vec<MonthlyBucket>> {
    dashboard::monthly_buckets(&state.pool, months.unwrap_or(12))
        .await
        .map_err(err)
}

#[tauri::command]
pub async fn dashboard_daily_buckets(
    state: State<'_, AppState>,
    year: i32,
    month: u32,
) -> CmdResult<Vec<DailyBucket>> {
    dashboard::daily_buckets(&state.pool, year, month).await.map_err(err)
}

// ---------- Filter-aware queries (Phase 3) ----------

#[tauri::command]
pub async fn dashboard_range_stats(
    state: State<'_, AppState>,
    start_day: String,
    end_day: String,
    scene_content_item_ids: Option<Vec<i64>>,
    performer_ids: Option<Vec<String>>,
    studio_ids: Option<Vec<String>>,
    tag_ids: Option<Vec<String>>,
) -> CmdResult<RangeStats> {
    let f = filter_from_args(start_day, end_day, scene_content_item_ids, performer_ids, studio_ids, tag_ids);
    dashboard::range_stats(&state.pool, &f).await.map_err(err)
}

#[tauri::command]
#[allow(clippy::too_many_arguments)] // Tauri command: args map 1:1 to the frontend call
pub async fn dashboard_filtered_buckets(
    state: State<'_, AppState>,
    start_day: String,
    end_day: String,
    granularity: String,
    scene_content_item_ids: Option<Vec<i64>>,
    performer_ids: Option<Vec<String>>,
    studio_ids: Option<Vec<String>>,
    tag_ids: Option<Vec<String>>,
) -> CmdResult<Vec<RangeBucket>> {
    let f = filter_from_args(start_day, end_day, scene_content_item_ids, performer_ids, studio_ids, tag_ids);
    dashboard::filtered_buckets(&state.pool, &f, &granularity).await.map_err(err)
}

#[tauri::command]
pub async fn dashboard_filtered_session_count(
    state: State<'_, AppState>,
    start_day: String,
    end_day: String,
    scene_content_item_ids: Option<Vec<i64>>,
    performer_ids: Option<Vec<String>>,
    studio_ids: Option<Vec<String>>,
    tag_ids: Option<Vec<String>>,
) -> CmdResult<i64> {
    let f = filter_from_args(start_day, end_day, scene_content_item_ids, performer_ids, studio_ids, tag_ids);
    dashboard::filtered_session_count(&state.pool, &f).await.map_err(err)
}

// ---------- Catalog list / search (for filter pickers) ----------

#[tauri::command]
pub async fn catalog_list_performers(state: State<'_, AppState>) -> CmdResult<Vec<NamedEntity>> {
    dashboard::list_performers(&state.pool).await.map_err(err)
}

#[tauri::command]
pub async fn catalog_list_studios(state: State<'_, AppState>) -> CmdResult<Vec<NamedEntity>> {
    dashboard::list_studios(&state.pool).await.map_err(err)
}

#[tauri::command]
pub async fn catalog_list_tags(state: State<'_, AppState>) -> CmdResult<Vec<NamedEntity>> {
    dashboard::list_tags(&state.pool).await.map_err(err)
}

#[tauri::command]
pub async fn catalog_search_scenes(
    state: State<'_, AppState>,
    query: String,
    limit: Option<i64>,
) -> CmdResult<Vec<SceneEntity>> {
    dashboard::search_scenes(&state.pool, &query, limit.unwrap_or(50)).await.map_err(err)
}

// ---------- Catalog browse (Phase 5: Scenes / Performers / Studios / Tags) ----------

#[tauri::command]
pub async fn catalog_browse_scenes(state: State<'_, AppState>) -> CmdResult<Vec<BrowseScene>> {
    catalog::list_scenes(&state.pool).await.map_err(err)
}

#[tauri::command]
pub async fn catalog_browse_performers(
    state: State<'_, AppState>,
) -> CmdResult<Vec<BrowsePerformer>> {
    catalog::list_performers(&state.pool).await.map_err(err)
}

#[tauri::command]
pub async fn catalog_browse_studios(state: State<'_, AppState>) -> CmdResult<Vec<BrowseStudio>> {
    catalog::list_studios(&state.pool).await.map_err(err)
}

#[tauri::command]
pub async fn catalog_browse_tags(state: State<'_, AppState>) -> CmdResult<Vec<BrowseTag>> {
    catalog::list_tags(&state.pool).await.map_err(err)
}

/// Kick off background enrichment of performers / studios / tags from Stash
/// (images, favorite, demographics, parent studio) for the given kind.
/// Fire-and-forget: returns immediately; the browse grid polls and picks up
/// results as they land. `kind` ∈ {performer, studio, tag}.
#[tauri::command]
pub async fn catalog_enrich_entities(state: State<'_, AppState>, kind: String) -> CmdResult<()> {
    let pool = state.pool.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(e) = catalog::enrich_entities(&pool, &kind, false).await {
            tracing::warn!("enrich_entities({}) failed: {:#}", kind, e);
        }
    });
    Ok(())
}

/// Earliest assigned_day in the catalog (None if no sessions yet). Used
/// by the filter store to dynamically clip the "All time" preset to
/// actual data.
#[tauri::command]
pub async fn dashboard_first_session_day(
    state: State<'_, AppState>,
) -> CmdResult<Option<String>> {
    dashboard::first_session_day(&state.pool).await.map_err(err)
}

/// Show the named window (tracker or dashboard) and hide its sibling.
/// Tracker and dashboard are mutually exclusive — opening one always hides
/// the other so the user never has both Climax windows on screen at once.
/// Used by the in-app "Dashboard →" / "Tracker" buttons, the tray menu,
/// the tray icon click, and the global hotkey.
pub fn show_solo<R: Runtime>(app: &AppHandle<R>, label: &str) {
    let sibling = match label {
        "tracker" => "dashboard",
        "dashboard" => "tracker",
        _ => return,
    };
    if let Some(win) = app.get_webview_window(label) {
        let _ = win.show();
        let _ = win.unminimize();
        let _ = win.set_focus();
    }
    if let Some(other) = app.get_webview_window(sibling) {
        let _ = other.hide();
    }
}

/// Show + focus the dashboard window. Hides the tracker if it's open.
#[tauri::command]
pub async fn open_dashboard<R: Runtime>(app: AppHandle<R>) -> CmdResult<()> {
    if app.get_webview_window("dashboard").is_none() {
        return Err("dashboard window not found in config".to_string());
    }
    show_solo(&app, "dashboard");
    Ok(())
}

/// Show + focus the tracker window. Hides the dashboard if it's open.
#[tauri::command]
pub async fn open_tracker<R: Runtime>(app: AppHandle<R>) -> CmdResult<()> {
    if app.get_webview_window("tracker").is_none() {
        return Err("tracker window not found in config".to_string());
    }
    show_solo(&app, "tracker");
    Ok(())
}

// ---------- Stash connection settings ----------

#[tauri::command]
pub async fn stash_connection_get(state: State<'_, AppState>) -> CmdResult<StashConnectionSetting> {
    settings::get_stash_connection(&state.pool).await.map_err(err)
}

#[tauri::command]
pub async fn stash_connection_set(
    state: State<'_, AppState>,
    url: String,
    api_key: Option<String>,
) -> CmdResult<StashConnectionSetting> {
    let value = StashConnectionSetting { url, api_key };
    settings::set_stash_connection(&state.pool, &value).await.map_err(err)?;
    // Mirror the /rpc arm: now that Stash is (re)configured, sync the play-counting
    // threshold from it if the user hasn't set one (best-effort, background). This
    // covers a first-run desktop install that connects Stash at a non-default URL,
    // where the boot-time sync couldn't reach it yet.
    {
        let pool = state.pool.clone();
        tokio::spawn(async move {
            match stash::sync_play_threshold_if_unset(&pool).await {
                Ok(Some(pct)) => tracing::info!("synced play threshold from Stash on connect: {}%", pct),
                Ok(None) => {}
                Err(e) => tracing::debug!("play threshold sync on connect skipped: {:#}", e),
            }
        });
    }
    // Re-read so callers see the normalised version (trimmed, slash-stripped,
    // empty api_key coerced to None).
    settings::get_stash_connection(&state.pool).await.map_err(err)
}

/// Round-trip ping to Stash to validate URL + API key. Doesn't write anything;
/// the caller can decide whether to persist the inputs after seeing the result.
#[tauri::command]
pub async fn stash_connection_test(state: State<'_, AppState>) -> CmdResult<TestConnectionResult> {
    Ok(stash::test_connection(&state.pool).await)
}

// ---------- Bridge plugin (onboarding) ----------

/// Is the Climax bridge installed in Stash, and is one connected right now?
/// Polled by the onboarding bridge step (and reusable in Settings later).
#[tauri::command]
pub async fn bridge_status(state: State<'_, AppState>) -> CmdResult<BridgeStatus> {
    let info = stash::bridge_plugin(&state.pool).await.map_err(err)?;
    Ok(bridge_status_of(info, state.bridge_connected()))
}

/// Install the bridge from the public plugin index, then return refreshed status.
/// Blocks until the package job lands + plugins reload (up to ~30s); the frontend
/// shows a spinner. The user still has to refresh their Stash tab to go live.
#[tauri::command]
pub async fn bridge_install(state: State<'_, AppState>) -> CmdResult<BridgeStatus> {
    stash::install_bridge(&state.pool).await.map_err(err)?;
    let info = stash::bridge_plugin(&state.pool).await.map_err(err)?;
    Ok(bridge_status_of(info, state.bridge_connected()))
}

// ---------- Metadata refresh ----------

#[tauri::command]
pub async fn metadata_refresh_get(state: State<'_, AppState>) -> CmdResult<MetadataRefreshSetting> {
    settings::get_metadata_refresh(&state.pool).await.map_err(err)
}

#[tauri::command]
pub async fn metadata_refresh_set(
    state: State<'_, AppState>,
    enabled: bool,
    cadence_minutes: u32,
) -> CmdResult<MetadataRefreshSetting> {
    let value = MetadataRefreshSetting { enabled, cadence_minutes };
    settings::set_metadata_refresh(&state.pool, &value).await.map_err(err)?;
    Ok(value)
}

/// Live progress of the library-metadata sync for the Settings UI to poll.
#[tauri::command]
pub async fn metadata_status_get(state: State<'_, AppState>) -> CmdResult<MirrorStatus> {
    Ok(state.metadata_status.read().await.clone())
}

/// Refresh library metadata now (the Settings "Refresh now" button): re-pull
/// scene details for every known scene plus performer / studio / tag details and
/// images, ignoring staleness. Background; poll `metadata_status_get` for
/// progress. (The same refresh also runs automatically on launch and on the
/// configured cadence, but only for stale items.)
#[tauri::command]
pub async fn metadata_sync_now(state: State<'_, AppState>) -> CmdResult<()> {
    let pool = state.pool.clone();
    let status = state.metadata_status.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(e) = mirror::sync_metadata(&pool, &status, true).await {
            tracing::warn!("metadata sync_metadata failed: {:#}", e);
        }
    });
    Ok(())
}

// ---------- Default date preset (user pref, per dashboard section) ----------

#[tauri::command]
pub async fn default_date_preset_get(
    state: State<'_, AppState>,
    scope: String,
) -> CmdResult<String> {
    settings::get_default_date_preset(&state.pool, &scope).await.map_err(err)
}

#[tauri::command]
pub async fn default_date_preset_set(
    state: State<'_, AppState>,
    scope: String,
    preset: String,
) -> CmdResult<String> {
    settings::set_default_date_preset(&state.pool, &scope, &preset).await.map_err(err)?;
    Ok(preset)
}

// ---------- Default chart metric (user pref) ----------

#[tauri::command]
pub async fn default_chart_metric_get(
    state: State<'_, AppState>,
    scope: String,
) -> CmdResult<String> {
    settings::get_default_chart_metric(&state.pool, &scope).await.map_err(err)
}

#[tauri::command]
pub async fn default_chart_metric_set(
    state: State<'_, AppState>,
    scope: String,
    metric: String,
) -> CmdResult<String> {
    settings::set_default_chart_metric(&state.pool, &scope, &metric).await.map_err(err)?;
    Ok(metric)
}

// ---------- Default performer gender (Trends Top-performers toggle) ----------

#[tauri::command]
pub async fn default_performer_gender_get(state: State<'_, AppState>) -> CmdResult<String> {
    settings::get_default_performer_gender(&state.pool).await.map_err(err)
}

#[tauri::command]
pub async fn default_performer_gender_set(
    state: State<'_, AppState>,
    gender: String,
) -> CmdResult<String> {
    settings::set_default_performer_gender(&state.pool, &gender).await.map_err(err)?;
    Ok(gender)
}

// ---------- Per-page browse default view ----------

/// Saved default view (search/filters/sort/rows JSON) for a browse page, or null
/// if the user hasn't set one. `kind` ∈ {scene, performer, studio, tag}.
#[tauri::command]
pub async fn browse_default_get(
    state: State<'_, AppState>,
    kind: String,
) -> CmdResult<Option<String>> {
    settings::get_browse_default(&state.pool, &kind).await.map_err(err)
}

#[tauri::command]
pub async fn browse_default_set(
    state: State<'_, AppState>,
    kind: String,
    view: String,
) -> CmdResult<()> {
    settings::set_browse_default(&state.pool, &kind, &view).await.map_err(err)
}

/// Force-refresh a single content_item's metadata from Stash GraphQL right
/// now, bypassing the TTL staleness check. The debounce against rapid
/// repeated calls still applies. Background; returns immediately.
#[tauri::command]
pub async fn refresh_scene_metadata(
    state: State<'_, AppState>,
    content_item_id: i64,
) -> CmdResult<()> {
    // Look up the (external_id, source_key) for this content_item.
    let row: Option<(Option<String>, String)> = sqlx::query_as(
        "SELECT ci.external_id, s.key
         FROM content_items ci
         JOIN sources s ON s.id = ci.source_id
         WHERE ci.id = ?1",
    )
    .bind(content_item_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(err)?;

    let (external_id, source_key) = match row {
        Some((Some(eid), src)) => (eid, src),
        _ => return Err(format!("content_item {} has no external_id", content_item_id)),
    };

    state
        .session
        .refresh_stash_metadata(content_item_id, external_id, &source_key, true)
        .await;
    Ok(())
}

/// Kick off background refreshes for EVERY stash-sourced content_item,
/// regardless of TTL/staleness. The user clicked the button — refresh
/// means refresh. The 60s per-content-item in-memory debounce still
/// applies so a frantic user can't trigger duplicates. Actual fetches
/// happen asynchronously after this returns.
#[tauri::command]
pub async fn refresh_all_stash_metadata(state: State<'_, AppState>) -> CmdResult<usize> {
    let all = state.session.all_stash_content_ids().await.map_err(err)?;
    let queued = all.len();
    for (id, external_id) in all {
        state
            .session
            .refresh_stash_metadata(id, external_id, "stash", true)
            .await;
    }
    Ok(queued)
}

/// Variant that only queues content_items whose metadata is currently stale
/// or never-fetched. Kept for future use (cron sweeper, etc); not currently
/// wired to a UI button — user-initiated refresh always force-refreshes
/// everything via `refresh_all_stash_metadata`.
#[tauri::command]
pub async fn refresh_all_stale_metadata(state: State<'_, AppState>) -> CmdResult<usize> {
    let stale = state.session.stale_stash_content_ids().await.map_err(err)?;
    let queued = stale.len();
    for (id, external_id) in stale {
        state
            .session
            .refresh_stash_metadata(id, external_id, "stash", true)
            .await;
    }
    Ok(queued)
}

// ---------- Stash-history mirror (Phase 7) ----------

/// Read the mirror sync setting (background reconcile enabled + cadence).
#[tauri::command]
pub async fn mirror_sync_get(state: State<'_, AppState>) -> CmdResult<settings::MirrorSyncSetting> {
    settings::get_mirror_sync(&state.pool).await.map_err(err)
}

/// Update the mirror sync setting.
#[tauri::command]
pub async fn mirror_sync_set(
    state: State<'_, AppState>,
    enabled: bool,
    cadence_minutes: u32,
) -> CmdResult<settings::MirrorSyncSetting> {
    let value = settings::MirrorSyncSetting { enabled, cadence_minutes };
    settings::set_mirror_sync(&state.pool, &value).await.map_err(err)?;
    Ok(value)
}

/// Live import / reconcile progress for the Settings UI to poll.
#[tauri::command]
pub async fn mirror_status_get(state: State<'_, AppState>) -> CmdResult<MirrorStatus> {
    Ok(state.mirror_status.read().await.clone())
}

/// Run the Stash-history mirror sync now (the Settings "Sync now" button): pull
/// every active Stash scene, discover ones watched while Climax was closed, and
/// refresh cumshots / play count / watch duration. Background; poll
/// `mirror_status_get` for progress. (The same sync runs automatically on launch
/// and on the configured cadence.)
#[tauri::command]
pub async fn mirror_sync_now(state: State<'_, AppState>) -> CmdResult<()> {
    let pool = state.pool.clone();
    let status = state.mirror_status.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(e) = mirror::sync_active(&pool, &status).await {
            tracing::warn!("mirror sync_active failed: {:#}", e);
        }
    });
    Ok(())
}

/// Mirror self-check for one Stash scene id (verification aid): compares Climax's
/// stash_synced O-event count to Stash's live `o_counter`.
#[tauri::command]
pub async fn mirror_check_scene(
    state: State<'_, AppState>,
    scene_id: String,
) -> CmdResult<MirrorCheck> {
    mirror::check_scene(&state.pool, &scene_id).await.map_err(err)
}

// ---------- Trends (Phase 6) ----------

/// Contiguous, zero-filled per-day {sessions, cumshots, watch_time_ms} over the
/// range. The page's backbone: the frontend slices this client-side for the
/// summary cards, area chart, busiest-days, heatmap, and time-grouped report
/// rows.
#[tauri::command]
pub async fn trends_daily_series(
    state: State<'_, AppState>,
    start_day: String,
    end_day: String,
) -> CmdResult<Vec<crate::trends::DailyActivity>> {
    crate::trends::daily_series(&state.pool, &start_day, &end_day)
        .await
        .map_err(err)
}

/// Per-local-hour {cumshots, sessions, active_days} for the range. Powers the
/// Overview "Peak hours" chart and the report's "Time of day" grouping.
#[tauri::command]
pub async fn trends_hour_histogram(
    state: State<'_, AppState>,
    start_day: String,
    end_day: String,
) -> CmdResult<crate::trends::HourHistogram> {
    crate::trends::hour_histogram(&state.pool, &start_day, &end_day)
        .await
        .map_err(err)
}

/// Range-scoped per-entity totals for `dimension` ∈ {scene, performer, studio,
/// tag}. Powers the leaderboards (top N) + the report's entity groupings.
#[tauri::command]
pub async fn trends_entity_breakdown(
    state: State<'_, AppState>,
    dimension: String,
    start_day: String,
    end_day: String,
) -> CmdResult<Vec<crate::trends::EntityBreakdownRow>> {
    crate::trends::entity_breakdown(&state.pool, &dimension, &start_day, &end_day)
        .await
        .map_err(err)
}

/// Export a report to CSV via a native Save dialog. The frontend builds the CSV
/// string + a default filename; this opens the OS save dialog and writes the
/// file. Returns the chosen path, or None if the user cancelled.
#[tauri::command]
pub async fn trends_export_csv<R: Runtime>(
    app: AppHandle<R>,
    contents: String,
    default_name: String,
) -> CmdResult<Option<String>> {
    use tauri_plugin_dialog::DialogExt;
    // blocking_save_file must not run on the main thread; Tauri async commands
    // run on a worker, so this is safe and keeps the UI responsive.
    let file = app
        .dialog()
        .file()
        .set_file_name(&default_name)
        .add_filter("CSV", &["csv"])
        .blocking_save_file();
    match file {
        Some(fp) => {
            let path = fp.to_string();
            std::fs::write(&path, contents.as_bytes()).map_err(err)?;
            Ok(Some(path))
        }
        None => Ok(None),
    }
}

// ---------- Retroactive session reconstruction (Phase 7, component 2) ----------

/// Cluster the session-less Stash-history imports in `[start_day, end_day]` into
/// candidate sessions. Read-only / proposes only - never creates a session.
/// `gap_minutes` overrides the saved `session_gap` setting when provided.
/// `respect_dismissed` (default true) hides previously-dismissed candidates - the
/// ongoing prompt passes true; the Settings "import older history" scan passes
/// false so declined activity is still reachable.
#[tauri::command]
pub async fn reconstruct_candidates(
    state: State<'_, AppState>,
    start_day: String,
    end_day: String,
    gap_minutes: Option<u32>,
    respect_dismissed: Option<bool>,
) -> CmdResult<Vec<crate::reconstruct::CandidateSession>> {
    let gap = match gap_minutes {
        Some(g) => g,
        None => settings::get_session_gap(&state.pool)
            .await
            .map_err(err)?
            .gap_minutes,
    };
    crate::reconstruct::candidate_sessions(
        &state.pool,
        gap,
        &start_day,
        &end_day,
        respect_dismissed.unwrap_or(true),
    )
    .await
    .map_err(err)
}

/// Accept a (possibly edited) candidate: create an `estimated` session, adopt its
/// loose cumshots, and write per-scene watch rows. Returns the new session id.
#[tauri::command]
pub async fn reconstruct_accept(
    state: State<'_, AppState>,
    start_ms: i64,
    end_ms: i64,
    assigned_day: String,
    o_event_ids: Vec<i64>,
    scenes: Vec<crate::reconstruct::AcceptScene>,
) -> CmdResult<i64> {
    let (sid, pushes) = crate::reconstruct::accept_candidate(
        &state.pool,
        start_ms,
        end_ms,
        &assigned_day,
        &o_event_ids,
        &scenes,
    )
    .await
    .map_err(err)?;

    // Push manually-added cumshots to Stash in the background, mirroring log_o:
    // sceneAddO, then flip stash_synced=1 on success so a later mirror dedups
    // against them instead of re-importing.
    if !pushes.is_empty() {
        let pool = state.pool.clone();
        tokio::spawn(async move {
            for p in pushes {
                match stash::add_o(&pool, &p.scene_id, p.at_ms).await {
                    Ok(count) => {
                        tracing::info!(
                            "reconstruct stash sync: scene {} o_counter now {}",
                            p.scene_id,
                            count
                        );
                        let _ = sqlx::query("UPDATE o_events SET stash_synced = 1 WHERE id = ?1")
                            .bind(p.o_event_id)
                            .execute(&pool)
                            .await;
                    }
                    Err(e) => tracing::warn!(
                        "reconstruct stash sync failed for o_event {} scene {}: {:#}",
                        p.o_event_id,
                        p.scene_id,
                        e
                    ),
                }
            }
        });
    }

    Ok(sid)
}

/// Dismiss candidates so the "while you were away" prompt won't re-offer them.
/// (Still reachable via the Settings full scan.) Returns rows recorded.
#[tauri::command]
pub async fn reconstruct_dismiss(
    state: State<'_, AppState>,
    play_import_ids: Vec<i64>,
    o_event_ids: Vec<i64>,
) -> CmdResult<usize> {
    crate::reconstruct::dismiss_candidates(&state.pool, &play_import_ids, &o_event_ids)
        .await
        .map_err(err)
}

/// The "while you were away" launch scan: reconstruction candidates AFTER the most
/// recent logged session (the recency floor), dismissed-respecting. Reads the saved
/// session-gap. Returns `{ since_ms, candidates }` (since_ms = null on a fresh DB).
#[tauri::command]
pub async fn reconstruct_away_candidates(
    state: State<'_, AppState>,
) -> CmdResult<crate::reconstruct::AwayResult> {
    let gap = settings::get_session_gap(&state.pool)
        .await
        .map_err(err)?
        .gap_minutes;
    crate::reconstruct::away_candidates(&state.pool, gap)
        .await
        .map_err(err)
}

// ---------- First-launch onboarding ----------

/// Whether the first-launch setup wizard should run: not yet completed AND this
/// profile talks to Stash (the scratch profile has no Stash to import from, so
/// onboarding is meaningless there).
#[tauri::command]
pub async fn onboarding_needed(state: State<'_, AppState>) -> CmdResult<bool> {
    if !state.stash_enabled {
        return Ok(false);
    }
    let done = settings::get_onboarding_completed(&state.pool)
        .await
        .map_err(err)?;
    Ok(!done)
}

/// Mark the setup wizard finished (its final step). Idempotent.
#[tauri::command]
pub async fn onboarding_complete(state: State<'_, AppState>) -> CmdResult<()> {
    settings::set_onboarding_completed(&state.pool, true)
        .await
        .map_err(err)
}

/// Reset the first-launch flag so the wizard runs again — the "re-run setup"
/// button + the safe test hook (back up, reset, re-run, restore).
#[tauri::command]
pub async fn onboarding_reset(state: State<'_, AppState>) -> CmdResult<()> {
    settings::set_onboarding_completed(&state.pool, false)
        .await
        .map_err(err)
}

/// Onboarding "estimate my past sessions": bulk-accept every reconstruction
/// candidate across all history at the saved gap. Returns the number of
/// estimated sessions created. Runs inline (the wizard shows a spinner); no
/// Stash writes (every cumshot is an adopted, already-synced import).
#[tauri::command]
pub async fn onboarding_estimate_sessions(state: State<'_, AppState>) -> CmdResult<usize> {
    let gap = settings::get_session_gap(&state.pool)
        .await
        .map_err(err)?
        .gap_minutes;
    crate::reconstruct::accept_all_candidates(&state.pool, gap)
        .await
        .map_err(err)
}

/// Read the session-gap reconstruction threshold (minutes).
#[tauri::command]
pub async fn session_gap_get(state: State<'_, AppState>) -> CmdResult<settings::SessionGapSetting> {
    settings::get_session_gap(&state.pool).await.map_err(err)
}

/// Update the session-gap reconstruction threshold (minutes).
#[tauri::command]
pub async fn session_gap_set(
    state: State<'_, AppState>,
    gap_minutes: u32,
) -> CmdResult<settings::SessionGapSetting> {
    let value = settings::SessionGapSetting { gap_minutes };
    settings::set_session_gap(&state.pool, &value)
        .await
        .map_err(err)?;
    Ok(value)
}

/// Stash play/o history inside a completed session's window that the session
/// doesn't track yet (the "Stash logged activity here" review).
#[tauri::command]
pub async fn session_pending_history(
    state: State<'_, AppState>,
    session_id: i64,
    include_active: bool,
    include_dismissed: Option<bool>,
) -> CmdResult<crate::reconstruct::SessionPendingHistory> {
    crate::reconstruct::session_pending_history(
        &state.pool,
        session_id,
        include_active,
        include_dismissed.unwrap_or(false),
    )
    .await
    .map_err(err)
}

/// Fold selected Stash history (scenes + session-less cumshots) into the session.
#[tauri::command]
pub async fn session_absorb_history(
    state: State<'_, AppState>,
    session_id: i64,
    content_item_ids: Vec<i64>,
    o_event_ids: Vec<i64>,
) -> CmdResult<()> {
    crate::reconstruct::absorb_session_history(
        &state.pool,
        session_id,
        &content_item_ids,
        &o_event_ids,
    )
    .await
    .map_err(err)
}

/// Skip selected Stash history so the review stops offering it for this session.
/// `mirror_to_reconstruction` (wrap-up skip only) also suppresses the items from
/// the reconstruction prompt; see `reconstruct::dismiss_session_history`.
#[tauri::command]
pub async fn session_dismiss_history(
    state: State<'_, AppState>,
    session_id: i64,
    content_item_ids: Vec<i64>,
    o_event_ids: Vec<i64>,
    mirror_to_reconstruction: Option<bool>,
) -> CmdResult<()> {
    crate::reconstruct::dismiss_session_history(
        &state.pool,
        session_id,
        &content_item_ids,
        &o_event_ids,
        mirror_to_reconstruction.unwrap_or(false),
    )
    .await
    .map_err(err)
}

/// Preview merging two sessions (enable the action + report the time gap).
#[tauri::command]
pub async fn session_merge_check(
    state: State<'_, AppState>,
    a_id: i64,
    b_id: i64,
) -> CmdResult<crate::session::MergeCheck> {
    state.session.merge_check(a_id, b_id).await.map_err(err)
}

/// Merge two consecutive sessions into one spanning both. `gap_as_pause` inserts
/// a pause for any time gap so the duration stays accurate. Returns the
/// surviving (earlier) session id.
#[tauri::command]
pub async fn session_merge(
    state: State<'_, AppState>,
    a_id: i64,
    b_id: i64,
    gap_as_pause: bool,
) -> CmdResult<i64> {
    state
        .session
        .merge_sessions(a_id, b_id, gap_as_pause)
        .await
        .map_err(err)
}

/// Preview reopening a session (Sessions-page "Reopen"): whether this session
/// can be made live again, plus how long ago it ended.
#[tauri::command]
pub async fn session_reopen_check(
    state: State<'_, AppState>,
    session_id: i64,
) -> CmdResult<crate::session::ReopenCheck> {
    state.session.reopen_check(session_id).await.map_err(err)
}

/// Reopen an ended session: it goes active again and tracks live (for a session
/// stopped by accident). `gap_as_pause` makes the time since it ended a pause
/// rather than counted session time. Returns null if it can't be reopened.
#[tauri::command]
pub async fn session_reopen(
    state: State<'_, AppState>,
    session_id: i64,
    gap_as_pause: bool,
) -> CmdResult<Option<crate::models::Session>> {
    state
        .session
        .reopen(session_id, gap_as_pause)
        .await
        .map_err(err)
}

/// Preview extending a session (Sessions-page "Extend"): whether there's
/// untracked activity right after it to fold in, plus the gap + counts.
#[tauri::command]
pub async fn session_extend_check(
    state: State<'_, AppState>,
    session_id: i64,
) -> CmdResult<crate::reconstruct::ExtendSessionCheck> {
    crate::reconstruct::extend_session_check(&state.pool, session_id)
        .await
        .map_err(err)
}

/// Continue a session: fold the untracked activity that follows it into the
/// session, extending its end. `gap_as_pause` makes the untracked gap a pause
/// (excluded from session time) rather than counted. Returns false if there was
/// nothing to continue.
#[tauri::command]
pub async fn session_extend(
    state: State<'_, AppState>,
    session_id: i64,
    gap_as_pause: bool,
) -> CmdResult<bool> {
    crate::reconstruct::extend_session(&state.pool, session_id, gap_as_pause)
        .await
        .map_err(err)
}

// ---------- Client-mode HTTP proxy (Phase 2 app-as-client) ----------

/// Desktop client-mode HTTP transport. The webview can't `fetch` the Climax
/// server cross-origin - the server sends no CORS headers (the browser web client
/// is served SAME-ORIGIN instead) - so when the desktop app runs in client mode
/// the frontend (`transport.ts`) forwards its DATA commands through here: this
/// POSTs `{cmd, args}` to the configured server's `POST /rpc` from Rust (reqwest,
/// free of the browser's CORS policy) and returns the raw JSON result. NATIVE
/// commands never reach here - they run locally over IPC. Mirrors `/rpc`'s
/// contract: the server's Ok -> the value; a non-2xx or transport failure -> an
/// Err string (which the frontend surfaces exactly like a rejected `invoke`).
#[tauri::command]
pub async fn rpc_http(
    base: String,
    cmd: String,
    args: serde_json::Value,
    token: Option<String>,
) -> CmdResult<serde_json::Value> {
    let url = format!("{}/rpc", base.trim_end_matches('/'));
    // A timeout so a configured-but-unreachable server can't hang data calls
    // forever (a routable-but-dead LAN host won't refuse the connection promptly).
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(err)?;
    let mut req = client
        .post(&url)
        .json(&serde_json::json!({ "cmd": cmd, "args": args }));
    // The server's shared token (Phase 3), when it requires one.
    if let Some(t) = token.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
        req = req.header("X-Climax-Token", t);
    }
    let resp = req
        .send()
        .await
        .map_err(|e| {
            tracing::warn!("rpc_http: POST {} ({}) failed: {:#}", url, cmd, e);
            "Couldn't reach the Climax server.".to_string()
        })?;
    let status = resp.status();
    // Parse leniently: an error status can carry a non-JSON body (a proxy's
    // HTML 502 page, say) - report the HTTP status for those, not a parse
    // failure. Only a 2xx with an unreadable body is its own error.
    let body: serde_json::Value = match resp.json().await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("rpc_http: reading the reply for {} failed: {:#}", cmd, e);
            if status.is_success() {
                return Err("The Climax server sent a reply that couldn't be read.".to_string());
            }
            serde_json::Value::Null
        }
    };
    if status.is_success() {
        Ok(body)
    } else {
        let msg = body
            .get("error")
            .and_then(|e| e.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| {
                tracing::warn!("rpc_http: {} -> HTTP {} with no error field: {}", cmd, status, body);
                format!("The Climax server returned an error (HTTP {}).", status.as_u16())
            });
        Err(msg)
    }
}

/// Connection probe for desktop client mode: GET `{base}/health` from Rust
/// (CORS-free, like `rpc_http`) and report whether the address is a reachable,
/// healthy Climax server. Drives the connection indicator's polling + the
/// Settings "test connection" button. Returns `Ok(false)` (not an error) for any
/// unreachable / non-Climax / timed-out endpoint so the caller treats it as a
/// plain boolean state. A 4s timeout keeps a dead LAN address from hanging.
#[tauri::command]
pub async fn server_ping(base: String) -> CmdResult<bool> {
    let url = format!("{}/health", base.trim_end_matches('/'));
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(4))
        .build()
    {
        Ok(c) => c,
        Err(_) => return Ok(false),
    };
    let ok = match client.get(&url).send().await {
        Ok(r) if r.status().is_success() => {
            let body: serde_json::Value = r.json().await.unwrap_or(serde_json::Value::Null);
            // Confirm it's actually a Climax server, not just any 200.
            body.get("service").and_then(|s| s.as_str()) == Some("climax")
        }
        _ => false,
    };
    Ok(ok)
}

/// The newest published release, plus which release the user has dismissed.
/// Reads the cache only - never touches the network, so any view can call it.
#[tauri::command]
pub async fn update_check(state: State<'_, AppState>) -> CmdResult<crate::rpc::UpdateInfo> {
    Ok(crate::rpc::UpdateInfo {
        backend_version: state.app_version.clone(),
        cache: settings::get_update_check(&state.pool).await.map_err(err)?,
        dismissed_version: settings::get_update_dismissed(&state.pool)
            .await
            .map_err(err)?,
    })
}

/// Ask GitHub now, ignoring the cache TTL. Awaited rather than spawned so the
/// button that calls it can go straight from a busy label to the answer.
#[tauri::command]
pub async fn update_check_now(state: State<'_, AppState>) -> CmdResult<crate::rpc::UpdateInfo> {
    Ok(crate::rpc::UpdateInfo {
        backend_version: state.app_version.clone(),
        cache: crate::update::check_now(&state.pool).await.map_err(err)?,
        dismissed_version: settings::get_update_dismissed(&state.pool)
            .await
            .map_err(err)?,
    })
}

/// Stop showing the update notice until something newer than `version` exists.
#[tauri::command]
pub async fn update_dismiss(state: State<'_, AppState>, version: String) -> CmdResult<()> {
    settings::set_update_dismissed(&state.pool, &version)
        .await
        .map_err(err)
}

/// The version of THIS app, from `tauri.conf.json` (the one that brands the
/// installer), not from any Cargo manifest.
///
/// NATIVE by necessity: in client mode `/rpc` reaches the server, which would
/// answer with its own version - and the whole point of this command is for the
/// desktop app to know what IT is running, since the two can differ.
#[tauri::command]
pub async fn app_version_get<R: Runtime>(app: AppHandle<R>) -> CmdResult<String> {
    Ok(app.package_info().version.to_string())
}

/// What port this app's own backend serves on, and whether it actually got it.
#[derive(serde::Serialize)]
pub struct ServerPortInfo {
    /// The configured port. In client mode nothing is bound, so this is the port
    /// the app WOULD serve on if it switched back to the built-in backend.
    pub port: u16,
    /// False only when the in-process server tried to bind this port and failed -
    /// which means the bridge cannot reach this app and nothing is being tracked.
    /// True in client mode, where no local server runs and so nothing went wrong.
    pub ok: bool,
    /// The failure, as a sentence fit to show, when `ok` is false.
    pub error: Option<String>,
}

/// NATIVE, not a data command: this describes THIS process's own listener, so in
/// client mode it must not travel to the external server (which would answer
/// about its own port). See the NATIVE_COMMANDS allowlist in transport.ts.
#[tauri::command]
pub async fn server_port_get(state: State<'_, AppState>) -> CmdResult<ServerPortInfo> {
    let bind = crate::SERVER_BIND.get();
    Ok(ServerPortInfo {
        port: match bind {
            Some(b) => b.port,
            // No bind was attempted (client mode) - report what's configured.
            None => settings::get_http_port(&state.pool).await.map_err(err)?,
        },
        ok: bind.map(|b| b.error.is_none()).unwrap_or(true),
        error: bind.and_then(|b| b.error.clone()),
    })
}

/// Persist the port for the in-process backend. Takes effect on the next launch,
/// since the server binds at boot - the Settings page restarts the app to apply.
#[tauri::command]
pub async fn server_port_set(state: State<'_, AppState>, port: u32) -> CmdResult<()> {
    settings::set_http_port(&state.pool, port).await.map_err(err)
}

/// Read the Rust-side client config (the boot record of whether this app is a thin
/// client of an external server). Mirrors the frontend's localStorage; the shell
/// reads THIS at boot to decide whether to spawn its in-process backend. Returns
/// the server URL, or null for the in-process default.
#[tauri::command]
pub async fn client_config_get(state: State<'_, AppState>) -> CmdResult<Option<String>> {
    Ok(crate::client_config::read_server_url(&state.db_dir))
}

/// Persist the client config. The Settings "Server / connection" page calls this
/// alongside its localStorage write so the Rust boot path + the frontend transport
/// agree. Takes effect on the NEXT launch (the boot branch reads it). null = the
/// in-process default. `token` is the server's shared auth token (Phase 3), if it
/// requires one.
#[tauri::command]
pub async fn client_config_set(
    state: State<'_, AppState>,
    server_url: Option<String>,
    token: Option<String>,
) -> CmdResult<()> {
    crate::client_config::write_server_url(&state.db_dir, server_url, token).map_err(err)
}

/// Relaunch the app so the Rust BOOT path re-reads client.json (to switch between
/// the in-process backend and thin-client mode - a webview reload can't do it).
/// Returns whether it actually relaunched: in a RELEASE build it does (the frontend
/// is bundled), so this never really returns. In DEV (`npm run tauri dev`) a
/// relaunch ORPHANS the Vite dev server - the tauri-cli tears Vite down when the
/// original binary exits, so the relaunched app lands on a dead "can't reach
/// localhost" page. So in dev we DON'T auto-restart; we return false and the caller
/// asks the user to restart manually (re-run the dev app).
/// The `.app` bundle this binary lives inside, if any:
/// `.../Climax.app/Contents/MacOS/climax` -> `.../Climax.app`. `None` when
/// running the bare binary (dev, or `target/release/climax` directly).
#[cfg(target_os = "macos")]
fn macos_bundle_path() -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let bundle = exe.parent()?.parent()?.parent()?;
    (bundle.extension()?.to_str()? == "app").then(|| bundle.to_path_buf())
}

#[tauri::command]
pub async fn restart_app<R: Runtime>(app: AppHandle<R>) -> CmdResult<bool> {
    if cfg!(debug_assertions) {
        return Ok(false);
    }
    server::release_port(RESTART_PORT_RELEASE).await;

    // macOS: hand the BUNDLE to LaunchServices instead of re-executing the
    // binary. `app.restart()` spawns `Contents/MacOS/climax` directly, which
    // does not reliably come back for a bundled .app - observed live: Apply and
    // restart quit the app and it simply never reappeared, and launching it by
    // hand afterwards worked fine. `open -n` starts it exactly as a double-click
    // would. The successor racing us for the port is already covered by
    // `bind_with_retry`, which exists for this case (see CLAUDE.md).
    #[cfg(target_os = "macos")]
    if let Some(bundle) = macos_bundle_path() {
        match std::process::Command::new("/usr/bin/open").arg("-n").arg(&bundle).spawn() {
            Ok(_) => {
                app.exit(0);
                return Ok(true);
            }
            Err(e) => tracing::warn!("macOS relaunch via `open` failed ({e}); falling back"),
        }
    }

    app.restart();
    #[allow(unreachable_code)]
    Ok(true)
}
