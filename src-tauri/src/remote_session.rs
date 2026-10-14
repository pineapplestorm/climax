// Session control surface for the native desktop loops (Phase 2c).
//
// The tray (live tooltip + Start/Stop/Pause menu + quit guard), the global
// hotkey, idle detection (presence.rs), and the shutdown guard all need the
// CURRENT session and sometimes drive it. In the default in-process backend they
// talk to the local `SessionManager` directly. In CLIENT mode the session lives
// on the EXTERNAL server, so the local `AppState.session` is an empty throwaway
// cache - these loops must instead read / command the server over HTTP.
//
// This module is the one place that branch lives: each fn dispatches on
// `state.client_server_url` (None = local manager, Some(base) = the server's
// /rpc). It mirrors the `rpc_http` proxy idea but for the Rust-side loops rather
// than the frontend (the webview can't fetch the server cross-origin; these are
// native Rust calls with no CORS to worry about).
//
// Reads degrade to "no session" on a server-unreachable error - the right
// fallback for a tray tooltip / idle check / shutdown guard when the server is
// briefly down (the ConnectionBadge surfaces the real connection state). Writes
// surface the error so callers can log it.

use std::sync::OnceLock;
use std::time::Duration;

use crate::models::Session;
use crate::settings::{self, IdleDetectionSetting};
use crate::state::AppState;

/// Shared HTTP client for the loops' server calls. Built ONCE - constructing a
/// fresh `reqwest::Client` per poll (the tray ticks every 1s) would rebuild a
/// connection pool each time.
fn http() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(6))
            .build()
            .expect("build native-session http client")
    })
}

/// POST `{cmd, args}` to the configured server's `/rpc` and return the raw JSON
/// result. Mirrors `rpc_http`'s contract: non-2xx / transport error -> Err.
/// `token` (the client-config token) rides as X-Climax-Token when the server
/// requires one (Phase 3).
async fn rpc(
    base: &str,
    token: Option<&str>,
    cmd: &str,
    args: serde_json::Value,
) -> anyhow::Result<serde_json::Value> {
    let url = format!("{}/rpc", base.trim_end_matches('/'));
    // Errors here can surface in the UI (idle/crash resolve -> IdleReturnModal),
    // so the message is plain English; cmd + detail go to the log. Transport
    // failures log at debug - the lenient read paths poll every few seconds and
    // would otherwise spam warns while the server is down.
    let mut req = http()
        .post(&url)
        .json(&serde_json::json!({ "cmd": cmd, "args": args }));
    if let Some(t) = token {
        req = req.header("X-Climax-Token", t);
    }
    let resp = req
        .send()
        .await
        .map_err(|e| {
            tracing::debug!("remote rpc {}: POST {} failed: {:#}", cmd, url, e);
            anyhow::anyhow!("Couldn't reach the Climax server.")
        })?;
    let status = resp.status();
    let body: serde_json::Value = resp.json().await.unwrap_or(serde_json::Value::Null);
    if status.is_success() {
        Ok(body)
    } else {
        tracing::warn!("remote rpc {} -> HTTP {}: {}", cmd, status, body);
        match body.get("error").and_then(|e| e.as_str()) {
            Some(msg) => anyhow::bail!("{}", msg),
            None => anyhow::bail!(
                "The Climax server returned an error (HTTP {}).",
                status.as_u16()
            ),
        }
    }
}

/// The active-OR-paused session (mirrors `SessionManager::active`), from the
/// server in client mode or the local manager otherwise. None on no session OR
/// on a server-unreachable error (the loops treat "can't tell" as "no session").
pub async fn active(state: &AppState) -> Option<Session> {
    match &state.client_server_url {
        Some(base) => match rpc(base, state.client_server_token.as_deref(), "session_active", serde_json::json!({})).await {
            Ok(v) => serde_json::from_value::<Option<Session>>(v).ok().flatten(),
            Err(e) => {
                tracing::debug!("remote session_active failed: {:#}", e);
                None
            }
        },
        None => state.session.active().await.ok().flatten(),
    }
}

/// Like `active`, but SURFACES a server-unreachable error instead of masking it
/// as None. Use this for START/STOP *decisions* (hotkey, tray toggle): the
/// lenient `active` reads a briefly-unreachable server as "no session", which
/// would make a toggle START a session - and the server's `session_start` isn't
/// idempotent (it ENDs the running session and inserts a fresh one). So the
/// decision paths only act on a CONFIRMED read; an uncertain read is a no-op.
/// The local (in-process) branch never errors out - a local hiccup masks to None
/// exactly as the old `.unwrap_or(None)` did - so the default path is unchanged.
pub async fn active_checked(state: &AppState) -> anyhow::Result<Option<Session>> {
    match &state.client_server_url {
        Some(base) => {
            let v = rpc(base, state.client_server_token.as_deref(), "session_active", serde_json::json!({})).await?;
            Ok(serde_json::from_value::<Option<Session>>(v).unwrap_or(None))
        }
        None => Ok(state.session.active().await.ok().flatten()),
    }
}

