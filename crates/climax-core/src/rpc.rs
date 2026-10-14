//! Transport-agnostic command dispatch (HTTP /rpc) - the headless command
//! surface, shared by the desktop app and the standalone server.
//!
//! The desktop app reaches these commands over Tauri IPC (the invoke_handler in
//! the shell's lib.rs). A browser / web client can't use Tauri IPC, so it reaches
//! the SAME command logic over HTTP via the server's POST /rpc endpoint
//! (server.rs), which calls `rpc_dispatch` below. Args arrive as the JSON object
//! the frontend's `call()` sends - the SAME camelCase keys the Tauri path uses
//! (Tauri converts them to snake_case params; here we read them straight), so no
//! per-key drift.
//!
//! Migration is command-by-command: each pure-data command gets an arm here that
//! mirrors its Tauri body (calling the same module fn). Unmigrated commands
//! return an error - harmless, since no web client exists yet. NATIVE commands
//! (window / tray / dialog / autostart / restart - the ones taking AppHandle)
//! are intentionally absent: they only make sense on the desktop client, which
//! keeps using Tauri IPC for them. They relocate to client capabilities that
//! report to the server in a later phase, not to this data RPC.
//!
//! Extracted from the shell's commands.rs into climax-core in Phase 2b so the
//! standalone (headless) server can serve /rpc without the Tauri shell.

use crate::catalog;
use crate::dashboard::{self, FilterParams};
use crate::mirror;
use crate::models::{SessionPage, SessionSort};
use crate::settings::{
    self, IdleDetectionSetting, MetadataRefreshSetting, PlayCountingSetting, RemoteTrackingSetting,
    StashConnectionSetting,
};
use crate::stash;
use crate::state::AppState;
use crate::update;

type CmdResult<T> = Result<T, String>;

fn err<E: std::fmt::Display>(e: E) -> String {
    format!("{:#}", e)
}

/// Everything the update UI needs, in one read.
///
/// Note what is NOT here: any judgement about whether the CALLER is out of date.
/// `backend_version` is the version of the process that answered, which in
/// client mode is the server rather than the desktop app asking. Each client
/// compares the release against its own version; mixing that in here is how you
/// end up telling someone their app is current because their server is.
#[derive(serde::Serialize)]
pub struct UpdateInfo {
    pub backend_version: String,
    #[serde(flatten)]
    pub cache: crate::settings::UpdateCheckCache,
    /// The release the user has already waved away, if any.
    pub dismissed_version: Option<String>,
}

/// The active launch profile, for the scratch-mode UI indicator.
#[derive(serde::Serialize)]
pub struct ProfileInfo {
    /// "real" or "scratch".
    pub profile: String,
    pub stash_enabled: bool,
    pub db_filename: String,
}

pub fn filter_from_args(
    start_day: String,
    end_day: String,
    scene_content_item_ids: Option<Vec<i64>>,
    performer_ids: Option<Vec<String>>,
    studio_ids: Option<Vec<String>>,
    tag_ids: Option<Vec<String>>,
) -> FilterParams {
    FilterParams {
        start_day,
        end_day,
        scene_content_item_ids: scene_content_item_ids.unwrap_or_default(),
        performer_ids: performer_ids.unwrap_or_default(),
        studio_ids: studio_ids.unwrap_or_default(),
        tag_ids: tag_ids.unwrap_or_default(),
    }
}

/// Combined bridge state for the onboarding bridge step: install/enabled/version
/// pulled from Stash's loaded-plugin list, plus `connected` = a bridge is live
/// on the local `/ws` socket right now (a Stash tab is open running it).
#[derive(serde::Serialize)]
pub struct BridgeStatus {
    pub installed: bool,
    pub enabled: bool,
    pub version: Option<String>,
    pub connected: bool,
}

pub fn bridge_status_of(info: stash::BridgePluginInfo, connected: bool) -> BridgeStatus {
    BridgeStatus {
        installed: info.installed,
        enabled: info.enabled,
        version: info.version,
        connected,
    }
}

/// Pull one named argument out of the JSON args object (absent / null -> the
/// type's null deserialization, so `Option<_>` args become `None`).
fn rpc_arg<T: serde::de::DeserializeOwned>(args: &serde_json::Value, name: &str) -> CmdResult<T> {
    serde_json::from_value(args.get(name).cloned().unwrap_or(serde_json::Value::Null))
        .map_err(|e| format!("rpc arg '{}': {}", name, e))
}

