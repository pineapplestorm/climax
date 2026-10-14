// Session state machine + persistence.
//
// One session is active at a time. Holds an in-memory active session id pointer
// plus a heartbeat timestamp; everything else is reconciled against SQLite on each
// transition so a crash midway through can be recovered on next boot.
//
// Multi-tab handling:
// - tab_states keeps the most recent heartbeat per tab_id (across all bridge clients)
// - record_heartbeat updates the cache but does NOT immediately flip session status
// - apply_aggregate_state, called from a background tick, aggregates across all
//   recent tabs and applies a single status update. This kills the "flipping
//   between active/paused" you'd otherwise get with multiple Stash tabs open.
// - Per-scene seconds_tracked is only credited when TWO consecutive heartbeats
//   from the SAME tab on the SAME scene both report state=playing. One-off
//   "playing" blips from autoplay don't add time.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use sqlx::SqlitePool;
use tokio::sync::{broadcast, RwLock};

use crate::db::now_ms;
use crate::models::{ContentItem, HeartbeatPayload, OEvent, Session, SessionSort, SessionStatus};
use crate::settings;
use crate::stash;

/// Garbage-collect tabs idle for longer than this (ms).
const TAB_PRUNE_MS: i64 = 60_000;

/// How often a live backend stamps a running session as still alive (see
/// `SessionManager::touch_liveness`). Far below the 10 minute crash-recovery
/// threshold both binaries use, so a restart that takes seconds always reads as
/// a quick relaunch.
const LIVENESS_INTERVAL_SECS: u64 = 30;
/// A gap longer than this opens a NEW watch run instead of extending the one in
/// progress. It is per-source because the two paths report at very different
/// cadences, and the threshold has to sit above the reporting interval or every
/// single credit lands in a run of its own.
///
/// Bridge: heartbeats every 5s, so four missed in a row means playback really
/// did stop. Keeping this tight is the point - dawdle on another scene for half
/// a minute and coming back must start a new run, otherwise the band paints
/// straight over the detour and we are back to drawing time the scene was not
/// playing.
const BRIDGE_RUN_GAP_MS: i64 = 20_000;
/// Off-bridge: the Stash poll only runs every ~28s and can only say "it advanced
/// some time since I last looked", so it cannot resolve a gap finer than this.
const OFF_BRIDGE_RUN_GAP_MS: i64 = 90_000;
/// Minimum spacing between Stash metadata fetch attempts for the SAME
/// content item. Stops failed fetches from looping at 5s heartbeat cadence
/// while Stash is down or returning errors.
const METADATA_FETCH_DEBOUNCE_MS: i64 = 60_000;

#[derive(Debug, Clone)]
struct TabHeartbeat {
    session_id: i64,
    content_item_id: i64,
    /// "playing" | "paused" | "idle" - what the bridge reported.
    /// CAUTION: this can lie (e.g. background tab where the bare HTML5 video
    /// element still says paused=false even though Stash's UI shows paused).
    /// Use `last_advance_at` for the authoritative "is actually playing" signal.
    /// Kept for debugging/clarity though nothing reads it at runtime.
    #[allow(dead_code)]
    video_state: String,
    /// Last reported video.currentTime (seconds) - the SOURCE OF TRUTH for
    /// whether the video is actually playing. If this doesn't advance between
    /// heartbeats, no time gets credited regardless of what video_state says.
    current_time: f64,
    /// Server wall-clock time (ms) of the last heartbeat we got from this tab.
    last_seen: i64,
    /// Server wall-clock time (ms) of the last heartbeat where current_time
    /// went UP relative to the previous heartbeat. Used by apply_aggregate_state
    /// to determine "is any tab actually playing".
    last_advance_at: i64,
}

/// Per-tab playback tracking while NO session is active — the raw signal the
/// passive-capture prompt (capture.rs) watches. Mirrors TabHeartbeat's
/// currentTime-advance logic but only lives during the no-session state.
#[derive(Debug, Clone)]
struct NoSessionWatch {
    content_item_id: i64,
    scene_id: String,
    source: String,
    current_time: f64,
    /// Wall-clock ms when this continuous watch began advancing — the backdated
    /// session start if the prompt is accepted.
    started_at: i64,
    last_seen: i64,
    /// Wall-clock ms of the last heartbeat where currentTime advanced.
    last_advance_at: i64,
}

pub struct SessionManager {
    pool: SqlitePool,
    tab_states: Arc<RwLock<HashMap<String, TabHeartbeat>>>,
    /// Playback seen while no session is running, keyed by tab_id. Populated by
    /// record_heartbeat when there's no active session; read by capture.rs.
    no_session_watch: Arc<RwLock<HashMap<String, NoSessionWatch>>>,
    /// In-memory debounce for Stash metadata fetches, keyed by content_item_id.
    /// Value is the wall-clock ms of the last attempt. We skip new attempts
    /// against the same item within METADATA_FETCH_DEBOUNCE_MS so a row that
    /// keeps failing doesn't get pounded once per 5s heartbeat.
    stash_metadata_attempts: Arc<RwLock<HashMap<i64, i64>>>,
    /// Broadcast channel for pushing session-state changes to connected WS
    /// clients (the Stash bridge's navbar indicator). `notify_state()` fires it
    /// after every status transition; each WS handler subscribes. No polling.
    ws_tx: broadcast::Sender<String>,
    /// Organising mode: a deliberate "I'm tidying my Stash library, don't record
    /// play history" state. While true, Stash's `trackActivity` (Settings ->
    /// Interface -> Scene Player -> Enable scene play history) is OFF and there
    /// is NO active session. IN-MEMORY ONLY (ephemeral) so an unclean exit can
    /// never leave it stuck: boot always starts NOT organising and self-heals
    /// `trackActivity` back on. The invariant: `trackActivity` may be off only
    /// while this is true.
    organising: Arc<AtomicBool>,
}

