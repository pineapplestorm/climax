// Typed accessors for app settings.
//
// Two backing stores:
// - `settings` table (key/value JSON, one row per logical setting). Used for
//   most things (idle detection, metadata refresh TTL, etc).
// - `sources.config_json` row for the 'stash' source. Used for Stash
//   connection info (URL + optional API key) because that data is logically
//   coupled to the source row, not a free-floating app setting.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use crate::db::now_ms;

// ---------------------------------------------------------------------------
// idle_detection — controls the AFK / idle-on-return prompt.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdleDetectionSetting {
    pub enabled: bool,
    /// How long the user can be idle (no keyboard / mouse input) before we
    /// consider the gap worth asking about. Default 10.
    pub threshold_minutes: u32,
}

impl Default for IdleDetectionSetting {
    fn default() -> Self {
        Self {
            enabled: true,
            threshold_minutes: 10,
        }
    }
}

pub async fn get_idle_detection(pool: &SqlitePool) -> Result<IdleDetectionSetting> {
    let raw: Option<String> = sqlx::query_scalar(
        "SELECT value_json FROM settings WHERE key = 'idle_detection'",
    )
    .fetch_optional(pool)
    .await
    .context("read idle_detection setting")?;

    match raw {
        Some(s) => match serde_json::from_str::<IdleDetectionSetting>(&s) {
            Ok(v) => Ok(v),
            Err(e) => {
                tracing::warn!(
                    "idle_detection setting was malformed ({:#}); falling back to defaults",
                    e
                );
                Ok(IdleDetectionSetting::default())
            }
        },
        None => Ok(IdleDetectionSetting::default()),
    }
}

pub async fn set_idle_detection(pool: &SqlitePool, value: &IdleDetectionSetting) -> Result<()> {
    let value_json = serde_json::to_string(value).context("serialize idle_detection")?;
    let now = now_ms();
    sqlx::query(
        "INSERT INTO settings (key, value_json, updated_at)
         VALUES ('idle_detection', ?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json, updated_at = excluded.updated_at",
    )
    .bind(value_json)
    .bind(now)
    .execute(pool)
    .await
    .context("write idle_detection setting")?;
    Ok(())
}

// ---------------------------------------------------------------------------
// mirror_sync — Phase 7 Stash-history mirror. `enabled` gates the lightweight
// periodic reconciliation pass (re-reads Stash's o_history / play counts for
// already-known scenes so the 1:1 mirror guarantee holds without user setup);
// it's on by default because that pass only touches scenes Climax already knows.
// The expensive full-catalog import is always on-demand (a Settings button),
// NOT driven by this flag. `cadence_minutes` is how often the background loop
// sweeps stale scenes.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MirrorSyncSetting {
    pub enabled: bool,
    pub cadence_minutes: u32,
}

impl Default for MirrorSyncSetting {
    fn default() -> Self {
        Self {
            enabled: true,
            // ~1 day: a sync runs on launch and then once per cadence while open.
            cadence_minutes: 1440,
        }
    }
}

pub async fn get_mirror_sync(pool: &SqlitePool) -> Result<MirrorSyncSetting> {
    let raw: Option<String> = sqlx::query_scalar(
        "SELECT value_json FROM settings WHERE key = 'mirror_sync'",
    )
    .fetch_optional(pool)
    .await
    .context("read mirror_sync setting")?;

    match raw {
        Some(s) => match serde_json::from_str::<MirrorSyncSetting>(&s) {
            Ok(v) => Ok(v),
            Err(e) => {
                tracing::warn!(
                    "mirror_sync setting was malformed ({:#}); falling back to defaults",
                    e
                );
                Ok(MirrorSyncSetting::default())
            }
        },
        None => Ok(MirrorSyncSetting::default()),
    }
}

pub async fn set_mirror_sync(pool: &SqlitePool, value: &MirrorSyncSetting) -> Result<()> {
    let value_json = serde_json::to_string(value).context("serialize mirror_sync")?;
    let now = now_ms();
    sqlx::query(
        "INSERT INTO settings (key, value_json, updated_at)
         VALUES ('mirror_sync', ?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json, updated_at = excluded.updated_at",
    )
    .bind(value_json)
    .bind(now)
    .execute(pool)
    .await
    .context("write mirror_sync setting")?;
    Ok(())
}

// ---------------------------------------------------------------------------
// remote_tracking — Phase 7 LIVE remote-device detection. While a session is
// active, a background poller (remote.rs) diffs Stash's per-scene play_duration
// to detect scenes being watched on OTHER devices (e.g. the StashAppAndroidTV
// app) and credits them to the session in real time, exactly like the local
// bridge. `enabled` gates the whole behaviour; `poll_seconds` is the diff
// cadence — kept under record_heartbeat's 30s clamp; ~15s catches both the TV
// app's ~10s sceneSaveActivity interval and the local web player's 10s commit
// chunks. The poller idles (no Stash calls) whenever no session is active.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct RemoteTrackingSetting {
    pub enabled: bool,
    pub poll_seconds: u32,
}

impl Default for RemoteTrackingSetting {
    fn default() -> Self {
        Self {
            enabled: true,
            poll_seconds: 15,
        }
    }
}

