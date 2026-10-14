//! Climax headless server (Phase 2b of the client-server pivot).
//!
//! The standalone "home": it owns the SQLite DB, runs the engine + Stash
//! sync/poll loops, and exposes the SAME HTTP `/rpc` + bridge `/ws` surface the
//! desktop app's in-process backend does - but with NO Tauri, NO WebView, NO
//! windows. A browser client, the Stash bridge, and (later) a thin desktop
//! client all talk to it over the network.
//!
//! It deliberately does NOT do the desktop-only things (tray, global hotkey,
//! idle/AFK detection, the crash-recovery / capture toast prompts, the
//! shutdown guard): those assume a human at the machine and relocate to the
//! desktop CLIENT in phase 4. The server broadcasts `UiSignal`s (e.g. a bridge
//! pill "open tracker" click) on `AppState.ui_tx`, but with no client
//! subscribed they are harmless no-ops until a desktop client connects.
//!
//! Env knobs:
//!   - CLIMAX_PROFILE = real (default) | test/scratch  -> which DB + Stash on/off
//!     (matches the desktop shell's lib.rs).
//!   - CLIMAX_DATA_DIR = override the data directory (Docker volumes, testing).
//!   - CLIMAX_PORT = override the bind port (default 9998).
//!   - CLIMAX_BIND = bind address (default 127.0.0.1). Set 0.0.0.0 to expose the
//!     server to the LAN / out of a Docker container - set CLIMAX_TOKEN with it.
//!   - CLIMAX_TOKEN = shared auth token. When set, the data endpoints (/rpc, the
//!     WebSockets, /backup) require it; /health + the web UI's static files stay
//!     open so a browser can load the page and prompt for it.
//!   - CLIMAX_ALLOWED_HOSTS = comma-separated extra hostnames allowed past the
//!     DNS-rebinding Host check (IP literals, localhost, single-label names, and
//!     *.local always pass; a public DNS name like a Tailscale ts.net one goes
//!     here).
//!
//! NOTE: only ONE Climax backend can hold the port at a time, and two writers
//! on one SQLite file corrupts it - so the standalone server and the desktop
//! app's in-process backend must not run against the same (port, DB) at once.
//! Phase 2c retires the in-process backend; until then this binary is for
//! running the home headlessly (or on a different port / scratch DB for testing).

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use climax_core::{
    capture, db, mirror, models::UiSignal, remote, server, settings, stash, state::AppState, update,
};
use tokio::sync::broadcast;

mod webui;

/// Same default as the desktop app, so a single-PC client connects with no
/// config. The desktop app takes its port from the `http_port` SETTING (it has a
/// UI to change it); this binary takes CLIMAX_PORT, which sits with the rest of
/// its env config. Both fall back to the one constant in core.
const DEFAULT_HTTP_PORT: u16 = settings::DEFAULT_HTTP_PORT;
/// The Tauri bundle identifier; the data dir is `<platform data dir>/<this>`,
/// identical to what Tauri's `app_data_dir()` resolves for the desktop app.
const APP_IDENTIFIER: &str = "com.climax.app";
/// A session left running by a previous process with a heartbeat gap under this
/// is silently resumed (a quick relaunch); beyond it the headless server auto-ends
/// the session at its last heartbeat (see the crash-recovery block in `main`).
const CRASH_RECOVERY_THRESHOLD_MS: i64 = 10 * 60 * 1000;