/// The active-OR-paused session's id, if any (convenience over `active`).
pub async fn active_id(state: &AppState) -> Option<i64> {
    match &state.client_server_url {
        Some(_) => active(state).await.map(|s| s.id),
        None => state.session.active_id().await.ok().flatten(),
    }
}

/// Start a session (no-op-ish if one is already active - the manager / server
/// owns that rule).
pub async fn start(state: &AppState) -> anyhow::Result<()> {
    match &state.client_server_url {
        Some(base) => {
            rpc(base, state.client_server_token.as_deref(), "session_start", serde_json::json!({})).await?;
            Ok(())
        }
        None => {
            state.session.start().await?;
            Ok(())
        }
    }
}

/// Stop the active session.
pub async fn stop(state: &AppState) -> anyhow::Result<()> {
    match &state.client_server_url {
        Some(base) => {
            rpc(base, state.client_server_token.as_deref(), "session_stop", serde_json::json!({})).await?;
            Ok(())
        }
        None => {
            state.session.stop().await?;
            Ok(())
        }
    }
}

/// Pause the active session (reason "manual" - the only runtime pause reason).
pub async fn pause(state: &AppState) -> anyhow::Result<()> {
    match &state.client_server_url {
        Some(base) => {
            rpc(base, state.client_server_token.as_deref(), "session_pause", serde_json::json!({})).await?;
            Ok(())
        }
        None => {
            state.session.pause("manual").await?;
            Ok(())
        }
    }
}

/// Resume a paused session.
pub async fn resume(state: &AppState) -> anyhow::Result<()> {
    match &state.client_server_url {
        Some(base) => {
            rpc(base, state.client_server_token.as_deref(), "session_resume", serde_json::json!({})).await?;
            Ok(())
        }
        None => {
            state.session.resume().await?;
            Ok(())
        }
    }
}

/// Record an idle/crash gap as a closed pause (the idle-return / crash-recovery
/// "Discard & Continue" path).
pub async fn record_gap_pause(
    state: &AppState,
    session_id: i64,
    paused_at: i64,
    resumed_at: i64,
    reason: &str,
) -> anyhow::Result<()> {
    match &state.client_server_url {
        Some(base) => {
            rpc(
                base,
                state.client_server_token.as_deref(),
                "record_gap_pause",
                serde_json::json!({
                    "sessionId": session_id,
                    "pausedAt": paused_at,
                    "resumedAt": resumed_at,
                    "reason": reason,
                }),
            )
            .await?;
            Ok(())
        }
        None => {
            state
                .session
                .record_gap_pause(session_id, paused_at, resumed_at, reason)
                .await?;
            Ok(())
        }
    }
}

/// End a session at a specific timestamp (the idle-return / crash-recovery
/// "Discard & End" path). Returns the updated session if it existed.
pub async fn end_session_at(
    state: &AppState,
    session_id: i64,
    ended_at: i64,
) -> anyhow::Result<Option<Session>> {
    match &state.client_server_url {
        Some(base) => {
            let v = rpc(
                base,
                state.client_server_token.as_deref(),
                "end_session_at",
                serde_json::json!({ "sessionId": session_id, "endedAt": ended_at }),
            )
            .await?;
            Ok(serde_json::from_value::<Option<Session>>(v).unwrap_or(None))
        }
        None => Ok(state.session.end_session_at(session_id, ended_at).await?),
    }
}

/// Accept the passive-capture prompt: start a session backdated to when watching
/// began, seeded with the scene. In client mode the SERVER detected the watch and
/// holds the `pending_capture` slot, so the accept runs there over /rpc; locally it
/// runs against the in-process manager. (The native command keeps the window op.)
pub async fn capture_accept(state: &AppState) -> anyhow::Result<Option<Session>> {
    match &state.client_server_url {
        Some(base) => {
            let v = rpc(base, state.client_server_token.as_deref(), "capture_accept", serde_json::json!({})).await?;
            Ok(serde_json::from_value::<Option<Session>>(v).unwrap_or(None))
        }
        None => {
            let payload = { state.pending_capture.write().await.take() };
            let Some(p) = payload else { return Ok(None) };
            let session = state
                .session
                .start_backdated_with_scene(p.watch_started_at, p.content_item_id)
                .await?;
            Ok(Some(session))
        }
    }
}