/// Drop the Stash play timestamps this session's rollback is about to remove
/// from Stash.
///
/// THE THIRD THING A DELETE HAS TO CLEAN, and it went unnoticed because each of
/// the other two behaved correctly. The Stash rollback only writes to Stash. The
/// session delete only removes session-scoped rows, and `play_imports` is not
/// session-scoped - it is Stash's history, held by Climax. So the rows survived
/// both, ended up covered by no session, and reconstruction offered them straight
/// back: **delete a session and Climax proposes recreating it**, on the Untracked
/// page and in the "while you were away" prompt at the next launch.
///
/// Scoped to this session's own scenes and window, so an unrelated import that
/// merely falls in the same minutes is left alone. Must run BEFORE the session
/// row goes, since it reads `scene_plays`, which cascade with it.
///
/// The mirror's `sweep_stale_imports` is the general safety net for anything that
/// still slips through; this is the direct causal fix so the phantom never
/// appears in the first place.
async fn drop_imports_for_session(
    conn: &mut sqlx::SqliteConnection,
    session_id: i64,
) -> Result<()> {
    let win: Option<(i64, Option<i64>)> =
        sqlx::query_as("SELECT started_at, ended_at FROM sessions WHERE id = ?1")
            .bind(session_id)
            .fetch_optional(&mut *conn)
            .await?;
    let Some((start, ended)) = win else { return Ok(()) };
    let end = ended.unwrap_or_else(now_ms);

    sqlx::query(
        "DELETE FROM play_imports
          WHERE played_at >= ?2 AND played_at <= ?3
            AND content_item_id IN
                (SELECT content_item_id FROM scene_plays WHERE session_id = ?1)",
    )
    .bind(session_id)
    .bind(start)
    .bind(end)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

impl SessionManager {
    pub fn new(pool: SqlitePool, ws_tx: broadcast::Sender<String>) -> Self {
        Self {
            pool,
            tab_states: Arc::new(RwLock::new(HashMap::new())),
            no_session_watch: Arc::new(RwLock::new(HashMap::new())),
            stash_metadata_attempts: Arc::new(RwLock::new(HashMap::new())),
            ws_tx,
            organising: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Current tracking state as a `session_state` message for the bridge's
    /// navbar indicator: `active` = a session exists (running OR paused),
    /// `status` = "active" | "paused" | null, `organising` = organising mode is
    /// on (a third pill state; mutually exclusive with `active`). Computed.
    pub async fn session_state_json(&self) -> String {
        let organising = self.is_organising();
        let (active, status) = match self.active().await {
            Ok(Some(s)) => (true, Some(s.status)),
            _ => (false, None),
        };
        serde_json::json!({
            "type": "session_state",
            "active": active,
            "status": status,
            "organising": organising,
        })
        .to_string()
    }

    /// Push the current session state to all connected WS clients. Called after
    /// every status transition (start/stop/discard/backdated/end) + organising
    /// toggle. Cheap and best-effort — ignores the error when no bridge is
    /// connected.
    async fn notify_state(&self) {
        let _ = self.ws_tx.send(self.session_state_json().await);
    }

    /// Whether organising mode is currently on (see the `organising` field).
    pub fn is_organising(&self) -> bool {
        self.organising.load(Ordering::Relaxed)
    }

    /// Enter (`on=true`) or leave (`on=false`) organising mode. Entering flips
    /// Stash's play-history tracking OFF; leaving flips it back ON. Refuses to
    /// enter while a session is active — you can't organise and track at once.
    ///
    /// The Stash write is awaited (so the navbar pill, flipped by `notify_state`
    /// below, only turns red once Stash has actually paused tracking) but
    /// best-effort: a Stash failure still flips the local flag + pill so the user
    /// isn't wedged, and is logged. The bridge runs INSIDE Stash, so if it sent
    /// this, Stash is reachable and the write normally succeeds.
    pub async fn set_organising(&self, on: bool) -> Result<()> {
        if on && self.active_id().await?.is_some() {
            return Err(anyhow!(
                "Stop the session before entering organising mode."
            ));
        }
        // on -> tracking OFF; off -> tracking ON.
        let want_track = !on;
        if let Err(e) = stash::set_track_activity(&self.pool, want_track).await {
            tracing::warn!(
                "organising={}: failed to set Stash trackActivity={}: {:#}",
                on,
                want_track,
                e
            );
        } else {
            tracing::info!("organising={}: Stash trackActivity={}", on, want_track);
        }
        self.organising.store(on, Ordering::Relaxed);
        self.notify_state().await;
        Ok(())
    }

    /// Spawn a best-effort background task that flips Stash's play-history
    /// tracking back ON if it was left off. Called on every session start so a
    /// session never tracks with it off (the invariant) — covers both "start
    /// straight out of organising mode" and "the user turned it off in Stash
    /// directly". Non-blocking: the session starts instantly and Stash heals a
    /// beat later (`trackActivity` only gates Stash's OWN recording, not Climax's
    /// crediting, so the brief lag is immaterial).
    fn ensure_tracking_on_bg(&self) {
        let pool = self.pool.clone();
        tokio::spawn(async move {
            match stash::ensure_track_activity_on(&pool).await {
                Ok(true) => tracing::info!("session start: re-enabled Stash play-history tracking"),
                Ok(false) => {}
                Err(e) => {
                    tracing::warn!("session start: could not verify Stash trackActivity: {:#}", e)
                }
            }
        });
    }

    /// Decide whether the given content_item needs a fresh Stash GraphQL
    /// fetch, and if so, kick one off in the background. Called from
    /// record_heartbeat (per sighting) and from the manual "refresh" command
    /// (which passes `force = true` to bypass the staleness check).
    ///
    /// Source filtering: only 'stash' content has metadata coming from Stash;
    /// other source kinds are skipped.
    ///
    /// On success the background task UPDATEs metadata_json + bumps
    /// metadata_fetched_at to now. Failures are logged and the next sighting
    /// will retry once the debounce expires.
    pub async fn refresh_stash_metadata(
        &self,
        content_id: i64,
        scene_id: String,
        source: &str,
        force: bool,
    ) {
        if source != "stash" {
            return;
        }

        // Staleness check (skipped when forced — manual refresh always fires).
        if !force {
            let fetched_at: Option<i64> = match sqlx::query_scalar::<_, Option<i64>>(
                "SELECT metadata_fetched_at FROM content_items WHERE id = ?1",
            )
            .bind(content_id)
            .fetch_optional(&self.pool)
            .await
            {
                Ok(Some(v)) => v,
                Ok(None) => return, // row vanished mid-call; nothing to refresh
                Err(e) => {
                    tracing::warn!("read metadata_fetched_at({}): {:#}", content_id, e);
                    return;
                }
            };
            let cfg = settings::get_metadata_refresh(&self.pool)
                .await
                .unwrap_or_default();
            let now = now_ms();
            let is_stale = match fetched_at {
                None => true,                       // never fetched: always fill the card
                Some(_) if !cfg.enabled => false,   // auto-refresh off: keep what we have
                Some(at) => (now - at) > cfg.ttl_seconds() * 1000,
            };
            if !is_stale {
                return;
            }
        }

        // Debounce against repeat attempts (applies even when force=true so
        // a frantic user can't trigger 50 fetches in a row).
        let now = now_ms();
        {
            let attempts = self.stash_metadata_attempts.read().await;
            if let Some(&last) = attempts.get(&content_id) {
                if now - last < METADATA_FETCH_DEBOUNCE_MS {
                    return;
                }
            }
        }
        {
            let mut attempts = self.stash_metadata_attempts.write().await;
            attempts.insert(content_id, now);
        }

        // Spawn the actual fetch.
        let pool = self.pool.clone();
        tokio::spawn(async move {
            match stash::fetch_scene(&pool, &scene_id).await {
                // None = Stash has no such scene (deleted, or merged away). Leave
                // the stored snapshot alone rather than overwriting it with an
                // empty one; the metadata sync is what resolves those.
                Ok(None) => {}
                Ok(Some(info)) => {
                    // Single-sourced snapshot shape (descriptive facets + Stash
                    // mirror counts) shared with the mirror reconcile, so the two
                    // write paths can't drift and a live refresh never drops the
                    // mirror count keys (o_counter / play_duration / last_played).
                    let metadata_json = info.to_metadata_json();
                    // title / thumbnail / duration use COALESCE so a transient
                    // null from Stash doesn't wipe a good value. metadata_json
                    // does NOT — when the user deletes a performer in Stash,
                    // we WANT that to take effect on Climax's side.
                    let r = sqlx::query(
                        "UPDATE content_items
                         SET title = COALESCE(?1, title),
                             thumbnail_url = COALESCE(?2, thumbnail_url),
                             duration_seconds = COALESCE(?3, duration_seconds),
                             metadata_json = ?4,
                             metadata_fetched_at = ?5
                         WHERE id = ?6",
                    )
                    .bind(info.title)
                    .bind(info.thumbnail_url)
                    .bind(info.duration_seconds)
                    .bind(metadata_json)
                    .bind(now_ms())
                    .bind(content_id)
                    .execute(&pool)
                    .await;
                    if let Err(e) = r {
                        tracing::warn!("refresh content_item {}: {:#}", content_id, e);
                    } else {
                        tracing::info!(
                            "refreshed content_item {} (scene {})",
                            content_id,
                            scene_id
                        );
                        // Duration may have just landed. Re-run the play-threshold
                        // check for any UNCOUNTED play of this scene in the LIVE
                        // session, so a scene watched past the threshold before its
                        // metadata loaded still counts — while a brief open whose
                        // duration we now know does NOT (fixes the "7s open counted"
                        // race). Scoped to active/paused sessions; cheap no-op when
                        // none is running.
                        let live_plays: Vec<i64> = sqlx::query_scalar(
                            "SELECT sp.id FROM scene_plays sp
                             JOIN sessions s ON s.id = sp.session_id
                             WHERE sp.content_item_id = ?1
                               AND sp.counted_at IS NULL
                               AND s.status IN ('active','paused')",
                        )
                        .bind(content_id)
                        .fetch_all(&pool)
                        .await
                        .unwrap_or_default();
                        for pid in live_plays {
                            if let Err(e) =
                                SessionManager::mark_counted_if_threshold(&pool, pid).await
                            {
                                tracing::warn!(
                                    "post-metadata threshold recheck failed for scene_play {}: {:#}",
                                    pid, e
                                );
                            }
                        }
                    }
                }
                Err(e) => tracing::warn!(
                    "fetch_scene {} failed (will retry after debounce): {:#}",
                    scene_id,
                    e
                ),
            }
        });
    }

    /// IDs of every stash-sourced content_item with a non-null external_id.
    /// Used by the user-initiated "force refresh from Stash" action — that
    /// path always refreshes everything regardless of TTL (the user clicked
    /// the button, they want the data refreshed).
    pub async fn all_stash_content_ids(&self) -> Result<Vec<(i64, String)>> {
        let rows: Vec<(i64, String)> = sqlx::query_as(
            "SELECT ci.id, ci.external_id
             FROM content_items ci
             JOIN sources s ON s.id = ci.source_id
             WHERE s.key = 'stash'
               AND ci.external_id IS NOT NULL",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// IDs of stash-sourced content_items whose metadata is stale or never
    /// fetched. Kept for future use (e.g. a cron-style background sweeper)
    /// but the user-facing "Refresh from Stash" button uses
    /// `all_stash_content_ids` instead — see the comment there.
    pub async fn stale_stash_content_ids(&self) -> Result<Vec<(i64, String)>> {
        let ttl_seconds = settings::get_metadata_refresh(&self.pool)
            .await
            .map(|s| s.ttl_seconds())
            .unwrap_or_else(|_| settings::MetadataRefreshSetting::default().ttl_seconds());
        // ttl_seconds <= 0 means auto-refresh disabled — but the manual
        // "refresh all stale" still works against rows that have NEVER been
        // fetched. With ttl > 0 we also include rows older than the cutoff.
        let cutoff = if ttl_seconds > 0 {
            Some(now_ms() - ttl_seconds * 1000)
        } else {
            None
        };
        let rows: Vec<(i64, String)> = match cutoff {
            Some(c) => sqlx::query_as(
                "SELECT ci.id, ci.external_id
                 FROM content_items ci
                 JOIN sources s ON s.id = ci.source_id
                 WHERE s.key = 'stash'
                   AND ci.external_id IS NOT NULL
                   AND (ci.metadata_fetched_at IS NULL OR ci.metadata_fetched_at < ?1)",
            )
            .bind(c)
            .fetch_all(&self.pool)
            .await?,
            None => sqlx::query_as(
                "SELECT ci.id, ci.external_id
                 FROM content_items ci
                 JOIN sources s ON s.id = ci.source_id
                 WHERE s.key = 'stash'
                   AND ci.external_id IS NOT NULL
                   AND ci.metadata_fetched_at IS NULL",
            )
            .fetch_all(&self.pool)
            .await?,
        };
        Ok(rows)
    }

    /// Start a new session. Returns the created session row.
    /// If there's already an active or paused session, ends it first (cleanly).
    ///
    /// The new session's `assigned_day` defaults to today's local date.
    /// (Wrap-up modal can override if the session ends up crossing midnight.)
    pub async fn start(&self) -> Result<Session> {
        // Starting a session is incompatible with organising mode: clear it and
        // make sure Stash's play-history tracking is back ON before we record
        // anything (the invariant — never track with trackActivity off).
        self.organising.store(false, Ordering::Relaxed);
        self.ensure_tracking_on_bg();

        // End any stragglers - mark as ended at their last_heartbeat or now.
        let now = now_ms();
        sqlx::query(
            "UPDATE sessions
             SET status = 'ended',
                 ended_at = COALESCE(last_heartbeat, ?1),
                 updated_at = ?1
             WHERE status IN ('active','paused')",
        )
        .bind(now)
        .execute(&self.pool)
        .await
        .context("close stragglers")?;

        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        let id = sqlx::query_scalar::<_, i64>(
            "INSERT INTO sessions
              (started_at, status, excluded, assigned_day, created_at, updated_at, last_heartbeat)
             VALUES (?1, 'active', 0, ?2, ?1, ?1, ?1)
             RETURNING id",
        )
        .bind(now)
        .bind(&today)
        .fetch_one(&self.pool)
        .await
        .context("insert session")?;

        // Snapshot Stash's per-scene play_duration so this session's TRUE share
        // of watch time is known when it ends (see stash_watch_attribution).
        // Best-effort + off the critical path: a slow or unreachable Stash must
        // never delay starting a session. A missing baseline just means the
        // rollback falls back to Climax's own tracked seconds, as before.
        Self::spawn_capture_baseline(self.pool.clone(), id);

        self.notify_state().await;
        self.load(id).await
    }

    /// Record Stash's play_duration for every already-watched scene as this
    /// session's baseline. Fire-and-forget; failures are logged, not surfaced.
    fn spawn_capture_baseline(pool: SqlitePool, session_id: i64) {
        tokio::spawn(async move {
            match stash::find_active_play_durations(&pool).await {
                Ok(rows) => {
                    let map: HashMap<String, f64> = rows.into_iter().collect();
                    let json = match serde_json::to_string(&map) {
                        Ok(j) => j,
                        Err(e) => {
                            tracing::warn!("session {}: baseline encode failed: {:#}", session_id, e);
                            return;
                        }
                    };
                    if let Err(e) =
                        sqlx::query("UPDATE sessions SET stash_pd_baseline = ?1 WHERE id = ?2")
                            .bind(&json)
                            .bind(session_id)
                            .execute(&pool)
                            .await
                    {
                        tracing::warn!("session {}: baseline store failed: {:#}", session_id, e);
                    } else {
                        tracing::info!(
                            "session {}: captured Stash watch-time baseline for {} scene(s)",
                            session_id,
                            map.len()
                        );
                    }
                }
                // Stash down / disabled: no baseline, so the delete rollback
                // falls back to SUM(seconds_tracked). Not an error worth
                // bothering the user with.
                Err(e) => tracing::info!(
                    "session {}: no Stash watch-time baseline ({:#})",
                    session_id,
                    e
                ),
            }
        });
    }

    /// Attribute each of the session's scenes its TRUE share of Stash watch
    /// time: (play_duration now) - (play_duration at session start), floored at
    /// 0. Called after a session ends. Best-effort - anything we can't attribute
    /// keeps `stash_watched_secs` NULL and falls back to the tracked seconds.
    fn spawn_attribute_watch_time(pool: SqlitePool, session_id: i64) {
        tokio::spawn(async move {
            let baseline_json: Option<String> =
                match sqlx::query_scalar("SELECT stash_pd_baseline FROM sessions WHERE id = ?1")
                    .bind(session_id)
                    .fetch_optional(&pool)
                    .await
                {
                    Ok(v) => v.flatten(),
                    Err(e) => {
                        tracing::warn!("session {}: baseline read failed: {:#}", session_id, e);
                        return;
                    }
                };
            let Some(baseline_json) = baseline_json else {
                tracing::info!(
                    "session {}: no baseline captured, watch-time rollback will use tracked seconds",
                    session_id
                );
                return;
            };
            let baseline: HashMap<String, f64> = match serde_json::from_str(&baseline_json) {
                Ok(m) => m,
                Err(e) => {
                    tracing::warn!("session {}: baseline decode failed: {:#}", session_id, e);
                    return;
                }
            };
            let now_rows = match stash::find_active_play_durations(&pool).await {
                Ok(r) => r,
                Err(e) => {
                    tracing::info!(
                        "session {}: couldn't read Stash to attribute watch time ({:#})",
                        session_id,
                        e
                    );
                    return;
                }
            };
            let now_map: HashMap<String, f64> = now_rows.into_iter().collect();

            // The session's scenes, by Stash id.
            let scenes: Vec<(i64, String)> = match sqlx::query_as(
                "SELECT sp.id, ci.external_id
                 FROM scene_plays sp
                 JOIN content_items ci ON ci.id = sp.content_item_id
                 JOIN sources s ON s.id = ci.source_id
                 WHERE sp.session_id = ?1 AND s.key = 'stash'
                   AND ci.external_id IS NOT NULL",
            )
            .bind(session_id)
            .fetch_all(&pool)
            .await
            {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!("session {}: scene lookup failed: {:#}", session_id, e);
                    return;
                }
            };

            let mut written = 0usize;
            for (play_id, external_id) in scenes {
                // Absent from the baseline = the scene had no watch time when
                // this session started, so its whole current total is ours.
                let before = baseline.get(&external_id).copied().unwrap_or(0.0);
                let Some(after) = now_map.get(&external_id).copied() else {
                    continue; // gone from Stash, or somehow zero - leave NULL
                };
                let secs = (after - before).max(0.0).round() as i64;
                if let Err(e) =
                    sqlx::query("UPDATE scene_plays SET stash_watched_secs = ?1 WHERE id = ?2")
                        .bind(secs)
                        .bind(play_id)
                        .execute(&pool)
                        .await
                {
                    tracing::warn!("scene_play {}: attribution store failed: {:#}", play_id, e);
                } else {
                    written += 1;
                }
            }
            tracing::info!(
                "session {}: attributed Stash watch time for {} scene(s)",
                session_id,
                written
            );
        });
    }

    /// Pull the Stash PLAY timestamps for a finished session's scenes into
    /// `play_imports`, so the session spine can tell one long watch from two
    /// separate ones.
    ///
    /// Stash logs a play per player load that crosses its minimum-play-percent,
    /// which is precisely "I opened this again" - a far better answer than any
    /// gap heuristic, which cannot distinguish a pause from a reload and gets it
    /// backwards when two scenes are playing at once.
    ///
    /// Why at session END rather than reading what the mirror already has: the
    /// mirror runs on a cadence (12 hours by default), so a session you just
    /// finished has none of its plays imported yet. Left to the mirror, the
    /// spine would show one card now and quietly become two tomorrow.
    ///
    /// Writes into `play_imports`, the mirror's own table, deliberately: the
    /// insert is idempotent through `UNIQUE(content_item_id, played_at)`, so the
    /// next mirror pass reconciles rather than duplicating, and
    /// `drop_imports_for_session` already removes these if the session is
    /// deleted. Best-effort - Stash being unreachable just leaves the spine
    /// drawing one card per scene.
    fn spawn_import_session_plays(pool: SqlitePool, session_id: i64) {
        tokio::spawn(async move {
            let scenes: Vec<(i64, String)> = match sqlx::query_as(
                "SELECT DISTINCT sp.content_item_id, ci.external_id
                 FROM scene_plays sp
                 JOIN content_items ci ON ci.id = sp.content_item_id
                 JOIN sources s ON s.id = ci.source_id
                 WHERE sp.session_id = ?1 AND s.key = 'stash'
                   AND ci.external_id IS NOT NULL",
            )
            .bind(session_id)
            .fetch_all(&pool)
            .await
            {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!("session {}: play-import scene lookup failed: {:#}", session_id, e);
                    return;
                }
            };
            if scenes.is_empty() {
                return;
            }

            let ids: Vec<String> = scenes.iter().map(|(_, ext)| ext.clone()).collect();
            let histories = match stash::find_play_histories(&pool, &ids).await {
                Ok(h) => h,
                Err(e) => {
                    tracing::info!(
                        "session {}: couldn't read Stash play history ({:#})",
                        session_id,
                        e
                    );
                    return;
                }
            };
            let by_ext: HashMap<String, Vec<i64>> = histories.into_iter().collect();

            let mut inserted = 0usize;
            for (content_id, ext) in scenes {
                let Some(plays) = by_ext.get(&ext) else { continue };
                for played_at in plays {
                    match sqlx::query(
                        "INSERT OR IGNORE INTO play_imports (content_item_id, played_at, created_at)
                         VALUES (?1, ?2, ?3)",
                    )
                    .bind(content_id)
                    .bind(played_at)
                    .bind(now_ms())
                    .execute(&pool)
                    .await
                    {
                        Ok(r) => inserted += r.rows_affected() as usize,
                        Err(e) => tracing::warn!(
                            "session {}: play import failed for content {}: {:#}",
                            session_id,
                            content_id,
                            e
                        ),
                    }
                }
            }
            tracing::info!(
                "session {}: imported {} new Stash play timestamp(s)",
                session_id,
                inserted
            );
        });
    }

    /// Stop the active or paused session. No-op if none.
    ///
    /// Auto-discard rule: if the session has zero counted scene_plays AND
    /// zero o_events, it's effectively empty (the user opened scenes too
    /// briefly to cross the play threshold and didn't log any cumshots), so
    /// we mark it 'discarded' rather than 'ended'. That keeps it out of
    /// every list / aggregate, matching the user's "brief opens shouldn't
    /// be logged" intent. Sessions with cumshot-only scenes still end
    /// normally — cumshot presence forces the session to count.
    ///
    /// The `keep_empty_sessions` setting switches the rule off: with it on, an
    /// empty session ends normally too, so Climax can be used as a plain timer.
    pub async fn stop(&self) -> Result<Option<Session>> {
        let active = self.active_id().await?;
        let Some(id) = active else { return Ok(None) };

        let now = now_ms();
        // If currently paused, close the open pause range at `now`.
        sqlx::query(
            "UPDATE session_pauses
             SET resumed_at = ?1
             WHERE session_id = ?2 AND resumed_at IS NULL",
        )
        .bind(now)
        .bind(id)
        .execute(&self.pool)
        .await?;

        // Did anything actually happen in this session?
        let has_content: i64 = sqlx::query_scalar(
            "SELECT CASE WHEN EXISTS (
                 SELECT 1 FROM scene_plays
                 WHERE session_id = ?1 AND counted_at IS NOT NULL
             ) OR EXISTS (
                 SELECT 1 FROM o_events WHERE session_id = ?1
             ) THEN 1 ELSE 0 END",
        )
        .bind(id)
        .fetch_one(&self.pool)
        .await?;
        // A failed settings read falls back to the default (discard), the
        // behaviour every install had before the setting existed.
        let keep_empty = crate::settings::get_keep_empty_sessions(&self.pool)
            .await
            .unwrap_or(false);
        let new_status = if has_content != 0 || keep_empty { "ended" } else { "discarded" };

        sqlx::query(
            "UPDATE sessions
             SET status = ?1, ended_at = ?2, updated_at = ?2
             WHERE id = ?3",
        )
        .bind(new_status)
        .bind(now)
        .bind(id)
        .execute(&self.pool)
        .await?;

        if new_status == "discarded" {
            tracing::info!(
                "session {} auto-discarded on stop (no counted scenes, no cumshots)",
                id
            );
        }

        // Now that watching has stopped, work out how much of Stash's watch time
        // this session was actually responsible for, so a later delete can roll
        // back the real amount instead of Climax's sampled approximation.
        Self::spawn_attribute_watch_time(self.pool.clone(), id);
        // And how many separate PLAYS Stash logged, which is what decides
        // whether the spine draws a scene once or twice.
        Self::spawn_import_session_plays(self.pool.clone(), id);

        self.notify_state().await;
        Ok(Some(self.load(id).await?))
    }

    /// Discard the active or paused session. Like stop, but sets status to
    /// 'discarded' instead of 'ended'. The row + its scene_plays + o_events
    /// stay in the DB but the dashboard's `status != 'discarded'` filters
    /// hide them from every aggregate. Reversible by flipping the status back.
    ///
    /// Stash rollback: discarding throws the session away, so the cumshots
    /// Climax *pushed* to Stash get un-pushed — every climax-origin, already-
    /// synced o_event in the session fires a sceneDecrementO. Stash-origin O's
    /// are left alone: the user logged those in Stash deliberately and they're
    /// their own record, independent of this Climax session. Rolled-back events
    /// are marked stash_synced=0 so Climax's state reflects "no longer in
    /// Stash" (re-syncing on un-discard is a separate, deferred concern).
    /// No-op if no active session.
    pub async fn discard(&self) -> Result<Option<Session>> {
        let active = self.active_id().await?;
        let Some(id) = active else { return Ok(None) };

        let now = now_ms();
        sqlx::query(
            "UPDATE session_pauses
             SET resumed_at = ?1
             WHERE session_id = ?2 AND resumed_at IS NULL",
        )
        .bind(now)
        .bind(id)
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "UPDATE sessions
             SET status = 'discarded', ended_at = ?1, updated_at = ?1
             WHERE id = ?2",
        )
        .bind(now)
        .bind(id)
        .execute(&self.pool)
        .await?;

        // Roll back the Stash counter for cumshots Climax pushed this session.
        let to_rollback: Vec<(i64, String)> = sqlx::query_as(
            "SELECT oe.id, c.external_id
             FROM o_events oe
             JOIN content_items c ON c.id = oe.content_item_id
             JOIN sources s ON s.id = c.source_id
             WHERE oe.session_id = ?1
               AND oe.origin = 'climax'
               AND oe.stash_synced = 1
               AND s.key = 'stash'
               AND c.external_id IS NOT NULL",
        )
        .bind(id)
        .fetch_all(&self.pool)
        .await?;

        if !to_rollback.is_empty() {
            // Mark unsynced up front — Climax's record now reflects "not in Stash".
            sqlx::query(
                "UPDATE o_events SET stash_synced = 0
                 WHERE session_id = ?1 AND origin = 'climax' AND stash_synced = 1",
            )
            .bind(id)
            .execute(&self.pool)
            .await?;

            // Fire the decrements in the background so discard returns instantly.
            let pool = self.pool.clone();
            tokio::spawn(async move {
                for (oid, external_id) in to_rollback {
                    match stash::remove_o(&pool, &external_id).await {
                        Ok(count) => tracing::info!(
                            "discard rollback: o_event {} scene {} decremented, o_counter now {}",
                            oid, external_id, count
                        ),
                        Err(e) => tracing::warn!(
                            "discard rollback failed for o_event {} scene {}: {:#}",
                            oid, external_id, e
                        ),
                    }
                }
            });
        }

        self.notify_state().await;
        Ok(Some(self.load(id).await?))
    }

    /// Pause the active session. No-op if not active.
    pub async fn pause(&self, reason: &str) -> Result<Option<Session>> {
        let active = self.active_id().await?;
        let Some(id) = active else { return Ok(None) };

        let row: Option<String> =
            sqlx::query_scalar("SELECT status FROM sessions WHERE id = ?1")
                .bind(id)
                .fetch_optional(&self.pool)
                .await?;
        if row.as_deref() != Some("active") {
            return Ok(Some(self.load(id).await?));
        }

        let now = now_ms();
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "UPDATE sessions SET status = 'paused', updated_at = ?1 WHERE id = ?2",
        )
        .bind(now)
        .bind(id)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO session_pauses (session_id, paused_at, reason) VALUES (?1, ?2, ?3)",
        )
        .bind(id)
        .bind(now)
        .bind(reason)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;

        // Pause/resume are status transitions like any other, so push the new
        // state to WS clients - otherwise the bridge's navbar pill keeps showing
        // "tracking" through a pause (it only ever learned about start/stop).
        self.notify_state().await;
        Ok(Some(self.load(id).await?))
    }

    /// Resume a paused session.
    pub async fn resume(&self) -> Result<Option<Session>> {
        let active = self.active_id().await?;
        let Some(id) = active else { return Ok(None) };

        let row: Option<String> =
            sqlx::query_scalar("SELECT status FROM sessions WHERE id = ?1")
                .bind(id)
                .fetch_optional(&self.pool)
                .await?;
        if row.as_deref() != Some("paused") {
            return Ok(Some(self.load(id).await?));
        }

        let now = now_ms();
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "UPDATE session_pauses
             SET resumed_at = ?1
             WHERE session_id = ?2 AND resumed_at IS NULL",
        )
        .bind(now)
        .bind(id)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE sessions SET status = 'active', updated_at = ?1 WHERE id = ?2",
        )
        .bind(now)
        .bind(id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;

        self.notify_state().await;
        Ok(Some(self.load(id).await?))
    }

    /// Preview reopening a session (the Sessions-page "Reopen"): can this one be
    /// made live again, and how long ago did it end.
    pub async fn reopen_check(&self, session_id: i64) -> Result<ReopenCheck> {
        let no = ReopenCheck { can_reopen: false, gap_ms: 0 };
        // Never two live sessions at once - the whole app assumes one.
        if self.active_id().await?.is_some() {
            return Ok(no);
        }
        let row: Option<(String, Option<i64>)> =
            sqlx::query_as("SELECT status, ended_at FROM sessions WHERE id = ?1")
                .bind(session_id)
                .fetch_optional(&self.pool)
                .await?;
        // Must be a real ENDED session. Discarded ones are deliberately excluded:
        // they're filtered out of the Sessions list, so there'd be no way to see
        // or select what you were reopening.
        let Some((status, Some(ended_at))) = row else {
            return Ok(no);
        };
        if status != "ended" {
            return Ok(no);
        }
        // Only the LATEST session can be reopened. Reopening an older one would
        // reopen a window that later sessions already sit inside, so its end
        // could swallow them.
        let latest: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM sessions WHERE status != 'discarded'
             ORDER BY started_at DESC, id DESC LIMIT 1",
        )
        .fetch_optional(&self.pool)
        .await?;
        if latest != Some(session_id) {
            return Ok(no);
        }
        Ok(ReopenCheck {
            can_reopen: true,
            gap_ms: (now_ms() - ended_at).max(0),
        })
    }

    /// Reopen an ended session: it goes ACTIVE again and keeps tracking live -
    /// the inverse of `stop()`, for "I stopped that by accident" or a glitch.
    /// Distinct from `resume()`, which un-pauses a PAUSED session.
    ///
    /// `gap_as_pause` makes the time since it ended a pause (excluded from
    /// session time) rather than counted - the same choice merge and continue
    /// offer. Pure-local: nothing is pushed to Stash, since nothing about the
    /// recorded activity changes.
    ///
    /// Returns None (a no-op) when `reopen_check` wouldn't allow it.
    pub async fn reopen(&self, session_id: i64, gap_as_pause: bool) -> Result<Option<Session>> {
        // Re-validate rather than trust the caller: the check ran when the row
        // was selected, and a session could have started since.
        if !self.reopen_check(session_id).await?.can_reopen {
            return Ok(None);
        }
        let now = now_ms();
        let old_end: i64 = sqlx::query_scalar("SELECT ended_at FROM sessions WHERE id = ?1")
            .bind(session_id)
            .fetch_one(&self.pool)
            .await?;

        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "UPDATE sessions SET status = 'active', ended_at = NULL, updated_at = ?1
             WHERE id = ?2",
        )
        .bind(now)
        .bind(session_id)
        .execute(&mut *tx)
        .await?;
        if gap_as_pause && now > old_end {
            sqlx::query(
                "INSERT INTO session_pauses (session_id, paused_at, resumed_at, reason)
                 VALUES (?1, ?2, ?3, 'manual')",
            )
            .bind(session_id)
            .bind(old_end)
            .bind(now)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;

        // The pill / tray / dashboard all need to know a session is live again.
        self.notify_state().await;
        Ok(Some(self.load(session_id).await?))
    }

    /// Returns the id of the current active-or-paused session, if any.
    pub async fn active_id(&self) -> Result<Option<i64>> {
        let id: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM sessions WHERE status IN ('active','paused') ORDER BY id DESC LIMIT 1",
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok(id)
    }

    pub async fn active(&self) -> Result<Option<Session>> {
        let Some(id) = self.active_id().await? else {
            return Ok(None);
        };
        Ok(Some(self.load(id).await?))
    }

    pub async fn list(&self, limit: i64, offset: i64) -> Result<Vec<Session>> {
        // Hide discarded sessions — they intentionally disappear from every
        // user-facing list (dashboard, day strips, sidebar). Reachable via
        // direct DB inspection if ever needed.
        let ids: Vec<i64> = sqlx::query_scalar(
            "SELECT id FROM sessions
             WHERE status != 'discarded'
             ORDER BY started_at DESC LIMIT ?1 OFFSET ?2",
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?;
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            out.push(self.load(id).await?);
        }
        Ok(out)
    }

    /// One page of sessions for the dashboard Sessions table — filtered by
    /// assigned_day range, sorted by a chosen column, paginated. Returns the
    /// page rows plus the TOTAL matching count (for pagination), so sort holds
    /// across pages (SQLite does the ORDER BY/LIMIT, not the client).
    ///
    /// Like `list()`, it selects ids then reuses `load()` for each so the
    /// `Session` shape stays single-sourced. The ORDER BY expression is chosen
    /// from a fixed enum (never raw user input); the duration variant embeds the
    /// server-computed `now` as an integer literal (also not user input), which
    /// keeps the bound params to just the WHERE range + limit/offset.
    pub async fn list_page(
        &self,
        start_day: &str,
        end_day: &str,
        sort: SessionSort,
        desc: bool,
        limit: i64,
        offset: i64,
    ) -> Result<(Vec<Session>, i64)> {
        let now = now_ms();
        // ORDER BY fragment mirrors load()'s computed fields so the sorted
        // value equals the displayed one. All fragments are constant strings
        // keyed off the SessionSort enum — no injection surface.
        let order_expr = match sort {
            SessionSort::Start => "s.started_at".to_string(),
            SessionSort::Duration => format!(
                "((COALESCE(s.ended_at, {now}) - s.started_at) - \
                  (SELECT COALESCE(SUM(MAX(0, \
                     MIN(COALESCE(p.resumed_at, {now}), COALESCE(s.ended_at, {now})) \
                     - MAX(p.paused_at, s.started_at))), 0) \
                   FROM session_pauses p WHERE p.session_id = s.id))"
            ),
            SessionSort::Scenes => "(SELECT COUNT(*) FROM scene_plays sp \
                 WHERE sp.session_id = s.id \
                   AND (sp.counted_at IS NOT NULL \
                        OR EXISTS (SELECT 1 FROM o_events oe \
                                   WHERE oe.session_id = sp.session_id \
                                     AND oe.content_item_id = sp.content_item_id)))"
                .to_string(),
            SessionSort::Cumshots =>
                "(SELECT COUNT(*) FROM o_events oe WHERE oe.session_id = s.id)".to_string(),
        };
        let dir = if desc { "DESC" } else { "ASC" };
        let sql = format!(
            "SELECT s.id FROM sessions s \
             WHERE s.status != 'discarded' \
               AND s.assigned_day >= ?1 AND s.assigned_day <= ?2 \
             ORDER BY {order_expr} {dir}, s.started_at DESC, s.id DESC \
             LIMIT ?3 OFFSET ?4"
        );
        let ids: Vec<i64> = sqlx::query_scalar(&sql)
            .bind(start_day)
            .bind(end_day)
            .bind(limit)
            .bind(offset)
            .fetch_all(&self.pool)
            .await?;

        let total: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sessions s \
             WHERE s.status != 'discarded' \
               AND s.assigned_day >= ?1 AND s.assigned_day <= ?2",
        )
        .bind(start_day)
        .bind(end_day)
        .fetch_one(&self.pool)
        .await?;

        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            out.push(self.load(id).await?);
        }
        Ok((out, total))
    }

    /// Load a single session with computed fields.
    /// Fetch a single session by id (any status), or None if it doesn't exist.
    /// Direct lookup that reuses `load()` for an identical Session shape —
    /// replaces SessionDetail's old "list 200 and filter client-side" path,
    /// which silently broke once a user had more than 200 sessions.
    pub async fn get(&self, id: i64) -> Result<Option<Session>> {
        let exists: Option<i64> = sqlx::query_scalar("SELECT id FROM sessions WHERE id = ?1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        match exists {
            Some(_) => Ok(Some(self.load(id).await?)),
            None => Ok(None),
        }
    }

    pub async fn load(&self, id: i64) -> Result<Session> {
        let row = sqlx::query_as::<_, (
            i64, i64, Option<i64>, Option<i64>, String, Option<String>, i64, Option<String>, Option<String>, i64,
        )>(
            "SELECT id, started_at, ended_at, last_heartbeat, status, notes, excluded, session_type, assigned_day, estimated
             FROM sessions WHERE id = ?1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| anyhow!("session {} not found", id))?;

        let now = now_ms();
        let end = row.2.unwrap_or(now);
        let elapsed = (end - row.1).max(0);

        // Sum paused time, CLIPPED to the session window [started_at, end]. Only the
        // portion of each pause that falls inside the window counts: a pause entirely
        // outside contributes 0, a partial one only its overlap. Without this, a
        // pause left dangling outside the window (e.g. an earlier merge's gap-pause
        // after the start was trimmed forward, or pauses accumulated across merges)
        // was subtracted in FULL and could exceed the span -> effective duration 0.
        let paused_total: i64 = sqlx::query_scalar(
            "SELECT COALESCE(SUM(
                MAX(0, MIN(COALESCE(resumed_at, ?1), ?2) - MAX(paused_at, ?3))
             ), 0)
             FROM session_pauses
             WHERE session_id = ?4",
        )
        .bind(now)
        .bind(end)
        .bind(row.1)
        .bind(id)
        .fetch_one(&self.pool)
        .await?;

        // "scene_count" reflects what the user perceives as part of the
        // session — counted plays (≥ threshold) plus any scene that has a
        // cumshot logged on it (cumshots force a scene into the session
        // regardless of watch time, per the user's design).
        let scene_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM scene_plays sp
             WHERE sp.session_id = ?1
               AND (
                 sp.counted_at IS NOT NULL
                 OR EXISTS (
                   SELECT 1 FROM o_events oe
                   WHERE oe.session_id = sp.session_id
                     AND oe.content_item_id = sp.content_item_id
                 )
               )",
        )
        .bind(id)
        .fetch_one(&self.pool)
        .await?;

        let o_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM o_events WHERE session_id = ?1",
        )
        .bind(id)
        .fetch_one(&self.pool)
        .await?;

        // assigned_day should always be set after the migration, but be defensive:
        // fall back to local-date-of-started_at if somehow null.
        let assigned_day = row.8.unwrap_or_else(|| {
            chrono::DateTime::<chrono::Local>::from(
                chrono::DateTime::<chrono::Utc>::from_timestamp_millis(row.1).unwrap_or_default()
            )
            .format("%Y-%m-%d")
            .to_string()
        });

        Ok(Session {
            id: row.0,
            started_at: row.1,
            ended_at: row.2,
            last_heartbeat: row.3,
            status: row.4,
            notes: row.5,
            excluded: row.6 != 0,
            session_type: row.7,
            estimated: row.9 != 0,
            assigned_day,
            effective_duration_ms: (elapsed - paused_total).max(0),
            scene_count,
            o_count,
        })
    }

    /// Stamp `scene_plays.counted_at` if this play has just crossed the
    /// configured threshold. Idempotent — does nothing if already counted.
    /// Called from the heartbeat path right after a successful
    /// `seconds_tracked` increment. Cheap (one SELECT + maybe one UPDATE).
    async fn maybe_mark_counted(&self, play_id: i64) -> Result<()> {
        Self::mark_counted_if_threshold(&self.pool, play_id).await
    }

    /// Threshold-counting core, callable with just a pool so the background
    /// metadata-refresh task can re-run it the moment a scene's duration lands.
    /// Idempotent — no-op if already counted.
    ///
    /// Unknown duration → do NOT count. We can't measure the threshold without a
    /// duration, and the old "count immediately when duration is unknown"
    /// fallback over-counted brief opens: a new scene's first heartbeat fires
    /// this check before its Stash metadata (duration) has loaded, so a 7-second
    /// tab-open on a 30-minute scene counted as a play. `refresh_stash_metadata`
    /// now re-runs this for the live session's plays once the duration is known,
    /// so a genuinely-watched scene still counts and a quick open never does.
    /// (A scene whose duration never loads — Stash down / deleted — simply isn't
    /// counted, since it can't be measured; the honest trade for not over-counting.)
    async fn mark_counted_if_threshold(pool: &SqlitePool, play_id: i64) -> Result<()> {
        let row: Option<(Option<i64>, i64, Option<i64>)> = sqlx::query_as(
            "SELECT sp.counted_at, sp.seconds_tracked, ci.duration_seconds
             FROM scene_plays sp
             JOIN content_items ci ON ci.id = sp.content_item_id
             WHERE sp.id = ?1",
        )
        .bind(play_id)
        .fetch_optional(pool)
        .await?;
        let Some((counted_at, secs, duration)) = row else { return Ok(()); };
        if counted_at.is_some() {
            return Ok(()); // sticky — already counted in this session
        }

        let setting = crate::settings::get_play_counting(pool)
            .await
            .unwrap_or_default();
        let frac = crate::settings::effective_play_threshold_frac(&setting);

        // Cumulative seconds_tracked vs. duration × pct. Unknown duration → do
        // NOT count yet; the metadata-refresh task re-checks once it lands.
        let should_count = match duration {
            Some(d) if d > 0 => (secs as f32) >= (d as f32) * frac,
            _ => false,
        };
        if should_count {
            sqlx::query("UPDATE scene_plays SET counted_at = ?1 WHERE id = ?2")
                .bind(now_ms())
                .bind(play_id)
                .execute(pool)
                .await?;
            tracing::info!(
                "scene_play {} crossed play threshold ({}s of {:?}s @ {}%)",
                play_id, secs, duration, frac * 100.0
            );
        }
        Ok(())
    }

    /// Record `seconds` of playback that finished at `at` as a watch run on
    /// `play_id` — either extending the run still in progress or opening a new
    /// one once the gap exceeds this source's reporting cadence.
    ///
    /// Called from the same place that credits `seconds_tracked`, and with the
    /// same number, so the runs can never drift from the total they decompose.
    ///
    /// A new run STARTS at `at - seconds`, i.e. the credit is laid back over the
    /// time it represents rather than forward from now, so an uninterrupted
    /// watch draws a band the same length as the time it credits. That start is
    /// clamped past the previous run's end: an off-bridge poll can credit a big
    /// delta at once (a missed interval, or the tail flush when a session ends),
    /// and without the clamp such a run would reach back over the one before it.
    async fn record_watch_run(
        pool: &SqlitePool,
        play_id: i64,
        at: i64,
        seconds: i64,
        off_bridge: bool,
    ) -> Result<()> {
        if seconds <= 0 {
            return Ok(());
        }
        let gap_ms = if off_bridge { OFF_BRIDGE_RUN_GAP_MS } else { BRIDGE_RUN_GAP_MS };
        let last: Option<(i64, i64)> = sqlx::query_as(
            "SELECT id, ended_at FROM scene_play_runs
             WHERE play_id = ?1 ORDER BY ended_at DESC LIMIT 1",
        )
        .bind(play_id)
        .fetch_optional(pool)
        .await?;

        match last {
            // Still the same stretch of watching - grow it. MAX() guards against
            // an out-of-order stamp shortening a run.
            Some((run_id, ended_at)) if at - ended_at <= gap_ms => {
                sqlx::query(
                    "UPDATE scene_play_runs
                     SET ended_at = MAX(ended_at, ?1),
                         seconds_tracked = seconds_tracked + ?2
                     WHERE id = ?3",
                )
                .bind(at)
                .bind(seconds)
                .bind(run_id)
                .execute(pool)
                .await?;
            }
            // First run for this play, or playback resumed after a real break.
            _ => {
                let floor = last.map(|(_, end)| end).unwrap_or(i64::MIN);
                let started_at = (at - seconds * 1000).clamp(floor, at);
                sqlx::query(
                    "INSERT INTO scene_play_runs
                        (play_id, started_at, ended_at, seconds_tracked, off_bridge)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                )
                .bind(play_id)
                .bind(started_at)
                .bind(at)
                .bind(seconds)
                .bind(off_bridge as i64)
                .execute(pool)
                .await?;
            }
        }
        Ok(())
    }

    /// Garbage-collect the tab heartbeat cache. Called occasionally so the
    /// cache doesn't grow indefinitely if tabs disconnect without notice.
    /// (We used to also auto-pause/resume the session based on aggregate video
    /// state here, but the user model is: session timer ticks from start to
    /// stop regardless of what's playing. Only manual pause + later AFK detection
    /// should pause the session.)
    pub async fn prune_tab_cache(&self) {
        let now = now_ms();
        let mut tabs = self.tab_states.write().await;
        tabs.retain(|_, hb| now - hb.last_seen <= TAB_PRUNE_MS);
    }

    /// Heartbeat from the Stash bridge plugin.
    /// - Upserts the content item for (source=stash, scene_id).
    /// - If a session is active (status='active'), upserts the scene_play row.
    /// - Updates session.last_heartbeat.
    /// - Auto-pauses/resumes the session based on video state.
    pub async fn record_heartbeat(&self, hb: HeartbeatPayload) -> Result<HeartbeatRecord> {
        let now = now_ms();

        // Look up the source by key.
        let source_id: i64 = sqlx::query_scalar(
            "SELECT id FROM sources WHERE key = ?1",
        )
        .bind(&hb.source)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| anyhow!("unknown source key: {}", hb.source))?;

        // Upsert the content item.
        let content_id: i64 = match sqlx::query_scalar::<_, i64>(
            "SELECT id FROM content_items WHERE source_id = ?1 AND external_id = ?2",
        )
        .bind(source_id)
        .bind(&hb.scene_id)
        .fetch_optional(&self.pool)
        .await?
        {
            Some(id) => {
                // Just bump last_seen_at. Title / url / duration / metadata
                // are owned by Climax via Stash GraphQL now — refreshed below
                // when stale. The bridge no longer authoritatively writes
                // catalog fields.
                sqlx::query(
                    "UPDATE content_items SET last_seen_at = ?1 WHERE id = ?2",
                )
                .bind(now)
                .bind(id)
                .execute(&self.pool)
                .await?;
                id
            }
            None => {
                // Brand-new content_item. We deliberately don't write title /
                // url / duration_seconds from the bridge payload anymore — all
                // catalog data now comes from Stash GraphQL via the refresh
                // call below. The row is created with bare bones (source +
                // external_id + timestamps) and filled in once the GraphQL
                // round-trip completes a moment later.
                let id = sqlx::query_scalar::<_, i64>(
                    "INSERT INTO content_items
                      (source_id, external_id, first_seen_at, last_seen_at)
                     VALUES (?1, ?2, ?3, ?3)
                     RETURNING id",
                )
                .bind(source_id)
                .bind(&hb.scene_id)
                .bind(now)
                .fetch_one(&self.pool)
                .await?;
                id
            }
        };

        // Whether the row was just created or already existed, kick the
        // refresh helper. For brand-new rows metadata_fetched_at is NULL
        // so this always fires. For existing rows it fires only when the
        // TTL has expired or metadata_fetched_at is somehow still NULL.
        // The helper is fully async + debounced so we don't block here.
        self.refresh_stash_metadata(content_id, hb.scene_id.clone(), &hb.source, false)
            .await;

        // Find current active-or-paused session (we want to record against
        // either, but only credit play time when status=active).
        let session_id: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM sessions WHERE status IN ('active','paused') ORDER BY id DESC LIMIT 1",
        )
        .fetch_optional(&self.pool)
        .await?;

        let mut recorded = false;
        if let Some(sid) = session_id {
            // Read the PREVIOUS heartbeat for this tab BEFORE we overwrite it.
            let prev = {
                let tabs = self.tab_states.read().await;
                tabs.get(&hb.tab_id).cloned()
            };

            // SOURCE OF TRUTH: did video.currentTime actually advance since the
            // previous heartbeat? video.paused can lie (background tabs, plugins
            // wrapping pause), but currentTime is authoritative.
            let video_delta_sec: i64 = match prev.as_ref() {
                Some(p) if p.content_item_id == content_id && p.session_id == sid => {
                    let raw = hb.video.current_time - p.current_time;
                    // Clamp: 0 = no playback (paused/buffering/stalled).
                    // 30s cap = sanity guard against currentTime jumps (seeking).
                    if raw > 0.0 && raw < 30.0 {
                        raw.round() as i64
                    } else if raw >= 30.0 {
                        // User seeked forward; don't credit the seek as playtime,
                        // but also don't pretend no time passed - cap at the
                        // wall-clock delta to be safe.
                        let wall_delta = ((now - p.last_seen) / 1000).clamp(0, 30);
                        wall_delta.min(raw.round() as i64)
                    } else {
                        // raw <= 0: paused, stalled, or seeked backward.
                        0
                    }
                }
                _ => 0, // First heartbeat for this tab/scene - no delta to credit.
            };

            match sqlx::query_scalar::<_, i64>(
                "SELECT id FROM scene_plays WHERE session_id = ?1 AND content_item_id = ?2",
            )
            .bind(sid)
            .bind(content_id)
            .fetch_optional(&self.pool)
            .await?
            {
                Some(play_id) => {
                    if video_delta_sec > 0 {
                        // last_advance_at = now: the video actually moved this
                        // heartbeat. Drives the "currently watching" visibility
                        // in the tracker / Overview (now authoritative backend
                        // state, not a client-observed delta).
                        sqlx::query(
                            "UPDATE scene_plays
                             SET last_seen_at = ?1,
                                 last_advance_at = ?1,
                                 seconds_tracked = seconds_tracked + ?2,
                                 off_bridge = 0
                             WHERE id = ?3",
                        )
                        .bind(now)
                        .bind(video_delta_sec)
                        .bind(play_id)
                        .execute(&self.pool)
                        .await?;

                        // Same credit, recorded as WHEN rather than how much, so
                        // the session spine can draw the real stretches of
                        // playback instead of one block from first_seen_at.
                        if let Err(e) =
                            Self::record_watch_run(&self.pool, play_id, now, video_delta_sec, false)
                                .await
                        {
                            tracing::warn!(
                                "watch-run record failed for scene_play {}: {:#}",
                                play_id,
                                e
                            );
                        }

                        // Threshold check — only meaningful when we actually
                        // credited playtime this heartbeat. Read the scene's
                        // duration and the new seconds_tracked; if it just
                        // crossed (counted_at IS NULL && tracked >= dur * pct),
                        // stamp counted_at = now. Sticky once set.
                        if let Err(e) = self.maybe_mark_counted(play_id).await {
                            tracing::warn!(
                                "play-threshold check failed for scene_play {}: {:#}",
                                play_id, e
                            );
                        }
                    } else {
                        // Even a paused bridge heartbeat means the bridge is on this
                        // scene, so it's not off-bridge - clear the flag.
                        sqlx::query(
                            "UPDATE scene_plays SET last_seen_at = ?1, off_bridge = 0 WHERE id = ?2",
                        )
                        .bind(now)
                        .bind(play_id)
                        .execute(&self.pool)
                        .await?;
                    }
                }
                None => {
                    sqlx::query(
                        "INSERT INTO scene_plays
                            (session_id, content_item_id, first_seen_at, last_seen_at, seconds_tracked)
                         VALUES (?1, ?2, ?3, ?3, 0)",
                    )
                    .bind(sid)
                    .bind(content_id)
                    .bind(now)
                    .execute(&self.pool)
                    .await?;
                }
            }

            sqlx::query(
                "UPDATE sessions SET last_heartbeat = ?1, updated_at = ?1 WHERE id = ?2",
            )
            .bind(now)
            .bind(sid)
            .execute(&self.pool)
            .await?;
            recorded = true;

            // Update the tab cache. last_advance_at advances only when video
            // actually moved (video_delta_sec > 0); otherwise carry forward
            // the previous value (or `now` for a first heartbeat).
            let last_advance_at = if video_delta_sec > 0 {
                now
            } else {
                prev.as_ref().map(|p| p.last_advance_at).unwrap_or(0)
            };

            if video_delta_sec > 0 {
                tracing::info!(
                    "+{}s scene_play (tab={} scene={} ct: {:.1} -> {:.1})",
                    video_delta_sec, hb.tab_id, hb.scene_id,
                    prev.as_ref().map(|p| p.current_time).unwrap_or(0.0),
                    hb.video.current_time
                );
            } else if prev.is_some() {
                tracing::info!(
                    "0s scene_play (tab={} scene={} ct unchanged at {:.1}, bridge state={})",
                    hb.tab_id, hb.scene_id, hb.video.current_time, hb.video.state
                );
            }

            let mut tabs = self.tab_states.write().await;
            tabs.insert(
                hb.tab_id.clone(),
                TabHeartbeat {
                    session_id: sid,
                    content_item_id: content_id,
                    video_state: hb.video.state.clone(),
                    current_time: hb.video.current_time,
                    last_seen: now,
                    last_advance_at,
                },
            );
        }

        // Passive-capture tracking. With a session active we don't need it (and
        // we drop any stale watch so a future prompt backdates fresh); with NO
        // session we record advancing playback so capture.rs can detect
        // "watched a while without tracking".
        if session_id.is_some() {
            self.no_session_watch.write().await.clear();
        } else {
            self.track_no_session_watch(&hb, content_id).await;
        }

        Ok(HeartbeatRecord {
            session_id,
            content_item_id: content_id,
            recorded,
        })
    }

    /// Record advancing playback while NO session is active. capture.rs polls
    /// `find_capture_candidate` over this to decide when to nudge. Same
    /// currentTime-advance gate as the session heartbeat path.
    async fn track_no_session_watch(&self, hb: &HeartbeatPayload, content_id: i64) {
        let now = now_ms();
        let mut map = self.no_session_watch.write().await;
        let prev = map.get(&hb.tab_id).cloned();
        let same_scene = prev.as_ref().map(|p| p.content_item_id == content_id).unwrap_or(false);
        let advanced = same_scene
            && {
                let d = hb.video.current_time - prev.as_ref().unwrap().current_time;
                d > 0.0 && d < 30.0
            };
        // started_at = when this continuous watch began. New scene → now. Same
        // scene advancing within a minute of the last advance → keep the origin.
        // Advancing after a long pause → treat as a fresh watch. Paused → hold.
        let started_at = if !same_scene {
            now
        } else {
            let p = prev.as_ref().unwrap();
            if advanced && (now - p.last_advance_at) <= 60_000 {
                p.started_at
            } else if advanced {
                now
            } else {
                p.started_at
            }
        };
        let last_advance_at = if advanced {
            now
        } else {
            prev.as_ref().map(|p| p.last_advance_at).unwrap_or(0)
        };
        map.insert(
            hb.tab_id.clone(),
            NoSessionWatch {
                content_item_id: content_id,
                scene_id: hb.scene_id.clone(),
                source: hb.source.clone(),
                current_time: hb.video.current_time,
                started_at,
                last_seen: now,
                last_advance_at,
            },
        );
        map.retain(|_, w| now - w.last_seen <= TAB_PRUNE_MS);
    }

    /// Find a stash scene that's been actively watched (no session) for at least
    /// `threshold_ms` and is still playing (advanced within `fresh_ms`). Returns
    /// (source, scene_id, content_item_id, watch_started_at) for capture.rs.
    pub async fn find_capture_candidate(
        &self,
        threshold_ms: i64,
        fresh_ms: i64,
    ) -> Option<(String, String, i64, i64)> {
        let now = now_ms();
        let map = self.no_session_watch.read().await;
        for w in map.values() {
            if w.source == "stash"
                && now - w.last_advance_at <= fresh_ms
                && w.last_advance_at - w.started_at >= threshold_ms
            {
                return Some((w.source.clone(), w.scene_id.clone(), w.content_item_id, w.started_at));
            }
        }
        None
    }

    /// Re-baseline the no-session watch (used on snooze, so the same scene must
    /// be watched afresh before the prompt can re-fire).
    pub async fn clear_no_session_watch(&self) {
        self.no_session_watch.write().await.clear();
    }

    /// Start a session BACKDATED to `started_at`, seeded with the scene the user
    /// was already watching (passive-capture accept path). The already-watched
    /// span (`started_at` → now) is credited up front; ongoing bridge heartbeats
    /// add to it. Closes any straggler session first, like `start`.
    pub async fn start_backdated_with_scene(
        &self,
        started_at: i64,
        content_item_id: i64,
    ) -> Result<Session> {
        // Same invariant as start(): accepting a passive-capture prompt makes a
        // live session, so leave organising mode + re-enable Stash tracking.
        self.organising.store(false, Ordering::Relaxed);
        self.ensure_tracking_on_bg();

        let now = now_ms();
        sqlx::query(
            "UPDATE sessions
             SET status = 'ended', ended_at = COALESCE(last_heartbeat, ?1), updated_at = ?1
             WHERE status IN ('active','paused')",
        )
        .bind(now)
        .execute(&self.pool)
        .await
        .context("close stragglers")?;

        let assigned_day = chrono::DateTime::<chrono::Local>::from(
            chrono::DateTime::<chrono::Utc>::from_timestamp_millis(started_at).unwrap_or_default(),
        )
        .format("%Y-%m-%d")
        .to_string();

        let id = sqlx::query_scalar::<_, i64>(
            "INSERT INTO sessions
               (started_at, status, excluded, assigned_day, created_at, updated_at, last_heartbeat)
             VALUES (?1, 'active', 0, ?2, ?3, ?3, ?3)
             RETURNING id",
        )
        .bind(started_at)
        .bind(&assigned_day)
        .bind(now)
        .fetch_one(&self.pool)
        .await
        .context("insert backdated session")?;

        let watched = ((now - started_at) / 1000).max(0);
        let play_id = sqlx::query_scalar::<_, i64>(
            "INSERT INTO scene_plays
                (session_id, content_item_id, first_seen_at, last_seen_at, seconds_tracked)
             VALUES (?1, ?2, ?3, ?4, ?5)
             RETURNING id",
        )
        .bind(id)
        .bind(content_item_id)
        .bind(started_at)
        .bind(now)
        .bind(watched)
        .fetch_one(&self.pool)
        .await
        .context("seed backdated scene_play")?;
        if let Err(e) = self.maybe_mark_counted(play_id).await {
            tracing::warn!(
                "backdated play-threshold check failed for scene_play {}: {:#}",
                play_id, e
            );
        }

        // A session is live now — drop the no-session tracking.
        self.no_session_watch.write().await.clear();

        self.notify_state().await;
        self.load(id).await
    }

    /// External (Stash) scene ids the LOCAL bridge is actively reporting right
    /// now — i.e. tabs whose last heartbeat landed within `within_ms`. The
    /// remote-playback poller (remote.rs) excludes these so it never double-
    /// counts a scene the bridge already credits (Stash's play_duration advances
    /// for local web-player playback too, not just remote devices). All
    /// tab_states entries are real bridge tabs (the remote poller writes its
    /// own scene_plays directly and never touches this cache).
    pub async fn live_tracked_external_ids(&self, within_ms: i64) -> Result<HashSet<String>> {
        let now = now_ms();
        let content_ids: Vec<i64> = {
            let tabs = self.tab_states.read().await;
            tabs.values()
                .filter(|hb| now - hb.last_seen <= within_ms)
                .map(|hb| hb.content_item_id)
                .collect()
        };
        let mut out: HashSet<String> = HashSet::new();
        if content_ids.is_empty() {
            return Ok(out);
        }
        // ids come from our own in-memory cache (never user input), so building
        // the IN list by hand is safe.
        let placeholders = content_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT external_id FROM content_items \
             WHERE external_id IS NOT NULL AND id IN ({placeholders})"
        );
        let mut q = sqlx::query_scalar::<_, String>(&sql);
        for id in &content_ids {
            q = q.bind(id);
        }
        for ext in q.fetch_all(&self.pool).await? {
            out.insert(ext);
        }
        Ok(out)
    }

    /// Credit watch time for a scene detected as playing on a REMOTE device
    /// (e.g. the Android TV app) by the Stash play_duration poller (remote.rs),
    /// against a SPECIFIC session. The poller passes the live active session for
    /// per-tick crediting, and the just-ended session for the final tail flush
    /// at stop (so the last interval between the final poll and Stop isn't lost).
    ///
    /// Unlike `record_heartbeat` — which derives the watched delta from a
    /// per-tab `video.currentTime` and dedups via `tab_states` — the poller has
    /// already computed the exact watched seconds from Stash's monotonic
    /// `play_duration` and already excluded scenes the local bridge owns. So
    /// this is a direct credit: find-or-create the stash content_item, enrich it
    /// (so the tracker card gets a title/thumb), upsert its `scene_play`, add the
    /// seconds, and run the play-count threshold check. It does NOT write to
    /// `tab_states` (that cache belongs to the bridge).
    ///
    /// Skips a discarded / deleted session (never resurrects one). The play's
    /// `last_seen_at` is clamped into the session window, so a tail credit landing
    /// just after `ended_at` can't push the scene_play outside its bounds.
    /// Returns the content_item id, or None when the session is gone/discarded.
    pub async fn credit_remote_play(
        &self,
        session_id: i64,
        scene_id: &str,
        delta_secs: i64,
    ) -> Result<Option<i64>> {
        let row: Option<(i64, Option<i64>, String)> = sqlx::query_as(
            "SELECT started_at, ended_at, status FROM sessions WHERE id = ?1",
        )
        .bind(session_id)
        .fetch_optional(&self.pool)
        .await?;
        let Some((started_at, ended_at, status)) = row else {
            return Ok(None); // session deleted out from under us
        };
        if status == "discarded" {
            return Ok(None);
        }
        let now = now_ms();
        // Clamp the play timestamp into [started_at, ended_at]. Matters for the
        // tail flush, which runs a tick or two AFTER ended_at.
        let stamp = match ended_at {
            Some(end) => now.min(end).max(started_at),
            None => now.max(started_at),
        };
        let credit = delta_secs.max(0);

        // Find-or-create the content_item and kick metadata enrichment (async +
        // debounced; fills the card's title/thumbnail/performers a moment later).
        let content_id = self.ensure_content_item("stash", scene_id).await?;
        self.refresh_stash_metadata(content_id, scene_id.to_string(), "stash", false)
            .await;

        match sqlx::query_scalar::<_, i64>(
            "SELECT id FROM scene_plays WHERE session_id = ?1 AND content_item_id = ?2",
        )
        .bind(session_id)
        .bind(content_id)
        .fetch_optional(&self.pool)
        .await?
        {
            Some(play_id) => {
                if credit > 0 {
                    // last_advance_at = stamp: remote playback advanced (e.g. the
                    // TV app), same "currently watching" signal as a local
                    // heartbeat. Clamped into the session window like last_seen_at.
                    sqlx::query(
                        "UPDATE scene_plays
                         SET last_seen_at = ?1, last_advance_at = ?1, seconds_tracked = seconds_tracked + ?2
                         WHERE id = ?3",
                    )
                    .bind(stamp)
                    .bind(credit)
                    .bind(play_id)
                    .execute(&self.pool)
                    .await?;
                    if let Err(e) =
                        Self::record_watch_run(&self.pool, play_id, stamp, credit, true).await
                    {
                        tracing::warn!(
                            "remote watch-run record failed for scene_play {}: {:#}",
                            play_id,
                            e
                        );
                    }
                    if let Err(e) = self.maybe_mark_counted(play_id).await {
                        tracing::warn!(
                            "remote play-threshold check failed for scene_play {}: {:#}",
                            play_id,
                            e
                        );
                    }
                } else {
                    sqlx::query("UPDATE scene_plays SET last_seen_at = ?1 WHERE id = ?2")
                        .bind(stamp)
                        .bind(play_id)
                        .execute(&self.pool)
                        .await?;
                }
            }
            None => {
                // A brand-new remote row that already carries credit was advancing
                // when first seen, so stamp last_advance_at; a zero-credit sighting
                // leaves it NULL (not yet "playing").
                let adv: Option<i64> = if credit > 0 { Some(stamp) } else { None };
                // off_bridge = 1: this scene_play was born from the poll path, i.e.
                // playback the bridge didn't report (a TV / phone app). Cleared to
                // 0 if a bridge heartbeat ever credits this same scene.
                let play_id = sqlx::query_scalar::<_, i64>(
                    "INSERT INTO scene_plays
                        (session_id, content_item_id, first_seen_at, last_seen_at, seconds_tracked, last_advance_at, off_bridge)
                     VALUES (?1, ?2, ?3, ?3, ?4, ?5, 1)
                     RETURNING id",
                )
                .bind(session_id)
                .bind(content_id)
                .bind(stamp)
                .bind(credit)
                .bind(adv)
                .fetch_one(&self.pool)
                .await?;
                if credit > 0 {
                    if let Err(e) =
                        Self::record_watch_run(&self.pool, play_id, stamp, credit, true).await
                    {
                        tracing::warn!(
                            "remote watch-run record failed for scene_play {}: {:#}",
                            play_id,
                            e
                        );
                    }
                    if let Err(e) = self.maybe_mark_counted(play_id).await {
                        tracing::warn!(
                            "remote play-threshold check failed for scene_play {}: {:#}",
                            play_id,
                            e
                        );
                    }
                }
            }
        }

        // Keep an active session marked alive, like a bridge heartbeat would.
        // (Skip for an already-ended session being tail-flushed.)
        if ended_at.is_none() {
            sqlx::query("UPDATE sessions SET last_heartbeat = ?1, updated_at = ?1 WHERE id = ?2")
                .bind(now)
                .bind(session_id)
                .execute(&self.pool)
                .await?;
        }

        Ok(Some(content_id))
    }

    /// Validate that a timestamp falls within a session's bounds.
    /// Returns Ok if valid, Err with a user-readable message otherwise.
    /// If the session has no ended_at (still running), only the start is enforced.
    async fn ensure_in_session_bounds(&self, session_id: i64, at_ms: i64) -> Result<()> {
        let row: Option<(i64, Option<i64>)> = sqlx::query_as(
            "SELECT started_at, ended_at FROM sessions WHERE id = ?1",
        )
        .bind(session_id)
        .fetch_optional(&self.pool)
        .await?;
        let Some((started_at, ended_at)) = row else {
            tracing::warn!("ensure_in_session_bounds: session {} not found", session_id);
            return Err(anyhow!("That session no longer exists."));
        };
        if at_ms < started_at {
            return Err(anyhow!(
                "Time is before the session's start. Move the session start earlier first, or pick a later time."
            ));
        }
        if let Some(end) = ended_at {
            if at_ms > end {
                return Err(anyhow!(
                    "Time is after the session's end. Move the session end later first, or pick an earlier time."
                ));
            }
        }
        Ok(())
    }

    /// Check that proposed session time bounds don't orphan existing cumshots
    /// or scene_plays. Returns a user-readable error if they would.
    async fn validate_session_time_change(
        &self,
        session_id: i64,
        new_started: Option<i64>,
        new_ended: Option<i64>,
    ) -> Result<()> {
        // Pull current values to fill in unchanged fields.
        let row: Option<(i64, Option<i64>)> = sqlx::query_as(
            "SELECT started_at, ended_at FROM sessions WHERE id = ?1",
        )
        .bind(session_id)
        .fetch_optional(&self.pool)
        .await?;
        let Some((cur_start, cur_end)) = row else {
            tracing::warn!("validate_session_time_change: session {} not found", session_id);
            return Err(anyhow!("That session no longer exists."));
        };

        let start = new_started.unwrap_or(cur_start);
        let end = new_ended.or(cur_end);

        if let Some(e) = end {
            if start > e {
                return Err(anyhow!("Start time can't be after end time."));
            }
        }

        // Any cumshot outside the proposed window?
        let oob_cumshot: Option<i64> = sqlx::query_scalar(
            "SELECT occurred_at FROM o_events
             WHERE session_id = ?1
               AND (occurred_at < ?2 OR (?3 IS NOT NULL AND occurred_at > ?3))
             ORDER BY occurred_at LIMIT 1",
        )
        .bind(session_id)
        .bind(start)
        .bind(end)
        .fetch_optional(&self.pool)
        .await?;
        if let Some(t) = oob_cumshot {
            // Local time, matching every other time the app shows the user.
            let when = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(t)
                .map(|d| d.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M:%S").to_string())
                .unwrap_or_else(|| t.to_string());
            return Err(anyhow!(
                "A cumshot at {} would fall outside the new session times. Delete or move it first.",
                when
            ));
        }

        // Any scene_play outside the proposed window?
        let oob_play: Option<i64> = sqlx::query_scalar(
            "SELECT first_seen_at FROM scene_plays
             WHERE session_id = ?1
               AND (first_seen_at < ?2 OR (?3 IS NOT NULL AND last_seen_at > ?3))
             ORDER BY first_seen_at LIMIT 1",
        )
        .bind(session_id)
        .bind(start)
        .bind(end)
        .fetch_optional(&self.pool)
        .await?;
        if oob_play.is_some() {
            return Err(anyhow!(
                "A watched scene would fall outside the new session times. Trim the session less aggressively."
            ));
        }

        Ok(())
    }

    /// Log a single O event.
    ///
    /// `origin` is one of 'climax' | 'stash' | 'manual'. It controls whether
    /// the Climax UI can later delete this event AND whether we write back to
    /// Stash. Stash-origin events are recorded as already-synced (Stash itself
    /// already incremented its counter).
    pub async fn log_o(
        &self,
        session_id: Option<i64>,
        content_item_id: Option<i64>,
        occurred_at: Option<i64>,
        intensity: Option<i64>,
        notes: Option<String>,
        origin: &str,
    ) -> Result<OEvent> {
        let skip_stash_sync = origin == "stash";
        let now = now_ms();
        let occurred = occurred_at.unwrap_or(now);
        let initial_synced: i64 = if skip_stash_sync { 1 } else { 0 };

        // Enforce session bounds when both a session and a timestamp are set.
        if let Some(sid) = session_id {
            if occurred_at.is_some() {
                self.ensure_in_session_bounds(sid, occurred).await?;
            }
        }

        let id: i64 = sqlx::query_scalar(
            "INSERT INTO o_events
                (session_id, content_item_id, occurred_at, intensity, notes, stash_synced, origin, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             RETURNING id",
        )
        .bind(session_id)
        .bind(content_item_id)
        .bind(occurred)
        .bind(intensity)
        .bind(&notes)
        .bind(initial_synced)
        .bind(origin)
        .bind(now)
        .fetch_one(&self.pool)
        .await?;

        if !skip_stash_sync {
            if let Some(cid) = content_item_id {
                let row: Option<(String, Option<String>)> = sqlx::query_as(
                    "SELECT s.key, c.external_id
                     FROM content_items c JOIN sources s ON s.id = c.source_id
                     WHERE c.id = ?1",
                )
                .bind(cid)
                .fetch_optional(&self.pool)
                .await?;

                if let Some((src_key, Some(external_id))) = row {
                    if src_key == "stash" {
                        let pool = self.pool.clone();
                        let scene_id = external_id;
                        let at = occurred;
                        tokio::spawn(async move {
                            match stash::add_o(&pool, &scene_id, at).await {
                                Ok(count) => {
                                    tracing::info!(
                                        "stash sync: scene {} o_counter now {}",
                                        scene_id, count
                                    );
                                    let _ = sqlx::query(
                                        "UPDATE o_events SET stash_synced = 1 WHERE id = ?1",
                                    )
                                    .bind(id)
                                    .execute(&pool)
                                    .await;
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        "stash sync failed for o_event {} scene {}: {:#}",
                                        id, scene_id, e
                                    );
                                }
                            }
                        });
                    }
                }
            }
        }

        Ok(OEvent {
            id,
            session_id,
            content_item_id,
            occurred_at: occurred,
            intensity,
            notes,
            stash_synced: skip_stash_sync,
            origin: origin.to_string(),
        })
    }

    /// Fire the best-effort Stash rollback for a set of scenes, in the
    /// background (so the caller returns instantly). For each scene this undoes
    /// exactly what the deleted session/scene contributed to Stash: pop N O's
    /// from o_history (`remove_o`), pop M plays from play_history
    /// (`remove_play`), and subtract the watched seconds from `play_duration`
    /// (`reduce_play_duration`). Failures are logged, not surfaced — the local
    /// Climax delete is authoritative and Stash can be reconciled later (the
    /// Phase 7 mirror-sync re-reads Stash truth and heals any residual).
    fn spawn_stash_rollback(&self, specs: Vec<SceneRollback>) {
        if specs.is_empty() {
            return;
        }
        let pool = self.pool.clone();
        tokio::spawn(async move {
            for sp in specs {
                // Cumshots: remove the exact O-history entries inside the
                // session window (not the most recent).
                match stash::remove_os_in_window(&pool, &sp.external_id, sp.window_start, sp.window_end).await {
                    Ok(n) if n > 0 => tracing::info!(
                        "delete rollback: scene {} removed {} O-history entr{} in window",
                        sp.external_id, n, if n == 1 { "y" } else { "ies" }
                    ),
                    Ok(_) => {}
                    Err(e) => tracing::warn!(
                        "delete rollback (O) failed scene {}: {:#}", sp.external_id, e
                    ),
                }
                // Plays: remove the exact play-history entries inside the window.
                match stash::remove_plays_in_window(&pool, &sp.external_id, sp.window_start, sp.window_end).await {
                    Ok(n) if n > 0 => tracing::info!(
                        "delete rollback: scene {} removed {} play-history entr{} in window",
                        sp.external_id, n, if n == 1 { "y" } else { "ies" }
                    ),
                    Ok(_) => {}
                    Err(e) => tracing::warn!(
                        "delete rollback (play) failed scene {}: {:#}", sp.external_id, e
                    ),
                }
                // Watch time: subtract this session's tracked seconds.
                if sp.watched_secs > 0 {
                    match stash::reduce_play_duration(&pool, &sp.external_id, sp.watched_secs).await {
                        Ok(d) => tracing::info!(
                            "delete rollback: scene {} play_duration now {:.0}s",
                            sp.external_id, d
                        ),
                        Err(e) => tracing::warn!(
                            "delete rollback (duration) failed scene {}: {:#}",
                            sp.external_id, e
                        ),
                    }
                }
            }
        });
    }

    /// Gather the per-scene Stash-rollback specs for an entire session, BEFORE
    /// the rows are deleted. Each spec carries the session's time window — the
    /// rollback removes the O- and play-history entries Stash recorded inside
    /// it (the ones this session contributed), and subtracts the watched
    /// seconds from `play_duration`. Only stash-sourced scenes with an
    /// external_id are included; a scene is included if it has a scene_play OR a
    /// synced O in the session (so cumshot-only scenes still get rolled back).
    async fn capture_session_rollback(&self, session_id: i64) -> Result<Vec<SceneRollback>> {
        use std::collections::HashMap;

        // Session window — every play/O this session logged falls inside it.
        let win: Option<(i64, Option<i64>)> = sqlx::query_as(
            "SELECT started_at, ended_at FROM sessions WHERE id = ?1",
        )
        .bind(session_id)
        .fetch_optional(&self.pool)
        .await?;
        let Some((start, ended)) = win else { return Ok(vec![]); };
        let end = ended.unwrap_or_else(now_ms);

        // external_id -> watched seconds (0 for cumshot-only scenes).
        let mut watched: HashMap<String, i64> = HashMap::new();

        // Prefer the TRUE Stash share (captured at stop from the play_duration
        // delta) over Climax's own sampled seconds. They differ because Stash
        // counts continuously while the bridge samples every ~5s, and
        // subtracting the smaller sampled figure used to strand the difference
        // on the scene forever. COALESCE per row, so a session that has the
        // attribution for some scenes and not others still gets the best number
        // available for each.
        let plays: Vec<(String, i64)> = sqlx::query_as(
            "SELECT ci.external_id,
                    COALESCE(SUM(COALESCE(sp.stash_watched_secs, sp.seconds_tracked)), 0) AS watched
             FROM scene_plays sp
             JOIN content_items ci ON ci.id = sp.content_item_id
             JOIN sources s ON s.id = ci.source_id
             WHERE sp.session_id = ?1 AND s.key = 'stash' AND ci.external_id IS NOT NULL
             GROUP BY ci.external_id",
        )
        .bind(session_id)
        .fetch_all(&self.pool)
        .await?;
        for (external_id, w) in plays {
            watched.insert(external_id, w);
        }

        // Scenes with a synced O but no scene_play still need O rollback.
        let o_scenes: Vec<(String,)> = sqlx::query_as(
            "SELECT DISTINCT ci.external_id
             FROM o_events oe
             JOIN content_items ci ON ci.id = oe.content_item_id
             JOIN sources s ON s.id = ci.source_id
             WHERE oe.session_id = ?1 AND oe.stash_synced = 1
               AND s.key = 'stash' AND ci.external_id IS NOT NULL",
        )
        .bind(session_id)
        .fetch_all(&self.pool)
        .await?;
        for (external_id,) in o_scenes {
            watched.entry(external_id).or_insert(0);
        }

        Ok(watched
            .into_iter()
            .map(|(external_id, watched_secs)| SceneRollback {
                external_id,
                watched_secs,
                window_start: start,
                window_end: end,
            })
            .collect())
    }

    /// Delete a session and roll its footprint back out of Stash for 1:1 parity.
    /// Captures the per-scene rollback amounts first (the rows vanish on
    /// cascade), explicitly deletes the session's o_events (so cumshots are
    /// truly gone from Climax reports, not left orphaned by `ON DELETE SET
    /// NULL`), deletes the session (cascading scene_plays + session_pauses),
    /// then fires the Stash rollback in the background.
    pub async fn delete_session(&self, session_id: i64) -> Result<()> {
        let specs = self.capture_session_rollback(session_id).await?;

        let mut tx = self.pool.begin().await?;
        drop_imports_for_session(&mut tx, session_id).await?;
        sqlx::query("DELETE FROM o_events WHERE session_id = ?1")
            .bind(session_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM sessions WHERE id = ?1")
            .bind(session_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;

        self.spawn_stash_rollback(specs);
        Ok(())
    }

    /// Bulk delete sessions, each with the same capture + Stash rollback as
    /// `delete_session`. Returns the number of sessions removed.
    pub async fn delete_sessions_many(&self, ids: &[i64]) -> Result<u64> {
        let mut total: u64 = 0;
        for id in ids {
            // Per-session so each gets its rollback captured before its rows go.
            let before = self.capture_session_rollback(*id).await?;
            let mut tx = self.pool.begin().await?;
            drop_imports_for_session(&mut tx, *id).await?;
            sqlx::query("DELETE FROM o_events WHERE session_id = ?1")
                .bind(id)
                .execute(&mut *tx)
                .await?;
            let r = sqlx::query("DELETE FROM sessions WHERE id = ?1")
                .bind(id)
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
            if r.rows_affected() > 0 {
                total += r.rows_affected();
                self.spawn_stash_rollback(before);
            }
        }
        Ok(total)
    }

    /// Validate two sessions for a merge, ordered by started_at. The INNER Err is
    /// a user-facing reason (shown in the UI); the OUTER Err is a real DB error.
    async fn merge_facts(&self, a_id: i64, b_id: i64) -> Result<std::result::Result<MergeData, String>> {
        if a_id == b_id {
            return Ok(Err("Pick two different sessions.".into()));
        }
        let rows: Vec<(i64, i64, Option<i64>, String)> = sqlx::query_as(
            "SELECT id, started_at, ended_at, status FROM sessions WHERE id IN (?1, ?2)",
        )
        .bind(a_id)
        .bind(b_id)
        .fetch_all(&self.pool)
        .await?;
        if rows.len() != 2 {
            return Ok(Err("One of the sessions no longer exists.".into()));
        }
        for r in &rows {
            if r.3 == "discarded" {
                return Ok(Err("Can't merge a discarded session.".into()));
            }
            if r.2.is_none() {
                return Ok(Err("Stop a session before merging it.".into()));
            }
        }
        let mut v = rows;
        v.sort_by_key(|r| r.1);
        let first_id = v[0].0;
        let first_start = v[0].1;
        let first_end = v[0].2.unwrap();
        let second_id = v[1].0;
        let second_start = v[1].1;
        let second_end = v[1].2.unwrap();
        // Consecutive: no non-discarded session starts strictly between them.
        let between: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sessions
             WHERE status != 'discarded' AND id NOT IN (?1, ?2)
               AND started_at > ?3 AND started_at < ?4",
        )
        .bind(first_id)
        .bind(second_id)
        .bind(first_start)
        .bind(second_start)
        .fetch_one(&self.pool)
        .await?;
        if between > 0 {
            return Ok(Err(
                "Those sessions aren't consecutive - there's another session between them.".into(),
            ));
        }
        Ok(Ok(MergeData {
            first_id,
            first_end,
            second_id,
            second_start,
            gap_ms: (second_start - first_end).max(0),
            merged_start: first_start,
            merged_end: first_end.max(second_end),
        }))
    }

    /// Preview a merge of two sessions (for the UI: enable the action + show the gap).
    pub async fn merge_check(&self, a_id: i64, b_id: i64) -> Result<MergeCheck> {
        Ok(match self.merge_facts(a_id, b_id).await? {
            Err(reason) => MergeCheck {
                ok: false,
                reason: Some(reason),
                gap_ms: 0,
                merged_start: 0,
                merged_end: 0,
            },
            Ok(m) => MergeCheck {
                ok: true,
                reason: None,
                gap_ms: m.gap_ms,
                merged_start: m.merged_start,
                merged_end: m.merged_end,
            },
        })
    }

    /// Merge two consecutive sessions into one spanning the earlier's start to the
    /// later's end. The later session's scenes/cumshots/pauses move onto the
    /// earlier; the later session is deleted. When there's a time gap and
    /// `gap_as_pause` is set, the gap becomes a pause so session time stays
    /// accurate (otherwise the gap counts as session time). Returns the surviving
    /// (earlier) session id. Pure-local - no Stash rollback (nothing is removed).
    pub async fn merge_sessions(&self, a_id: i64, b_id: i64, gap_as_pause: bool) -> Result<i64> {
        let m = match self.merge_facts(a_id, b_id).await? {
            Err(reason) => return Err(anyhow!("{}", reason)),
            Ok(m) => m,
        };
        let now = now_ms();
        let mut tx = self.pool.begin().await?;

        // A real gap -> pause on the survivor, so its duration excludes the gap.
        if gap_as_pause && m.gap_ms > 0 {
            sqlx::query(
                "INSERT INTO session_pauses (session_id, paused_at, resumed_at, reason)
                 VALUES (?1, ?2, ?3, 'manual')",
            )
            .bind(m.first_id)
            .bind(m.first_end)
            .bind(m.second_start)
            .execute(&mut *tx)
            .await?;
        }

        // Move the later session's pauses + cumshots onto the survivor.
        sqlx::query("UPDATE session_pauses SET session_id = ?1 WHERE session_id = ?2")
            .bind(m.first_id)
            .bind(m.second_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE o_events SET session_id = ?1 WHERE session_id = ?2")
            .bind(m.first_id)
            .bind(m.second_id)
            .execute(&mut *tx)
            .await?;

        // Merge scene_plays: upsert each of the later's rows into the survivor
        // (combining a scene played in both), then drop the later's rows.
        let plays: Vec<(i64, i64, i64, i64, i64, Option<i64>)> = sqlx::query_as(
            "SELECT id, content_item_id, first_seen_at, last_seen_at, seconds_tracked, counted_at
             FROM scene_plays WHERE session_id = ?1",
        )
        .bind(m.second_id)
        .fetch_all(&mut *tx)
        .await?;
        for (old_play_id, cid, first, last, secs, counted) in plays {
            sqlx::query(
                "INSERT INTO scene_plays
                    (session_id, content_item_id, first_seen_at, last_seen_at, seconds_tracked, counted_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(session_id, content_item_id) DO UPDATE SET
                    first_seen_at = MIN(scene_plays.first_seen_at, excluded.first_seen_at),
                    last_seen_at = MAX(scene_plays.last_seen_at, excluded.last_seen_at),
                    seconds_tracked = scene_plays.seconds_tracked + excluded.seconds_tracked,
                    counted_at = COALESCE(scene_plays.counted_at, excluded.counted_at)",
            )
            .bind(m.first_id)
            .bind(cid)
            .bind(first)
            .bind(last)
            .bind(secs)
            .bind(counted)
            .execute(&mut *tx)
            .await?;

            // Watch runs describe real playback, so they follow the row they
            // belong to. This MUST happen before the later session's plays are
            // deleted below, or the CASCADE takes the runs with them and the
            // merged session loses the detail of everything it absorbed.
            let survivor_play_id: i64 = sqlx::query_scalar(
                "SELECT id FROM scene_plays WHERE session_id = ?1 AND content_item_id = ?2",
            )
            .bind(m.first_id)
            .bind(cid)
            .fetch_one(&mut *tx)
            .await?;
            sqlx::query("UPDATE scene_play_runs SET play_id = ?1 WHERE play_id = ?2")
                .bind(survivor_play_id)
                .bind(old_play_id)
                .execute(&mut *tx)
                .await?;
        }
        sqlx::query("DELETE FROM scene_plays WHERE session_id = ?1")
            .bind(m.second_id)
            .execute(&mut *tx)
            .await?;

        // Extend the survivor's span + delete the later session. A merge that
        // pulls in reconstructed data flags the survivor estimated too (so the
        // best-guess nature follows the merged result).
        sqlx::query(
            "UPDATE sessions SET started_at = ?1, ended_at = ?2, updated_at = ?3,
                estimated = MAX(estimated, (SELECT estimated FROM sessions WHERE id = ?5))
             WHERE id = ?4",
        )
            .bind(m.merged_start)
            .bind(m.merged_end)
            .bind(now)
            .bind(m.first_id)
            .bind(m.second_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM sessions WHERE id = ?1")
            .bind(m.second_id)
            .execute(&mut *tx)
            .await?;

        tx.commit().await?;
        Ok(m.first_id)
    }

    /// Remove a single scene from a session (the "a stray tab logged scenes I
    /// wasn't watching" case) without touching the rest of the session. Pulls
    /// the scene's watch time + counted play out of the session AND rolls the
    /// same back out of Stash; if the scene had cumshots in this session, those
    /// go too (the scene is leaving the session entirely). Captures the
    /// rollback amounts first, then deletes the o_events + scene_play rows for
    /// (session, content_item), then fires the background Stash rollback.
    pub async fn delete_scene_from_session(
        &self,
        session_id: i64,
        content_item_id: i64,
    ) -> Result<()> {
        // Session window for precise (in-window) history rollback.
        let win: Option<(i64, Option<i64>)> = sqlx::query_as(
            "SELECT started_at, ended_at FROM sessions WHERE id = ?1",
        )
        .bind(session_id)
        .fetch_optional(&self.pool)
        .await?;

        // This scene's stash external_id + this session's watched seconds on it.
        let scene: Option<(Option<String>, String)> = sqlx::query_as(
            "SELECT ci.external_id, s.key
             FROM content_items ci JOIN sources s ON s.id = ci.source_id
             WHERE ci.id = ?1",
        )
        .bind(content_item_id)
        .fetch_optional(&self.pool)
        .await?;
        // Same preference as the whole-session rollback: the TRUE Stash share
        // when we captured it, else Climax's sampled seconds.
        let watched: i64 = sqlx::query_scalar(
            "SELECT COALESCE(SUM(COALESCE(stash_watched_secs, seconds_tracked)), 0)
             FROM scene_plays
             WHERE session_id = ?1 AND content_item_id = ?2",
        )
        .bind(session_id)
        .bind(content_item_id)
        .fetch_one(&self.pool)
        .await?;

        let spec = match (win, scene) {
            (Some((start, ended)), Some((Some(external_id), src))) if src == "stash" => {
                Some(SceneRollback {
                    external_id,
                    watched_secs: watched,
                    window_start: start,
                    window_end: ended.unwrap_or_else(now_ms),
                })
            }
            _ => None,
        };

        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM o_events WHERE session_id = ?1 AND content_item_id = ?2")
            .bind(session_id)
            .bind(content_item_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM scene_plays WHERE session_id = ?1 AND content_item_id = ?2")
            .bind(session_id)
            .bind(content_item_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;

        if let Some(spec) = spec {
            self.spawn_stash_rollback(vec![spec]);
        }
        Ok(())
    }

    /// Non-destructive "remove from THIS session": detach the scene without
    /// rolling it back out of Stash, so it stays recoverable / re-homeable
    /// elsewhere. The cumshots are KEPT (session_id cleared -> session-less
    /// imports; stash_synced unchanged so the mirror still dedups them); the
    /// scene_play is dropped (the Stash play_history mirror / play_imports survive
    /// for reconstruction); and the scene + its cumshots are skipped for THIS
    /// session so the now-uncounted history doesn't immediately bounce back into
    /// the "not counted in this session" review. NO Stash mutation. To turn the
    /// detached activity into its own / another session: trim this session's
    /// window off it (so it uncovers) and use Untracked Sessions.
    pub async fn detach_scene_from_session(
        &self,
        session_id: i64,
        content_item_id: i64,
    ) -> Result<()> {
        let now = now_ms();
        // The cumshots we're about to detach (skip them so they don't re-surface).
        let o_ids: Vec<i64> = sqlx::query_scalar(
            "SELECT id FROM o_events WHERE session_id = ?1 AND content_item_id = ?2",
        )
        .bind(session_id)
        .bind(content_item_id)
        .fetch_all(&self.pool)
        .await?;

        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "UPDATE o_events SET session_id = NULL WHERE session_id = ?1 AND content_item_id = ?2",
        )
        .bind(session_id)
        .bind(content_item_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query("DELETE FROM scene_plays WHERE session_id = ?1 AND content_item_id = ?2")
            .bind(session_id)
            .bind(content_item_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "INSERT OR IGNORE INTO session_history_dismissed (session_id, kind, ref_id, dismissed_at)
             VALUES (?1, 'play', ?2, ?3)",
        )
        .bind(session_id)
        .bind(content_item_id)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        for oid in &o_ids {
            sqlx::query(
                "INSERT OR IGNORE INTO session_history_dismissed (session_id, kind, ref_id, dismissed_at)
                 VALUES (?1, 'o', ?2, ?3)",
            )
            .bind(session_id)
            .bind(oid)
            .bind(now)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Update editable fields on a session. Each Option<T> is "leave alone" if None,
    /// "set to this value" if Some. For nullable string fields, pass Some("") to clear.
    pub async fn update_session(
        &self,
        id: i64,
        started_at: Option<i64>,
        ended_at: Option<i64>,
        notes: Option<String>,
        excluded: Option<bool>,
        session_type: Option<String>,
    ) -> Result<Session> {
        // If start/end is changing, ensure no cumshots or scene_plays are orphaned.
        if started_at.is_some() || ended_at.is_some() {
            self.validate_session_time_change(id, started_at, ended_at).await?;
        }
        let now = now_ms();
        let mut tx = self.pool.begin().await?;

        if let Some(t) = started_at {
            sqlx::query("UPDATE sessions SET started_at = ?1, updated_at = ?2 WHERE id = ?3")
                .bind(t).bind(now).bind(id)
                .execute(&mut *tx).await?;
        }
        if let Some(t) = ended_at {
            sqlx::query("UPDATE sessions SET ended_at = ?1, updated_at = ?2 WHERE id = ?3")
                .bind(t).bind(now).bind(id)
                .execute(&mut *tx).await?;
        }
        if let Some(n) = notes {
            let cleaned: Option<String> = if n.trim().is_empty() { None } else { Some(n) };
            sqlx::query("UPDATE sessions SET notes = ?1, updated_at = ?2 WHERE id = ?3")
                .bind(cleaned).bind(now).bind(id)
                .execute(&mut *tx).await?;
        }
        if let Some(e) = excluded {
            sqlx::query("UPDATE sessions SET excluded = ?1, updated_at = ?2 WHERE id = ?3")
                .bind(if e { 1 } else { 0 }).bind(now).bind(id)
                .execute(&mut *tx).await?;
        }
        if let Some(st) = session_type {
            let cleaned: Option<String> = if st.trim().is_empty() { None } else { Some(st) };
            sqlx::query("UPDATE sessions SET session_type = ?1, updated_at = ?2 WHERE id = ?3")
                .bind(cleaned).bind(now).bind(id)
                .execute(&mut *tx).await?;
        }

        tx.commit().await?;
        self.load(id).await
    }

    /// Move an o_event to a different timestamp and/or scene.
    pub async fn update_o_event(
        &self,
        o_id: i64,
        occurred_at: Option<i64>,
        content_item_id: Option<i64>,
        notes: Option<String>,
        intensity: Option<i64>,
    ) -> Result<OEvent> {
        // Bounds check: if changing the time AND the event is in a session.
        if let Some(t) = occurred_at {
            let session_id: Option<Option<i64>> = sqlx::query_scalar(
                "SELECT session_id FROM o_events WHERE id = ?1",
            )
            .bind(o_id)
            .fetch_optional(&self.pool)
            .await?;
            if let Some(Some(sid)) = session_id {
                self.ensure_in_session_bounds(sid, t).await?;
            }
        }

        let mut tx = self.pool.begin().await?;
        if let Some(t) = occurred_at {
            sqlx::query("UPDATE o_events SET occurred_at = ?1 WHERE id = ?2")
                .bind(t).bind(o_id).execute(&mut *tx).await?;
        }
        if let Some(ci) = content_item_id {
            sqlx::query("UPDATE o_events SET content_item_id = ?1 WHERE id = ?2")
                .bind(ci).bind(o_id).execute(&mut *tx).await?;
        }
        if let Some(n) = notes {
            let cleaned: Option<String> = if n.trim().is_empty() { None } else { Some(n) };
            sqlx::query("UPDATE o_events SET notes = ?1 WHERE id = ?2")
                .bind(cleaned).bind(o_id).execute(&mut *tx).await?;
        }
        if let Some(i) = intensity {
            sqlx::query("UPDATE o_events SET intensity = ?1 WHERE id = ?2")
                .bind(i).bind(o_id).execute(&mut *tx).await?;
        }
        tx.commit().await?;

        let row = sqlx::query_as::<_, (i64, Option<i64>, Option<i64>, i64, Option<i64>, Option<String>, i64, String)>(
            "SELECT id, session_id, content_item_id, occurred_at, intensity, notes, stash_synced, origin
             FROM o_events WHERE id = ?1",
        )
        .bind(o_id)
        .fetch_one(&self.pool)
        .await?;

        Ok(OEvent {
            id: row.0,
            session_id: row.1,
            content_item_id: row.2,
            occurred_at: row.3,
            intensity: row.4,
            notes: row.5,
            stash_synced: row.6 != 0,
            origin: row.7,
        })
    }

    /// Look up or create a content_item by (source_key, external_id).
    /// Returns the content_item_id. For Stash-sourced items, ALSO kicks off
    /// a background metadata refresh if the row is new or stale — same
    /// helper that record_heartbeat uses.
    pub async fn ensure_content_item(
        &self,
        source_key: &str,
        external_id: &str,
    ) -> Result<i64> {
        let now = now_ms();
        let source_id: i64 = sqlx::query_scalar(
            "SELECT id FROM sources WHERE key = ?1",
        )
        .bind(source_key)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| anyhow!("unknown source key: {}", source_key))?;

        let id = if let Some(existing) = sqlx::query_scalar::<_, i64>(
            "SELECT id FROM content_items WHERE source_id = ?1 AND external_id = ?2",
        )
        .bind(source_id)
        .bind(external_id)
        .fetch_optional(&self.pool)
        .await?
        {
            sqlx::query(
                "UPDATE content_items SET last_seen_at = ?1 WHERE id = ?2",
            )
            .bind(now)
            .bind(existing)
            .execute(&self.pool)
            .await?;
            existing
        } else {
            sqlx::query_scalar::<_, i64>(
                "INSERT INTO content_items
                    (source_id, external_id, first_seen_at, last_seen_at)
                 VALUES (?1, ?2, ?3, ?3)
                 RETURNING id",
            )
            .bind(source_id)
            .bind(external_id)
            .bind(now)
            .fetch_one(&self.pool)
            .await?
        };

        // Keep catalog metadata fresh (Stash sources only; the helper is
        // a no-op for other sources).
        self.refresh_stash_metadata(id, external_id.to_string(), source_key, false)
            .await;

        Ok(id)
    }

    /// All O events for a session, ordered by time.
    pub async fn o_events_for_session(&self, session_id: i64) -> Result<Vec<OEvent>> {
        let rows = sqlx::query_as::<_, (
            i64, Option<i64>, Option<i64>, i64, Option<i64>, Option<String>, i64, String,
        )>(
            "SELECT id, session_id, content_item_id, occurred_at, intensity, notes, stash_synced, origin
             FROM o_events
             WHERE session_id = ?1
             ORDER BY occurred_at ASC",
        )
        .bind(session_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|r| OEvent {
                id: r.0,
                session_id: r.1,
                content_item_id: r.2,
                occurred_at: r.3,
                intensity: r.4,
                notes: r.5,
                stash_synced: r.6 != 0,
                origin: r.7,
            })
            .collect())
    }

    /// Today's session-less, scene-less ("unlinked") cumshots, ordered by time.
    /// These are O events logged with NO session AND NO scene (session_id NULL,
    /// content_item_id NULL, origin 'climax') - the tracker's "I came, not to
    /// anything in Stash, and I'm not tracking" path. Scoped to the local
    /// calendar day (attributed by occurred_at, the same basis the dashboard uses
    /// for session-less O's) so the empty-tracker counter shows today's tally and
    /// lets the user peel today's mistakes back off.
    pub async fn sessionless_unlinked_today(&self) -> Result<Vec<OEvent>> {
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        let (lo, hi) = crate::db::local_day_bounds_ms(&today, &today)?;
        let rows = sqlx::query_as::<_, (
            i64, Option<i64>, Option<i64>, i64, Option<i64>, Option<String>, i64, String,
        )>(
            "SELECT id, session_id, content_item_id, occurred_at, intensity, notes, stash_synced, origin
             FROM o_events
             WHERE session_id IS NULL AND content_item_id IS NULL AND origin = 'climax'
               AND occurred_at >= ?1 AND occurred_at < ?2
             ORDER BY occurred_at ASC",
        )
        .bind(lo)
        .bind(hi)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|r| OEvent {
                id: r.0,
                session_id: r.1,
                content_item_id: r.2,
                occurred_at: r.3,
                intensity: r.4,
                notes: r.5,
                stash_synced: r.6 != 0,
                origin: r.7,
            })
            .collect())
    }

    /// Delete an O event by id.
    ///
    /// Delete an o_event of ANY origin from inside Climax.
    ///
    /// On successful delete of an event that was synced to Stash (climax-origin
    /// that we pushed, OR stash-origin that Stash itself counted), decrements
    /// Stash's counter via sceneDecrementO so the two stay aligned. This is the
    /// USER-initiated delete path (Climax UI) — it intentionally pushes back to
    /// Stash. The inbound mirror path (`remove_external_o`) is separate and does
    /// NOT push, since Stash already decremented there.
    ///
    /// Note: sceneDecrementO is LIFO on Stash's o_history (pops the most recent
    /// entry), not a targeted removal — acceptable for single-user use.
    pub async fn delete_o(&self, o_id: i64) -> Result<()> {
        let row: Option<(i64, Option<i64>, String, i64)> = sqlx::query_as(
            "SELECT stash_synced, content_item_id, origin, occurred_at FROM o_events WHERE id = ?1",
        )
        .bind(o_id)
        .fetch_optional(&self.pool)
        .await?;

        let Some((synced, content_id, _origin, occurred_at)) = row else {
            tracing::warn!("delete_o: o_event {} not found", o_id);
            return Err(anyhow!("That cumshot no longer exists."));
        };

        sqlx::query("DELETE FROM o_events WHERE id = ?1")
            .bind(o_id)
            .execute(&self.pool)
            .await?;

        if synced != 0 {
            if let Some(cid) = content_id {
                let scene_lookup: Option<(String, Option<String>)> = sqlx::query_as(
                    "SELECT s.key, c.external_id
                     FROM content_items c JOIN sources s ON s.id = c.source_id
                     WHERE c.id = ?1",
                )
                .bind(cid)
                .fetch_optional(&self.pool)
                .await?;
                if let Some((src, Some(external_id))) = scene_lookup {
                    if src == "stash" {
                        let pool = self.pool.clone();
                        // Surgical: remove the o_history entry for THIS cumshot's
                        // time, not just the most recent one.
                        tokio::spawn(async move {
                            match stash::remove_o_at(&pool, &external_id, occurred_at).await {
                                Ok(true) => tracing::info!(
                                    "stash sync (delete): scene {} O at {} removed",
                                    external_id, occurred_at
                                ),
                                Ok(false) => tracing::info!(
                                    "stash sync (delete): scene {} had no O history to remove",
                                    external_id
                                ),
                                Err(e) => tracing::warn!(
                                    "stash sync (delete) failed for scene {}: {:#}",
                                    external_id, e
                                ),
                            }
                        });
                    }
                }
            }
        }

        Ok(())
    }

    /// Delete the N most recent o_events for a given (session, scene), removing
    /// Climax-origin cumshots first and only dipping into Stash-origin ones once
    /// the Climax ones are exhausted. Each removal routes through `delete_o`, so
    /// any synced event (climax- or stash-origin) decrements Stash's counter.
    /// Used by the wrap-up modal AND the inline minus button on scene rows.
    ///
    /// Ordering rationale: `(origin = 'climax') DESC` puts your own Climax taps
    /// at the front of the removal queue, so pressing − peels those off before
    /// touching anything that came from Stash — which makes the wrap-up's
    /// projected "N via Stash" label computable client-side.
    pub async fn delete_recent_o_for_scene(
        &self,
        session_id: i64,
        content_item_id: i64,
        count: i64,
    ) -> Result<u64> {
        if count <= 0 {
            return Ok(0);
        }
        let ids: Vec<i64> = sqlx::query_scalar(
            "SELECT id FROM o_events
             WHERE session_id = ?1 AND content_item_id = ?2
             ORDER BY (origin = 'climax') DESC, occurred_at DESC
             LIMIT ?3",
        )
        .bind(session_id)
        .bind(content_item_id)
        .bind(count)
        .fetch_all(&self.pool)
        .await?;

        let mut total = 0u64;
        for id in ids {
            self.delete_o(id).await?;
            total += 1;
        }
        Ok(total)
    }

    /// Inbound mirror of a Stash O-counter decrement (user clicked −O / removed
    /// an O in Stash's own UI; the bridge forwards it as an `o_remove`). Removes
    /// the single most-recent o_event for this scene in the ACTIVE session —
    /// any origin, faithfully mirroring Stash's LIFO decrement — and does NOT
    /// push back to Stash, because Stash already decremented its own counter.
    /// No-op (returns false) if there's no active session or no matching event.
    pub async fn remove_external_o(&self, content_item_id: i64) -> Result<bool> {
        let Some(session_id) = self.active_id().await? else {
            tracing::info!(
                "external o-remove: no active session, ignoring (content {})",
                content_item_id
            );
            return Ok(false);
        };

        let id: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM o_events
             WHERE session_id = ?1 AND content_item_id = ?2
             ORDER BY occurred_at DESC
             LIMIT 1",
        )
        .bind(session_id)
        .bind(content_item_id)
        .fetch_optional(&self.pool)
        .await?;

        let Some(oid) = id else {
            tracing::info!(
                "external o-remove: no o_event for session {} content {}, ignoring",
                session_id,
                content_item_id
            );
            return Ok(false);
        };

        sqlx::query("DELETE FROM o_events WHERE id = ?1")
            .bind(oid)
            .execute(&self.pool)
            .await?;
        tracing::info!(
            "external o-remove: deleted o_event {} (session {} content {}); no stash push (stash already decremented)",
            oid,
            session_id,
            content_item_id
        );
        Ok(true)
    }

    /// Scenes played within a session (for the dashboard "current session" list).
    /// Each row carries per-scene cumshot_count for this session via a correlated subquery.
    /// `counted_at` is exposed so the frontend can dim / hide rows that
    /// haven't crossed the play threshold yet.
    pub async fn scenes_in_session(&self, session_id: i64) -> Result<Vec<PlayedScene>> {
        // 17 fields now exceed sqlx's tuple FromRow limit (16), so this maps
        // onto a named #[derive(FromRow)] struct keyed by column alias.
        #[derive(sqlx::FromRow)]
        struct Row {
            play_id: i64,
            session_id: i64,
            content_item_id: i64,
            first_seen_at: i64,
            last_seen_at: i64,
            seconds_tracked: i64,
            external_id: Option<String>,
            title: Option<String>,
            url: Option<String>,
            duration_seconds: Option<i64>,
            source_key: String,
            cumshot_count: i64,
            climax_cumshot_count: i64,
            thumbnail_url: Option<String>,
            metadata_json: Option<String>,
            counted_at: Option<i64>,
            last_advance_at: Option<i64>,
            off_bridge: i64,
        }
        let rows = sqlx::query_as::<_, Row>(
            "SELECT sp.id AS play_id, sp.session_id, sp.content_item_id, sp.first_seen_at, sp.last_seen_at,
                    sp.seconds_tracked,
                    ci.external_id, ci.title, ci.url, ci.duration_seconds,
                    s.key AS source_key,
                    (SELECT COUNT(*) FROM o_events oe
                       WHERE oe.session_id = sp.session_id
                         AND oe.content_item_id = sp.content_item_id) AS cumshot_count,
                    (SELECT COUNT(*) FROM o_events oe
                       WHERE oe.session_id = sp.session_id
                         AND oe.content_item_id = sp.content_item_id
                         AND oe.origin = 'climax') AS climax_cumshot_count,
                    ci.thumbnail_url,
                    ci.metadata_json,
                    sp.counted_at,
                    sp.last_advance_at,
                    sp.off_bridge
             FROM scene_plays sp
             JOIN content_items ci ON ci.id = sp.content_item_id
             JOIN sources s ON s.id = ci.source_id
             WHERE sp.session_id = ?1
             ORDER BY sp.first_seen_at ASC",
        )
        .bind(session_id)
        .fetch_all(&self.pool)
        .await?;

        // Watch runs for the whole session in one query, then bucketed by play,
        // rather than a query per scene. Plays recorded before runs existed (and
        // reconstructed / absorbed ones, which are built from Stash history and
        // have no run detail to give) simply come back with an empty list - the
        // spine falls back to drawing a single band from the aggregate.
        let run_rows: Vec<(i64, i64, i64, i64, i64)> = sqlx::query_as(
            "SELECT r.play_id, r.started_at, r.ended_at, r.seconds_tracked, r.off_bridge
             FROM scene_play_runs r
             JOIN scene_plays sp ON sp.id = r.play_id
             WHERE sp.session_id = ?1
             ORDER BY r.started_at ASC",
        )
        .bind(session_id)
        .fetch_all(&self.pool)
        .await?;
        let mut runs_by_play: HashMap<i64, Vec<PlayRun>> = HashMap::new();
        for (play_id, started_at, ended_at, seconds_tracked, off_bridge) in run_rows {
            runs_by_play.entry(play_id).or_default().push(PlayRun {
                started_at,
                ended_at,
                seconds_tracked,
                off_bridge: off_bridge != 0,
            });
        }

        // Stash's own play timestamps falling inside this session's window - one
        // per player load that crossed its minimum-play-percent. This is what
        // tells a pause apart from a genuine re-open, and it comes from Stash
        // rather than from guesswork about gaps. Imported at session end (see
        // spawn_import_session_plays); an active session has none yet, and a
        // session that ended while Stash was unreachable never will.
        //
        // The window is bounded by the session's own end (or now, while it is
        // still running), so a later play of the same scene can't leak in.
        let play_times: Vec<(i64, i64)> = sqlx::query_as(
            "SELECT sp.content_item_id, pi.played_at
             FROM scene_plays sp
             JOIN play_imports pi ON pi.content_item_id = sp.content_item_id
             JOIN sessions s ON s.id = sp.session_id
             WHERE sp.session_id = ?1
               AND pi.played_at >= s.started_at
               AND pi.played_at <= COALESCE(s.ended_at, ?2)
             ORDER BY pi.played_at ASC",
        )
        .bind(session_id)
        .bind(now_ms())
        .fetch_all(&self.pool)
        .await?;
        let mut plays_by_content: HashMap<i64, Vec<i64>> = HashMap::new();
        for (content_id, at) in play_times {
            plays_by_content.entry(content_id).or_default().push(at);
        }

        Ok(rows
            .into_iter()
            .map(|r| PlayedScene {
                runs: runs_by_play.remove(&r.play_id).unwrap_or_default(),
                stash_plays: plays_by_content
                    .get(&r.content_item_id)
                    .cloned()
                    .unwrap_or_default(),
                play_id: r.play_id,
                session_id: r.session_id,
                content_item_id: r.content_item_id,
                first_seen_at: r.first_seen_at,
                last_seen_at: r.last_seen_at,
                seconds_tracked: r.seconds_tracked,
                external_id: r.external_id,
                title: r.title,
                url: r.url,
                duration_seconds: r.duration_seconds,
                source_key: r.source_key,
                cumshot_count: r.cumshot_count,
                climax_cumshot_count: r.climax_cumshot_count,
                thumbnail_url: r.thumbnail_url,
                metadata_json: r.metadata_json,
                counted_at: r.counted_at,
                last_advance_at: r.last_advance_at,
                off_bridge: r.off_bridge != 0,
            })
            .collect())
    }

    /// Sessions whose started_at falls within [start_ms, end_ms). Used by the
    /// dashboard's day view and week summary. Discarded sessions hidden.
    pub async fn list_in_range(&self, start_ms: i64, end_ms: i64) -> Result<Vec<Session>> {
        let ids: Vec<i64> = sqlx::query_scalar(
            "SELECT id FROM sessions
             WHERE started_at >= ?1 AND started_at < ?2
               AND status != 'discarded'
             ORDER BY started_at ASC",
        )
        .bind(start_ms)
        .bind(end_ms)
        .fetch_all(&self.pool)
        .await?;
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            out.push(self.load(id).await?);
        }
        Ok(out)
    }

    /// Sessions whose assigned_day == `day` (YYYY-MM-DD local). Preferred over
    /// list_in_range for day-bucketed views (week strip, day list, heatmap).
    /// Discarded sessions hidden — the SessionStrip on the dashboard reads
    /// this and would otherwise still render rows the user explicitly threw
    /// away from the wrap-up modal.
    pub async fn list_for_day(&self, day: &str) -> Result<Vec<Session>> {
        let ids: Vec<i64> = sqlx::query_scalar(
            "SELECT id FROM sessions
             WHERE assigned_day = ?1
               AND status != 'discarded'
             ORDER BY started_at ASC",
        )
        .bind(day)
        .fetch_all(&self.pool)
        .await?;
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            out.push(self.load(id).await?);
        }
        Ok(out)
    }

    /// Record a gap (idle or post-crash downtime) as a closed `session_pauses`
    /// row with the given `reason` ('idle' / 'crash'). Session status is NOT
    /// changed - the session keeps running. Used by the idle-return modal's
    /// "Discard & Continue" (reason='idle') and crash recovery's "Discard &
    /// Continue" (reason='crash').
    ///
    /// No-op if the session isn't active (e.g. user manually stopped between
    /// the prompt firing and the user responding). Crash recovery resumes a
    /// paused leftover to active on boot, so this guard doesn't block it.
    pub async fn record_gap_pause(
        &self,
        session_id: i64,
        paused_at: i64,
        resumed_at: i64,
        reason: &str,
    ) -> Result<()> {
        let status: Option<String> = sqlx::query_scalar(
            "SELECT status FROM sessions WHERE id = ?1",
        )
        .bind(session_id)
        .fetch_optional(&self.pool)
        .await?;
        if status.as_deref() != Some("active") {
            tracing::info!(
                "record_gap_pause: session {} no longer active, ignoring",
                session_id
            );
            return Ok(());
        }

        sqlx::query(
            "INSERT INTO session_pauses (session_id, paused_at, resumed_at, reason)
             VALUES (?1, ?2, ?3, ?4)",
        )
        .bind(session_id)
        .bind(paused_at)
        .bind(resumed_at)
        .bind(reason)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Stamp the running session as alive right now. Returns how many rows it
    /// touched (0 when nothing is running).
    ///
    /// `last_heartbeat` is what crash recovery measures the offline gap from, and
    /// the name is misleading: it was only ever written when a SCENE advanced (a
    /// bridge heartbeat or an off-bridge credit). A session with no scene playing
    /// never moved it past its start, so ten minutes in, ANY restart read the
    /// whole session as downtime. The headless server then ended it at its start
    /// time, zero seconds long, and the desktop app offered to discard all of it.
    /// That hit anyone who starts a session well before a scene, and every
    /// session of someone using Climax as a plain timer.
    ///
    /// Paused sessions are stamped too: a session paused until the crash was
    /// alive until the crash, and recovery closes its open pause at this time.
    pub async fn touch_liveness(pool: &SqlitePool) -> Result<u64> {
        let now = now_ms();
        let done = sqlx::query(
            "UPDATE sessions SET last_heartbeat = ?1
             WHERE status IN ('active','paused') AND ended_at IS NULL
               AND (last_heartbeat IS NULL OR last_heartbeat < ?1)",
        )
        .bind(now)
        .execute(pool)
        .await?;
        Ok(done.rows_affected())
    }

    /// Keep a running session's "last seen alive" time current for as long as
    /// this backend is up. Call it once per process, AFTER boot crash recovery
    /// has read the old value - the loop sleeps before its first stamp, but the
    /// ordering is what guarantees a restart still sees the real downtime.
    /// Whichever process owns the database runs it: the desktop app in host
    /// mode and climax-server, never a desktop app in client mode.
    pub fn spawn_liveness(pool: SqlitePool) {
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(LIVENESS_INTERVAL_SECS)).await;
                if let Err(e) = Self::touch_liveness(&pool).await {
                    tracing::warn!("session liveness stamp failed: {:#}", e);
                }
            }
        });
    }

    /// Detect a session left 'active'/'paused' by an unclean prior run (crash,
    /// power loss, container restart). Returns `(session_id, gap_started_at)` for
    /// the most-recent such session, where `gap_started_at` is its last heartbeat
    /// (or `started_at` if it never beat) - the point to measure the offline gap
    /// from. None if nothing was left running. Pure read; the caller decides the
    /// recovery policy (the desktop boot prompts the user; the headless server
    /// auto-resumes a quick relaunch / auto-ends a long downtime).
    pub async fn crash_leftover(&self) -> Result<Option<(i64, i64)>> {
        let row: Option<(i64, Option<i64>, i64)> = sqlx::query_as(
            "SELECT id, last_heartbeat, started_at FROM sessions \
             WHERE status IN ('active','paused') ORDER BY id DESC LIMIT 1",
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|(sid, last_hb, started)| (sid, last_hb.unwrap_or(started))))
    }

    /// Resume a leftover session to 'active' for crash recovery. Closes any open
    /// pause at `gap_started_at` (= last heartbeat) so a pre-crash pause is
    /// bounded, then flips status to 'active'. Idempotent for an already-active
    /// session (the pause update is a no-op and the status set is harmless). The
    /// gap [gap_started_at, now] itself is handled by the recovery choice:
    /// Keep counts it, Discard & Continue inserts a 'crash' pause over it.
    pub async fn resume_for_crash(&self, session_id: i64, gap_started_at: i64) -> Result<()> {
        let now = now_ms();
        sqlx::query(
            "UPDATE session_pauses SET resumed_at = ?1
             WHERE session_id = ?2 AND resumed_at IS NULL",
        )
        .bind(gap_started_at)
        .bind(session_id)
        .execute(&self.pool)
        .await?;
        sqlx::query(
            "UPDATE sessions SET status = 'active', updated_at = ?1 WHERE id = ?2",
        )
        .bind(now)
        .bind(session_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// End an active session at a specific timestamp (rather than `now`). Used
    /// by the idle-return modal's "Discard & End" action: the user was AFK
    /// from `ended_at` onward and wants the session to stop at that point.
    ///
    /// Scene_plays.last_seen_at is capped to ended_at so their times stay
    /// inside the session window. Any open pause is closed at ended_at; any
    /// pause that started after ended_at is removed (shouldn't exist, but
    /// defensive). Session status flips to 'ended'.
    ///
    /// No-op if the session isn't active OR ended_at is before started_at.
    pub async fn end_session_at(&self, session_id: i64, ended_at: i64) -> Result<Option<Session>> {
        let row: Option<(String, i64)> = sqlx::query_as(
            "SELECT status, started_at FROM sessions WHERE id = ?1",
        )
        .bind(session_id)
        .fetch_optional(&self.pool)
        .await?;
        let Some((status, started_at)) = row else {
            return Ok(None);
        };
        if status != "active" && status != "paused" {
            tracing::info!(
                "end_session_at: session {} status={}, ignoring",
                session_id, status
            );
            return Ok(Some(self.load(session_id).await?));
        }
        if ended_at < started_at {
            return Err(anyhow!(
                "That end time is before the session started."
            ));
        }

        let now = now_ms();
        let mut tx = self.pool.begin().await?;

        sqlx::query(
            "UPDATE scene_plays
             SET last_seen_at = ?1
             WHERE session_id = ?2 AND last_seen_at > ?1",
        )
        .bind(ended_at)
        .bind(session_id)
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            "UPDATE session_pauses
             SET resumed_at = ?1
             WHERE session_id = ?2 AND resumed_at IS NULL AND paused_at <= ?1",
        )
        .bind(ended_at)
        .bind(session_id)
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            "DELETE FROM session_pauses WHERE session_id = ?1 AND paused_at > ?2",
        )
        .bind(session_id)
        .bind(ended_at)
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            "UPDATE sessions
             SET status = 'ended',
                 ended_at = ?1,
                 last_heartbeat = ?1,
                 updated_at = ?2
             WHERE id = ?3",
        )
        .bind(ended_at)
        .bind(now)
        .bind(session_id)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;

        // Ended here too (the idle prompt's "Discard & End", and the headless
        // crash-straggler cleanup), so do the same end-of-session Stash reads
        // stop() does.
        Self::spawn_attribute_watch_time(self.pool.clone(), session_id);
        Self::spawn_import_session_plays(self.pool.clone(), session_id);

        self.notify_state().await;
        Ok(Some(self.load(session_id).await?))
    }

    /// Set/override the assigned_day for a session (from the wrap-up modal day
    /// picker, or the edit modal). `day` is YYYY-MM-DD local.
    pub async fn set_assigned_day(&self, id: i64, day: &str) -> Result<Session> {
        let now = now_ms();
        sqlx::query(
            "UPDATE sessions SET assigned_day = ?1, updated_at = ?2 WHERE id = ?3",
        )
        .bind(day)
        .bind(now)
        .bind(id)
        .execute(&self.pool)
        .await?;
        self.load(id).await
    }
}