/// Resolve the data directory the same way the desktop app does, so the
/// standalone server reads/writes the SAME database file. CLIMAX_DATA_DIR wins
/// (for Docker volumes / explicit testing); otherwise the platform data dir
/// (Windows %APPDATA%/Roaming, Linux ~/.local/share) joined with the bundle id.
fn resolve_data_dir() -> Result<PathBuf> {
    if let Ok(custom) = std::env::var("CLIMAX_DATA_DIR") {
        let custom = custom.trim();
        if !custom.is_empty() {
            return Ok(PathBuf::from(custom));
        }
    }
    let base = dirs::data_dir()
        .context("could not resolve a platform data dir; set CLIMAX_DATA_DIR explicitly")?;
    Ok(base.join(APP_IDENTIFIER))
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                tracing_subscriber::EnvFilter::new(
                    "climax_server=info,climax_core=info,axum=info,sqlx=warn",
                )
            }),
        )
        .init();

    // Launch profile, mirroring the desktop shell: real (default) uses
    // climax.sqlite + full Stash; test/scratch uses a disposable DB with Stash
    // OFF so a test run never touches the real library.
    let profile = std::env::var("CLIMAX_PROFILE").unwrap_or_default().to_lowercase();
    let is_scratch = matches!(profile.as_str(), "test" | "scratch");
    let stash_enabled = !is_scratch;
    let db_filename =
        if is_scratch { "climax-test.sqlite" } else { "climax.sqlite" }.to_string();
    stash::set_enabled(stash_enabled);

    let port: u16 = std::env::var("CLIMAX_PORT")
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(DEFAULT_HTTP_PORT);

    // Phase 3 knobs: bind address (localhost unless deliberately exposed),
    // shared token, extra allowed hostnames. A LAN/Docker bind without a token
    // is legal but loudly warned - anyone on the network can read everything.
    let bind: std::net::IpAddr = match std::env::var("CLIMAX_BIND") {
        Ok(s) if !s.trim().is_empty() => s.trim().parse().unwrap_or_else(|_| {
            tracing::warn!("CLIMAX_BIND {:?} is not a valid IP; binding 127.0.0.1", s);
            std::net::IpAddr::from([127, 0, 0, 1])
        }),
        _ => std::net::IpAddr::from([127, 0, 0, 1]),
    };
    let token = std::env::var("CLIMAX_TOKEN")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let allowed_hosts: Vec<String> = std::env::var("CLIMAX_ALLOWED_HOSTS")
        .map(|s| {
            s.split(',')
                .map(|h| h.trim().to_ascii_lowercase())
                .filter(|h| !h.is_empty())
                .collect()
        })
        .unwrap_or_default();
    if !bind.is_loopback() && token.is_none() {
        tracing::warn!(
            "server is bound to {} with NO token (CLIMAX_TOKEN unset) - anyone on the network can read and write everything. Set a token.",
            bind,
        );
    }

    let data_dir = resolve_data_dir()?;
    std::fs::create_dir_all(&data_dir)
        .with_context(|| format!("create data dir {}", data_dir.display()))?;
    tracing::info!(
        "climax-server starting: profile={} data_dir={} db={} stash={} port={}",
        if is_scratch { "scratch" } else { "real" },
        data_dir.display(),
        db_filename,
        stash_enabled,
        port,
    );

    // Apply any staged wipe / restore BEFORE the pool opens (same order as the
    // desktop boot), so a restore/reset staged from any client takes effect.
    if let Err(e) = db::apply_pending_wipe(&data_dir, &db_filename).await {
        tracing::error!("apply pending wipe failed: {:#}", e);
    }
    let restored = match db::apply_pending_restore(&data_dir, &db_filename).await {
        Ok(b) => b,
        Err(e) => {
            tracing::error!("apply pending restore failed: {:#}", e);
            false
        }
    };
    // Sweep temps a cancelled /restore upload or interrupted /backup left behind
    // (their inline cleanup can't run when the client disconnects mid-stream).
    db::sweep_web_temps(&data_dir, &db_filename);

    let pool = db::init_pool(data_dir.clone(), &db_filename)
        .await
        .context("db init")?;

    // Broadcast channels: ws_tx pushes session-state to connected bridges; ui_tx
    // carries UiSignals to a desktop client. We keep no initial receiver - the
    // bridge subscribes per /ws connection, and a desktop client subscribes when
    // it arrives; until then ui_tx sends are no-ops (send returns Err, ignored).
    let (ws_tx, _) = broadcast::channel::<String>(64);
    let (ui_tx, _) = broadcast::channel::<UiSignal>(64);

    // `None, None`: the standalone server IS the home - it never runs as a client
    // of another server (those fields only steer the desktop shell's native loops).
    let state = AppState::new(
        pool,
        ws_tx,
        ui_tx,
        data_dir,
        db_filename,
        stash_enabled,
        None,
        None,
        env!("CARGO_PKG_VERSION").to_string(),
    );

    // Crash recovery (headless policy). A session left 'active'/'paused' by an
    // unclean prior run (crash, power loss, container restart) would otherwise
    // stay "running" forever: a runaway wall-clock timer + passive capture blocked
    // (it only fires when no session is active). With no human at a headless home
    // to prompt (and /rpc exposing no crash-RESOLVE command yet), apply an
    // automatic policy:
    //   - gap < threshold  -> silent resume (a quick relaunch; the session continues).
    //   - gap >= threshold -> auto-end the session at its last heartbeat. The
    //     downtime had no tracking and nobody's here to decide "keep", so close it
    //     cleanly at the last known activity; a fresh session starts naturally when
    //     watching resumes. Excludes the offline time + unblocks capture.
    // Skipped on a fresh restore (the leftover came from the restored snapshot, not
    // a crash). When the desktop client connects to the server (phase 4) it can
    // offer the richer Keep / Discard-continue / Discard-end prompt instead.
    if !restored {
        match state.session.crash_leftover().await {
            Ok(Some((sid, gap_start))) => {
                let now = db::now_ms();
                if now - gap_start > CRASH_RECOVERY_THRESHOLD_MS {
                    match state.session.end_session_at(sid, gap_start).await {
                        Ok(_) => tracing::warn!(
                            "crash recovery: session {} was left active across a {}min downtime; auto-ended at its last heartbeat",
                            sid,
                            (now - gap_start) / 60_000,
                        ),
                        Err(e) => {
                            tracing::warn!("crash recovery: end_session_at({}) failed: {:#}", sid, e)
                        }
                    }
                } else if let Err(e) = state.session.resume_for_crash(sid, gap_start).await {
                    tracing::warn!("crash recovery: resume_for_crash({}) failed: {:#}", sid, e);
                } else {
                    tracing::info!(
                        "crash recovery: session {} resumed (gap {}s, under threshold - a quick relaunch)",
                        sid,
                        (now - gap_start) / 1000,
                    );
                }
            }
            Ok(None) => {}
            Err(e) => tracing::warn!("crash recovery: leftover check failed: {:#}", e),
        }
    }

    // HTTP/WS server: /health, /heartbeat, /active_session, /rpc, the bridge /ws,
    // plus (server-only) the embedded web UI fallback + GET /backup (Phase 5),
    // guarded by the Host check + optional token (Phase 3).
    server::spawn(
        state.clone(),
        server::SpawnOpts {
            addr: std::net::SocketAddr::from((bind, port)),
            token: token.clone(),
            allowed_hosts,
            extra: Some(webui::router()),
        },
    )
    .await
    .context("server spawn")?;
    tracing::info!(
        "climax-server listening on http://{}:{} (token {})",
        bind,
        port,
        if token.is_some() { "required" } else { "off" },
    );

    // Keep a running session's "last seen alive" time current, so the next boot's
    // crash recovery (above) measures the real downtime instead of the time since
    // a scene last played. Started here, after that recovery has run.
    climax_core::session::SessionManager::spawn_liveness(state.pool.clone());

    // Periodic GC of the per-tab heartbeat cache (tabs that disconnect silently).
    {
        let state = state.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(60)).await;
                state.session.prune_tab_cache().await;
            }
        });
    }

    // Look for a new release, at most once a day. Not gated on the Stash profile:
    // this asks GitHub, not Stash, and a scratch server wants the answer too. The
    // web UI reads the cache this fills.
    {
        let pool = state.pool.clone();
        tokio::spawn(async move {
            update::check_if_stale(&pool).await;
        });
    }

    // Stash-dependent engine loops. Skipped entirely in the scratch profile (Stash
    // off). These mirror the desktop app's lib.rs boot loops, adapted to hold the
    // AppState directly instead of fetching it from Tauri's managed state. (The
    // desktop app still runs its own copies for now; the in-process backend
    // retires in phase 2c, at which point this is the single home.)
    if stash_enabled {
        remote::spawn(state.clone());
        capture::spawn(state.clone());
        spawn_play_percent_boot_sync(state.clone());
        spawn_track_activity_heal(state.clone());
        spawn_mirror_loop(state.clone());
        spawn_metadata_loop(state.clone());
    }

    // Run until Ctrl-C / SIGTERM. The server + loops are detached tasks; main just
    // parks here so the process stays alive (the whole point of an always-on home).
    tokio::signal::ctrl_c().await.ok();
    tracing::info!("climax-server shutting down");
    Ok(())
}