/// Serialize a command result into the generic JSON the /rpc handler returns.
fn rpc_ok<T: serde::Serialize>(v: T) -> CmdResult<serde_json::Value> {
    serde_json::to_value(v).map_err(|e| format!("rpc serialize: {}", e))
}

/// Dispatch a command by name over the HTTP transport. Returns the result as
/// JSON, or an error string (the /rpc handler maps Ok -> 200 + value, Err ->
/// 400 + {error}). Mirrors each command's Tauri body exactly.
pub async fn rpc_dispatch(
    state: &AppState,
    cmd: &str,
    args: &serde_json::Value,
) -> CmdResult<serde_json::Value> {
    match cmd {
        // -- no-arg reads --
        "profile_get" => rpc_ok(ProfileInfo {
            profile: if state.stash_enabled { "real" } else { "scratch" }.to_string(),
            stash_enabled: state.stash_enabled,
            db_filename: state.db_filename.clone(),
        }),
        // Update check. Deliberately three arms rather than one: the plain read
        // never touches the network (so any view can call it freely), the
        // explicit check always does, and the dismissal is a separate write.
        "update_check" => rpc_ok(UpdateInfo {
            backend_version: state.app_version.clone(),
            cache: settings::get_update_check(&state.pool).await.map_err(err)?,
            dismissed_version: settings::get_update_dismissed(&state.pool)
                .await
                .map_err(err)?,
        }),
        "update_check_now" => rpc_ok(UpdateInfo {
            backend_version: state.app_version.clone(),
            // Awaited, not spawned: the caller is a button that shows a busy
            // label, and it should end by showing the answer rather than by
            // starting a poll. Bounded by the fetch timeout.
            cache: update::check_now(&state.pool).await.map_err(err)?,
            dismissed_version: settings::get_update_dismissed(&state.pool)
                .await
                .map_err(err)?,
        }),
        "update_dismiss" => {
            let version: String = rpc_arg(args, "version")?;
            settings::set_update_dismissed(&state.pool, &version)
                .await
                .map_err(err)?;
            rpc_ok(())
        }
        "dashboard_hero_stats" => rpc_ok(dashboard::hero_stats(&state.pool).await.map_err(err)?),
        "catalog_list_performers" => {
            rpc_ok(dashboard::list_performers(&state.pool).await.map_err(err)?)
        }

        // -- scalar args --
        "dashboard_daily_buckets" => {
            let year: i32 = rpc_arg(args, "year")?;
            let month: u32 = rpc_arg(args, "month")?;
            rpc_ok(dashboard::daily_buckets(&state.pool, year, month).await.map_err(err)?)
        }

        // -- filter-struct args (camelCase keys; null/absent -> None) --
        "dashboard_filtered_buckets" => {
            let f = filter_from_args(
                rpc_arg(args, "startDay")?,
                rpc_arg(args, "endDay")?,
                rpc_arg(args, "sceneContentItemIds")?,
                rpc_arg(args, "performerIds")?,
                rpc_arg(args, "studioIds")?,
                rpc_arg(args, "tagIds")?,
            );
            let granularity: String = rpc_arg(args, "granularity")?;
            rpc_ok(
                dashboard::filtered_buckets(&state.pool, &f, &granularity)
                    .await
                    .map_err(err)?,
            )
        }

        "session_start" => rpc_ok(state.session.start().await.map_err(err)?),
        "session_stop" => rpc_ok(state.session.stop().await.map_err(err)?),
        "session_discard" => rpc_ok(state.session.discard().await.map_err(err)?),
        "session_pause" => rpc_ok(state.session.pause("manual").await.map_err(err)?),
        "session_resume" => rpc_ok(state.session.resume().await.map_err(err)?),
        "session_active" => rpc_ok(state.session.active().await.map_err(err)?),
        // Idle/crash gap resolution, server-side. The desktop client's native
        // idle loop (presence.rs) can't reach the server's SessionManager
        // directly, so its "Discard & Continue" / "Discard & End" resolves route
        // here (via the shell's `remote_session`). Mirror the idle_* command
        // bodies. Not exposed as Tauri commands - the desktop's idle_* commands
        // are the native entry points; these are the server-side counterparts.
        "record_gap_pause" => {
            let session_id: i64 = rpc_arg(args, "sessionId")?;
            let paused_at: i64 = rpc_arg(args, "pausedAt")?;
            let resumed_at: i64 = rpc_arg(args, "resumedAt")?;
            let reason: String = rpc_arg(args, "reason")?;
            state
                .session
                .record_gap_pause(session_id, paused_at, resumed_at, &reason)
                .await
                .map_err(err)?;
            rpc_ok(())
        }
        "end_session_at" => {
            let session_id: i64 = rpc_arg(args, "sessionId")?;
            let ended_at: i64 = rpc_arg(args, "endedAt")?;
            rpc_ok(
                state
                    .session
                    .end_session_at(session_id, ended_at)
                    .await
                    .map_err(err)?,
            )
        }
        "session_get" => {
    let id: i64 = rpc_arg(args, "id")?;
    rpc_ok(state.session.get(id).await.map_err(err)?)
},
        "sessions_list" => {
    let limit: Option<i64> = rpc_arg(args, "limit")?;
    let offset: Option<i64> = rpc_arg(args, "offset")?;
    rpc_ok(state.session.list(limit.unwrap_or(50), offset.unwrap_or(0)).await.map_err(err)?)
},
        "sessions_page" => {
    let start_day: String = rpc_arg(args, "startDay")?;
    let end_day: String = rpc_arg(args, "endDay")?;
    let sort: Option<String> = rpc_arg(args, "sort")?;
    let desc: Option<bool> = rpc_arg(args, "desc")?;
    let limit: Option<i64> = rpc_arg(args, "limit")?;
    let offset: Option<i64> = rpc_arg(args, "offset")?;
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
    rpc_ok(SessionPage { rows, total })
},
        "sessions_in_range" => {
    let start_ms: i64 = rpc_arg(args, "startMs")?;
    let end_ms: i64 = rpc_arg(args, "endMs")?;
    rpc_ok(state.session.list_in_range(start_ms, end_ms).await.map_err(err)?)
},
        "sessions_for_day" => {
    let day: String = rpc_arg(args, "day")?;
    rpc_ok(state.session.list_for_day(&day).await.map_err(err)?)
},
        "session_set_assigned_day" => {
    let session_id: i64 = rpc_arg(args, "sessionId")?;
    let day: String = rpc_arg(args, "day")?;
    rpc_ok(state.session.set_assigned_day(session_id, &day).await.map_err(err)?)
},
        "session_scenes" => {
    let session_id: i64 = rpc_arg(args, "sessionId")?;
    rpc_ok(state.session.scenes_in_session(session_id).await.map_err(err)?)
},
        "o_log" => {
    let session_id: Option<i64> = rpc_arg(args, "sessionId")?;
    let content_item_id: Option<i64> = rpc_arg(args, "contentItemId")?;
    let occurred_at: Option<i64> = rpc_arg(args, "occurredAt")?;
    let intensity: Option<i64> = rpc_arg(args, "intensity")?;
    let notes: Option<String> = rpc_arg(args, "notes")?;
    rpc_ok(
        state
            .session
            .log_o(session_id, content_item_id, occurred_at, intensity, notes, "climax")
            .await
            .map_err(err)?,
    )
},
        "o_list_for_session" => {
    let session_id: i64 = rpc_arg(args, "sessionId")?;
    rpc_ok(state.session.o_events_for_session(session_id).await.map_err(err)?)
},
        "o_delete" => {
    let o_id: i64 = rpc_arg(args, "oId")?;
    state.session.delete_o(o_id).await.map_err(err)?;
    rpc_ok(())
},
        "o_delete_recent_for_scene" => {
    let session_id: i64 = rpc_arg(args, "sessionId")?;
    let content_item_id: i64 = rpc_arg(args, "contentItemId")?;
    let count: i64 = rpc_arg(args, "count")?;
    rpc_ok(state.session.delete_recent_o_for_scene(session_id, content_item_id, count).await.map_err(err)?)
},
        "o_event_update" => {
    let o_id: i64 = rpc_arg(args, "oId")?;
    let occurred_at: Option<i64> = rpc_arg(args, "occurredAt")?;
    let content_item_id: Option<i64> = rpc_arg(args, "contentItemId")?;
    let notes: Option<String> = rpc_arg(args, "notes")?;
    let intensity: Option<i64> = rpc_arg(args, "intensity")?;
    rpc_ok(
        state
            .session
            .update_o_event(o_id, occurred_at, content_item_id, notes, intensity)
            .await
            .map_err(err)?,
    )
},
        "o_sessionless_unlinked_today" => {
    rpc_ok(state.session.sessionless_unlinked_today().await.map_err(err)?)
},
        "session_delete" => {
    let session_id: i64 = rpc_arg(args, "sessionId")?;
    state.session.delete_session(session_id).await.map_err(err)?;
    rpc_ok(())
},
        "scene_delete_from_session" => {
    let session_id: i64 = rpc_arg(args, "sessionId")?;
    let content_item_id: i64 = rpc_arg(args, "contentItemId")?;
    state.session.delete_scene_from_session(session_id, content_item_id).await.map_err(err)?;
    rpc_ok(())
},
        "scene_detach_from_session" => {
    let session_id: i64 = rpc_arg(args, "sessionId")?;
    let content_item_id: i64 = rpc_arg(args, "contentItemId")?;
    state.session.detach_scene_from_session(session_id, content_item_id).await.map_err(err)?;
    rpc_ok(())
},
        "session_delete_many" => {
    let session_ids: Vec<i64> = rpc_arg(args, "sessionIds")?;
    rpc_ok(state.session.delete_sessions_many(&session_ids).await.map_err(err)?)
},
        "session_update" => {
    let session_id: i64 = rpc_arg(args, "sessionId")?;
    let started_at: Option<i64> = rpc_arg(args, "startedAt")?;
    let ended_at: Option<i64> = rpc_arg(args, "endedAt")?;
    let notes: Option<String> = rpc_arg(args, "notes")?;
    let excluded: Option<bool> = rpc_arg(args, "excluded")?;
    let session_type: Option<String> = rpc_arg(args, "sessionType")?;
    rpc_ok(state.session.update_session(session_id, started_at, ended_at, notes, excluded, session_type).await.map_err(err)?)
},
        "idle_detection_get" => rpc_ok(settings::get_idle_detection(&state.pool).await.map_err(err)?),
        "idle_detection_set" => {
    let enabled: bool = rpc_arg(args, "enabled")?;
    let threshold_minutes: u32 = rpc_arg(args, "thresholdMinutes")?;
    let value = IdleDetectionSetting {
        enabled,
        threshold_minutes: threshold_minutes.max(1),
    };
    settings::set_idle_detection(&state.pool, &value).await.map_err(err)?;
    rpc_ok(value)
},
        "remote_tracking_get" => rpc_ok(settings::get_remote_tracking(&state.pool).await.map_err(err)?),
        "remote_tracking_set" => {
    let enabled: bool = rpc_arg(args, "enabled")?;
    let poll_seconds: u32 = rpc_arg(args, "pollSeconds")?;
    let value = RemoteTrackingSetting {
        enabled,
        poll_seconds: poll_seconds.clamp(5, 60),
    };
    settings::set_remote_tracking(&state.pool, &value).await.map_err(err)?;
    rpc_ok(value)
},
        "capture_prompt_get" => rpc_ok(settings::get_capture_prompt(&state.pool).await.map_err(err)?),
        "capture_prompt_set" => {
    let enabled: bool = rpc_arg(args, "enabled")?;
    let threshold_minutes: u32 = rpc_arg(args, "thresholdMinutes")?;
    let snooze_minutes: u32 = rpc_arg(args, "snoozeMinutes")?;
    let value = settings::CapturePromptSetting {
        enabled,
        threshold_minutes: threshold_minutes.max(1),
        snooze_minutes: snooze_minutes.max(1),
    };
    settings::set_capture_prompt(&state.pool, &value).await.map_err(err)?;
    rpc_ok(value)
},
        "capture_pending_get" => rpc_ok(state.pending_capture.read().await.clone()),
        // Server-side passive-capture resolve, for a desktop CLIENT (the server
        // detected the watch + holds pending_capture; the client's native command
        // does the window op + delegates the session work here). Mirror the
        // capture_accept / capture_snooze command bodies.
        "capture_accept" => {
            let payload = { state.pending_capture.write().await.take() };
            let session = match payload {
                Some(p) => Some(
                    state
                        .session
                        .start_backdated_with_scene(p.watch_started_at, p.content_item_id)
                        .await
                        .map_err(err)?,
                ),
                None => None,
            };
            rpc_ok(session)
        }
        "capture_snooze" => {
            let cfg = settings::get_capture_prompt(&state.pool).await.map_err(err)?;
            *state.capture_snooze_until.write().await =
                crate::db::now_ms() + (cfg.snooze_minutes as i64).max(1) * 60_000;
            *state.pending_capture.write().await = None;
            state.session.clear_no_session_watch().await;
            rpc_ok(())
        }
        "play_counting_get" => rpc_ok(settings::get_play_counting(&state.pool).await.map_err(err)?),
        "play_counting_set" => {
    let threshold_pct: Option<f32> = rpc_arg(args, "thresholdPct")?;
    let value = PlayCountingSetting {
        threshold_pct: threshold_pct.map(|p| p.clamp(0.0, 100.0)),
    };
    settings::set_play_counting(&state.pool, &value).await.map_err(err)?;
    rpc_ok(value)
},
        "play_counting_sync_from_stash" => {
    match crate::stash::fetch_minimum_play_percent(&state.pool).await {
        Ok(Some(pct)) => {
            let value = PlayCountingSetting { threshold_pct: Some(pct) };
            settings::set_play_counting(&state.pool, &value).await.map_err(err)?;
            rpc_ok(value)
        }
        Ok(None) => rpc_ok(settings::get_play_counting(&state.pool).await.map_err(err)?),
        Err(e) => {
            // Renders next to the Settings sync button; the top-level message is
            // already user-shaped, the chain goes to the log.
            tracing::warn!("play threshold sync from Stash failed: {:#}", e);
            Err(format!("{}", e))
        }
    }
},
        "idle_pending_get" => rpc_ok(state.pending_idle.read().await.clone()),
        "dashboard_monthly_buckets" => {
    let months: Option<u32> = rpc_arg(args, "months")?;
    rpc_ok(dashboard::monthly_buckets(&state.pool, months.unwrap_or(12)).await.map_err(err)?)
},
        "stash_connection_get" => rpc_ok(settings::get_stash_connection(&state.pool).await.map_err(err)?),
        "stash_connection_set" => {
    let url: String = rpc_arg(args, "url")?;
    let api_key: Option<String> = rpc_arg(args, "apiKey")?;
    let value = StashConnectionSetting { url, api_key };
    settings::set_stash_connection(&state.pool, &value).await.map_err(err)?;
    // A fresh / headless server connects Stash AFTER boot (in onboarding), when
    // the boot-time threshold sync couldn't reach it - so sync the play-counting
    // threshold from Stash now too, but only if the user hasn't set one.
    // Best-effort + background so a slow / wrong Stash can't delay the save.
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
    // Re-read so callers see the normalised version (trimmed, slash-stripped, empty api_key -> None).
    rpc_ok(settings::get_stash_connection(&state.pool).await.map_err(err)?)
},
        "stash_connection_test" => rpc_ok(stash::test_connection(&state.pool).await),
        "bridge_status" => {
    let info = stash::bridge_plugin(&state.pool).await.map_err(err)?;
    rpc_ok(bridge_status_of(info, state.bridge_connected()))
},
        "bridge_install" => {
    stash::install_bridge(&state.pool).await.map_err(err)?;
    let info = stash::bridge_plugin(&state.pool).await.map_err(err)?;
    rpc_ok(bridge_status_of(info, state.bridge_connected()))
},
        "metadata_refresh_get" => rpc_ok(settings::get_metadata_refresh(&state.pool).await.map_err(err)?),
        "metadata_refresh_set" => {
    let enabled: bool = rpc_arg(args, "enabled")?;
    let cadence_minutes: u32 = rpc_arg(args, "cadenceMinutes")?;
    let value = MetadataRefreshSetting { enabled, cadence_minutes };
    settings::set_metadata_refresh(&state.pool, &value).await.map_err(err)?;
    rpc_ok(value)
},
        "metadata_status_get" => rpc_ok(state.metadata_status.read().await.clone()),
        "metadata_sync_now" => {
    let pool = state.pool.clone();
    let status = state.metadata_status.clone();
    tokio::spawn(async move {
        if let Err(e) = mirror::sync_metadata(&pool, &status, true).await {
            tracing::warn!("metadata sync_metadata failed: {:#}", e);
        }
    });
    rpc_ok(())
},
        "refresh_scene_metadata" => {
    let content_item_id: i64 = rpc_arg(args, "contentItemId")?;
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
    rpc_ok(())
},
        "refresh_all_stash_metadata" => {
    let all = state.session.all_stash_content_ids().await.map_err(err)?;
    let queued = all.len();
    for (id, external_id) in all {
        state
            .session
            .refresh_stash_metadata(id, external_id, "stash", true)
            .await;
    }
    rpc_ok(queued)
},
        "refresh_all_stale_metadata" => {
    let stale = state.session.stale_stash_content_ids().await.map_err(err)?;
    let queued = stale.len();
    for (id, external_id) in stale {
        state
            .session
            .refresh_stash_metadata(id, external_id, "stash", true)
            .await;
    }
    rpc_ok(queued)
},
        "dashboard_range_stats" => {
    let f = filter_from_args(
        rpc_arg(args, "startDay")?,
        rpc_arg(args, "endDay")?,
        rpc_arg(args, "sceneContentItemIds")?,
        rpc_arg(args, "performerIds")?,
        rpc_arg(args, "studioIds")?,
        rpc_arg(args, "tagIds")?,
    );
    rpc_ok(dashboard::range_stats(&state.pool, &f).await.map_err(err)?)
},
        "dashboard_filtered_session_count" => {
    let f = filter_from_args(
        rpc_arg(args, "startDay")?,
        rpc_arg(args, "endDay")?,
        rpc_arg(args, "sceneContentItemIds")?,
        rpc_arg(args, "performerIds")?,
        rpc_arg(args, "studioIds")?,
        rpc_arg(args, "tagIds")?,
    );
    rpc_ok(dashboard::filtered_session_count(&state.pool, &f).await.map_err(err)?)
},
        "catalog_list_studios" => rpc_ok(dashboard::list_studios(&state.pool).await.map_err(err)?),
        "catalog_list_tags" => rpc_ok(dashboard::list_tags(&state.pool).await.map_err(err)?),
        "catalog_search_scenes" => {
            let query: String = rpc_arg(args, "query")?;
            let limit: Option<i64> = rpc_arg(args, "limit")?;
            rpc_ok(dashboard::search_scenes(&state.pool, &query, limit.unwrap_or(50)).await.map_err(err)?)
        },
        "catalog_browse_scenes" => rpc_ok(catalog::list_scenes(&state.pool).await.map_err(err)?),
        "catalog_browse_performers" => rpc_ok(catalog::list_performers(&state.pool).await.map_err(err)?),
        "catalog_browse_studios" => rpc_ok(catalog::list_studios(&state.pool).await.map_err(err)?),
        "catalog_browse_tags" => rpc_ok(catalog::list_tags(&state.pool).await.map_err(err)?),
        "catalog_enrich_entities" => {
            let kind: String = rpc_arg(args, "kind")?;
            let pool = state.pool.clone();
            tokio::spawn(async move {
                if let Err(e) = catalog::enrich_entities(&pool, &kind, false).await {
                    tracing::warn!("enrich_entities({}) failed: {:#}", kind, e);
                }
            });
            rpc_ok(())
        },
        "dashboard_first_session_day" => rpc_ok(dashboard::first_session_day(&state.pool).await.map_err(err)?),
        "default_date_preset_get" => {
            let scope: String = rpc_arg(args, "scope")?;
            rpc_ok(settings::get_default_date_preset(&state.pool, &scope).await.map_err(err)?)
        },
        "default_date_preset_set" => {
            let scope: String = rpc_arg(args, "scope")?;
            let preset: String = rpc_arg(args, "preset")?;
            settings::set_default_date_preset(&state.pool, &scope, &preset).await.map_err(err)?;
            rpc_ok(preset)
        },
        "default_chart_metric_get" => {
            let scope: String = rpc_arg(args, "scope")?;
            rpc_ok(settings::get_default_chart_metric(&state.pool, &scope).await.map_err(err)?)
        },
        "default_chart_metric_set" => {
            let scope: String = rpc_arg(args, "scope")?;
            let metric: String = rpc_arg(args, "metric")?;
            settings::set_default_chart_metric(&state.pool, &scope, &metric).await.map_err(err)?;
            rpc_ok(metric)
        },
        "browse_default_get" => {
            let kind: String = rpc_arg(args, "kind")?;
            rpc_ok(settings::get_browse_default(&state.pool, &kind).await.map_err(err)?)
        },
        "browse_default_set" => {
            let kind: String = rpc_arg(args, "kind")?;
            let view: String = rpc_arg(args, "view")?;
            rpc_ok(settings::set_browse_default(&state.pool, &kind, &view).await.map_err(err)?)
        },
        "default_performer_gender_get" => rpc_ok(settings::get_default_performer_gender(&state.pool).await.map_err(err)?),
        "default_performer_gender_set" => {
            let gender: String = rpc_arg(args, "gender")?;
            settings::set_default_performer_gender(&state.pool, &gender).await.map_err(err)?;
            rpc_ok(gender)
        },
        "trends_daily_series" => {
            let start_day: String = rpc_arg(args, "startDay")?;
            let end_day: String = rpc_arg(args, "endDay")?;
            rpc_ok(crate::trends::daily_series(&state.pool, &start_day, &end_day).await.map_err(err)?)
        },
        "trends_hour_histogram" => {
            let start_day: String = rpc_arg(args, "startDay")?;
            let end_day: String = rpc_arg(args, "endDay")?;
            rpc_ok(crate::trends::hour_histogram(&state.pool, &start_day, &end_day).await.map_err(err)?)
        },
        "trends_entity_breakdown" => {
            let dimension: String = rpc_arg(args, "dimension")?;
            let start_day: String = rpc_arg(args, "startDay")?;
            let end_day: String = rpc_arg(args, "endDay")?;
            rpc_ok(crate::trends::entity_breakdown(&state.pool, &dimension, &start_day, &end_day).await.map_err(err)?)
        },
        "mirror_sync_get" => rpc_ok(settings::get_mirror_sync(&state.pool).await.map_err(err)?),
        "mirror_sync_set" => {
    let enabled: bool = rpc_arg(args, "enabled")?;
    let cadence_minutes: u32 = rpc_arg(args, "cadenceMinutes")?;
    let value = settings::MirrorSyncSetting { enabled, cadence_minutes };
    settings::set_mirror_sync(&state.pool, &value).await.map_err(err)?;
    rpc_ok(value)
},
        "mirror_status_get" => rpc_ok(state.mirror_status.read().await.clone()),
        "mirror_sync_now" => {
    let pool = state.pool.clone();
    let status = state.mirror_status.clone();
    tokio::spawn(async move {
        if let Err(e) = mirror::sync_active(&pool, &status).await {
            tracing::warn!("mirror sync_active failed: {:#}", e);
        }
    });
    rpc_ok(())
},
        "mirror_check_scene" => {
    let scene_id: String = rpc_arg(args, "sceneId")?;
    rpc_ok(mirror::check_scene(&state.pool, &scene_id).await.map_err(err)?)
},
        "reconstruct_candidates" => {
    let start_day: String = rpc_arg(args, "startDay")?;
    let end_day: String = rpc_arg(args, "endDay")?;
    let gap_minutes: Option<u32> = rpc_arg(args, "gapMinutes")?;
    let respect_dismissed: Option<bool> = rpc_arg(args, "respectDismissed")?;
    let gap = match gap_minutes {
        Some(g) => g,
        None => settings::get_session_gap(&state.pool).await.map_err(err)?.gap_minutes,
    };
    rpc_ok(
        crate::reconstruct::candidate_sessions(
            &state.pool,
            gap,
            &start_day,
            &end_day,
            respect_dismissed.unwrap_or(true),
        )
        .await
        .map_err(err)?,
    )
},
        "reconstruct_accept" => {
    let start_ms: i64 = rpc_arg(args, "startMs")?;
    let end_ms: i64 = rpc_arg(args, "endMs")?;
    let assigned_day: String = rpc_arg(args, "assignedDay")?;
    let o_event_ids: Vec<i64> = rpc_arg(args, "oEventIds")?;
    let scenes: Vec<crate::reconstruct::AcceptScene> = rpc_arg(args, "scenes")?;
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
    rpc_ok(sid)
},
        "reconstruct_dismiss" => {
    let play_import_ids: Vec<i64> = rpc_arg(args, "playImportIds")?;
    let o_event_ids: Vec<i64> = rpc_arg(args, "oEventIds")?;
    rpc_ok(crate::reconstruct::dismiss_candidates(&state.pool, &play_import_ids, &o_event_ids).await.map_err(err)?)
},
        "reconstruct_away_candidates" => {
    let gap = settings::get_session_gap(&state.pool).await.map_err(err)?.gap_minutes;
    rpc_ok(crate::reconstruct::away_candidates(&state.pool, gap).await.map_err(err)?)
},
        "session_gap_get" => rpc_ok(settings::get_session_gap(&state.pool).await.map_err(err)?),
        "session_gap_set" => {
    let gap_minutes: u32 = rpc_arg(args, "gapMinutes")?;
    let value = settings::SessionGapSetting { gap_minutes };
    settings::set_session_gap(&state.pool, &value).await.map_err(err)?;
    rpc_ok(value)
},
        "session_pending_history" => {
    let session_id: i64 = rpc_arg(args, "sessionId")?;
    let include_active: bool = rpc_arg(args, "includeActive")?;
    let include_dismissed: Option<bool> = rpc_arg(args, "includeDismissed")?;
    rpc_ok(crate::reconstruct::session_pending_history(&state.pool, session_id, include_active, include_dismissed.unwrap_or(false)).await.map_err(err)?)
},
        "session_absorb_history" => {
    let session_id: i64 = rpc_arg(args, "sessionId")?;
    let content_item_ids: Vec<i64> = rpc_arg(args, "contentItemIds")?;
    let o_event_ids: Vec<i64> = rpc_arg(args, "oEventIds")?;
    crate::reconstruct::absorb_session_history(&state.pool, session_id, &content_item_ids, &o_event_ids).await.map_err(err)?;
    rpc_ok(())
},
        "session_dismiss_history" => {
    let session_id: i64 = rpc_arg(args, "sessionId")?;
    let content_item_ids: Vec<i64> = rpc_arg(args, "contentItemIds")?;
    let o_event_ids: Vec<i64> = rpc_arg(args, "oEventIds")?;
    let mirror_to_reconstruction: Option<bool> = rpc_arg(args, "mirrorToReconstruction")?;
    crate::reconstruct::dismiss_session_history(&state.pool, session_id, &content_item_ids, &o_event_ids, mirror_to_reconstruction.unwrap_or(false)).await.map_err(err)?;
    rpc_ok(())
},
        "session_merge_check" => {
    let a_id: i64 = rpc_arg(args, "aId")?;
    let b_id: i64 = rpc_arg(args, "bId")?;
    rpc_ok(state.session.merge_check(a_id, b_id).await.map_err(err)?)
},
        "session_merge" => {
    let a_id: i64 = rpc_arg(args, "aId")?;
    let b_id: i64 = rpc_arg(args, "bId")?;
    let gap_as_pause: bool = rpc_arg(args, "gapAsPause")?;
    rpc_ok(state.session.merge_sessions(a_id, b_id, gap_as_pause).await.map_err(err)?)
},
        "session_reopen_check" => {
    let session_id: i64 = rpc_arg(args, "sessionId")?;
    rpc_ok(state.session.reopen_check(session_id).await.map_err(err)?)
},
        "session_reopen" => {
    let session_id: i64 = rpc_arg(args, "sessionId")?;
    let gap_as_pause: bool = rpc_arg(args, "gapAsPause")?;
    rpc_ok(state.session.reopen(session_id, gap_as_pause).await.map_err(err)?)
},
        "session_extend_check" => {
    let session_id: i64 = rpc_arg(args, "sessionId")?;
    rpc_ok(crate::reconstruct::extend_session_check(&state.pool, session_id).await.map_err(err)?)
},
        "session_extend" => {
    let session_id: i64 = rpc_arg(args, "sessionId")?;
    let gap_as_pause: bool = rpc_arg(args, "gapAsPause")?;
    rpc_ok(crate::reconstruct::extend_session(&state.pool, session_id, gap_as_pause).await.map_err(err)?)
},
        "quit_pending_get" => rpc_ok(*state.pending_quit.read().await),
        "crash_pending_get" => rpc_ok(state.pending_crash_recovery.read().await.clone()),
        "launch_behavior_get" => rpc_ok(settings::get_launch_behavior(&state.pool).await.map_err(err)?),
        "launch_behavior_set" => {
    let mode: String = rpc_arg(args, "mode")?;
    rpc_ok(settings::set_launch_behavior(&state.pool, &mode).await.map_err(err)?)
},
        "hotkey_enabled_get" => rpc_ok(settings::get_hotkey_enabled(&state.pool).await.map_err(err)?),
        "wording_get" => rpc_ok(settings::get_wording(&state.pool).await.map_err(err)?),
        "wording_set" => {
    let value: String = rpc_arg(args, "value")?;
    rpc_ok(settings::set_wording(&state.pool, &value).await.map_err(err)?)
}
        "keep_empty_sessions_get" => rpc_ok(settings::get_keep_empty_sessions(&state.pool).await.map_err(err)?),
        "keep_empty_sessions_set" => {
    let enabled: bool = rpc_arg(args, "enabled")?;
    settings::set_keep_empty_sessions(&state.pool, enabled).await.map_err(err)?;
    rpc_ok(enabled)
}
        "backup_settings_get" => rpc_ok(settings::get_backup(&state.pool).await.map_err(err)?),
        "backup_data_folder" => rpc_ok(state.db_dir.to_string_lossy().to_string()),
        "onboarding_needed" => {
    if !state.stash_enabled {
        rpc_ok(false)
    } else {
        let done = settings::get_onboarding_completed(&state.pool).await.map_err(err)?;
        rpc_ok(!done)
    }
},
        "onboarding_complete" => rpc_ok(settings::set_onboarding_completed(&state.pool, true).await.map_err(err)?),
        "onboarding_reset" => rpc_ok(settings::set_onboarding_completed(&state.pool, false).await.map_err(err)?),
        "onboarding_estimate_sessions" => {
    let gap = settings::get_session_gap(&state.pool).await.map_err(err)?.gap_minutes;
    rpc_ok(crate::reconstruct::accept_all_candidates(&state.pool, gap).await.map_err(err)?)
},
        other => Err(format!(
            "The connected Climax server doesn't support '{}' yet. Update the server.",
            other
        )),
    }
}