#[derive(Debug, Clone)]
pub struct HeartbeatRecord {
    pub session_id: Option<i64>,
    /// Carried for completeness; no current reader.
    #[allow(dead_code)]
    pub content_item_id: i64,
    pub recorded: bool,
}

/// What to roll back out of Stash for one scene when a session/scene is deleted.
/// `external_id` is the Stash scene id. The O- and play-history entries that
/// fall inside [window_start, window_end] (the session's span) are the ones the
/// deletion logged, so they're removed precisely (not LIFO). `watched_secs` is
/// subtracted from the scene's `play_duration`.
#[derive(Debug, Clone)]
struct SceneRollback {
    external_id: String,
    watched_secs: i64,
    window_start: i64,
    window_end: i64,
}

/// Validated facts for a merge of two sessions (internal to the merge path).
struct MergeData {
    first_id: i64,
    first_end: i64,
    second_id: i64,
    second_start: i64,
    gap_ms: i64,
    merged_start: i64,
    merged_end: i64,
}

/// Preview of a session reopen for the UI (enable the action + show the gap).
#[derive(Debug, Clone, serde::Serialize)]
pub struct ReopenCheck {
    pub can_reopen: bool,
    /// How long ago the session ended (ms) - what becomes a pause, or counts as
    /// session time, per the user's choice.
    pub gap_ms: i64,
}