/// One-shot: sync Stash's "minimum play percent" into Climax's play-counting
/// threshold, but only if Climax has never had one configured. Best-effort.
fn spawn_play_percent_boot_sync(state: AppState) {
    tokio::spawn(async move {
        let current = match settings::get_play_counting(&state.pool).await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("read play_counting on boot failed: {:#}", e);
                return;
            }
        };
        if current.threshold_pct.is_some() {
            return; // already configured - don't clobber
        }
        match stash::fetch_minimum_play_percent(&state.pool).await {
            Ok(Some(pct)) => {
                let _ = settings::set_play_counting(
                    &state.pool,
                    &settings::PlayCountingSetting { threshold_pct: Some(pct) },
                )
                .await;
                tracing::info!("synced play_counting threshold from Stash: {}%", pct);
            }
            Ok(None) => tracing::info!(
                "Stash did not return minimumPlayPercent (history off?); using 0% fallback"
            ),
            Err(e) => tracing::warn!(
                "could not fetch play threshold from Stash on boot (using 0% fallback): {:#}",
                e
            ),
        }
    });
}

/// One-shot self-heal: re-enable Stash's play-history tracking if a previous
/// organising session left it off (the invariant: off only while organising,
/// and boot is never organising). Best-effort.
fn spawn_track_activity_heal(state: AppState) {
    tokio::spawn(async move {
        match stash::ensure_track_activity_on(&state.pool).await {
            Ok(true) => tracing::info!(
                "launch: re-enabled Stash play-history tracking (a previous organising session had left it off)"
            ),
            Ok(false) => {}
            Err(e) => tracing::warn!(
                "launch: could not verify Stash trackActivity (will heal on next session start): {:#}",
                e
            ),
        }
    });
}

