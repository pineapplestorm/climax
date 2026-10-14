// Climax — personal activity tracker for adult content viewing.
//
// Tauri 2 entry point. Wires up:
//   - SQLite pool (sqlx) with auto-migration
//   - Session manager (in-memory transitions + DB persistence)
//   - Local HTTP server on 127.0.0.1 (heartbeat endpoint for the Stash bridge
//     plugin); the port is the `http_port` setting, default 9998
//   - System tray with live timer + menu (Start/Stop/Pause/Open/Quit)
//   - Global hotkey (Ctrl+Shift+L by default) to toggle session start/stop
//   - Tauri commands invokable from the Svelte frontend

// Shell / desktop-client modules (Tauri + OS-native) stay in this crate.
mod client_config;
mod commands;
mod hotkey;
mod presence;
mod remote_session;
mod shutdown_guard;
mod tray;

// Core engine modules now live in the `climax-core` crate (Phase 2a/2b). Re-export
// them at the crate root so every existing `crate::db::...` / `crate::state::...`
// / `crate::server::...` / `crate::rpc::...` path across the shell modules keeps
// resolving unchanged. `rpc` + `server` joined core in Phase 2b (the headless
// server now lives entirely in the core crate; this shell just drives it).
pub use climax_core::{
    capture, catalog, dashboard, db, mirror, models, reconstruct, remote, rpc, server, session,
    settings, stash, state, trends, update,
};

use tauri::{Emitter, Manager, RunEvent};

use crate::state::AppState;

pub(crate) const DEFAULT_HOTKEY: &str = "CommandOrControl+Shift+L";

/// How the in-process server's bind went, recorded once at boot.
///
/// A failed bind used to be logged and shrugged off: the app opened, looked
/// completely normal, and the only symptom was that the bridge never connected -
/// with nothing on screen to explain why. The port is configurable now, so the
/// user CAN fix it, but only if they're told. Settings and the two windows read
/// this to say so.
///
/// Stays unset in client mode, where an external server owns the port and this
/// app binds nothing at all.
pub(crate) struct BindOutcome {
    pub port: u16,
    /// A designed sentence when the port couldn't be bound; None = serving.
    pub error: Option<String>,
}