/// Preview of a session merge for the UI (enable the action + show the gap).
#[derive(Debug, Clone, serde::Serialize)]
pub struct MergeCheck {
    pub ok: bool,
    /// Why the merge isn't allowed (shown to the user) when `ok` is false.
    pub reason: Option<String>,
    /// Gap (ms) between the earlier session's end and the later's start (0 when
    /// they touch or overlap).
    pub gap_ms: i64,
    pub merged_start: i64,
    pub merged_end: i64,
}

/// One stretch of playback inside a scene_play. See the
/// `20260814000001_scene_play_runs` migration for why these exist.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PlayRun {
    pub started_at: i64,
    pub ended_at: i64,
    pub seconds_tracked: i64,
    /// Credited by the Stash play_duration poll rather than the bridge, so
    /// bounded by the poll interval rather than measured to the second.
    pub off_bridge: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct PlayedScene {
    /// The stretches of playback that make up `seconds_tracked`, oldest first.
    /// EMPTY for plays recorded before runs existed, and for reconstructed or
    /// absorbed plays built from Stash history - readers must fall back to the
    /// aggregate rather than treating empty as "never watched".
    pub runs: Vec<PlayRun>,
    /// Stash's play timestamps for this scene INSIDE this session's window, in
    /// order - one per player load that crossed Stash's minimum-play-percent.
    /// Two of these means the scene was genuinely opened twice, as opposed to
    /// paused and resumed. Empty while a session is still running (they are
    /// imported at the end), for a session that ended with Stash unreachable,
    /// and for any watch too brief for Stash to log a play at all.
    pub stash_plays: Vec<i64>,
    pub play_id: i64,
    pub session_id: i64,
    pub content_item_id: i64,
    pub first_seen_at: i64,
    pub last_seen_at: i64,
    pub seconds_tracked: i64,
    pub external_id: Option<String>,
    pub title: Option<String>,
    pub url: Option<String>,
    pub duration_seconds: Option<i64>,
    pub source_key: String,
    /// Total cumshots logged against this scene WITHIN this session.
    pub cumshot_count: i64,
    /// Subset of cumshot_count that originated in Climax (and is therefore
    /// removable from inside Climax). Stash-originated cumshots are read-only.
    pub climax_cumshot_count: i64,
    /// Populated from Stash's findScene query (background enrichment).
    pub thumbnail_url: Option<String>,
    /// Raw JSON string with { performers, studio, date, tags }. Frontend parses.
    pub metadata_json: Option<String>,
    /// ms-since-epoch at which seconds_tracked first crossed the play
    /// threshold (Stash's "minimum play percent" mirrored locally). None
    /// means the scene was opened but never watched long enough to count
    /// as a play — the tracker / Overview hide it once it's no longer
    /// being actively watched (unless it has a cumshot, which forces it
    /// to stay visible). The wrap-up modal still shows it, dimmed.
    pub counted_at: Option<i64>,
    /// ms-since-epoch of the last heartbeat that actually advanced
    /// seconds_tracked (the video was playing). The tracker / Overview treat a
    /// scene as "currently watching" while this is within their grace window
    /// (or it has a cumshot). Authoritative backend state, so it survives
    /// frontend remounts + window-hide timer throttling. None = never advanced
    /// live (paused-only, or pre-migration history).
    pub last_advance_at: Option<i64>,
    /// True when this scene was tracked via the OFF-BRIDGE poll path (remote.rs)
    /// and never seen by the browser bridge - e.g. watched on a TV or phone app.
    /// The wrap-up modal tags these rows. False for bridge / backdated / historic
    /// (pre-migration) rows.
    pub off_bridge: bool,
}