pub async fn get_remote_tracking(pool: &SqlitePool) -> Result<RemoteTrackingSetting> {
    let raw: Option<String> = sqlx::query_scalar(
        "SELECT value_json FROM settings WHERE key = 'remote_tracking'",
    )
    .fetch_optional(pool)
    .await
    .context("read remote_tracking setting")?;

    match raw {
        Some(s) => match serde_json::from_str::<RemoteTrackingSetting>(&s) {
            Ok(v) => Ok(v),
            Err(e) => {
                tracing::warn!(
                    "remote_tracking setting was malformed ({:#}); falling back to defaults",
                    e
                );
                Ok(RemoteTrackingSetting::default())
            }
        },
        None => Ok(RemoteTrackingSetting::default()),
    }
}

pub async fn set_remote_tracking(pool: &SqlitePool, value: &RemoteTrackingSetting) -> Result<()> {
    let value_json = serde_json::to_string(value).context("serialize remote_tracking")?;
    let now = now_ms();
    sqlx::query(
        "INSERT INTO settings (key, value_json, updated_at)
         VALUES ('remote_tracking', ?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json, updated_at = excluded.updated_at",
    )
    .bind(value_json)
    .bind(now)
    .execute(pool)
    .await
    .context("write remote_tracking setting")?;
    Ok(())
}

// ---------------------------------------------------------------------------
// capture_prompt — Phase 7 component 3. When you start watching a scene with NO
// session running, after `threshold_minutes` of actual playback Climax nudges
// you (a bottom-right toast) to track it. Accepting starts a session BACKDATED
// to when watching began. `snooze_minutes` = after you dismiss, how long before
// fresh watching (on any scene) can re-trigger the nudge. Off by default; the
// user opts in. The bridge must be reporting playback for this to fire.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct CapturePromptSetting {
    pub enabled: bool,
    pub threshold_minutes: u32,
    pub snooze_minutes: u32,
}

impl Default for CapturePromptSetting {
    fn default() -> Self {
        Self {
            enabled: false,
            threshold_minutes: 3,
            snooze_minutes: 30,
        }
    }
}

pub async fn get_capture_prompt(pool: &SqlitePool) -> Result<CapturePromptSetting> {
    let raw: Option<String> = sqlx::query_scalar(
        "SELECT value_json FROM settings WHERE key = 'capture_prompt'",
    )
    .fetch_optional(pool)
    .await
    .context("read capture_prompt setting")?;

    match raw {
        Some(s) => match serde_json::from_str::<CapturePromptSetting>(&s) {
            Ok(v) => Ok(v),
            Err(e) => {
                tracing::warn!(
                    "capture_prompt setting was malformed ({:#}); falling back to defaults",
                    e
                );
                Ok(CapturePromptSetting::default())
            }
        },
        None => Ok(CapturePromptSetting::default()),
    }
}

pub async fn set_capture_prompt(pool: &SqlitePool, value: &CapturePromptSetting) -> Result<()> {
    let value_json = serde_json::to_string(value).context("serialize capture_prompt")?;
    let now = now_ms();
    sqlx::query(
        "INSERT INTO settings (key, value_json, updated_at)
         VALUES ('capture_prompt', ?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json, updated_at = excluded.updated_at",
    )
    .bind(value_json)
    .bind(now)
    .execute(pool)
    .await
    .context("write capture_prompt setting")?;
    Ok(())
}

// ---------------------------------------------------------------------------
// session_gap — Phase 7 component 2. The single knob for retroactive session
// reconstruction: how long a break (minutes) before clustered play/o activity
// counts as a NEW session. Shorter = more, tighter sessions (a break splits one
// sitting); longer = fewer, fuller ones (separate sittings can merge). The dev
// data is stable in the 30-60 zone; default 45.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct SessionGapSetting {
    pub gap_minutes: u32,
}

impl Default for SessionGapSetting {
    fn default() -> Self {
        Self { gap_minutes: 45 }
    }
}

pub async fn get_session_gap(pool: &SqlitePool) -> Result<SessionGapSetting> {
    let raw: Option<String> = sqlx::query_scalar(
        "SELECT value_json FROM settings WHERE key = 'session_gap'",
    )
    .fetch_optional(pool)
    .await
    .context("read session_gap setting")?;

    match raw {
        Some(s) => match serde_json::from_str::<SessionGapSetting>(&s) {
            Ok(v) => Ok(v),
            Err(e) => {
                tracing::warn!(
                    "session_gap setting was malformed ({:#}); falling back to defaults",
                    e
                );
                Ok(SessionGapSetting::default())
            }
        },
        None => Ok(SessionGapSetting::default()),
    }
}