pub(crate) static SERVER_BIND: std::sync::OnceLock<BindOutcome> = std::sync::OnceLock::new();
/// On boot, a session left running from a previous run is silently resumed if
/// the gap since its last heartbeat is under this; beyond it, the tracker shows
/// the crash-recovery prompt (continue / discard-and-continue / discard-and-end).
const CRASH_RECOVERY_THRESHOLD_MS: i64 = 10 * 60 * 1000;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("climax=info,climax_core=info,axum=info,sqlx=warn")),
        )
        .init();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_store::Builder::default().build())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new().build(),
        )
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .invoke_handler(tauri::generate_handler![
            commands::session_start,
            commands::session_stop,
            commands::session_discard,
            commands::session_pause,
            commands::session_resume,
            commands::session_active,
            commands::session_get,
            commands::sessions_list,
            commands::sessions_page,
            commands::sessions_in_range,
            commands::sessions_for_day,
            commands::session_set_assigned_day,
            commands::session_scenes,
            commands::o_log,
            commands::o_list_for_session,
            commands::o_delete,
            commands::o_delete_recent_for_scene,
            commands::o_event_update,
            commands::o_sessionless_unlinked_today,
            commands::session_delete,
            commands::scene_delete_from_session,
            commands::scene_detach_from_session,
            commands::session_delete_many,
            commands::session_update,
            commands::open_dashboard,
            commands::open_tracker,
            commands::idle_detection_get,
            commands::idle_detection_set,
            commands::wording_get,
            commands::wording_set,
            commands::keep_empty_sessions_get,
            commands::keep_empty_sessions_set,
            commands::remote_tracking_get,
            commands::remote_tracking_set,
            commands::capture_prompt_get,
            commands::capture_prompt_set,
            commands::capture_pending_get,
            commands::capture_accept,
            commands::capture_snooze,
            commands::play_counting_get,
            commands::play_counting_set,
            commands::play_counting_sync_from_stash,
            commands::idle_pending_get,
            commands::idle_keep,
            commands::idle_discard_continue,
            commands::idle_discard_end,
            commands::dashboard_hero_stats,
            commands::dashboard_monthly_buckets,
            commands::dashboard_daily_buckets,
            commands::stash_connection_get,
            commands::stash_connection_set,
            commands::stash_connection_test,
            commands::bridge_status,
            commands::bridge_install,
            commands::metadata_refresh_get,
            commands::metadata_refresh_set,
            commands::metadata_status_get,
            commands::metadata_sync_now,
            commands::refresh_scene_metadata,
            commands::refresh_all_stash_metadata,
            commands::refresh_all_stale_metadata,
            commands::dashboard_range_stats,
            commands::dashboard_filtered_buckets,
            commands::dashboard_filtered_session_count,
            commands::catalog_list_performers,
            commands::catalog_list_studios,
            commands::catalog_list_tags,
            commands::catalog_search_scenes,
            commands::catalog_browse_scenes,
            commands::catalog_browse_performers,
            commands::catalog_browse_studios,
            commands::catalog_browse_tags,
            commands::catalog_enrich_entities,
            commands::dashboard_first_session_day,
            commands::default_date_preset_get,
            commands::default_date_preset_set,
            commands::default_chart_metric_get,
            commands::default_chart_metric_set,
            commands::browse_default_get,
            commands::browse_default_set,
            commands::default_performer_gender_get,
            commands::default_performer_gender_set,
            commands::trends_daily_series,
            commands::trends_hour_histogram,
            commands::trends_entity_breakdown,
            commands::trends_export_csv,
            commands::mirror_sync_get,
            commands::mirror_sync_set,
            commands::mirror_status_get,
            commands::mirror_sync_now,
            commands::mirror_check_scene,
            commands::reconstruct_candidates,
            commands::reconstruct_accept,
            commands::reconstruct_dismiss,
            commands::reconstruct_away_candidates,
            commands::session_gap_get,
            commands::session_gap_set,
            commands::session_pending_history,
            commands::session_absorb_history,
            commands::session_dismiss_history,
            commands::session_merge_check,
            commands::session_merge,
            commands::session_extend_check,
            commands::session_extend,
            commands::session_reopen_check,
            commands::session_reopen,
            commands::quit_pending_get,
            commands::confirm_quit,
            commands::cancel_quit,
            commands::crash_pending_get,
            commands::crash_keep,
            commands::crash_discard_continue,
            commands::crash_discard_end,
            commands::launch_behavior_get,
            commands::launch_behavior_set,
            commands::hotkey_enabled_get,
            commands::hotkey_enabled_set,
            commands::autostart_get,
            commands::autostart_set,
            commands::backup_settings_get,
            commands::backup_data_folder,
            commands::backup_now,
            commands::backup_restore_pick,
            commands::backup_restore_apply,
            commands::backup_reset,
            commands::profile_get,
            commands::onboarding_needed,
            commands::onboarding_complete,
            commands::onboarding_reset,
            commands::onboarding_estimate_sessions,
            commands::rpc_http,
            commands::server_ping,
            commands::client_config_get,
            commands::client_config_set,
            commands::server_port_get,
            commands::server_port_set,
            commands::update_check,
            commands::update_check_now,
            commands::update_dismiss,
            commands::app_version_get,
            commands::restart_app,
        ])
        .setup(|app| {
            // Resolve database directory: appdata/Roaming/com.climax.app/
            let db_dir = app
                .path()
                .app_data_dir()
                .expect("app data dir");

            // Launch profile: real (default) uses climax.sqlite + full Stash;
            // scratch/test uses a SEPARATE disposable DB with Stash fully OFF, so
            // test sessions never touch the real library. Chosen via the
            // CLIMAX_PROFILE env var (set by launch-climax-test.vbs). One port, so
            // only one profile runs at a time.
            let profile = std::env::var("CLIMAX_PROFILE").unwrap_or_default().to_lowercase();
            let is_scratch = matches!(profile.as_str(), "test" | "scratch");

            // Client mode (Phase 2c): if a server is configured, this app is a THIN
            // CLIENT - the external server owns the DB / session / Stash sync / the
            // UiSignal source. So skip spawning the in-process backend (HTTP server,
            // engine loops, the reactor) below, and run NO app-side Stash. We still
            // build an AppState so the native (window / prompt-slot / config) commands
            // resolve, but against a THROWAWAY local cache DB - it's barely touched
            // (all real data flows to the server via the frontend transport; events
            // arrive over /events). The native session-features (tray timer / hotkey /
            // idle / shutdown-guard) ARE server-aware now: they read + drive the
            // session through `remote_session`, which routes to the server's /rpc in
            // client mode (so they reflect + control the SERVER's session, not the
            // empty cache). AppState carries the server URL so they know which to use.
            let client_server = client_config::read_server_url(&db_dir);
            // The server's shared token (Phase 3), if it requires one - rides on
            // every Rust-side proxy call (remote_session) as X-Climax-Token.
            let client_token = client_config::read_token(&db_dir);
            let is_client = client_server.is_some();
            let stash_enabled = !is_scratch && !is_client;
            let db_filename = if is_client {
                "client-cache.sqlite"
            } else if is_scratch {
                "climax-test.sqlite"
            } else {
                "climax.sqlite"
            }
            .to_string();
            stash::set_enabled(stash_enabled);
            if is_scratch {
                tracing::warn!(
                    "CLIMAX_PROFILE={:?} -> SCRATCH profile: db={} stash=OFF (disposable test sandbox)",
                    profile, db_filename
                );
            }
            if let Some(url) = &client_server {
                tracing::info!(
                    "CLIENT mode: external server {} -> skipping the in-process backend \
                     (thin client; native session-features inert until server-aware)",
                    url
                );
            }

            let handle = app.handle().clone();
            let db_dir_for_state = db_dir.clone();
            let db_filename_boot = db_filename.clone();
            // Moved into AppState so the native loops know whether (and where) to
            // reach an external server vs the local in-process session manager.
            let client_server_for_state = client_server.clone();
            let client_token_for_state = client_token.clone();
            let client_server_boot = client_server.clone();
            let client_token_boot = client_token.clone();
            // Read before the block below moves `app`. This is the tauri.conf.json
            // version - the one that brands the installer - not a Cargo one.
            let app_version = app.package_info().version.to_string();
            let (crash_pending, launch_behavior, hotkey_enabled, onboarding_pending) = tauri::async_runtime::block_on(async move {
                // If a restore was staged last session, swap it into place BEFORE
                // the pool opens (validates the staged file, replaces the live DB
                // only once the replacement is in place, clears stale WAL/SHM). The
                // bool reports whether a restore actually happened this boot.
                // A staged full reset wipes the DB before the pool opens, so
                // init_pool recreates a pristine, migrated DB (onboarding re-arms).
                // Runs before the restore swap; stage_wipe drops any pending
                // restore, so the two never fight.
                if let Err(e) = db::apply_pending_wipe(&db_dir, &db_filename_boot).await {
                    tracing::error!("apply pending wipe failed: {:#}", e);
                }
                let restored = match db::apply_pending_restore(&db_dir, &db_filename_boot).await {
                    Ok(b) => b,
                    Err(e) => {
                        tracing::error!("apply pending restore failed: {:#}", e);
                        false
                    }
                };
                let pool = db::init_pool(db_dir, &db_filename_boot).await.expect("db init");
                // Broadcast channel for pushing session-state to WS clients
                // (the Stash bridge's navbar indicator). Drop the initial
                // receiver; the WS handlers subscribe as they connect.
                let (ws_tx, _ws_rx) = tokio::sync::broadcast::channel::<String>(64);
                // UI-signal channel: the server core can't show windows / run the
                // wrap-up flow itself, so it broadcasts UiSignals here and the
                // client reactor task below (which holds the AppHandle) acts.
                let (ui_tx, _ui_rx) = tokio::sync::broadcast::channel::<models::UiSignal>(64);
                let state = AppState::new(
                    pool.clone(),
                    ws_tx,
                    ui_tx.clone(),
                    db_dir_for_state,
                    db_filename_boot,
                    stash_enabled,
                    client_server_for_state,
                    client_token_for_state,
                    app_version,
                );

                // Client-side reactor for UiSignals. Decouples server.rs (which no
                // longer holds an AppHandle) from the native window/wrap-up actions:
                // it broadcasts, we perform them here where the AppHandle lives. In
                // CLIENT mode the external server's UiSignals instead arrive over
                // /events (server-events.ts re-emits them), so skip the in-process
                // reactor - it would have nothing to react to (and double-fire if the
                // app were pointed at its own server).
                if !is_client {
                    let app_for_ui = handle.clone();
                    let mut ui_rx = ui_tx.subscribe();
                    tauri::async_runtime::spawn(async move {
                        loop {
                            match ui_rx.recv().await {
                                Ok(models::UiSignal::ShowTracker) => {
                                    commands::show_solo(&app_for_ui, "tracker");
                                }
                                Ok(models::UiSignal::ShowDashboard) => {
                                    commands::show_solo(&app_for_ui, "dashboard");
                                }
                                Ok(models::UiSignal::TrackerStopRequested) => {
                                    commands::show_solo(&app_for_ui, "tracker");
                                    let _ = app_for_ui.emit("tracker_stop_requested", ());
                                }
                                Ok(models::UiSignal::CapturePrompt(payload)) => {
                                    // Passive-capture toast: emit the payload + raise
                                    // the tracker WITHOUT stealing focus from playback.
                                    let _ = app_for_ui.emit("capture_prompt", &payload);
                                    if let Some(win) = app_for_ui.get_webview_window("tracker") {
                                        let _ = win.set_always_on_top(true);
                                        let _ = win.show();
                                        let _ = win.unminimize();
                                    }
                                }
                                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                            }
                        }
                    });
                }

                // Crash recovery: a session left 'active'/'paused' from a
                // previous run that never wrapped up (crash, force-kill, power
                // loss, OS shutdown). If the gap since its last heartbeat exceeds
                // the threshold, resume it to 'active' and stash a payload so the
                // tracker prompts on mount (same flow as idle return). Under the
                // threshold it just resumes silently — a quick relaunch.
                let crash_pending = {
                    let now = db::now_ms();
                    match sqlx::query_as::<_, (i64, Option<i64>, i64)>(
                        "SELECT id, last_heartbeat, started_at FROM sessions \
                         WHERE status IN ('active','paused') ORDER BY id DESC LIMIT 1",
                    )
                    .fetch_optional(&pool)
                    .await
                    {
                        Ok(Some((sid, last_hb, started))) => {
                            let gap_start = last_hb.unwrap_or(started);
                            if now - gap_start > CRASH_RECOVERY_THRESHOLD_MS {
                                if let Err(e) =
                                    state.session.resume_for_crash(sid, gap_start).await
                                {
                                    tracing::warn!(
                                        "crash recovery: resume_for_crash({}) failed: {:#}",
                                        sid, e
                                    );
                                }
                                if restored {
                                    // Fresh restore, not a crash: the leftover came
                                    // from the restored snapshot. It's already
                                    // resumed silently above; don't fire the
                                    // misleading crash prompt.
                                    tracing::info!(
                                        "restore boot: session {} resumed silently (no crash prompt)",
                                        sid
                                    );
                                    false
                                } else {
                                    *state.pending_crash_recovery.write().await =
                                        Some(crate::models::CrashPayload {
                                            session_id: sid,
                                            gap_started_at: gap_start,
                                        });
                                    tracing::info!(
                                        "crash recovery: session {} left running, gap {}min - prompting",
                                        sid,
                                        (now - gap_start) / 60_000
                                    );
                                    true
                                }
                            } else {
                                tracing::info!(
                                    "crash recovery: session {} resuming silently (gap {}s)",
                                    sid,
                                    (now - gap_start) / 1000
                                );
                                false
                            }
                        }
                        Ok(None) => false,
                        Err(e) => {
                            tracing::warn!("crash recovery: leftover check failed: {:#}", e);
                            false
                        }
                    }
                };

                // Startup prefs, read once so the window + hotkey wiring below
                // (which runs after this block) can apply them.
                // In CLIENT mode these live on the server (the Settings UI writes
                // them over /rpc); the local DB here is the throwaway client
                // cache and never has them, so reading it always fell back to
                // the default and the launch choice was silently ignored.
                let (launch_behavior, hotkey_enabled) = match client_server_boot.as_deref() {
                    Some(base) => {
                        let (lb, hk) = remote_session::boot_startup_prefs(
                            base,
                            client_token_boot.as_deref(),
                        )
                        .await;
                        (
                            lb.unwrap_or_else(|| "tracker".to_string()),
                            hk.unwrap_or(true),
                        )
                    }
                    None => (
                        settings::get_launch_behavior(&pool)
                            .await
                            .unwrap_or_else(|_| "tracker".to_string()),
                        settings::get_hotkey_enabled(&pool).await.unwrap_or(true),
                    ),
                };
                // First-launch onboarding: a fresh install (real profile) shows the
                // setup wizard (forces the dashboard window below so it's seen).
                // Grandfather EXISTING installs that predate the flag: if it's unset
                // but the DB already has sessions, this is not a fresh install, so
                // mark onboarding done and skip the wizard. A truly fresh DB has no
                // sessions yet (the launch mirror sync runs later), so it still shows.
                let onboarding_pending = if !stash_enabled
                    || settings::get_onboarding_completed(&pool).await.unwrap_or(false)
                {
                    false
                } else {
                    let has_sessions: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions")
                        .fetch_one(&pool)
                        .await
                        .unwrap_or(0);
                    if has_sessions > 0 {
                        let _ = settings::set_onboarding_completed(&pool, true).await;
                        false
                    } else {
                        true
                    }
                };

                // Spawn the in-process HTTP server - UNLESS this is a thin client,
                // where the external server owns the port (and the DB). Skipping it is
                // what frees the port for the standalone server on a single PC.
                // `SpawnOpts::local`: localhost bind, no token, no extra routes -
                // the desktop's server deliberately never grows the standalone
                // server's web-client surface (embedded SPA, GET /backup) or its
                // LAN exposure; those live in climax-server only.
                if !is_client {
                    // Read once, here: the port is only consulted at bind time, so
                    // changing it in Settings takes effect on the next launch.
                    let port = settings::get_http_port(&pool)
                        .await
                        .unwrap_or(settings::DEFAULT_HTTP_PORT);
                    // `server::spawn` binds the listener before it returns, so an
                    // Err here really does mean "this port is unusable" - not a
                    // failure that might still resolve in the background.
                    let error = match server::spawn(
                        state.clone(),
                        server::SpawnOpts::local(port),
                    )
                    .await
                    {
                        Ok(()) => None,
                        Err(e) => {
                            tracing::error!("server spawn failed on port {}: {:#}", port, e);
                            Some(format!(
                                "Climax couldn't start on port {}. Another program may be \
                                 using it, or your system may have reserved it. Choose a \
                                 different port in Settings.",
                                port
                            ))
                        }
                    };
                    let _ = SERVER_BIND.set(BindOutcome { port, error });

                    // Look for a new release, at most once a day. Backgrounded
                    // and failure-tolerant: it must never delay a launch, and a
                    // machine with no network is an ordinary case, not a fault.
                    // Skipped in client mode, where the server this app talks to
                    // keeps the cache and answers `update_check` from it.
                    let update_pool = pool.clone();
                    tokio::spawn(async move {
                        update::check_if_stale(&update_pool).await;
                    });

                    // Keep a running session's "last seen alive" time current, so
                    // the next launch's crash recovery (above) measures the real
                    // gap instead of the time since a scene last played. Host mode
                    // only: in client mode the server owns the session and runs
                    // this itself.
                    crate::session::SessionManager::spawn_liveness(pool.clone());
                }

                // Idle / AFK detection. Polls every 30s while a session is
                // active. The moment the idle threshold is crossed, presence
                // shows the floating idle-prompt window and stashes the
                // payload on AppState for it to read. Runs in BOTH modes now: in
                // client mode it reads the active session + the idle setting from
                // the SERVER (via `remote_session`), and its resolve commands
                // route the gap-pause / end back to the server.
                presence::spawn(handle.clone(), state.clone());

                // Live remote-device playback detection (Phase 7). While a
                // session is active, polls Stash's per-scene play_duration and
                // credits scenes being watched on OTHER devices (e.g. the
                // Android TV app) to the session in real time — the bridge can
                // only see local web-player playback. Gated on the
                // remote_tracking setting + an active session; idle otherwise.
                // Stash-polling, so skipped entirely in the scratch profile.
                if stash_enabled {
                    remote::spawn(state.clone());
                }

                // Passive-capture prompt (Phase 7). While no session is running,
                // watches the bridge's playback; once a scene is watched past
                // the user's threshold it nudges (bottom-right toast in the
                // tracker) to start a backdated session. Gated on the
                // capture_prompt setting; off by default. Also Stash-polling, so
                // skipped entirely in the scratch profile.
                if stash_enabled {
                    capture::spawn(state.clone());
                }

                // Block / prompt on Windows shutdown while a session is active
                // (unsaved-changes pattern) so an in-progress session isn't
                // silently killed. Windows-only; no-op elsewhere. Runs in both
                // modes, but only GUARDS when the server is LOCAL: in client mode
                // it reads the server's session and registers the block only for a
                // localhost server (shutting this PC down would kill it); a remote
                // (NAS/LAN) server is left unguarded - the session is safe there
                // (see CLAUDE.md "SHUTDOWN GUARD semantics in the split").
                shutdown_guard::spawn(handle.clone(), state.clone());

                // First-launch sync of Stash's "minimum play percent" into
                // Climax's play_counting setting. Only runs when Climax has
                // never had a threshold configured (Option::None) — once set
                // (by this sync OR by the user editing it), it stays put.
                // Failures are best-effort: if Stash is unreachable, we keep
                // None and try again next launch. The threshold-check code
                // falls back to 0% (Stash's own default) until this succeeds.
                if stash_enabled {
                    let pool = pool.clone();
                    tauri::async_runtime::spawn(async move {
                        let current = match settings::get_play_counting(&pool).await {
                            Ok(v) => v,
                            Err(e) => {
                                tracing::warn!("read play_counting on boot failed: {:#}", e);
                                return;
                            }
                        };
                        if current.threshold_pct.is_some() {
                            return; // Already configured — don't clobber.
                        }
                        match stash::fetch_minimum_play_percent(&pool).await {
                            Ok(Some(pct)) => {
                                let _ = settings::set_play_counting(
                                    &pool,
                                    &settings::PlayCountingSetting { threshold_pct: Some(pct) },
                                ).await;
                                tracing::info!(
                                    "synced play_counting threshold from Stash: {}%", pct
                                );
                            }
                            Ok(None) => {
                                tracing::info!(
                                    "Stash did not return minimumPlayPercent (history off?); \
                                     leaving Climax threshold unset, using 0% fallback"
                                );
                            }
                            Err(e) => {
                                tracing::warn!(
                                    "could not fetch play threshold from Stash on boot \
                                     (will retry next launch, using 0% fallback): {:#}", e
                                );
                            }
                        }
                    });
                }

                // Self-heal Stash's play-history tracking on launch. Organising
                // mode (which pauses it) is in-memory only, so an unclean exit
                // mid-organise could leave Stash's `trackActivity` off with no
                // session running — violating the invariant (off is allowed only
                // while organising). At boot we're never organising, so flip it
                // back on if a previous run left it off. Best-effort.
                if stash_enabled {
                    let pool = pool.clone();
                    tauri::async_runtime::spawn(async move {
                        match stash::ensure_track_activity_on(&pool).await {
                            Ok(true) => tracing::info!(
                                "launch: re-enabled Stash play-history tracking \
                                 (a previous organising session had left it off)"
                            ),
                            Ok(false) => {}
                            Err(e) => tracing::warn!(
                                "launch: could not verify Stash trackActivity \
                                 (will heal on next session start): {:#}",
                                e
                            ),
                        }
                    });
                }

                handle.manage(state);
                (crash_pending, launch_behavior, hotkey_enabled, onboarding_pending)
            });

            // Build the tray + start the timer-update loop.
            let _tray = tray::build(app.handle()).expect("build tray");
            tray::spawn_timer_updater(app.handle().clone());

            // Note on session pause:
            // Session-level pause is ONLY triggered by explicit user action (the
            // Pause button or hotkey). Per-tab video state is still tracked for
            // per-scene watched-time accuracy (see record_heartbeat's video-time
            // delta logic), but the session timer always ticks from start to stop.
            // AFK / system-sleep auto-pause is a separate feature for later.
            //
            // We do periodically GC the per-tab cache so it doesn't leak memory
            // when tabs disconnect without notice.
            {
                let app_for_tick = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    loop {
                        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                        if let Some(state) = app_for_tick.try_state::<AppState>() {
                            state.session.prune_tab_cache().await;
                        }
                    }
                });
            }

            // Stash-history mirror (Phase 7). Runs sync_active ON LAUNCH (shortly
            // after boot) and then once per cadence while open, so Climax stays a
            // faithful mirror of Stash — DISCOVERING scenes watched/O'd while it
            // was closed (not just refreshing known ones) and updating cumshots /
            // play count / watch-duration together. Gated on the mirror_sync
            // setting; when disabled it just re-checks every few minutes.
            if stash_enabled {
                let app_for_mirror = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    // Brief settle delay so the launch sync doesn't compete with
                    // boot work.
                    tokio::time::sleep(std::time::Duration::from_secs(20)).await;
                    loop {
                        let mut sleep_secs = 300u64;
                        if let Some(state) = app_for_mirror.try_state::<AppState>() {
                            match settings::get_mirror_sync(&state.pool).await {
                                Ok(cfg) if cfg.enabled => {
                                    let status = state.mirror_status.clone();
                                    match mirror::sync_active(&state.pool, &status).await {
                                        Ok(n) => {
                                            tracing::info!("mirror: synced {} active scene(s)", n)
                                        }
                                        Err(e) => {
                                            tracing::warn!("mirror sync pass failed: {:#}", e)
                                        }
                                    }
                                    sleep_secs = (cfg.cadence_minutes as u64).max(1) * 60;
                                }
                                Ok(_) => {} // disabled — re-check shortly
                                Err(e) => tracing::warn!("read mirror_sync setting failed: {:#}", e),
                            }
                        }
                        tokio::time::sleep(std::time::Duration::from_secs(sleep_secs)).await;
                    }
                });
            }

            // Library-metadata sync (Phase 7): scene + entity DETAILS (titles,
            // thumbnails, performers, studios, tags, images). Runs on launch and
            // on its own (longer) cadence, refreshing only STALE items since
            // details change rarely. Separate from the play-history loop above.
            if stash_enabled {
                let app_for_meta = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                    loop {
                        let mut sleep_secs = 3600u64;
                        if let Some(state) = app_for_meta.try_state::<AppState>() {
                            match settings::get_metadata_refresh(&state.pool).await {
                                Ok(cfg) if cfg.enabled => {
                                    let status = state.metadata_status.clone();
                                    match mirror::sync_metadata(&state.pool, &status, false).await {
                                        Ok(n) => tracing::info!(
                                            "metadata: refreshed {} stale scene(s) + entities",
                                            n
                                        ),
                                        Err(e) => {
                                            tracing::warn!("metadata sync pass failed: {:#}", e)
                                        }
                                    }
                                    sleep_secs = (cfg.cadence_minutes as u64).max(1) * 60;
                                }
                                Ok(_) => {} // disabled - re-check shortly
                                Err(e) => {
                                    tracing::warn!("read metadata_sync setting failed: {:#}", e)
                                }
                            }
                        }
                        tokio::time::sleep(std::time::Duration::from_secs(sleep_secs)).await;
                    }
                });
            }

            // Register global hotkey, unless the user disabled it in settings.
            if hotkey_enabled {
                if let Err(e) = hotkey::register(app.handle(), DEFAULT_HOTKEY) {
                    tracing::warn!("could not register hotkey {}: {:#}", DEFAULT_HOTKEY, e);
                }
            } else {
                tracing::info!("global hotkey disabled in settings; not registering");
            }

            // Both windows start hidden; user shows them explicitly via tray
            // or hotkey. presence.rs also shows the tracker when an idle
            // prompt fires.
            // Launch behavior: silent (tray only) / tracker / dashboard. A
            // pending crash-recovery prompt forces the tracker so it's seen.
            let target: &str = if crash_pending {
                "tracker"
            } else if onboarding_pending {
                // Fresh install: force the dashboard so the setup wizard shows,
                // regardless of the saved launch behavior.
                "dashboard"
            } else {
                match launch_behavior.as_str() {
                    "tracker" => "tracker",
                    "dashboard" => "dashboard",
                    _ => "", // silent — both windows stay hidden
                }
            };
            for label in ["tracker", "dashboard"] {
                if let Some(window) = app.get_webview_window(label) {
                    let _ = window.hide();
                }
            }
            if !target.is_empty() {
                if let Some(win) = app.get_webview_window(target) {
                    if crash_pending {
                        let _ = win.set_always_on_top(true);
                    }
                    let _ = win.show();
                    let _ = win.unminimize();
                    let _ = win.set_focus();
                }
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing any window hides it (keeps the tray + backend running).
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|_app, event| {
            if let RunEvent::ExitRequested { .. } = event {
                // Allow exit when triggered (only via tray Quit).
            }
        });
}