// Silence unused warnings on ContentItem (used by frontend via commands later).
#[allow(dead_code)]
fn _unused_marker(_: ContentItem, _: SessionStatus) {}

#[cfg(test)]
mod watch_run_tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    async fn fresh_pool() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open in-memory db");
        sqlx::migrate!("./migrations").run(&pool).await.expect("migrate");
        pool
    }

    /// A session with one scene_play, returning the play id. `ext` keeps each
    /// call's scene distinct (content_items is unique per source + external id).
    async fn play_of(pool: &SqlitePool, ext: &str) -> i64 {
        let item: i64 = sqlx::query_scalar(
            "INSERT INTO content_items (source_id, external_id, first_seen_at, last_seen_at)
             VALUES ((SELECT id FROM sources WHERE key='stash'), ?1, 1, 1) RETURNING id",
        )
        .bind(ext)
        .fetch_one(pool)
        .await
        .unwrap();
        let session: i64 = sqlx::query_scalar(
            "INSERT INTO sessions (started_at, status, assigned_day, created_at, updated_at)
             VALUES (1000, 'active', '2026-08-14', 1, 1) RETURNING id",
        )
        .fetch_one(pool)
        .await
        .unwrap();
        sqlx::query_scalar(
            "INSERT INTO scene_plays (session_id, content_item_id, first_seen_at, last_seen_at, seconds_tracked)
             VALUES (?1, ?2, 1000, 1000, 0) RETURNING id",
        )
        .bind(session)
        .bind(item)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    async fn play(pool: &SqlitePool) -> i64 {
        play_of(pool, "x").await
    }

    async fn runs(pool: &SqlitePool, play_id: i64) -> Vec<(i64, i64, i64)> {
        sqlx::query_as("SELECT started_at, ended_at, seconds_tracked FROM scene_play_runs WHERE play_id = ?1 ORDER BY started_at")
            .bind(play_id)
            .fetch_all(pool)
            .await
            .unwrap()
    }

    /// Heartbeats arriving one after another are ONE stretch of watching, not
    /// one run each.
    #[tokio::test]
    async fn consecutive_heartbeats_extend_a_single_run() {
        let pool = fresh_pool().await;
        let p = play(&pool).await;
        for i in 1..=4 {
            SessionManager::record_watch_run(&pool, p, 1_000_000 + i * 5_000, 5, false)
                .await
                .unwrap();
        }
        let r = runs(&pool, p).await;
        assert_eq!(r.len(), 1, "one continuous watch is one run");
        // Starts a heartbeat before the first credit, ends at the last.
        assert_eq!(r[0], (1_000_000, 1_020_000, 20));
    }

    /// Come back after a real break and it is a separate watch - this is what
    /// makes a rewatch show up twice on the spine.
    #[tokio::test]
    async fn a_long_gap_opens_a_new_run() {
        let pool = fresh_pool().await;
        let p = play(&pool).await;
        SessionManager::record_watch_run(&pool, p, 1_000_000, 5, false).await.unwrap();
        // Just inside the gap - still the same watch.
        SessionManager::record_watch_run(&pool, p, 1_000_000 + BRIDGE_RUN_GAP_MS, 5, false)
            .await
            .unwrap();
        // Past it - a new one.
        SessionManager::record_watch_run(&pool, p, 1_000_000 + BRIDGE_RUN_GAP_MS * 3, 5, false)
            .await
            .unwrap();

        let r = runs(&pool, p).await;
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].2, 10, "both sides of the short gap credit one run");
        assert_eq!(r[1].2, 5);
    }

    /// The poll path cannot resolve a gap as fine as the bridge can, so the same
    /// spacing that splits a bridge run must NOT split an off-bridge one - else
    /// a TV watch draws as a dotted line instead of a band.
    #[tokio::test]
    async fn the_gap_follows_the_source_cadence() {
        let pool = fresh_pool().await;
        let bridge = play_of(&pool, "bridge-scene").await;
        let tv = play_of(&pool, "tv-scene").await;
        let spacing = 60_000; // between the two thresholds

        for at in [1_000_000, 1_000_000 + spacing] {
            SessionManager::record_watch_run(&pool, bridge, at, 5, false).await.unwrap();
            SessionManager::record_watch_run(&pool, tv, at, 28, true).await.unwrap();
        }
        assert_eq!(runs(&pool, bridge).await.len(), 2, "bridge resolves the gap");
        assert_eq!(runs(&pool, tv).await.len(), 1, "the poll cannot");
    }

    /// The off-bridge poll can hand over a big delta at once (a missed interval,
    /// or the tail flush at session end). Laying that credit back over its own
    /// duration must not reach back across the previous run.
    #[tokio::test]
    async fn a_large_late_credit_cannot_overlap_the_previous_run() {
        let pool = fresh_pool().await;
        let p = play(&pool).await;
        SessionManager::record_watch_run(&pool, p, 1_000_000, 30, false).await.unwrap();
        // 10 minutes later, credited with a full hour of watching.
        SessionManager::record_watch_run(&pool, p, 1_600_000, 3600, true).await.unwrap();

        let r = runs(&pool, p).await;
        assert_eq!(r.len(), 2);
        assert!(r[1].0 >= r[0].1, "second run starts at or after the first ends");
        assert_eq!(r[1].1, 1_600_000);
    }

    /// Zero-credit heartbeats (paused, buffering, seeking backwards) are not
    /// watching and must leave no trace.
    #[tokio::test]
    async fn no_credit_records_nothing() {
        let pool = fresh_pool().await;
        let p = play(&pool).await;
        SessionManager::record_watch_run(&pool, p, 1_000_000, 0, false).await.unwrap();
        SessionManager::record_watch_run(&pool, p, 1_000_000, -5, false).await.unwrap();
        assert!(runs(&pool, p).await.is_empty());
    }

    /// Removing a scene from a session (or deleting the session) must take its
    /// runs with it, or they outlive the row they describe.
    #[tokio::test]
    async fn runs_cascade_with_the_play() {
        let pool = fresh_pool().await;
        let p = play(&pool).await;
        SessionManager::record_watch_run(&pool, p, 1_000_000, 5, false).await.unwrap();
        sqlx::query("DELETE FROM scene_plays WHERE id = ?1").bind(p).execute(&pool).await.unwrap();
        assert!(runs(&pool, p).await.is_empty());
    }
}