pub async fn set_session_gap(pool: &SqlitePool, value: &SessionGapSetting) -> Result<()> {
    let value_json = serde_json::to_string(value).context("serialize session_gap")?;
    let now = now_ms();
    sqlx::query(
        "INSERT INTO settings (key, value_json, updated_at)
         VALUES ('session_gap', ?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json, updated_at = excluded.updated_at",
    )
    .bind(value_json)
    .bind(now)
    .execute(pool)
    .await
    .context("write session_gap setting")?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Stash connection — URL + optional API key, stored on the 'stash' source row's
// config_json. (NOT in the `settings` table, because this data is logically
// tied to the source it configures.)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StashConnectionSetting {
    /// Base URL, no trailing slash. e.g. "http://localhost:9999".
    pub url: String,
    /// Stash API key for the `ApiKey` HTTP header. None if Stash has no auth on.
    #[serde(default)]
    pub api_key: Option<String>,
}

impl Default for StashConnectionSetting {
    fn default() -> Self {
        Self {
            url: "http://localhost:9999".to_string(),
            api_key: None,
        }
    }
}

pub async fn get_stash_connection(pool: &SqlitePool) -> Result<StashConnectionSetting> {
    let raw: Option<String> = sqlx::query_scalar(
        "SELECT config_json FROM sources WHERE key = 'stash'",
    )
    .fetch_optional(pool)
    .await
    .context("read stash source config")?;

    match raw {
        Some(s) => {
            // The config_json on 'stash' has historically held only {"url": "..."}.
            // We extend it with api_key; serde's default keeps backwards compat for
            // rows written before the column existed.
            match serde_json::from_str::<StashConnectionSetting>(&s) {
                Ok(v) => Ok(v),
                Err(e) => {
                    tracing::warn!(
                        "stash source config_json malformed ({:#}); falling back to defaults",
                        e
                    );
                    Ok(StashConnectionSetting::default())
                }
            }
        }
        None => Ok(StashConnectionSetting::default()),
    }
}

pub async fn set_stash_connection(pool: &SqlitePool, value: &StashConnectionSetting) -> Result<()> {
    // Normalise: strip trailing slashes; treat empty api_key as None.
    let normalised = StashConnectionSetting {
        url: value.url.trim().trim_end_matches('/').to_string(),
        api_key: value
            .api_key
            .as_ref()
            .map(|k| k.trim().to_string())
            .filter(|k| !k.is_empty()),
    };
    let config_json = serde_json::to_string(&normalised).context("serialize stash connection")?;
    sqlx::query(
        "UPDATE sources SET config_json = ?1 WHERE key = 'stash'",
    )
    .bind(config_json)
    .execute(pool)
    .await
    .context("write stash source config")?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Metadata refresh — how often Climax re-queries Stash GraphQL for a scene's
// performers/studio/tags/etc when a content_item is sighted again.
// ---------------------------------------------------------------------------

// "Library metadata" sync (scene + entity DETAILS: titles, thumbnails,
// performers, studios, tags, images). Separate from the play-history mirror
// (mirror_sync) because details are "what things are" and change rarely, while
// history is "what happened" and carries the 1:1 Stash guarantee. `enabled`
// gates the periodic refresh; `cadence_minutes` is both how often the background
// pass runs and the staleness TTL the per-sighting enrichment uses before
// re-fetching a scene it has already seen.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct MetadataRefreshSetting {
    pub enabled: bool,
    pub cadence_minutes: u32,
}

impl Default for MetadataRefreshSetting {
    fn default() -> Self {
        Self {
            enabled: true,
            cadence_minutes: 7 * 24 * 60, // 7 days; scene/entity details change rarely
        }
    }
}

impl MetadataRefreshSetting {
    /// Staleness TTL in seconds (the cadence expressed in seconds).
    pub fn ttl_seconds(&self) -> i64 {
        (self.cadence_minutes as i64) * 60
    }
}

pub async fn get_metadata_refresh(pool: &SqlitePool) -> Result<MetadataRefreshSetting> {
    let raw: Option<String> = sqlx::query_scalar(
        "SELECT value_json FROM settings WHERE key = 'metadata_sync'",
    )
    .fetch_optional(pool)
    .await
    .context("read metadata_sync setting")?;

    match raw {
        Some(s) => match serde_json::from_str::<MetadataRefreshSetting>(&s) {
            Ok(v) => Ok(v),
            Err(e) => {
                tracing::warn!(
                    "metadata_sync setting was malformed ({:#}); falling back to defaults",
                    e
                );
                Ok(MetadataRefreshSetting::default())
            }
        },
        None => Ok(MetadataRefreshSetting::default()),
    }
}

pub async fn set_metadata_refresh(
    pool: &SqlitePool,
    value: &MetadataRefreshSetting,
) -> Result<()> {
    let value_json = serde_json::to_string(value).context("serialize metadata_sync")?;
    let now = now_ms();
    sqlx::query(
        "INSERT INTO settings (key, value_json, updated_at)
         VALUES ('metadata_sync', ?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json, updated_at = excluded.updated_at",
    )
    .bind(value_json)
    .bind(now)
    .execute(pool)
    .await
    .context("write metadata_sync setting")?;
    Ok(())
}

// ---------------------------------------------------------------------------
// default_date_preset — which date-range preset a section's date picker starts
// on when the dashboard loads. PER-SCOPE: each section ("overview", "sessions",
// ...) remembers its own default independently, set from the "Set default"
// buttons in that section's date dropdown. Frontend resolves the value against
// the preset list; validation happens client-side. Falls back to
// "last_12_months" if unset or malformed.
// ---------------------------------------------------------------------------

const DEFAULT_DATE_PRESET_FALLBACK: &str = "last_12_months";

/// Settings key for a scope's default preset. "overview" keeps the original
/// `default_date_preset` key so existing user choices carry over; other scopes
/// get a `default_date_preset_<scope>` key.
fn default_preset_key(scope: &str) -> String {
    if scope == "overview" {
        "default_date_preset".to_string()
    } else {
        format!("default_date_preset_{scope}")
    }
}

pub async fn get_default_date_preset(pool: &SqlitePool, scope: &str) -> Result<String> {
    let key = default_preset_key(scope);
    let raw: Option<String> = sqlx::query_scalar(
        "SELECT value_json FROM settings WHERE key = ?1",
    )
    .bind(&key)
    .fetch_optional(pool)
    .await
    .context("read default_date_preset setting")?;

    let parsed = raw
        .as_deref()
        .and_then(|s| serde_json::from_str::<String>(s).ok());
    Ok(parsed.unwrap_or_else(|| DEFAULT_DATE_PRESET_FALLBACK.to_string()))
}

pub async fn set_default_date_preset(pool: &SqlitePool, scope: &str, value: &str) -> Result<()> {
    let key = default_preset_key(scope);
    let value_json = serde_json::to_string(value).context("serialize default_date_preset")?;
    let now = now_ms();
    sqlx::query(
        "INSERT INTO settings (key, value_json, updated_at)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json, updated_at = excluded.updated_at",
    )
    .bind(&key)
    .bind(value_json)
    .bind(now)
    .execute(pool)
    .await
    .context("write default_date_preset setting")?;
    Ok(())
}

// default_chart_metric — which metric a chart's metric toggle starts on. PER
// SCOPE (e.g. "overview", "trends_area", "trends_rankings"): each chart keeps
// its own default independently, set via the "Default" pill on that toggle.
const DEFAULT_CHART_METRIC_FALLBACK: &str = "cumshots";

fn chart_metric_key(scope: &str) -> String {
    format!("default_chart_metric_{scope}")
}

pub async fn get_default_chart_metric(pool: &SqlitePool, scope: &str) -> Result<String> {
    let key = chart_metric_key(scope);
    let raw: Option<String> = sqlx::query_scalar("SELECT value_json FROM settings WHERE key = ?1")
        .bind(&key)
        .fetch_optional(pool)
        .await
        .context("read default_chart_metric setting")?;
    let parsed = raw
        .as_deref()
        .and_then(|s| serde_json::from_str::<String>(s).ok());
    Ok(parsed.unwrap_or_else(|| DEFAULT_CHART_METRIC_FALLBACK.to_string()))
}

pub async fn set_default_chart_metric(pool: &SqlitePool, scope: &str, value: &str) -> Result<()> {
    let key = chart_metric_key(scope);
    let value_json = serde_json::to_string(value).context("serialize default_chart_metric")?;
    let now = now_ms();
    sqlx::query(
        "INSERT INTO settings (key, value_json, updated_at)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json, updated_at = excluded.updated_at",
    )
    .bind(&key)
    .bind(value_json)
    .bind(now)
    .execute(pool)
    .await
    .context("write default_chart_metric setting")?;
    Ok(())
}

// default_performer_gender — which side of the Trends Top-performers
// female/male toggle is selected on load. "female" unless the user pins male
// via the toggle's Default pill.
const DEFAULT_PERFORMER_GENDER_FALLBACK: &str = "female";

pub async fn get_default_performer_gender(pool: &SqlitePool) -> Result<String> {
    let raw: Option<String> = sqlx::query_scalar(
        "SELECT value_json FROM settings WHERE key = 'default_performer_gender'",
    )
    .fetch_optional(pool)
    .await
    .context("read default_performer_gender setting")?;
    let parsed = raw
        .as_deref()
        .and_then(|s| serde_json::from_str::<String>(s).ok());
    Ok(parsed.unwrap_or_else(|| DEFAULT_PERFORMER_GENDER_FALLBACK.to_string()))
}

pub async fn set_default_performer_gender(pool: &SqlitePool, value: &str) -> Result<()> {
    let value_json =
        serde_json::to_string(value).context("serialize default_performer_gender")?;
    let now = now_ms();
    sqlx::query(
        "INSERT INTO settings (key, value_json, updated_at)
         VALUES ('default_performer_gender', ?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json, updated_at = excluded.updated_at",
    )
    .bind(value_json)
    .bind(now)
    .execute(pool)
    .await
    .context("write default_performer_gender setting")?;
    Ok(())
}

// browse_default — a saved "default view" for a browse page (scenes / performers
// / studios / tags): search + filter chips + sort + rows, as a JSON blob the
// frontend owns. Per-kind, independent. We store the frontend's JSON object
// string directly as value_json (it's already valid JSON) and hand it back
// verbatim. None when the user hasn't saved a default for that page.

pub async fn get_browse_default(pool: &SqlitePool, kind: &str) -> Result<Option<String>> {
    let key = format!("browse_default_{kind}");
    let raw: Option<String> = sqlx::query_scalar("SELECT value_json FROM settings WHERE key = ?1")
        .bind(&key)
        .fetch_optional(pool)
        .await
        .context("read browse_default setting")?;
    Ok(raw)
}

pub async fn set_browse_default(pool: &SqlitePool, kind: &str, view_json: &str) -> Result<()> {
    let key = format!("browse_default_{kind}");
    let now = now_ms();
    sqlx::query(
        "INSERT INTO settings (key, value_json, updated_at)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json, updated_at = excluded.updated_at",
    )
    .bind(&key)
    .bind(view_json)
    .bind(now)
    .execute(pool)
    .await
    .context("write browse_default setting")?;
    Ok(())
}

// ---------------------------------------------------------------------------
// play_counting — threshold-based play counting, mirroring Stash's
// "minimum play percent" setting. The value is fetched FROM Stash on first
// launch (so every user starts with their actual Stash threshold), then
// owned by Climax — user can edit it in Settings and it stays put. None
// means "never been initialised"; consumers fall back to 0% (Stash's own
// default) until first sync lands.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct PlayCountingSetting {
    /// Percent of the scene's duration that must be watched (cumulative,
    /// pauses excluded) before the play "counts". 0-100. None until the
    /// first-launch sync from Stash succeeds OR the user manually sets it.
    pub threshold_pct: Option<f32>,
}

/// Effective threshold in 0.0..=1.0 form. Caller multiplies this by the
/// scene's duration_seconds to get the "this counts" boundary. Falls back
/// to 0.0 (Stash's default) when the user has never set a value and the
/// initial Stash sync hasn't succeeded yet.
pub fn effective_play_threshold_frac(s: &PlayCountingSetting) -> f32 {
    let pct = s.threshold_pct.unwrap_or(0.0);
    (pct / 100.0).clamp(0.0, 1.0)
}

pub async fn get_play_counting(pool: &SqlitePool) -> Result<PlayCountingSetting> {
    let raw: Option<String> = sqlx::query_scalar(
        "SELECT value_json FROM settings WHERE key = 'play_counting'",
    )
    .fetch_optional(pool)
    .await
    .context("read play_counting setting")?;

    match raw {
        Some(s) => match serde_json::from_str::<PlayCountingSetting>(&s) {
            Ok(v) => Ok(v),
            Err(e) => {
                tracing::warn!(
                    "play_counting setting malformed ({:#}); falling back to defaults",
                    e
                );
                Ok(PlayCountingSetting::default())
            }
        },
        None => Ok(PlayCountingSetting::default()),
    }
}

pub async fn set_play_counting(pool: &SqlitePool, value: &PlayCountingSetting) -> Result<()> {
    let value_json = serde_json::to_string(value).context("serialize play_counting")?;
    let now = now_ms();
    sqlx::query(
        "INSERT INTO settings (key, value_json, updated_at)
         VALUES ('play_counting', ?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json, updated_at = excluded.updated_at",
    )
    .bind(value_json)
    .bind(now)
    .execute(pool)
    .await
    .context("write play_counting setting")?;
    Ok(())
}

// ---------------------------------------------------------------------------
// launch_behavior — what to show when Climax opens. One of "silent" (tray only),
// "tracker", or "dashboard". Read once at boot (lib.rs). Default "tracker".
// ---------------------------------------------------------------------------

pub async fn get_launch_behavior(pool: &SqlitePool) -> Result<String> {
    let raw: Option<String> = sqlx::query_scalar(
        "SELECT value_json FROM settings WHERE key = 'launch_behavior'",
    )
    .fetch_optional(pool)
    .await
    .context("read launch_behavior setting")?;
    let val = raw
        .and_then(|s| serde_json::from_str::<String>(&s).ok())
        .unwrap_or_else(|| "tracker".to_string());
    Ok(match val.as_str() {
        "silent" | "tracker" | "dashboard" => val,
        _ => "tracker".to_string(),
    })
}

pub async fn set_launch_behavior(pool: &SqlitePool, mode: &str) -> Result<()> {
    let mode = match mode {
        "silent" | "tracker" | "dashboard" => mode,
        _ => "tracker",
    };
    let value_json = serde_json::to_string(mode).context("serialize launch_behavior")?;
    sqlx::query(
        "INSERT INTO settings (key, value_json, updated_at)
         VALUES ('launch_behavior', ?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json, updated_at = excluded.updated_at",
    )
    .bind(value_json)
    .bind(now_ms())
    .execute(pool)
    .await
    .context("write launch_behavior setting")?;
    Ok(())
}

// ---------------------------------------------------------------------------
// hotkey_enabled — whether the global Ctrl+Shift+L shortcut is registered.
// Read at boot (only registers when true) + toggled live. Default true.
// ---------------------------------------------------------------------------

pub async fn get_hotkey_enabled(pool: &SqlitePool) -> Result<bool> {
    let raw: Option<String> = sqlx::query_scalar(
        "SELECT value_json FROM settings WHERE key = 'hotkey_enabled'",
    )
    .fetch_optional(pool)
    .await
    .context("read hotkey_enabled setting")?;
    Ok(raw
        .and_then(|s| serde_json::from_str::<bool>(&s).ok())
        .unwrap_or(true))
}

pub async fn set_hotkey_enabled(pool: &SqlitePool, enabled: bool) -> Result<()> {
    let value_json = serde_json::to_string(&enabled).context("serialize hotkey_enabled")?;
    sqlx::query(
        "INSERT INTO settings (key, value_json, updated_at)
         VALUES ('hotkey_enabled', ?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json, updated_at = excluded.updated_at",
    )
    .bind(value_json)
    .bind(now_ms())
    .execute(pool)
    .await
    .context("write hotkey_enabled setting")?;
    Ok(())
}

// ---------------------------------------------------------------------------
// wording - which word the interface uses for an O: "cumshot" (the default) or
// "orgasm". DISPLAY ONLY. Every table, column, command name and setting key
// keeps saying cumshot whichever is chosen; the frontend swaps the visible word
// (src/lib/wording.svelte.ts) and the tray tooltip follows it. Stored here, not
// per device, so the choice follows the user to every client of this backend.
// ---------------------------------------------------------------------------

pub const WORDING_DEFAULT: &str = "cumshot";

/// Anything that is not a known value reads as the default, so a hand-edited or
/// future value can never leave the interface without a word to use.
pub fn normalise_wording(raw: &str) -> &'static str {
    match raw {
        "orgasm" => "orgasm",
        _ => WORDING_DEFAULT,
    }
}

pub async fn get_wording(pool: &SqlitePool) -> Result<String> {
    let raw: Option<String> = sqlx::query_scalar(
        "SELECT value_json FROM settings WHERE key = 'wording'",
    )
    .fetch_optional(pool)
    .await
    .context("read wording setting")?;
    let val = raw
        .and_then(|s| serde_json::from_str::<String>(&s).ok())
        .unwrap_or_else(|| WORDING_DEFAULT.to_string());
    Ok(normalise_wording(&val).to_string())
}

pub async fn set_wording(pool: &SqlitePool, value: &str) -> Result<String> {
    let value = normalise_wording(value);
    let value_json = serde_json::to_string(value).context("serialize wording")?;
    sqlx::query(
        "INSERT INTO settings (key, value_json, updated_at)
         VALUES ('wording', ?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json, updated_at = excluded.updated_at",
    )
    .bind(value_json)
    .bind(now_ms())
    .execute(pool)
    .await
    .context("write wording setting")?;
    Ok(value.to_string())
}

// ---------------------------------------------------------------------------
// keep_empty_sessions - whether stopping a session with nothing in it (no
// counted scene, no cumshot) keeps it instead of discarding it. Default false,
// which is the behaviour every install had before the setting existed. On, it
// lets Climax be used as a plain timer by someone who is not watching anything
// in Stash. Read by SessionManager::stop, the one place that auto-discards.
// ---------------------------------------------------------------------------

pub async fn get_keep_empty_sessions(pool: &SqlitePool) -> Result<bool> {
    let raw: Option<String> = sqlx::query_scalar(
        "SELECT value_json FROM settings WHERE key = 'keep_empty_sessions'",
    )
    .fetch_optional(pool)
    .await
    .context("read keep_empty_sessions setting")?;
    Ok(raw
        .and_then(|s| serde_json::from_str::<bool>(&s).ok())
        .unwrap_or(false))
}

pub async fn set_keep_empty_sessions(pool: &SqlitePool, enabled: bool) -> Result<()> {
    let value_json =
        serde_json::to_string(&enabled).context("serialize keep_empty_sessions")?;
    sqlx::query(
        "INSERT INTO settings (key, value_json, updated_at)
         VALUES ('keep_empty_sessions', ?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json, updated_at = excluded.updated_at",
    )
    .bind(value_json)
    .bind(now_ms())
    .execute(pool)
    .await
    .context("write keep_empty_sessions setting")?;
    Ok(())
}

// ---------------------------------------------------------------------------
// http_port - the port a Climax backend serves on (/rpc, /health, and the
// bridge's /ws). The desktop app reads it once at boot, since that's when the
// server binds, so a change takes effect on the next launch. The standalone
// server takes CLIMAX_PORT instead (env, alongside its token) and never reads
// this key - it has no settings UI to change it from.
//
// Default 9998: one below Stash's 9999, so it's easy to remember and to explain
// when pointing the bridge at it. That is a MEMORABILITY choice, not a safety
// one - Windows reserves blocks of ports that are redrawn on every boot, and any
// fixed port can land inside one, at which point the bind fails outright. Being
// able to move off it is the actual fix, which is why this setting exists and
// why lib.rs surfaces a failed bind instead of only logging it.
// ---------------------------------------------------------------------------

/// The port a Climax backend serves on when nothing overrides it.
pub const DEFAULT_HTTP_PORT: u16 = 9998;
/// Below 1024 is privileged, and 0 means "any free port" - which would leave the
/// bridge with no fixed address to connect to. Both are refused.
pub const MIN_HTTP_PORT: u16 = 1024;

pub async fn get_http_port(pool: &SqlitePool) -> Result<u16> {
    let raw: Option<String> =
        sqlx::query_scalar("SELECT value_json FROM settings WHERE key = 'http_port'")
            .fetch_optional(pool)
            .await
            .context("read http_port setting")?;
    // A stored port below the floor is treated as absent rather than honoured -
    // it could only get there by hand-editing the DB, and binding it would fail.
    Ok(raw
        .and_then(|s| serde_json::from_str::<u16>(&s).ok())
        .filter(|p| *p >= MIN_HTTP_PORT)
        .unwrap_or(DEFAULT_HTTP_PORT))
}

/// Takes a `u32`, not a `u16`, on purpose. With a `u16` an out-of-range number
/// fails during argument DESERIALIZATION, before this function runs, so the
/// designed sentence below never reaches the user - they get a raw serde message
/// that the frontend's error gate then replaces with a generic "try again", for a
/// condition retrying cannot fix. Widening the door lets one message cover both
/// ends, rather than duplicating the bounds in the frontend where they would
/// drift.
pub async fn set_http_port(pool: &SqlitePool, port: u32) -> Result<()> {
    if port < MIN_HTTP_PORT as u32 || port > u16::MAX as u32 {
        anyhow::bail!("Pick a port between {} and {}.", MIN_HTTP_PORT, u16::MAX);
    }
    let port = port as u16;
    let value_json = serde_json::to_string(&port).context("serialize http_port")?;
    sqlx::query(
        "INSERT INTO settings (key, value_json, updated_at)
         VALUES ('http_port', ?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json, updated_at = excluded.updated_at",
    )
    .bind(value_json)
    .bind(now_ms())
    .execute(pool)
    .await
    .context("write http_port setting")?;
    Ok(())
}

#[cfg(test)]
mod http_port_tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    /// A brand-new install: every migration applied, nothing configured.
    async fn fresh_pool() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open in-memory db");
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .expect("run migrations");
        pool
    }

    /// The regression that made this test worth writing: the initial migration
    /// seeded ('http_port', '9876') years before anything read that key, so the
    /// moment `get_http_port` existed the dead seed outranked the new default and
    /// a fresh install would have bound 9876 while the bridge looked for 9998.
    /// Nothing in the app would have said so.
    #[tokio::test]
    async fn a_fresh_install_gets_the_current_default_not_a_stale_seed() {
        let pool = fresh_pool().await;
        assert_eq!(get_http_port(&pool).await.unwrap(), DEFAULT_HTTP_PORT);
    }

    /// The other half: dropping the seed must not stop someone deliberately
    /// choosing that same port afterwards.
    #[tokio::test]
    async fn an_explicitly_chosen_port_is_kept() {
        let pool = fresh_pool().await;
        set_http_port(&pool, 9876).await.unwrap();
        assert_eq!(get_http_port(&pool).await.unwrap(), 9876);
    }

    #[tokio::test]
    async fn out_of_range_ports_are_refused_at_both_ends() {
        let pool = fresh_pool().await;
        for bad in [0, 80, 1023, 65_536, 70_000] {
            let err = set_http_port(&pool, bad).await.unwrap_err().to_string();
            // Must be a sentence the UI can show as-is: capitalised, ends with a
            // period, no ": " (the frontend's designed-error gate rejects those).
            assert!(err.starts_with("Pick a port between"), "{bad}: {err}");
            assert!(err.ends_with('.') && !err.contains(": "), "{bad}: {err}");
        }
        // Refusing must not have written anything.
        assert_eq!(get_http_port(&pool).await.unwrap(), DEFAULT_HTTP_PORT);
    }

    /// A hand-edited or version-skewed row must not brick the bind.
    #[tokio::test]
    async fn a_nonsense_stored_value_falls_back_to_the_default() {
        let pool = fresh_pool().await;
        for junk in ["\"banana\"", "-1", "17"] {
            sqlx::query(
                "INSERT INTO settings (key, value_json, updated_at) VALUES ('http_port', ?1, 0)
                 ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json",
            )
            .bind(junk)
            .execute(&pool)
            .await
            .unwrap();
            assert_eq!(
                get_http_port(&pool).await.unwrap(),
                DEFAULT_HTTP_PORT,
                "stored {junk}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// update_check - the cached result of the last look at GitHub's releases, and
// the release the user has already been told about.
//
// Cached rather than fetched on demand because the check is an outbound call to
// a third party: GitHub allows 60 unauthenticated requests an hour PER IP, and a
// household can have a server plus several browser clients behind one address.
// The boot check honours the TTL; an explicit "check for updates" ignores it,
// because a user pressing a button has asked for a fresh answer.
// ---------------------------------------------------------------------------

/// The newest published release, as much of it as the UI needs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseInfo {
    /// The tag with any leading "v" stripped, e.g. "0.2.0", so it compares
    /// directly against a crate version.
    pub version: String,
    /// Unix ms the release was published, when GitHub gave a parseable date.
    pub published_at: Option<i64>,
    /// The release page. The Download button opens this rather than an asset
    /// directly, so the user sees the notes and picks the right file.
    pub url: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UpdateCheckCache {
    /// Unix ms of the last COMPLETED check, successful or not. None = never
    /// checked, which the UI says plainly rather than implying "up to date".
    pub checked_at: Option<i64>,
    /// The newest release, or None when the repository has published none yet.
    /// Those are different states and the UI words them differently.
    pub latest: Option<ReleaseInfo>,
    /// Designed sentence from the last failure. Cleared by a successful check.
    pub last_error: Option<String>,
}

pub async fn get_update_check(pool: &SqlitePool) -> Result<UpdateCheckCache> {
    let raw: Option<String> =
        sqlx::query_scalar("SELECT value_json FROM settings WHERE key = 'update_check'")
            .fetch_optional(pool)
            .await
            .context("read update_check setting")?;

    match raw {
        Some(s) => match serde_json::from_str::<UpdateCheckCache>(&s) {
            Ok(v) => Ok(v),
            Err(e) => {
                tracing::warn!(
                    "update_check cache was malformed ({:#}); treating it as never checked",
                    e
                );
                Ok(UpdateCheckCache::default())
            }
        },
        None => Ok(UpdateCheckCache::default()),
    }
}

pub async fn set_update_check(pool: &SqlitePool, value: &UpdateCheckCache) -> Result<()> {
    let value_json = serde_json::to_string(value).context("serialize update_check")?;
    sqlx::query(
        "INSERT INTO settings (key, value_json, updated_at)
         VALUES ('update_check', ?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json, updated_at = excluded.updated_at",
    )
    .bind(value_json)
    .bind(now_ms())
    .execute(pool)
    .await
    .context("write update_check setting")?;
    Ok(())
}

// ---------------------------------------------------------------------------
// update_dismissed_version - the release the user has waved away. The update
// notice reappears only when a version NEWER than this one is published, so
// dismissing is per-release and can never turn into a permanent nag.
// ---------------------------------------------------------------------------

pub async fn get_update_dismissed(pool: &SqlitePool) -> Result<Option<String>> {
    let raw: Option<String> =
        sqlx::query_scalar("SELECT value_json FROM settings WHERE key = 'update_dismissed_version'")
            .fetch_optional(pool)
            .await
            .context("read update_dismissed_version setting")?;
    Ok(raw.and_then(|s| serde_json::from_str::<String>(&s).ok()))
}

pub async fn set_update_dismissed(pool: &SqlitePool, version: &str) -> Result<()> {
    let value_json = serde_json::to_string(version).context("serialize update_dismissed_version")?;
    sqlx::query(
        "INSERT INTO settings (key, value_json, updated_at)
         VALUES ('update_dismissed_version', ?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json, updated_at = excluded.updated_at",
    )
    .bind(value_json)
    .bind(now_ms())
    .execute(pool)
    .await
    .context("write update_dismissed_version setting")?;
    Ok(())
}

// ---------------------------------------------------------------------------
// onboarding_completed — whether the first-launch setup wizard has run. Read at
// boot (real profile only; the scratch profile has no Stash to import from), and
// the dashboard shows the wizard while it's false. Default false so a fresh
// install runs setup once. The "re-run setup" affordance flips it back to false.
// ---------------------------------------------------------------------------

pub async fn get_onboarding_completed(pool: &SqlitePool) -> Result<bool> {
    let raw: Option<String> = sqlx::query_scalar(
        "SELECT value_json FROM settings WHERE key = 'onboarding_completed'",
    )
    .fetch_optional(pool)
    .await
    .context("read onboarding_completed setting")?;
    Ok(raw
        .and_then(|s| serde_json::from_str::<bool>(&s).ok())
        .unwrap_or(false))
}

pub async fn set_onboarding_completed(pool: &SqlitePool, done: bool) -> Result<()> {
    let value_json = serde_json::to_string(&done).context("serialize onboarding_completed")?;
    sqlx::query(
        "INSERT INTO settings (key, value_json, updated_at)
         VALUES ('onboarding_completed', ?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json, updated_at = excluded.updated_at",
    )
    .bind(value_json)
    .bind(now_ms())
    .execute(pool)
    .await
    .context("write onboarding_completed setting")?;
    Ok(())
}

// ---------------------------------------------------------------------------
// backup — Backup and restore page. Remembers the last destination folder (so
// repeat backups default there) and when the last backup ran. A future auto /
// reminder cadence will extend this struct; #[serde(default)] keeps old rows
// readable as new fields land.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BackupSetting {
    /// Folder the user last backed up into (absolute). None until first backup.
    #[serde(default)]
    pub dest_folder: Option<String>,
    /// Unix-ms of the last successful backup. None until first backup.
    #[serde(default)]
    pub last_backup_at: Option<i64>,
}

pub async fn get_backup(pool: &SqlitePool) -> Result<BackupSetting> {
    let raw: Option<String> =
        sqlx::query_scalar("SELECT value_json FROM settings WHERE key = 'backup'")
            .fetch_optional(pool)
            .await
            .context("read backup setting")?;

    match raw {
        Some(s) => match serde_json::from_str::<BackupSetting>(&s) {
            Ok(v) => Ok(v),
            Err(e) => {
                tracing::warn!(
                    "backup setting was malformed ({:#}); falling back to defaults",
                    e
                );
                Ok(BackupSetting::default())
            }
        },
        None => Ok(BackupSetting::default()),
    }
}

pub async fn set_backup(pool: &SqlitePool, value: &BackupSetting) -> Result<()> {
    let value_json = serde_json::to_string(value).context("serialize backup")?;
    sqlx::query(
        "INSERT INTO settings (key, value_json, updated_at)
         VALUES ('backup', ?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json, updated_at = excluded.updated_at",
    )
    .bind(value_json)
    .bind(now_ms())
    .execute(pool)
    .await
    .context("write backup setting")?;
    Ok(())
}