/// Play-history mirror loop: runs sync_active on launch (after a settle delay)
/// and then once per cadence, DISCOVERING scenes watched/O'd while offline.
/// Gated on the mirror_sync setting; re-checks every few minutes when disabled.
fn spawn_mirror_loop(state: AppState) {
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(20)).await;
        loop {
            let mut sleep_secs = 300u64;
            match settings::get_mirror_sync(&state.pool).await {
                Ok(cfg) if cfg.enabled => {
                    let status = state.mirror_status.clone();
                    match mirror::sync_active(&state.pool, &status).await {
                        Ok(n) => tracing::info!("mirror: synced {} active scene(s)", n),
                        Err(e) => tracing::warn!("mirror sync pass failed: {:#}", e),
                    }
                    sleep_secs = (cfg.cadence_minutes as u64).max(1) * 60;
                }
                Ok(_) => {} // disabled - re-check shortly
                Err(e) => tracing::warn!("read mirror_sync setting failed: {:#}", e),
            }
            tokio::time::sleep(Duration::from_secs(sleep_secs)).await;
        }
    });
}

/// Library-metadata sync loop: refreshes STALE scene/entity details (titles,
/// thumbnails, performers, studios, tags, images) on launch + a longer cadence.
fn spawn_metadata_loop(state: AppState) {
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(30)).await;
        loop {
            let mut sleep_secs = 3600u64;
            match settings::get_metadata_refresh(&state.pool).await {
                Ok(cfg) if cfg.enabled => {
                    let status = state.metadata_status.clone();
                    match mirror::sync_metadata(&state.pool, &status, false).await {
                        Ok(n) => {
                            tracing::info!("metadata: refreshed {} stale scene(s) + entities", n)
                        }
                        Err(e) => tracing::warn!("metadata sync pass failed: {:#}", e),
                    }
                    sleep_secs = (cfg.cadence_minutes as u64).max(1) * 60;
                }
                Ok(_) => {} // disabled - re-check shortly
                Err(e) => tracing::warn!("read metadata_sync setting failed: {:#}", e),
            }
            tokio::time::sleep(Duration::from_secs(sleep_secs)).await;
        }
    });
}