/// Snooze/dismiss the passive-capture prompt: suppress prompts for the configured
/// window + re-baseline the no-session watch. In client mode this state lives on
/// the SERVER (its capture loop checks it), so route there; locally it runs against
/// the in-process state. (The native command keeps the window op.)
pub async fn capture_snooze(state: &AppState) -> anyhow::Result<()> {
    match &state.client_server_url {
        Some(base) => {
            rpc(base, state.client_server_token.as_deref(), "capture_snooze", serde_json::json!({})).await?;
            Ok(())
        }
        None => {
            let cfg = settings::get_capture_prompt(&state.pool).await?;
            *state.capture_snooze_until.write().await =
                crate::db::now_ms() + (cfg.snooze_minutes as i64).max(1) * 60_000;
            *state.pending_capture.write().await = None;
            state.session.clear_no_session_watch().await;
            Ok(())
        }
    }
}

/// The idle-detection setting. In client mode this is read from the SERVER (the
/// user configures it there; the local cache is empty), so the idle loop honours
/// the real enabled flag + threshold. Falls back to the default on any error.
pub async fn idle_setting(state: &AppState) -> IdleDetectionSetting {
    match &state.client_server_url {
        Some(base) => match rpc(base, state.client_server_token.as_deref(), "idle_detection_get", serde_json::json!({})).await {
            Ok(v) => serde_json::from_value(v).unwrap_or_default(),
            Err(_) => IdleDetectionSetting::default(),
        },
        None => settings::get_idle_detection(&state.pool)
            .await
            .unwrap_or_default(),
    }
}

/// Whether the interface says "orgasm" instead of the default "cumshot". Only
/// the tray tooltip needs it on the Rust side. Client mode reads the SERVER's
/// choice for the same reason `idle_setting` does; any error reads as the
/// default.
pub async fn says_orgasm(state: &AppState) -> bool {
    let value = match &state.client_server_url {
        Some(base) => rpc(base, state.client_server_token.as_deref(), "wording_get", serde_json::json!({}))
            .await
            .ok()
            .and_then(|v| v.as_str().map(str::to_string)),
        None => settings::get_wording(&state.pool).await.ok(),
    };
    value.as_deref() == Some("orgasm")
}

/// Startup prefs, read at BOOT in client mode - before `AppState` exists, so
/// this takes the address/token directly rather than a state handle.
///
/// These live on the SERVER, not locally: the Settings UI writes them over
/// `/rpc`, and a client's own DB is the throwaway `client-cache.sqlite`, which
/// never has them. Reading them locally therefore always missed and fell back
/// to the default, so "When Climax opens, show" was silently ignored in client
/// mode no matter what you picked. Same class of bug as `idle_setting` above,
/// which was fixed for idle detection but not for these two.
///
/// Returns `None` per field when the server can't be reached, so the caller
/// keeps its own default rather than inventing one here.
pub async fn boot_startup_prefs(
    base: &str,
    token: Option<&str>,
) -> (Option<String>, Option<bool>) {
    let launch = rpc(base, token, "launch_behavior_get", serde_json::json!({}))
        .await
        .ok()
        .and_then(|v| v.as_str().map(str::to_string));
    let hotkey = rpc(base, token, "hotkey_enabled_get", serde_json::json!({}))
        .await
        .ok()
        .and_then(|v| v.as_bool());
    (launch, hotkey)
}

/// Whether shutting down THIS machine would take down the server holding the
/// session = the server is local. True for the in-process backend (`None`) and
/// for a client pointed at localhost / 127.0.0.1 / ::1; false for a LAN/NAS
/// server. The shutdown guard uses this: a remote-server client must NOT block
/// this PC's shutdown (the session is safe on the other host) - see CLAUDE.md
/// "SHUTDOWN GUARD semantics in the split".
pub fn server_is_local(state: &AppState) -> bool {
    match &state.client_server_url {
        None => true,
        Some(url) => matches!(host_of(url), "localhost" | "127.0.0.1" | "::1"),
    }
}

/// Extract the host out of a `scheme://[user@]host[:port][/...]` URL. Handles the
/// IPv6 `[::1]` bracket form. Returns "" if it can't parse one.
fn host_of(url: &str) -> &str {
    let after = url.split("://").nth(1).unwrap_or(url);
    let authority = after.split('/').next().unwrap_or("");
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    if let Some(rest) = authority.strip_prefix('[') {
        rest.split(']').next().unwrap_or("")
    } else {
        authority.split(':').next().unwrap_or("")
    }
}