#[cfg(test)]
mod delete_import_tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    async fn fresh_pool() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open in-memory db");
        sqlx::migrate!("./migrations").run(&pool).await.expect("migrate");
        pool
    }

    async fn content(pool: &SqlitePool, ext: &str) -> i64 {
        sqlx::query_scalar(
            "INSERT INTO content_items (source_id, external_id, first_seen_at, last_seen_at)
             VALUES ((SELECT id FROM sources WHERE key='stash'), ?1, 1, 1) RETURNING id",
        )
        .bind(ext)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    async fn import(pool: &SqlitePool, item: i64, at: i64) {
        sqlx::query("INSERT INTO play_imports (content_item_id, played_at, created_at) VALUES (?1, ?2, 1)")
            .bind(item).bind(at).execute(pool).await.unwrap();
    }

    /// A delete must take the Stash timestamps it is about to roll back with it.
    /// Leave them and reconstruction offers the session straight back.
    #[tokio::test]
    async fn drops_only_this_session_s_imports_in_window() {
        let pool = fresh_pool().await;
        let watched = content(&pool, "8009").await;
        let unrelated = content(&pool, "7000").await;

        let session: i64 = sqlx::query_scalar(
            "INSERT INTO sessions (started_at, ended_at, status, assigned_day, created_at, updated_at)
             VALUES (1000, 2000, 'ended', '2026-08-06', 1, 1) RETURNING id",
        ).fetch_one(&pool).await.unwrap();
        sqlx::query(
            "INSERT INTO scene_plays (session_id, content_item_id, first_seen_at, last_seen_at, seconds_tracked)
             VALUES (?1, ?2, 1100, 1900, 30)",
        ).bind(session).bind(watched).execute(&pool).await.unwrap();

        import(&pool, watched, 1500).await;    // in window, this session's scene -> goes
        import(&pool, watched, 5000).await;    // same scene, outside the window -> stays
        import(&pool, unrelated, 1500).await;  // in window, different scene    -> stays

        let mut conn = pool.acquire().await.unwrap();
        drop_imports_for_session(&mut conn, session).await.unwrap();
        drop(conn);

        let left: Vec<(i64, i64)> =
            sqlx::query_as("SELECT content_item_id, played_at FROM play_imports ORDER BY played_at")
                .fetch_all(&pool).await.unwrap();
        assert_eq!(left, vec![(unrelated, 1500), (watched, 5000)],
                   "only the in-window import for this session's own scene should go");
    }

    /// Deleting a session that imported nothing must not touch anything.
    #[tokio::test]
    async fn leaves_everything_alone_when_the_session_has_no_scenes() {
        let pool = fresh_pool().await;
        let other = content(&pool, "7000").await;
        let session: i64 = sqlx::query_scalar(
            "INSERT INTO sessions (started_at, ended_at, status, assigned_day, created_at, updated_at)
             VALUES (1000, 2000, 'ended', '2026-08-06', 1, 1) RETURNING id",
        ).fetch_one(&pool).await.unwrap();
        import(&pool, other, 1500).await;

        let mut conn = pool.acquire().await.unwrap();
        drop_imports_for_session(&mut conn, session).await.unwrap();
        drop(conn);

        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM play_imports")
            .fetch_one(&pool).await.unwrap();
        assert_eq!(n, 1);
    }
}

#[cfg(test)]
mod liveness_tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    async fn fresh_pool() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open in-memory db");
        sqlx::migrate!("./migrations").run(&pool).await.expect("migrate");
        pool
    }

    /// A session that started `mins_ago` and has not been heard from since,
    /// which is every scene-less session before this fix.
    async fn session(pool: &SqlitePool, status: &str, mins_ago: i64) -> i64 {
        let started = now_ms() - mins_ago * 60_000;
        let ended: Option<i64> = if status == "ended" { Some(started + 1000) } else { None };
        sqlx::query_scalar(
            "INSERT INTO sessions (started_at, ended_at, status, assigned_day, created_at, updated_at, last_heartbeat)
             VALUES (?1, ?2, ?3, '2026-10-13', ?1, ?1, ?1) RETURNING id",
        )
        .bind(started)
        .bind(ended)
        .bind(status)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    async fn last_beat(pool: &SqlitePool, id: i64) -> i64 {
        sqlx::query_scalar("SELECT last_heartbeat FROM sessions WHERE id = ?1")
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    fn manager(pool: &SqlitePool) -> SessionManager {
        let (tx, _rx) = broadcast::channel(8);
        SessionManager::new(pool.clone(), tx)
    }

    /// The bug: 80 minutes into a session with no scene, a restart measured the
    /// gap from the session's START.
    #[tokio::test]
    async fn a_scene_less_session_looks_abandoned_without_the_stamp() {
        let pool = fresh_pool().await;
        let id = session(&pool, "active", 80).await;
        let (sid, gap_start) = manager(&pool).crash_leftover().await.unwrap().unwrap();
        assert_eq!(sid, id);
        assert!(now_ms() - gap_start > 79 * 60_000, "gap is the whole session");
    }

    /// The fix: once stamped, the same restart sees no gap at all.
    #[tokio::test]
    async fn the_stamp_makes_a_restart_see_only_the_real_downtime() {
        let pool = fresh_pool().await;
        let id = session(&pool, "active", 80).await;
        assert_eq!(SessionManager::touch_liveness(&pool).await.unwrap(), 1);
        let (sid, gap_start) = manager(&pool).crash_leftover().await.unwrap().unwrap();
        assert_eq!(sid, id);
        assert!(now_ms() - gap_start < 5_000, "gap is measured from the stamp");
    }

    #[tokio::test]
    async fn a_paused_session_is_stamped_too() {
        let pool = fresh_pool().await;
        let id = session(&pool, "paused", 30).await;
        let before = last_beat(&pool, id).await;
        assert_eq!(SessionManager::touch_liveness(&pool).await.unwrap(), 1);
        assert!(last_beat(&pool, id).await > before);
    }

    /// A finished session's last-seen time is history and must never move.
    #[tokio::test]
    async fn ended_sessions_are_left_alone() {
        let pool = fresh_pool().await;
        let ended = session(&pool, "ended", 30).await;
        let before = last_beat(&pool, ended).await;
        assert_eq!(SessionManager::touch_liveness(&pool).await.unwrap(), 0);
        assert_eq!(last_beat(&pool, ended).await, before);
    }

    /// A stamp must never pull the time BACKWARDS (a session end writes its own
    /// value, and a clock adjustment must not undo a newer one).
    #[tokio::test]
    async fn a_newer_value_is_never_overwritten() {
        let pool = fresh_pool().await;
        let id = session(&pool, "active", 5).await;
        let future = now_ms() + 60_000;
        sqlx::query("UPDATE sessions SET last_heartbeat = ?1 WHERE id = ?2")
            .bind(future)
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(SessionManager::touch_liveness(&pool).await.unwrap(), 0);
        assert_eq!(last_beat(&pool, id).await, future);
    }
}

#[cfg(test)]
mod keep_empty_tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    async fn fresh_pool() -> SqlitePool {
        // stop() kicks off background Stash reads. Tests must never reach a real
        // Stash that happens to be running on the developer's machine.
        crate::stash::set_enabled(false);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open in-memory db");
        sqlx::migrate!("./migrations").run(&pool).await.expect("migrate");
        pool
    }

    fn manager(pool: &SqlitePool) -> SessionManager {
        let (tx, _rx) = broadcast::channel(8);
        SessionManager::new(pool.clone(), tx)
    }

    async fn active_session(pool: &SqlitePool) -> i64 {
        let now = now_ms();
        sqlx::query_scalar(
            "INSERT INTO sessions (started_at, status, assigned_day, created_at, updated_at)
             VALUES (?1, 'active', '2026-10-13', ?1, ?1) RETURNING id",
        )
        .bind(now - 60_000)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    async fn status(pool: &SqlitePool, id: i64) -> String {
        sqlx::query_scalar("SELECT status FROM sessions WHERE id = ?1")
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    /// The default must not move: every install that never touches the setting
    /// keeps discarding an empty session on stop.
    #[tokio::test]
    async fn empty_session_is_discarded_by_default() {
        let pool = fresh_pool().await;
        let id = active_session(&pool).await;
        manager(&pool).stop().await.unwrap();
        assert_eq!(status(&pool, id).await, "discarded");
    }

    #[tokio::test]
    async fn empty_session_is_kept_when_the_setting_is_on() {
        let pool = fresh_pool().await;
        crate::settings::set_keep_empty_sessions(&pool, true).await.unwrap();
        let id = active_session(&pool).await;
        let stopped = manager(&pool).stop().await.unwrap().expect("a session was stopped");
        assert_eq!(status(&pool, id).await, "ended");
        assert!(stopped.ended_at.is_some());
    }

    /// Turning it back off restores the old rule for the NEXT stop.
    #[tokio::test]
    async fn turning_the_setting_off_restores_the_discard() {
        let pool = fresh_pool().await;
        crate::settings::set_keep_empty_sessions(&pool, true).await.unwrap();
        crate::settings::set_keep_empty_sessions(&pool, false).await.unwrap();
        let id = active_session(&pool).await;
        manager(&pool).stop().await.unwrap();
        assert_eq!(status(&pool, id).await, "discarded");
    }

    /// A scene opened too briefly to count is still "empty". Off discards it,
    /// on keeps it.
    #[tokio::test]
    async fn an_uncounted_scene_alone_follows_the_setting() {
        for (keep, expected) in [(false, "discarded"), (true, "ended")] {
            let pool = fresh_pool().await;
            crate::settings::set_keep_empty_sessions(&pool, keep).await.unwrap();
            let id = active_session(&pool).await;
            let item: i64 = sqlx::query_scalar(
                "INSERT INTO content_items (source_id, external_id, first_seen_at, last_seen_at)
                 VALUES ((SELECT id FROM sources WHERE key='stash'), 'x', 1, 1) RETURNING id",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO scene_plays (session_id, content_item_id, first_seen_at, last_seen_at, seconds_tracked)
                 VALUES (?1, ?2, 1, 1, 3)",
            )
            .bind(id)
            .bind(item)
            .execute(&pool)
            .await
            .unwrap();
            manager(&pool).stop().await.unwrap();
            assert_eq!(status(&pool, id).await, expected, "keep_empty = {keep}");
        }
    }

    /// A cumshot with no scene has always been enough to keep a session, with
    /// the setting off. The setting is only about the truly empty one.
    #[tokio::test]
    async fn an_unlinked_cumshot_keeps_the_session_without_the_setting() {
        let pool = fresh_pool().await;
        let id = active_session(&pool).await;
        sqlx::query(
            "INSERT INTO o_events (session_id, content_item_id, occurred_at, created_at)
             VALUES (?1, NULL, ?2, ?2)",
        )
        .bind(id)
        .bind(now_ms())
        .execute(&pool)
        .await
        .unwrap();
        manager(&pool).stop().await.unwrap();
        assert_eq!(status(&pool, id).await, "ended");
    }
}
