<!--
  Session detail — central scrollable time-spine with alternating cards
  on either side. Scenes are anchored at their first_seen_at along the
  spine; their tracked watch-duration is rendered as a coral band on the
  spine itself. Cumshots show as droplets on the spine at their exact
  occurred_at. Bigger cards (proper thumbnails) alternate L/R and shift
  down when same-side neighbours would overlap. Connector lines (CSS
  divs, L-shape when shifted) join each card back to its spine anchor.

  Zoom controls in the header scale the spine (px per hour). The whole
  panel takes ~80vh; the spine itself scrolls inside the body.

  Expanded inline below the SessionStrip when the user clicks a session.
-->
<script lang="ts">
  import { w } from "$lib/wording.svelte";
  import { onMount, onDestroy } from "svelte";
  import {
    api,
    formatDuration,
    formatTimeOnly,
    epochToLocalInput,
    localInputToEpoch,
    parseSceneMetadata,
    type PlayedScene,
    type OEvent,
    type Session,
    type SessionPendingHistory,
  } from "$lib/api";
  import Icon from "$lib/Icon.svelte";
  import ConfirmDialog from "$lib/ConfirmDialog.svelte";
  import DateTimePicker from "$lib/DateTimePicker.svelte";
  import { nav } from "$lib/dashboard/nav.svelte";

  type Performer = { id: string; name: string };
  type Studio = { id: string; name: string };

  type Props = {
    sessionId: number;
    onClose: () => void;
    /** Called when the user clicks a performer chip. Wired here so the
     *  upcoming Performers section in the sidebar can register a handler
     *  that filters the dashboard by that performer. No-op fallback for now. */
    onPerformerSelect?: (performer: Performer) => void;
    /** Called when the user clicks the studio chip. Same as above. */
    onStudioSelect?: (studio: Studio) => void;
    /** Called when the user clicks a scene card's thumbnail or title —
     *  parents wire it to the scene's browse detail (nav.goto). */
    onSceneSelect?: (contentItemId: number) => void;
    /** Called after the user deletes THIS session from the detail view, so the
     *  parent (Sessions table / Overview strip) can collapse + refetch. Falls
     *  back to onClose when not provided. */
    onDeleted?: () => void;
    /** Restore the spine's INNER scroll to this position once content loads —
     *  set by parents on a nav Back-restore (see nav detailScroll). */
    initialScroll?: number;
    /** When set, the header shows an "Open in Sessions" link that jumps to this
     *  session in the Sessions page. Only Overview wires it (you're already in
     *  the Sessions page when this renders there), so the link is hidden there. */
    onOpenInSessions?: () => void;
  };
  let { sessionId, onClose, onPerformerSelect, onStudioSelect, onSceneSelect, onDeleted, initialScroll, onOpenInSessions }: Props = $props();

  // Edit mode gates the destructive controls (per-cumshot / per-scene delete)
  // so they never sit live during normal viewing.
  let editing = $state(false);

  // Single pending confirm action (cumshot / scene / session delete).
  type Pending =
    | { kind: "cumshot"; oId: number }
    | { kind: "scene"; contentItemId: number; title: string; oCount: number }
    | { kind: "session" };
  let pending = $state<Pending | null>(null);
  let busy = $state(false);

  async function deleteCumshot(oId: number) {
    busy = true;
    try {
      await api.oDelete(oId);
      await refresh();
    } catch (e) {
      console.error("delete cumshot failed", e);
    } finally {
      busy = false;
      pending = null;
    }
  }

  async function deleteScene(contentItemId: number) {
    if (!session) return;
    busy = true;
    try {
      await api.sceneDeleteFromSession(session.id, contentItemId);
      await refresh();
    } catch (e) {
      console.error("delete scene from session failed", e);
    } finally {
      busy = false;
      pending = null;
    }
  }

  // Non-destructive: detach the scene from this session but keep its Stash
  // history, so it stays recoverable / re-homeable (own session, fold elsewhere).
  async function detachScene(contentItemId: number) {
    if (!session) return;
    busy = true;
    try {
      await api.sceneDetachFromSession(session.id, contentItemId);
      await refresh();
    } catch (e) {
      console.error("detach scene from session failed", e);
    } finally {
      busy = false;
      pending = null;
    }
  }

  async function deleteSession() {
    if (!session) return;
    busy = true;
    try {
      await api.sessionDelete(session.id);
      pending = null;
      (onDeleted ?? onClose)();
    } catch (e) {
      console.error("delete session failed", e);
      busy = false;
      pending = null;
    }
  }

  // Edit mode: log a cumshot on a specific scene in this session, timestamped at
  // the end of the VISIT the + was clicked on. origin='climax' (via o_log) so it
  // pushes to Stash (sceneAddO). Click again to add more; works on any scene in
  // the session, and on more than one.
  async function addCumshot(scene: PlayedScene, watchedUntil: number) {
    if (!session || busy) return;
    busy = true;
    try {
      // End-of-watch for this visit, clamped into the session window (the backend
      // also enforces bounds; clamp client-side so we never trip its error).
      const end = session.ended_at ?? watchedUntil;
      const at = Math.min(Math.max(watchedUntil, session.started_at), end);
      await api.oLog({
        sessionId: session.id,
        contentItemId: scene.content_item_id,
        occurredAt: at,
      });
      await refresh();
    } catch (e) {
      console.error("add cumshot failed", e);
    } finally {
      busy = false;
    }
  }

  // Edit mode: log a cumshot tied to NO scene ("came to something not in Stash").
  // content_item_id is null, so it's counted in this session's o_count but never
  // pushed to Stash (nothing to attach to). Timestamped at the session end (or now
  // while active), clamped into the window so the backend bound-check passes.
  async function addUnlinkedCumshot() {
    if (!session || busy) return;
    busy = true;
    try {
      const end = session.ended_at ?? Date.now();
      const at = Math.max(session.started_at, end);
      await api.oLog({ sessionId: session.id, occurredAt: at });
      await refresh();
    } catch (e) {
      console.error("add unlinked cumshot failed", e);
    } finally {
      busy = false;
    }
  }

  // An unlinked cumshot has no scene-play to anchor its time, so its timestamp is
  // whatever you set - editable here, clamped into [started_at, ended_at] (the
  // backend also enforces session bounds). Without this the time would be a
  // meaningless wall-clock from whenever the button was pressed (nonsensical when
  // editing an old session). datetime-local has minute precision; that's fine.
  async function setUnlinkedTime(oId: number, ms: number) {
    if (!session) return;
    if (!Number.isFinite(ms)) return;
    const hi = session.ended_at ?? Date.now();
    const clamped = Math.min(Math.max(ms, session.started_at), hi);
    busy = true;
    try {
      await api.oEventUpdate(oId, { occurredAt: clamped });
      await refresh();
    } catch (e) {
      console.error("update unlinked cumshot time failed", e);
    } finally {
      busy = false;
    }
  }

  // ----- "Stash logged activity here" review (fold Stash history into this session) -----
  // Stash play/o history that falls in this session's window but isn't tracked
  // (e.g. watched on another device). Review-first: Add or Skip each.
  let pendingHistory = $state<SessionPendingHistory | null>(null);
  let absorbing = $state(false);
  let checkingStash = $state(false);
  // When on, the review also surfaces previously-skipped items so Skip is
  // reversible (you can re-add them). Toggled from the header.
  let showSkipped = $state(false);
  // Fold the review panel down to its header row (a long list otherwise
  // dominates the card). Per-open state; reopening the detail expands it.
  let pendingCollapsed = $state(false);
  // Monotonic guard: a slow background-poll refresh must not overwrite newer
  // state (e.g. re-show an item just Added/Skipped). Last writer by seq wins.
  let refreshSeq = 0;

  async function pendingAct(fn: () => Promise<void>) {
    if (absorbing) return;
    absorbing = true;
    try {
      await fn();
      await refresh();
    } catch (e) {
      console.error("session history action failed", e);
    } finally {
      absorbing = false;
    }
  }
  const absorbScene = (cid: number) => pendingAct(() => api.sessionAbsorbHistory(sessionId, [cid], []));
  const skipScene = (cid: number) => pendingAct(() => api.sessionDismissHistory(sessionId, [cid], []));
  const absorbO = (oid: number) => pendingAct(() => api.sessionAbsorbHistory(sessionId, [], [oid]));
  const skipO = (oid: number) => pendingAct(() => api.sessionDismissHistory(sessionId, [], [oid]));
  function addAllPending() {
    if (!pendingHistory) return;
    const cids = pendingHistory.scenes.map((s) => s.content_item_id);
    const oids = pendingHistory.cumshots.map((o) => o.o_event_id);
    pendingAct(() => api.sessionAbsorbHistory(sessionId, cids, oids));
  }
  function skipAllPending() {
    if (!pendingHistory) return;
    const cids = pendingHistory.scenes.map((s) => s.content_item_id);
    const oids = pendingHistory.cumshots.map((o) => o.o_event_id);
    pendingAct(() => api.sessionDismissHistory(sessionId, cids, oids));
  }

  // Trigger a Stash mirror sync (runs in the background) then poll a few times so
  // newly-imported play history surfaces in the review without a manual reload.
  // Transient note shown after a manual Check Stash that surfaced nothing, so the
  // button doesn't feel like it did nothing.
  let stashCheckNote = $state<string | null>(null);

  async function checkStash() {
    if (checkingStash) return;
    checkingStash = true;
    stashCheckNote = null;
    const count = () =>
      (pendingHistory?.scenes.length ?? 0) + (pendingHistory?.cumshots.length ?? 0);
    const before = count();
    let found = false;
    try {
      await api.mirrorSyncNow();
      // Poll until the sync surfaces something NEW (the panel may already show
      // bridge-seen scenes), then stop; else give up after ~15s.
      for (let i = 0; i < 10; i++) {
        await new Promise((r) => setTimeout(r, 1500));
        await refresh();
        if (count() > before) {
          found = true;
          break;
        }
      }
    } catch (e) {
      console.error("check stash failed", e);
    } finally {
      checkingStash = false;
    }
    if (!found) {
      stashCheckNote = "Nothing new in Stash for this session's time window.";
      setTimeout(() => (stashCheckNote = null), 5000);
    }
  }

  const confirmTitle = $derived.by(() => {
    if (!pending) return "";
    if (pending.kind === "cumshot") return w("Delete this cumshot?");
    if (pending.kind === "scene") return "Remove this scene from the session?";
    return "Delete this session?";
  });
  const confirmMessage = $derived.by(() => {
    if (!pending) return "";
    if (pending.kind === "cumshot") {
      return "It will be removed from both Climax and Stash. This cannot be undone.";
    }
    if (pending.kind === "scene") {
      // Deliberately "Any cumshots" rather than a count: the old wording read
      // "Its 1 cumshot here are kept too" whenever there was exactly one. The
      // count still gates whether the sentence shows, it just isn't printed.
      const extra =
        pending.oCount > 0
          ? w(" Any cumshots on this scene are kept too, sitting outside any session until you re-home them.")
          : "";
      return (
        `"Remove from session" detaches this scene but KEEPS its history in Stash, so you can re-home it later (its own session, or fold into another).${extra}\n\n` +
        w(`"Delete everywhere" also deletes its plays/cumshots in Stash - use this button only if it never really happened.`)
      );
    }
    return (
      w("Permanently delete this session from Climax. Its watch time, play counts, and cumshots will also be deleted in Stash. This cannot be undone.")
    );
  });

  let session = $state<Session | null>(null);
  // `scenes` is the "part of the session" list — counted plays OR cumshot-
  // tagged scenes. Below-threshold brief-open scenes that never earned a
  // cumshot stay in the DB but never reach this array (filtered on fetch),
  // matching the user's "don't count it as part of the session" intent.
  let scenes = $state<PlayedScene[]>([]);
  let oEvents = $state<OEvent[]>([]);
  let loading = $state(true);
  let now = $state(Date.now());
  let poll: ReturnType<typeof setInterval> | null = null;
  let tick: ReturnType<typeof setInterval> | null = null;
  // Which scene's metadata is currently being force-refreshed (content_item_id).
  let refreshingContentId = $state<number | null>(null);
  // Which scene's overflow-performers popover is open. null = none open.
  /** Which card's overflow-performer popover is open, by watch-block key. */
  let popoverCardId = $state<string | null>(null);

  // The spine's inner scroller. Registered with the nav store so cross-nav
  // goto() can snapshot the INNER scroll position too (detailScroll) — Back
  // then restores both the page scroll and the position within this session.
  let bodyEl = $state<HTMLDivElement | undefined>();
  $effect(() => {
    nav.registerDetailScroller(bodyEl ?? null);
  });

  // Apply a Back-restored inner position once content has loaded. Two frames
  // for the spine layout to settle, plus one late set for image-driven
  // reflow. Applied at most once per mount.
  let initialApplied = false;
  $effect(() => {
    if (initialApplied || initialScroll == null || loading || !bodyEl) return;
    initialApplied = true;
    const el = bodyEl;
    const want = initialScroll;
    requestAnimationFrame(() =>
      requestAnimationFrame(() => {
        el.scrollTop = want;
        setTimeout(() => { el.scrollTop = want; }, 120);
      }),
    );
  });

  /** Max performer chips shown inline on a card before the rest spill into
   *  the +N popover. Two leaves room for the studio chip on the same row
   *  without immediately overflowing. */
  const MAX_VISIBLE_PERFORMERS = 2;

  /** Base pixels-per-hour used as the empty-bucket floor on the spine.
   *  Each hour bucket auto-stretches when it has more scenes than fit at
   *  this base, so user-driven zoom was a no-op for dense days and is
   *  gone. Quiet hours still respect this base. */
  const BASE_PX_PER_HOUR = 180;

  /** Which watch block is currently hovered (cumshot drop, band, or scene
   *  card). When set, every element tied to that block gets highlighted so the
   *  relationship between a cumshot on the spine and the card it belongs to is
   *  visible. Keyed by BLOCK, not scene: watch something twice and hovering the
   *  second visit must not light up the first. */
  let hoveredKey = $state<string | null>(null);

  async function refresh() {
    const my = ++refreshSeq;
    try {
      const [s, sc, oe, ph] = await Promise.all([
        api.sessionActive().then(async (a) => {
          if (a && a.id === sessionId) return a;
          return await api.sessionGet(sessionId);
        }),
        api.sessionScenes(sessionId),
        api.oListForSession(sessionId),
        api.sessionPendingHistory(sessionId, false, showSkipped).catch(() => null),
      ]);
      if (my !== refreshSeq) return; // a newer refresh superseded this one
      session = s;
      scenes = sc.filter((sp) => sp.counted_at !== null || sp.cumshot_count > 0);
      oEvents = oe;
      pendingHistory = ph;
    } catch (e) {
      console.error("session detail refresh failed", e);
    } finally {
      loading = false;
    }
  }

  $effect(() => {
    sessionId; // re-fetch when sessionId changes
    loading = true;
    refresh();
  });

  onMount(() => {
    poll = setInterval(refresh, 5_000);
    tick = setInterval(() => { now = Date.now(); }, 1000);
  });
  onDestroy(() => {
    if (poll) clearInterval(poll);
    if (tick) clearInterval(tick);
    nav.registerDetailScroller(null);
  });

  // ---------- Time math ----------
  const sessionStartMs = $derived(session?.started_at ?? 0);
  // For active sessions, the right edge of the spine is "now". For ended,
  // it's the ended_at.
  const sessionEndMs = $derived(
    session ? (session.ended_at ?? now) : 0,
  );
  const sessionDurationMs = $derived(Math.max(60_000, sessionEndMs - sessionStartMs));

  // ---------- Watch blocks ----------
  //
  // A scene is ONE row in the database however many times you came back to it,
  // but on a timeline that is not what happened. The backend now records each
  // stretch of playback as a RUN (scene_play_runs), and the spine groups those
  // runs into BLOCKS - one card, one visit.
  //
  // WHAT COUNTS AS A SECOND VISIT: Stash logging a second PLAY. Stash records a
  // play per player load that crosses its minimum-play-percent, which is exactly
  // "I opened this again" - so a scene shows twice here precisely when Stash
  // counts it twice, and the two apps can be checked against each other.
  //
  // This deliberately replaced a gap heuristic (a long pause, or another scene
  // playing in between). That could not tell a pause from a reload, and it got
  // it BACKWARDS when two scenes played at once: with a tab left running in the
  // background, the other scene's playback sat between this one's runs and read
  // as a return, while the scene that really was re-opened showed as one. Tested
  // against a real session, it inverted both scenes.
  //
  // Pausing therefore never splits the card. It shows as a break between bands,
  // which is what a pause looks like.
  //
  // Plays with NO runs fall back to a single synthesised block. That covers all
  // history recorded before runs existed, plus reconstructed and absorbed plays,
  // which are built from Stash history and have no run detail to give. Their
  // band ends at last_advance_at (the last moment playback actually moved),
  // which is the closest honest answer available from the aggregate.

  type WatchRun = {
    started_at: number;
    ended_at: number;
    seconds_tracked: number;
    off_bridge: boolean;
  };
  type WatchBlock = {
    /** Stable across refreshes: keys the {#each}, hover linkage and popovers. */
    key: string;
    scene: PlayedScene;
    runs: WatchRun[];
    startMs: number;
    endMs: number;
    /** Watch time inside THIS visit, not the scene's session total. */
    seconds: number;
    /** True when synthesised from the aggregate rather than measured. */
    approximate: boolean;
  };

  const watchBlocks = $derived.by<WatchBlock[]>(() => {
    const out: WatchBlock[] = [];
    for (const scene of scenes) {
      const runs = [...(scene.runs ?? [])].sort((a, b) => a.started_at - b.started_at);

      if (runs.length === 0) {
        const fallbackEnd = scene.last_advance_at ?? scene.first_seen_at + scene.seconds_tracked * 1000;
        out.push({
          key: `${scene.play_id}:0`,
          scene,
          runs: [
            {
              started_at: scene.first_seen_at,
              ended_at: Math.max(fallbackEnd, scene.first_seen_at),
              seconds_tracked: scene.seconds_tracked,
              off_bridge: scene.off_bridge,
            },
          ],
          startMs: scene.first_seen_at,
          endMs: Math.max(fallbackEnd, scene.first_seen_at),
          seconds: scene.seconds_tracked,
          approximate: true,
        });
        continue;
      }

      let group: WatchRun[] = [];
      const flush = () => {
        if (group.length === 0) return;
        out.push({
          key: `${scene.play_id}:${group[0].started_at}`,
          scene,
          runs: group,
          startMs: group[0].started_at,
          endMs: group[group.length - 1].ended_at,
          seconds: group.reduce((s, r) => s + r.seconds_tracked, 0),
          approximate: false,
        });
        group = [];
      };

      // Only the SECOND play onward opens a new card. The first play is just
      // this scene starting, wherever in the runs it happened to register -
      // treating it as a boundary would split a first visit that was too brief
      // for Stash to log at all from the one that followed it.
      const laterPlays = [...(scene.stash_plays ?? [])].sort((a, b) => a - b).slice(1);

      for (const run of runs) {
        // A play "belongs" to the run containing it; a play landing in a gap
        // (the bridge missed the moment it registered) attaches to the next run
        // to start, which is where that viewing actually resumed.
        const startsNewVisit =
          group.length > 0 &&
          laterPlays.some((at) => at <= run.ended_at && at > group[group.length - 1].ended_at);
        if (startsNewVisit) flush();
        group.push(run);
      }
      flush();
    }
    return out.sort((a, b) => a.startMs - b.startMs);
  });

  // ---------- Session start/end editing (edit mode) ----------
  // Local datetime-local input mirrors, seeded when edit mode opens so the 5s
  // poll refresh can't clobber in-progress typing. The window can't be trimmed
  // past the content: start <= the first watched moment, end >= the last watched
  // moment. The backend (update_session) enforces this too; we bound the inputs
  // to guide rather than reject after the fact.
  let editStart = $state("");
  let editEnd = $state("");
  let editNotes = $state("");
  let savingTimes = $state(false);
  let timesError = $state<string | null>(null);

  const hasContent = $derived(scenes.length > 0 || oEvents.length > 0);
  // Earliest / latest watched moment across scenes + cumshots, or null when the
  // session has no content yet (then only start < end is enforced).
  const contentStartMs = $derived.by(() =>
    hasContent
      ? Math.min(...scenes.map((s) => s.first_seen_at), ...oEvents.map((o) => o.occurred_at))
      : null,
  );
  const contentEndMs = $derived.by(() =>
    hasContent
      ? Math.max(...scenes.map((s) => s.last_seen_at), ...oEvents.map((o) => o.occurred_at))
      : null,
  );

  // datetime-local inputs are MINUTE precision but session/scene times carry
  // SECONDS, so compare everything floored to the minute. Without this the seeded
  // end (e.g. "21:55" from a real 21:55:18) reads as both "before the last watched
  // moment" AND "changed", which permanently disabled Save + showed a phantom
  // error - the bug that broke the whole edit panel.
  const MIN_MS = 60_000;
  const floorMin = (ms: number) => Math.floor(ms / MIN_MS) * MIN_MS;

  function seedTimeInputs() {
    if (!session) return;
    editStart = epochToLocalInput(session.started_at);
    editEnd = session.ended_at ? epochToLocalInput(session.ended_at) : "";
    editNotes = session.notes ?? "";
    timesError = null;
  }

  function toggleEdit() {
    editing = !editing;
    if (editing) seedTimeInputs();
  }

  // Freeform per-session note. Saved on blur (optimistic local update so the
  // view reflects it instantly); notes-only update, so no bounds machinery.
  async function saveNotes() {
    if (!session) return;
    const trimmed = editNotes.trim();
    if (trimmed === (session.notes ?? "")) return;
    session.notes = trimmed || null;
    try {
      await api.sessionUpdate(session.id, { notes: trimmed });
    } catch (e) {
      console.error("save notes failed", e);
    }
  }

  // Has the user changed start/end from the saved session?
  const timesChanged = $derived.by(() => {
    if (!session) return false;
    const s = localInputToEpoch(editStart);
    if (!Number.isNaN(s) && floorMin(s) !== floorMin(session.started_at)) return true;
    if (editEnd && session.ended_at !== null) {
      const e = localInputToEpoch(editEnd);
      if (!Number.isNaN(e) && floorMin(e) !== floorMin(session.ended_at)) return true;
    }
    return false;
  });

  // Live validation of the typed window vs the content bounds. null = OK.
  function timesProblem(): string | null {
    if (!editStart) return "Start time is required.";
    const start = localInputToEpoch(editStart);
    if (Number.isNaN(start)) return "Start time is invalid.";
    if (contentStartMs !== null && floorMin(start) > floorMin(contentStartMs))
      return "Start cannot be later than the first thing recorded in this session.";
    if (editEnd) {
      const end = localInputToEpoch(editEnd);
      if (Number.isNaN(end)) return "End time is invalid.";
      if (start >= end) return "Start must be before end.";
      if (contentEndMs !== null && floorMin(end) < floorMin(contentEndMs))
        return "End cannot be earlier than the last thing recorded in this session.";
    }
    return null;
  }
  const liveProblem = $derived.by(timesProblem);

  // The allowed window, shown in the hint so the limits are never a mystery.
  const boundsHint = $derived.by(() => {
    const parts: string[] = [];
    if (contentStartMs !== null) parts.push(`start no later than ${formatTimeOnly(contentStartMs)}`);
    if (contentEndMs !== null) parts.push(`end no earlier than ${formatTimeOnly(contentEndMs)}`);
    if (parts.length === 0) return "Nothing is recorded in this session yet, so you can set any start and end time.";
    return `Set the window. It must ${parts.join(", ")} (the first and last thing recorded).`;
  });

  async function saveTimes() {
    if (!session || savingTimes) return;
    const problem = timesProblem();
    if (problem) {
      timesError = problem;
      return;
    }
    const startMs = localInputToEpoch(editStart);
    let endMs = editEnd ? localInputToEpoch(editEnd) : undefined;
    // A minute-precision end can't represent a second-precise last-watched moment;
    // bump it up so the window still contains the content (no backend orphan).
    if (endMs !== undefined && contentEndMs !== null && endMs < contentEndMs) {
      endMs = contentEndMs;
    }
    // Only send a field whose MINUTE actually changed (the seeded values are
    // minute-truncated, so a raw !== would falsely "change" the end every time).
    const patch: Parameters<typeof api.sessionUpdate>[1] = {};
    if (floorMin(startMs) !== floorMin(session.started_at)) patch.startedAt = startMs;
    if (
      endMs !== undefined &&
      session.ended_at !== null &&
      floorMin(endMs) !== floorMin(session.ended_at)
    ) {
      patch.endedAt = endMs;
    }
    if (Object.keys(patch).length === 0) {
      timesError = null;
      return;
    }
    savingTimes = true;
    timesError = null;
    try {
      await api.sessionUpdate(session.id, patch);
      await refresh();
      seedTimeInputs(); // re-sync inputs to the saved values
    } catch (e) {
      console.error("save session times failed", e);
      const msg = (e instanceof Error ? e.message : String(e)).trim();
      // The backend's time-validation messages are written for the user
      // (plain sentences); anything else (transport / DB failure) is
      // internals - keep it in the console only.
      timesError =
        /^[A-Z]/.test(msg) && msg.endsWith(".") && !msg.includes(": ") && msg.length <= 200
          ? msg
          : "Couldn't save the new times.";
    } finally {
      savingTimes = false;
    }
  }

  // ---------- Card layout constants ----------
  // CARD_HEIGHT is the rendered slot height we reserve per card. The
  // pushdown logic and the hour-bucket sizing both rely on it. Bumped to
  // accommodate the performer + studio pill rows (two-line meta layout).
  const CARD_HEIGHT = 152;
  const CARD_GAP = 10;
  const BUCKET_BOTTOM_PADDING = 16;
  const HOUR_MS = 3_600_000;
  // Min vertical gap between same-side cumshot drops; also reserved in a
  // segment's height so a cumshot-heavy stretch has room for its drops.
  const DROP_MIN_GAP_PX = 24;
  // The closest a drop may sit to the top of the timeline. The session's start
  // time label lives there, is opaque, is drawn ABOVE the drops, and (unlike
  // every other time mark) is never removed to make way for one. So a cumshot
  // logged in the first moments of a session used to be drawn exactly under it,
  // fully covered: the count said 3 and the timeline showed 2. Half the label
  // (13px) plus half a drop (10px), plus a pixel of air.
  const START_MARK_CLEAR_PX = 24;
  // Comfortable minimum height for the gap between two adjacent time marks.
  // Quiet stretches collapse to this floor; busy ones expand past it. This is
  // the lever that keeps short sessions legible without over-stretching long,
  // sparse ones (their proportional floor stays higher than this anyway).
  const MIN_SEGMENT_PX = 88;

  // ---------- Elastic time segments ----------
  // The spine is a sequence of segments BETWEEN adaptive time marks. Each
  // segment's height = max of (a) a time-proportional floor (so long sessions
  // keep their familiar density), (b) the comfortable MIN_SEGMENT_PX floor, and
  // (c) the room its scenes (cards stack per side) + cumshots (drops, ~half per
  // side) actually need. So the distance between two marks GROWS where content
  // crowds rather than cramming it. Within a segment, time→px is linear.

  // Adaptive spacing between time marks, from the session length.
  const tickIntervalMs = $derived.by(() => {
    const dur = sessionDurationMs;
    return dur > 3 * HOUR_MS ? HOUR_MS
      : dur > 80 * 60_000 ? 30 * 60_000
      : dur > 40 * 60_000 ? 15 * 60_000
      : dur > 16 * 60_000 ? 10 * 60_000
      : 5 * 60_000;
  });

  // Segment boundaries: session start, each interval mark inside the session,
  // and the session end. The marks render at these boundaries.
  const segmentBounds = $derived.by<number[]>(() => {
    if (!session) return [];
    const interval = tickIntervalMs;
    const bounds = [sessionStartMs];
    const firstMark = Math.ceil((sessionStartMs + 1) / interval) * interval;
    for (let t = firstMark; t < sessionEndMs; t += interval) bounds.push(t);
    bounds.push(sessionEndMs);
    return bounds.filter((v, i) => i === 0 || v > bounds[i - 1]);
  });

  type TimeBucket = { startMs: number; endMs: number; height: number; top: number };

  const timeBuckets = $derived.by<TimeBucket[]>(() => {
    if (!session) return [];
    const bounds = segmentBounds;
    const out: TimeBucket[] = [];
    for (let i = 0; i < bounds.length - 1; i++) {
      const bStart = bounds[i];
      const bEnd = bounds[i + 1];
      if (bEnd <= bStart) continue;
      const timeFloor = BASE_PX_PER_HOUR * ((bEnd - bStart) / HOUR_MS);
      // Visits anchored in this segment — cards stack per side. Counts BLOCKS,
      // not scenes: a scene returned to twice needs room for two cards.
      const sceneCount = watchBlocks.filter(
        (b) => b.startMs >= bStart && b.startMs < bEnd,
      ).length;
      const sameSideScenes = Math.ceil(sceneCount / 2);
      const sceneNeed = sameSideScenes > 0
        ? sameSideScenes * (CARD_HEIGHT + CARD_GAP) + BUCKET_BOTTOM_PADDING
        : 0;
      // Cumshots in this segment — drops split L/R, so ~half per side need room.
      const cumshotCount = oEvents.filter(
        (oe) => oe.occurred_at >= bStart && oe.occurred_at < bEnd,
      ).length;
      const sameSideCumshots = Math.ceil(cumshotCount / 2);
      const cumshotNeed = cumshotCount > 0
        ? sameSideCumshots * (DROP_MIN_GAP_PX + 8) + 28
        : 0;
      out.push({
        startMs: bStart,
        endMs: bEnd,
        height: Math.max(timeFloor, MIN_SEGMENT_PX, sceneNeed, cumshotNeed),
        top: 0,
      });
    }
    let cum = 0;
    for (const b of out) {
      b.top = cum;
      cum += b.height;
    }
    return out;
  });

  const totalSpineHeightPx = $derived(
    timeBuckets.reduce((s, b) => s + b.height, 0),
  );

  function timeToPx(atMs: number): number {
    if (timeBuckets.length === 0) return 0;
    const clamped = Math.max(sessionStartMs, Math.min(sessionEndMs, atMs));
    const bucket =
      timeBuckets.find((b) => clamped >= b.startMs && clamped < b.endMs) ??
      timeBuckets[timeBuckets.length - 1];
    const span = bucket.endMs - bucket.startMs;
    const fraction = span > 0 ? (clamped - bucket.startMs) / span : 0;
    return bucket.top + fraction * bucket.height;
  }

  // ---------- Card positioning with overlap pushdown ----------
  // With the bucket-stretching above, pushdown should rarely fire — but
  // it still handles sub-hour clustering (e.g. 5 scenes in 10 minutes
  // inside a single bucket that's stretched to fit the AVERAGE density).
  type PositionedCard = {
    block: WatchBlock;
    /** Convenience alias for block.scene - the card's metadata all comes from it. */
    scene: PlayedScene;
    side: "left" | "right";
    anchorPx: number;
    topPx: number;
  };

  const positionedCards = $derived.by<PositionedCard[]>(() => {
    let lastBottomLeft = -Infinity;
    let lastBottomRight = -Infinity;
    return watchBlocks.map((block, i) => {
      const scene = block.scene;
      const side = i % 2 === 0 ? "left" : "right";
      const anchorPx = timeToPx(block.startMs);
      // Align top of card slightly above the anchor so the anchor sits
      // visually near the card's title row, not its top edge.
      const idealTop = anchorPx - 14;
      const lastBottom = side === "left" ? lastBottomLeft : lastBottomRight;
      const topPx = Math.max(idealTop, lastBottom + CARD_GAP);
      const bottom = topPx + CARD_HEIGHT;
      if (side === "left") lastBottomLeft = bottom;
      else lastBottomRight = bottom;
      return { block, scene, side, anchorPx, topPx };
    });
  });

  // ---------- Cumshot drop layout ----------
  // Drops sit just OFF the spine, on the SAME side as their scene's card, so
  // left-scene and right-scene cumshots separate cleanly and never land on top
  // of the centred time pills. Same-side drops that would overlap are nudged
  // down by a small gap (their exact time still shows on the card's pill).
  //
  // A cumshot belongs to ONE visit, not just to a scene: with the same scene
  // watched twice, it has to attach to the card it actually happened during, or
  // the highlight links the wrong pair and the pill appears under the wrong
  // card. Pick the visit whose window contains it, else the nearest one - a
  // cumshot logged at the wrap-up, minutes after playback stopped, still belongs
  // to the last visit rather than to nothing.
  const cardForCumshot = $derived.by(() => {
    const m = new Map<number, PositionedCard>();
    for (const oe of oEvents) {
      if (oe.content_item_id == null) continue;
      const candidates = positionedCards.filter(
        (pc) => pc.scene.content_item_id === oe.content_item_id,
      );
      if (candidates.length === 0) continue;
      const at = oe.occurred_at;
      const containing = candidates.find((pc) => at >= pc.block.startMs && at <= pc.block.endMs);
      const chosen =
        containing ??
        candidates.reduce((best, pc) => {
          const d = (p: PositionedCard) =>
            at < p.block.startMs ? p.block.startMs - at : at - p.block.endMs;
          return d(pc) < d(best) ? pc : best;
        });
      m.set(oe.id, chosen);
    }
    return m;
  });

  type PositionedDrop = {
    oe: OEvent;
    side: "left" | "right";
    topPx: number;
    /** Visit this cumshot belongs to; null when unlinked (no scene). */
    blockKey: string | null;
  };
  const positionedDrops = $derived.by<PositionedDrop[]>(() => {
    const sorted = [...oEvents].sort((a, b) => a.occurred_at - b.occurred_at);
    let lastLeft = -Infinity;
    let lastRight = -Infinity;
    return sorted.map((oe) => {
      const card = cardForCumshot.get(oe.id);
      const side = card?.side ?? "left";
      const blockKey = card?.block.key ?? null;
      // Never above the start label's footprint - see START_MARK_CLEAR_PX. The
      // exact time is still on the drop's tooltip, so nudging it down a few
      // pixels costs nothing and hiding it costs the whole mark.
      const ideal = Math.max(timeToPx(oe.occurred_at), START_MARK_CLEAR_PX);
      let topPx: number;
      if (side === "left") {
        topPx = Math.max(ideal, lastLeft + DROP_MIN_GAP_PX);
        lastLeft = topPx;
      } else {
        topPx = Math.max(ideal, lastRight + DROP_MIN_GAP_PX);
        lastRight = topPx;
      }
      return { oe, side, topPx, blockKey };
    });
  });

  // The time-based spine height (totalSpineHeightPx) is only a FLOOR. Overlap-
  // pushdown can land a card BELOW it — common in short / dense sessions, and in
  // reconstructed ones whose last event anchors a card at the very end — and the
  // canvas was sized to that floor alone, so the content overflowed into a
  // scrollbar and the panel looked cropped + too short. Grow the spine to the
  // lowest card / drop so the panel always sizes to its real content extent.
  const contentBottomPx = $derived.by(() => {
    let maxB = 0;
    for (const pc of positionedCards) maxB = Math.max(maxB, pc.topPx + CARD_HEIGHT);
    for (const pd of positionedDrops) maxB = Math.max(maxB, pd.topPx + DROP_MIN_GAP_PX);
    return maxB > 0 ? maxB + BUCKET_BOTTOM_PADDING : 0;
  });
  const spineHeightPx = $derived(Math.max(totalSpineHeightPx, contentBottomPx));

  // ---------- Time marks ----------
  // One mark at each segment boundary (session start + interval marks), placed
  // at the segment top. Since every segment is ≥ MIN_SEGMENT_PX, the marks are
  // always comfortably apart. A mark is dropped if a cumshot lands right on it
  // (the drop wins — its time is on the card pill anyway); the start mark is
  // always kept as the top reference.
  type TimeTick = { atMs: number; topPx: number; label: string };
  // How close a drop may come to a time mark before the mark gives way. It has
  // to cover the two half-heights (label 13px, drop 10px): at the old 15, a drop
  // 15 to 22px from a mark kept the mark and sat up to a third hidden under it.
  const TICK_DROP_CLEAR_PX = 23;
  const timeTicks = $derived.by<TimeTick[]>(() => {
    if (!session) return [];
    const dropTops = positionedDrops.map((d) => d.topPx);
    return timeBuckets
      .map((b) => ({ atMs: b.startMs, topPx: b.top, label: formatTimeOnly(b.startMs) }))
      .filter(
        (tk, idx) =>
          idx === 0 ||
          !dropTops.some((dt) => Math.abs(dt - tk.topPx) < TICK_DROP_CLEAR_PX),
      );
  });

  // ---------- Aggregates ----------
  const totalSceneTimeMs = $derived(
    scenes.reduce((s, sc) => s + sc.seconds_tracked * 1000, 0),
  );
  const totalCumshots = $derived(oEvents.length);
  // Cumshots in this session not tied to any scene (content_item_id null) - listed
  // + addable/deletable in edit mode, since they have no scene card to live on.
  const unlinkedOEvents = $derived(oEvents.filter((oe) => oe.content_item_id == null));

  /** Per-scene watch time as an explicit, labelled duration: "45s watched",
   *  "17m watched", "1h 37m watched". Takes seconds_tracked. */
  function watchedLabel(secs: number): string {
    if (secs < 60) return `${Math.max(0, Math.round(secs))}s watched`;
    const totalMin = Math.floor(secs / 60);
    const h = Math.floor(totalMin / 60);
    const m = totalMin % 60;
    if (h === 0) return `${m}m watched`;
    if (m === 0) return `${h}h watched`;
    return `${h}h ${m}m watched`;
  }

  function performersFor(s: PlayedScene): Performer[] {
    const meta = parseSceneMetadata(s.metadata_json);
    return meta?.performers ?? [];
  }
  function studioFor(s: PlayedScene): Studio | null {
    const meta = parseSceneMetadata(s.metadata_json);
    return meta?.studio?.name ? meta.studio : null;
  }

  /** Cumshots belonging to one VISIT, so a scene watched twice lists each pill
   *  under the card it happened during rather than repeating them on both. */
  function cumshotsForBlock(key: string): OEvent[] {
    return oEvents.filter((oe) => cardForCumshot.get(oe.id)?.block.key === key);
  }

  function selectPerformer(p: Performer) {
    onPerformerSelect?.(p);
  }
  function selectStudio(s: Studio) {
    onStudioSelect?.(s);
  }
  function selectScene(contentItemId: number) {
    onSceneSelect?.(contentItemId);
  }

  /** Click anywhere outside the popover or press Escape → close. The
   *  popover-trigger button calls stopPropagation so its own click
   *  doesn't immediately close what it just opened. */
  $effect(() => {
    if (popoverCardId === null) return;
    function onDocClick(e: MouseEvent) {
      // composedPath(), not target.closest() - see FilterBar: a node removed by
      // its own click handler is detached by the time this runs, so closest()
      // returns null and an inside click reads as outside.
      const inside = e.composedPath().some(
        (n) => n instanceof Element && (n.classList.contains("performer-popover")),
      );
      if (inside) return;
      popoverCardId = null;
    }
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") popoverCardId = null;
    }
    document.addEventListener("click", onDocClick);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("click", onDocClick);
      document.removeEventListener("keydown", onKey);
    };
  });

  /** Force-refresh a single scene's Stash metadata. Ignores the TTL check.
   *  The backend kicks off the GraphQL fetch in the background, so we wait
   *  a beat before re-fetching so the UI shows the updated row. */
  async function refreshSceneMetadata(contentItemId: number) {
    if (refreshingContentId !== null) return; // one at a time is fine
    refreshingContentId = contentItemId;
    try {
      await api.refreshSceneMetadata(contentItemId);
      // Stash GraphQL roundtrip on localhost is ~50-200ms; 800ms is a safe
      // buffer that covers slow Stash startup too.
      await new Promise((r) => setTimeout(r, 800));
      await refresh();
    } catch (e) {
      console.error("refresh scene metadata failed", e);
    } finally {
      refreshingContentId = null;
    }
  }

</script>

<div class="detail-card">
  <header>
    <div class="head-left">
      <div class="title-row">
        <h2>Session detail</h2>
        {#if session}
          <span class="time-range">
            {formatTimeOnly(session.started_at)} - {session.ended_at ? formatTimeOnly(session.ended_at) : "now"}
          </span>
        {/if}
      </div>
      {#if session}
        <div class="head-totals">
          <span><strong>{formatDuration(session.effective_duration_ms)}</strong> session time</span>
          <span><strong>{formatDuration(totalSceneTimeMs)}</strong> watched</span>
          <span>{scenes.length} scene{scenes.length === 1 ? "" : "s"}</span>
          <span class="cumshot-count">{totalCumshots} {w(totalCumshots === 1 ? "cumshot" : "cumshots")}</span>
        </div>
      {/if}
    </div>
    <div class="head-right">
      {#if session}
        {#if onOpenInSessions}
          <button
            class="hdr-btn"
            onclick={onOpenInSessions}
            title="Open this session in the Sessions page"
          >
            <Icon name="external-link" size={13} />
            <span>Open in Sessions</span>
          </button>
        {/if}
        {#if session.status === "ended"}
          <button
            class="hdr-btn"
            onclick={checkStash}
            disabled={checkingStash || absorbing}
            title="Re-check Stash for activity in this session's time window that isn't counted yet."
          >
            <Icon name="refresh-cw" size={13} />
            <span>{checkingStash ? "Checking..." : "Check Stash"}</span>
          </button>
          <button
            class="hdr-btn"
            class:active={showSkipped}
            onclick={() => { showSkipped = !showSkipped; refresh(); }}
            title="Show items you previously skipped here, so you can re-add them"
          >
            <Icon name="list" size={13} />
            <span>{showSkipped ? "Hide skipped" : "Show skipped"}</span>
          </button>
        {/if}
        <button
          class="hdr-btn"
          class:active={editing}
          onclick={toggleEdit}
          title={editing ? "Finish editing" : "Edit this session"}
        >
          <Icon name="edit" size={13} />
          <span>{editing ? "Done" : "Edit session"}</span>
        </button>
        <button
          class="hdr-btn danger"
          onclick={() => (pending = { kind: "session" })}
          title="Delete this session"
        >
          <Icon name="trash-2" size={13} />
          <span>Delete</span>
        </button>
      {/if}
      <button class="close-btn" onclick={onClose} aria-label="Close detail">
        <Icon name="x" size={16} />
      </button>
    </div>
  </header>

  {#if loading && !session}
    <div class="loading">loading...</div>
  {:else if !session}
    <div class="loading">Session not found.</div>
  {:else}
    {#if editing}
      <div class="time-edit">
        <div class="te-row">
          <div class="te-field">
            <span class="te-label">Start</span>
            <DateTimePicker
              value={localInputToEpoch(editStart) || Date.now()}
              onChange={(ms) => (editStart = epochToLocalInput(ms))}
            />
          </div>
          <div class="te-field">
            <span class="te-label">End</span>
            <DateTimePicker
              value={localInputToEpoch(editEnd) || Date.now()}
              disabled={session.status === "active"}
              onChange={(ms) => (editEnd = epochToLocalInput(ms))}
            />
          </div>
          <button class="te-save" onclick={saveTimes} disabled={savingTimes || !timesChanged || liveProblem !== null}>
            {savingTimes ? "Saving..." : "Save times"}
          </button>
        </div>
        {#if timesError}
          <div class="te-msg err">{w(timesError)}</div>
        {:else if timesChanged && liveProblem}
          <div class="te-msg warn">{liveProblem}</div>
        {:else}
          <div class="te-msg hint">
            {boundsHint}{session.status === "active" ? " End is locked while the session is active." : ""}
          </div>
        {/if}
        <div class="te-unlinked">
          <span class="te-label">{w("Unlinked cumshots")}</span>
          {#if unlinkedOEvents.length > 0}
            <div class="unlinked-items">
              {#each unlinkedOEvents as oe (oe.id)}
                <div class="unlinked-item">
                  <Icon name="cumshot" size={13} filled color="var(--accent)" />
                  <DateTimePicker
                    value={oe.occurred_at}
                    min={session.started_at}
                    max={session.ended_at ?? Date.now()}
                    disabled={busy}
                    onChange={(ms) => setUnlinkedTime(oe.id, ms)}
                  />
                  <button
                    class="cumshot-del"
                    onclick={() => (pending = { kind: "cumshot", oId: oe.id })}
                    aria-label={w("Delete this cumshot")}
                    title={w("Delete cumshot")}
                  ><Icon name="x" size={10} /></button>
                </div>
              {/each}
            </div>
          {/if}
          <button
            class="te-add-unlinked"
            disabled={busy}
            onclick={addUnlinkedCumshot}
            title={w("Log a cumshot with no scene attached. It counts towards this session but is never sent to Stash.")}
          >
            <Icon name="cumshot" size={12} filled color="var(--accent)" />
            <Icon name="plus" size={10} />
            <span>{w("Add cumshot (no scene)")}</span>
          </button>
          <div class="te-msg hint">No scene attached, so nothing is sent to Stash. Its time always stays inside the session.</div>
        </div>
        <div class="te-notes">
          <span class="te-label">Notes</span>
          <textarea
            class="te-notes-input"
            bind:value={editNotes}
            onblur={saveNotes}
            placeholder="Add a note about this session (optional)"
            rows="2"
          ></textarea>
        </div>
      </div>
    {/if}
    {#if !editing && session && session.notes}
      <div class="notes-view">
        <span class="notes-view-label">Notes</span>
        <p class="notes-view-text">{session.notes}</p>
      </div>
    {/if}
    {#if stashCheckNote}
      <div class="stash-note">{stashCheckNote}</div>
    {/if}
    {#if pendingHistory && (pendingHistory.scenes.length > 0 || pendingHistory.cumshots.length > 0)}
      {@const pendingCount = pendingHistory.scenes.length + pendingHistory.cumshots.length}
      <div class="stash-pending">
        <div class="sp-head">
          <Icon name="video" size={14} color="var(--accent)" />
          <button
            class="sp-fold"
            onclick={() => (pendingCollapsed = !pendingCollapsed)}
            title={pendingCollapsed ? "Expand this list" : "Collapse this list"}
          >
            <span class="sp-title">Scenes not counted in this session</span>
            {#if pendingCollapsed}
              <span class="sp-count">{pendingCount} item{pendingCount === 1 ? "" : "s"}</span>
            {/if}
            <Icon name={pendingCollapsed ? "chevron-down" : "chevron-up"} size={13} color="var(--fg-muted)" />
          </button>
          {#if !pendingCollapsed}
            <button class="sp-addall" disabled={absorbing} onclick={addAllPending}>Add all</button>
            <button class="sp-skipall" disabled={absorbing} onclick={skipAllPending} title="Skip everything listed here; bring items back via Show skipped">Skip all</button>
          {/if}
        </div>
        {#if !pendingCollapsed}
        <!-- The {" "} is deliberate: a literal leading space inside the {#if}
             gets trimmed, which ran this straight onto the previous full stop
             ("...estimated).The list now..."). An explicit expression survives. -->
        <div class="sp-sub">These scenes were open during the session but were not counted as watched, either because they were too short or because playback never moved. Add any you actually watched. Watch times are estimates.{#if showSkipped}{" "}The list now also includes items you skipped earlier. Adding one puts it back into the session.{/if}</div>
        <div class="sp-list">
          {#each pendingHistory.scenes as s (s.content_item_id)}
            <div class="sp-row">
              {#if s.thumbnail_url}
                <img class="sp-thumb" src={s.thumbnail_url} alt="" />
              {:else}
                <div class="sp-thumb ph"><Icon name="clapperboard" size={16} color="var(--fg-muted)" /></div>
              {/if}
              <div class="sp-meta">
                <span class="sp-name">{s.title ?? `Scene ${s.external_id ?? s.content_item_id}`}</span>
                <span class="sp-info">
                  {s.play_times.length > 0
                    ? `${s.play_times.length} play${s.play_times.length === 1 ? "" : "s"}`
                    : "seen here"} · ~{formatDuration(s.est_seconds * 1000)}
                </span>
              </div>
              <div class="sp-acts">
                <button class="sp-add" disabled={absorbing} onclick={() => absorbScene(s.content_item_id)}>Add</button>
                <button class="sp-skip" disabled={absorbing} onclick={() => skipScene(s.content_item_id)}>Skip</button>
              </div>
            </div>
          {/each}
          {#each pendingHistory.cumshots as o (o.o_event_id)}
            <div class="sp-row">
              <div class="sp-thumb ph"><Icon name="cumshot" size={14} filled color="var(--accent)" /></div>
              <div class="sp-meta">
                <span class="sp-name">{w("Cumshot")}</span>
                <span class="sp-info">{formatTimeOnly(o.occurred_at)} · from Stash</span>
              </div>
              <div class="sp-acts">
                <button class="sp-add" disabled={absorbing} onclick={() => absorbO(o.o_event_id)}>Add</button>
                <button class="sp-skip" disabled={absorbing} onclick={() => skipO(o.o_event_id)}>Skip</button>
              </div>
            </div>
          {/each}
        </div>
        {/if}
      </div>
    {/if}
    <div class="body" bind:this={bodyEl}>
      <div class="canvas" style={`height: ${spineHeightPx}px;`}>
        <!-- Central spine line -->
        <div class="spine"></div>

        <!-- Time labels sitting on the spine. Above the drops (z-order) so a
             cumshot near a tick never hides it. -->
        {#each timeTicks as h (h.atMs)}
          <div class="hour-mark" style={`top: ${h.topPx}px;`}>
            <span class="hour-pill">{h.label}</span>
          </div>
        {/each}

        <!-- Watch bands: one per RUN of playback, so the coral marks when the
             video was actually moving and a pause shows as a break rather than
             being painted over. Highlights with its card + connector.

             This used to be a single band per scene, drawn as
             first_seen .. first_seen + seconds_tracked - a duration laid on a
             timeline, which silently assumed you watched straight through from
             the moment the scene opened. Pause, rewind or switch away and it ran
             out early, dragging everything anchored to real time (the cumshot
             drops especially) outside it.

             A play with no recorded runs gets one synthesised band ending at
             last_advance_at; see the watchBlocks comment for which plays those
             are and why they can never be better than that. -->
        {#each watchBlocks as blk (blk.key)}
          {#each blk.runs as run, i (i)}
            {@const startPx = timeToPx(run.started_at)}
            {@const endPx = timeToPx(run.ended_at)}
            {@const bandH = Math.max(2, endPx - startPx)}
            <div
              class="band"
              class:highlighted={hoveredKey === blk.key}
              class:faded={hoveredKey !== null && hoveredKey !== blk.key}
              style={`top: ${startPx}px; height: ${bandH}px;`}
              title={`${blk.scene.title ?? "Scene"} • ${formatDuration(blk.seconds * 1000)} watched`}
            ></div>
          {/each}
        {/each}

        <!-- Cumshot drops, offset to their scene's side of the spine.
             Hovering one highlights the scene card + band + connector it
             belongs to (matched by content_item_id). -->
        {#each positionedDrops as d (d.oe.id)}
          {@const unlinked = d.oe.content_item_id == null}
          <!-- svelte-ignore a11y_no_static_element_interactions -->
          <div
            class={`cumshot-drop ${d.side}`}
            class:unlinked
            class:highlighted={hoveredKey !== null && d.blockKey === hoveredKey}
            style={`top: ${d.topPx}px;`}
            onmouseenter={() => (hoveredKey = d.blockKey)}
            onmouseleave={() => (hoveredKey = null)}
            title={`${formatTimeOnly(d.oe.occurred_at)} • ${w(unlinked ? "unlinked cumshot (no scene)" : "cumshot")}`}
          >
            <Icon name="cumshot" size={14} filled color="var(--accent)" />
          </div>
        {/each}

        <!-- Cards + connectors -->
        {#each positionedCards as pc (pc.block.key)}
          {@const performers = performersFor(pc.scene)}
          {@const studio = studioFor(pc.scene)}
          {@const visiblePerformers = performers.slice(0, MAX_VISIBLE_PERFORMERS)}
          {@const overflowPerformers = performers.slice(MAX_VISIBLE_PERFORMERS)}
          {@const cumshots = cumshotsForBlock(pc.block.key)}
          {@const landingY = pc.topPx + 14}
          {@const vDrop = Math.max(0, landingY - pc.anchorPx)}

          <!-- Connector: horizontal stub from spine to the card's inner
               edge at the anchor's Y, plus an optional vertical drop
               down to the card's title-row Y when the card was shifted
               to avoid overlap. -->
          <div
            class={`connector h ${pc.side}`}
            class:highlighted={hoveredKey === pc.block.key}
            style={`top: ${pc.anchorPx}px;`}
          ></div>
          {#if vDrop > 0.5}
            <div
              class={`connector v ${pc.side}`}
              class:highlighted={hoveredKey === pc.block.key}
              style={`top: ${pc.anchorPx}px; height: ${vDrop}px;`}
            ></div>
          {/if}

          <!-- The card itself -->
          <!-- svelte-ignore a11y_no_static_element_interactions -->
          <div
            class={`scene-card ${pc.side}`}
            class:highlighted={hoveredKey === pc.block.key}
            class:dimmed={hoveredKey !== null && hoveredKey !== pc.block.key}
            style={`top: ${pc.topPx}px;`}
            onmouseenter={() => (hoveredKey = pc.block.key)}
            onmouseleave={() => (hoveredKey = null)}
          >
            <button
              class="thumb-link"
              onclick={() => selectScene(pc.scene.content_item_id)}
              title="Open this scene's page"
              aria-label="Open this scene's page"
            >
              {#if pc.scene.thumbnail_url}
                <img class="thumb" src={pc.scene.thumbnail_url} alt="" />
              {:else}
                <div class="thumb placeholder">
                  <Icon name="clapperboard" size={28} color="var(--fg-muted)" />
                </div>
              {/if}
            </button>
            <div class="card-info">
              <div class="card-top">
                <!-- The VISIT's own start and watch time, not the scene's
                     session totals: a scene returned to shows one card per
                     visit, and each has to describe itself. -->
                <span class="card-time">{formatTimeOnly(pc.block.startMs)}</span>
                <span class="card-watched">{watchedLabel(pc.block.seconds)}</span>
                <button
                  class="card-refresh"
                  class:spinning={refreshingContentId === pc.scene.content_item_id}
                  disabled={refreshingContentId !== null}
                  onclick={() => refreshSceneMetadata(pc.scene.content_item_id)}
                  aria-label="Refresh this scene's metadata from Stash"
                  title="Refresh from Stash"
                >
                  <Icon name="refresh-cw" size={11} />
                </button>
                {#if editing}
                  <button
                    class="card-del"
                    onclick={(e) => {
                      e.stopPropagation();
                      pending = {
                        kind: "scene",
                        contentItemId: pc.scene.content_item_id,
                        title: pc.scene.title ?? `Scene ${pc.scene.external_id ?? pc.scene.content_item_id}`,
                        oCount: cumshots.length,
                      };
                    }}
                    aria-label="Remove this scene from the session"
                    title="Remove scene from session"
                  >
                    <Icon name="trash-2" size={11} />
                  </button>
                {/if}
              </div>
              <button
                class="card-title"
                onclick={() => selectScene(pc.scene.content_item_id)}
                title="Open this scene's page"
              >
                {pc.scene.title ?? `Scene ${pc.scene.external_id ?? pc.scene.content_item_id}`}
              </button>
              {#if studio}
                <div class="card-pills studio-row">
                  <button
                    class="pill studio-pill"
                    onclick={() => selectStudio(studio)}
                    title={`Studio: ${studio.name}`}
                  >
                    <Icon name="clapperboard" size={10} />
                    <span>{studio.name}</span>
                  </button>
                </div>
              {/if}
              {#if performers.length > 0}
                <div class="card-pills">
                  {#each visiblePerformers as p (p.id)}
                    <button
                      class="pill performer-pill"
                      onclick={() => selectPerformer(p)}
                      title={p.name}
                    >{p.name}</button>
                  {/each}
                  {#if overflowPerformers.length > 0}
                    <button
                      class="pill overflow-pill"
                      onclick={(e) => {
                        e.stopPropagation();
                        popoverCardId = popoverCardId === pc.block.key
                          ? null
                          : pc.block.key;
                      }}
                      title={`${overflowPerformers.length} more performer${overflowPerformers.length === 1 ? "" : "s"}`}
                      aria-expanded={popoverCardId === pc.block.key}
                    >+{overflowPerformers.length}</button>
                  {/if}
                </div>
              {/if}
              {#if cumshots.length > 0 || editing}
                <div class="card-cumshots">
                  {#each cumshots as oe (oe.id)}
                    <span class="cumshot-pill" class:editing title={formatTimeOnly(oe.occurred_at)}>
                      <Icon name="cumshot" size={10} filled />
                      <span>{formatTimeOnly(oe.occurred_at)}</span>
                      {#if editing}
                        <button
                          class="cumshot-del"
                          onclick={(e) => { e.stopPropagation(); pending = { kind: "cumshot", oId: oe.id }; }}
                          aria-label={w("Delete this cumshot")}
                          title={w("Delete cumshot")}
                        ><Icon name="x" size={9} /></button>
                      {/if}
                    </span>
                  {/each}
                  {#if editing}
                    <button
                      class="cumshot-add"
                      disabled={busy}
                      onclick={(e) => { e.stopPropagation(); addCumshot(pc.scene, pc.block.endMs); }}
                      title={w("Log a cumshot on this scene. It counts towards this session and is sent to Stash.")}
                      aria-label={w("Log a cumshot on this scene")}
                    >
                      <Icon name="cumshot" size={10} filled />
                      <Icon name="plus" size={9} />
                    </button>
                  {/if}
                </div>
              {/if}
            </div>
          </div>

          <!-- Overflow-performers popover, sibling of the card so it's
               not clipped by .scene-card's overflow:hidden. Sits just
               below the card on the same side. -->
          {#if popoverCardId === pc.block.key && overflowPerformers.length > 0}
            <div
              class={`performer-popover ${pc.side}`}
              style={`top: ${pc.topPx + CARD_HEIGHT + 6}px;`}
              role="dialog"
              aria-label="Additional performers"
            >
              <div class="popover-head">
                <span class="popover-title">
                  {performers.length} performer{performers.length === 1 ? "" : "s"}
                </span>
                <button
                  class="popover-close"
                  onclick={() => (popoverCardId = null)}
                  aria-label="Close"
                  title="Close"
                ><Icon name="x" size={12} /></button>
              </div>
              <div class="popover-body">
                {#each overflowPerformers as p (p.id)}
                  <button
                    class="pill performer-pill"
                    onclick={() => { selectPerformer(p); popoverCardId = null; }}
                    title={p.name}
                  >{p.name}</button>
                {/each}
              </div>
            </div>
          {/if}
        {/each}
      </div>
    </div>
  {/if}
</div>

{#if pending}
  <ConfirmDialog
    title={confirmTitle}
    message={confirmMessage}
    confirmLabel={pending.kind === "scene" ? "Remove from session" : "Delete"}
    variant={pending.kind === "scene" ? "default" : "danger"}
    extraLabel={pending.kind === "scene" ? "Delete everywhere" : undefined}
    onExtra={pending.kind === "scene"
      ? () => { if (!busy && pending?.kind === "scene") deleteScene(pending.contentItemId); }
      : undefined}
    onConfirm={() => {
      if (busy || !pending) return;
      if (pending.kind === "cumshot") deleteCumshot(pending.oId);
      else if (pending.kind === "scene") detachScene(pending.contentItemId);
      else deleteSession();
    }}
    onCancel={() => (pending = null)}
  />
{/if}

<style>
  .detail-card {
    background: var(--bg-card);
    border: 1px solid var(--accent);
    border-radius: var(--radius-lg, 10px);
    padding: 14px 16px 14px;
    /* Height tracks content: a small session is a small card (no empty space),
       capped near full-screen — past that the body scrolls internally. */
    max-height: 80vh;
    min-height: 200px;
    display: flex;
    flex-direction: column;
    animation: pop-in 220ms cubic-bezier(0.2, 0.8, 0.2, 1);
  }
  @keyframes pop-in {
    from { transform: translateY(-6px); opacity: 0 }
    to   { transform: none; opacity: 1 }
  }

  header {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    gap: 12px;
    padding-bottom: 12px;
    margin-bottom: 8px;
    border-bottom: 1px solid var(--border);
    flex-shrink: 0;
  }
  .head-left { flex: 1; min-width: 0; }
  .head-right { display: flex; align-items: center; gap: 8px; flex-shrink: 0; }

  .title-row { display: flex; align-items: baseline; gap: 12px; flex-wrap: wrap; }
  .title-row h2 {
    margin: 0;
    font-size: 14px;
    color: var(--text);
    font-weight: 600;
    letter-spacing: -0.01em;
  }
  .time-range {
    font-size: 12px;
    color: var(--text-muted);
    font-variant-numeric: tabular-nums;
  }

  .head-totals {
    display: flex;
    gap: 14px;
    margin-top: 6px;
    font-size: 12px;
    color: var(--text-muted);
    font-variant-numeric: tabular-nums;
    flex-wrap: wrap;
  }
  .head-totals strong { color: var(--text); font-weight: 600; }
  .cumshot-count { color: var(--accent); font-weight: 600; }

  /* ---------- Header edit / delete buttons ---------- */
  .hdr-btn {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    background: transparent;
    border: 1px solid var(--border);
    color: var(--text-muted);
    font-family: inherit;
    font-size: 12px;
    font-weight: 500;
    padding: 5px 10px;
    border-radius: var(--radius-sm, 6px);
    cursor: pointer;
    transition: background var(--dur-2, 120ms), color var(--dur-2, 120ms), border-color var(--dur-2, 120ms);
  }
  .hdr-btn:hover { background: var(--bg-card-hover); color: var(--text); border-color: var(--border-strong, var(--border)); }
  .hdr-btn.active {
    background: var(--accent-soft, var(--bg-card-hover));
    border-color: var(--accent);
    color: var(--accent);
  }
  .hdr-btn.danger:hover {
    color: var(--danger);
    border-color: var(--danger);
    background: transparent;
  }

  /* ---------- Zoom controls ---------- */
  .close-btn {
    background: transparent;
    border: 1px solid transparent;
    color: var(--text-muted);
    width: 28px;
    height: 28px;
    border-radius: var(--radius-sm, 6px);
    cursor: pointer;
    display: flex;
    align-items: center;
    justify-content: center;
    transition: background var(--dur-2, 120ms), color var(--dur-2, 120ms), border-color var(--dur-2, 120ms);
  }
  .close-btn:hover {
    color: var(--text);
    border-color: var(--border);
    background: var(--bg-card-hover);
  }

  .loading {
    text-align: center;
    padding: 36px;
    color: var(--text-muted);
    font-size: 13px;
  }

  /* ---------- Spine viewport (the scrollable area) ---------- */
  .body {
    /* grow:0 so the body never stretches past its content (no empty space);
       shrink:1 + min-height:0 so it can shrink and scroll once the card hits
       its max-height. */
    flex: 0 1 auto;
    min-height: 0;
    overflow-y: auto;
    overflow-x: hidden;
    position: relative;
    padding: 16px 0 24px;
    scrollbar-width: thin;
    scrollbar-color: var(--border-strong, var(--border)) transparent;
    /* Horizontal distance from the spine to the inner edge of each card.
       Doubles as the length of the horizontal connector stub. Used to be
       16px, which left only ~32px total gap and forced the ~60px hour
       pills to overlap the card edges — hover then raised the card to
       z-index:4 over the pill, which read as cards "jumping on top".
       Bumped to 36px so the pills sit cleanly between the cards. */
    --spine-clearance: 36px;
  }
  .body::-webkit-scrollbar { width: 10px; }
  .body::-webkit-scrollbar-thumb {
    background: var(--border-strong, var(--border));
    border-radius: 5px;
    border: 2px solid var(--bg-card);
  }

  .canvas {
    position: relative;
    width: 100%;
    min-height: 100%;
  }

  /* ---------- Spine line ---------- */
  .spine {
    position: absolute;
    top: 0;
    bottom: 0;
    left: 50%;
    width: 2px;
    background: var(--border);
    transform: translateX(-50%);
  }

  /* ---------- Hour marks ---------- */
  .hour-mark {
    position: absolute;
    left: 50%;
    transform: translate(-50%, -50%);
    /* Above the cumshot drops so a cumshot near a tick never hides it. */
    z-index: 6;
    pointer-events: none;
  }
  .hour-pill {
    display: inline-block;
    padding: 2px 8px;
    background: var(--bg-card);
    border: 1px solid var(--border);
    border-radius: var(--radius-pill, 999px);
    font-size: 10px;
    font-weight: 600;
    color: var(--text-muted);
    font-variant-numeric: tabular-nums;
    letter-spacing: 0.04em;
    white-space: nowrap;
  }

  /* ---------- Watch-duration band ---------- */
  .band {
    position: absolute;
    left: 50%;
    width: 8px;
    transform: translateX(-50%);
    background: var(--accent);
    opacity: 0.78;
    border-radius: 2px;
    z-index: 2;
    transition: opacity var(--dur-2, 120ms), width var(--dur-2, 120ms), box-shadow var(--dur-2, 120ms);
  }
  /* Brightens with its card/connector when the scene is hovered. */
  .band.highlighted {
    opacity: 1;
    box-shadow: 0 0 0 1.5px var(--accent-glow, rgba(239, 107, 122, 0.24));
  }
  /* When ANOTHER scene is hovered, the conflicting bands vanish so the
     hovered one is unambiguous even where watch ranges overlap. */
  .band.faded { opacity: 0; }

  /* ---------- Cumshot drop ---------- */
  .cumshot-drop {
    position: absolute;
    left: 50%;
    width: 18px;
    height: 18px;
    transform: translate(-50%, -50%);
    background: var(--bg-card);
    border-radius: 50%;
    border: 1.5px solid var(--accent);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 5;
    color: var(--accent);
    cursor: pointer;
    transition: border-color var(--dur-2, 120ms);
  }
  /* Sit just off the spine on the scene's side, so left/right cumshots
     separate and the centred time pills stay clear. */
  .cumshot-drop.left  { transform: translate(calc(-50% - 20px), -50%); }
  .cumshot-drop.right { transform: translate(calc(-50% + 20px), -50%); }
  /* Minimal highlight when its scene is the hovered one — just swap the
     border to white. No fill change, no glow, no scale. */
  .cumshot-drop.highlighted {
    border-color: var(--fg-strong, #fff);
  }
  /* No scene to belong to — mark with a dashed ring so it reads as "unlinked"
     at a glance, distinct from the solid scene-anchored drops. */
  .cumshot-drop.unlinked {
    border-style: dashed;
    border-color: var(--border-strong, #353a47);
  }

  /* ---------- Connectors ----------
     The spine is at x = 50%. Cards sit with their inner edge
     --spine-clearance (defined on .body) away from the spine. The
     horizontal stub runs from spine to inner-edge at the time anchor.
     The vertical drop (only when the card was shifted down to avoid
     overlap) sits AT the card's inner edge x, dropping from the anchor
     down to the card title row. */
  .connector {
    position: absolute;
    background: var(--border-strong, var(--border));
    z-index: 1;
    pointer-events: none;
  }
  .connector.h {
    height: 1px;
    width: var(--spine-clearance);
  }
  .connector.h.left  { right: 50%; }
  .connector.h.right { left: 50%; }
  .connector.v {
    width: 1px;
  }
  .connector.v.left  { right: calc(50% + var(--spine-clearance)); }
  .connector.v.right { left:  calc(50% + var(--spine-clearance)); }
  /* On scene hover, the connector goes coral + a touch thicker so it's easy
     to trace which card a spine anchor belongs to. Kept at the base z-index
     so the drop circles still sit on top where they cross it. */
  .connector.highlighted { background: var(--accent); }
  .connector.h.highlighted { height: 2px; }
  .connector.v.highlighted { width: 2px; }

  /* ---------- Scene cards ---------- */
  .scene-card {
    position: absolute;
    height: calc(152px - 12px * 2); /* CARD_HEIGHT - padding */
    box-sizing: content-box;
    padding: 12px;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: var(--radius-md, 8px);
    display: flex;
    align-items: center;            /* vertically centre thumb (and info) in the card */
    gap: 12px;
    overflow: hidden;
    transition: border-color var(--dur-2, 120ms), background var(--dur-2, 120ms),
                box-shadow var(--dur-2, 120ms), opacity var(--dur-2, 120ms);
    z-index: 3;
  }
  .scene-card.highlighted {
    border-color: var(--accent);
    background: var(--bg-card-hover);
    box-shadow: 0 0 0 1px var(--accent);
    z-index: 4;
  }
  /* When another scene is hovered, fade the rest back so the active one
     (and its spine band/connector) stands out. */
  .scene-card.dimmed { opacity: 0.3; }
  .scene-card:hover {
    border-color: var(--accent);
    background: var(--bg-card-hover);
  }
  .scene-card.left {
    right: calc(50% + var(--spine-clearance));    /* inner edge at connector's outer end */
    width: calc(50% - var(--spine-clearance) - 16px);  /* clearance + 16px outer breathing room */
    max-width: 480px;
    flex-direction: row;           /* thumb on outside (left), info on inside (right) */
  }
  .scene-card.right {
    left: calc(50% + var(--spine-clearance));
    width: calc(50% - var(--spine-clearance) - 16px);
    max-width: 480px;
    flex-direction: row-reverse;   /* thumb on outside (right), info on inside (left) */
  }

  /* Thumbnail (and title below) link to the scene's browse detail. The
     button wrapper keeps the thumb's footprint; hover gives a gentle lift. */
  .thumb-link {
    padding: 0;
    border: none;
    background: none;
    display: block;
    flex-shrink: 0;
    cursor: pointer;
    border-radius: var(--radius-sm, 6px);
    line-height: 0;
    transition: filter var(--dur-2, 120ms);
  }
  .thumb-link:hover { filter: brightness(1.12); }
  .thumb {
    width: 160px;
    height: 90px;
    object-fit: cover;
    border-radius: var(--radius-sm, 6px);
    flex-shrink: 0;
    background: var(--bg-card-hover);
  }
  .thumb.placeholder {
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--text-muted);
  }

  .card-info {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 3px;
  }
  /* On RIGHT cards, info sits visually on the LEFT of the card (i.e. on
     the inside, near the spine). Text alignment stays left in both cases
     because that reads more comfortably than a mirrored layout. */

  .card-top {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    font-size: 10px;
    font-variant-numeric: tabular-nums;
    color: var(--text-muted);
    letter-spacing: 0.04em;
  }
  .card-time { font-weight: 600; color: var(--text); }
  .card-watched { opacity: 0.85; flex: 1; }

  /* Small per-card "refresh metadata from Stash" affordance. Sits on the
     end of the card-top row; spins while a refresh is in flight. */
  .card-refresh {
    background: transparent;
    border: 1px solid transparent;
    color: var(--text-muted);
    padding: 0;
    width: 18px;
    height: 18px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    border-radius: 4px;
    cursor: pointer;
    transition: background var(--dur-2, 120ms), color var(--dur-2, 120ms),
                border-color var(--dur-2, 120ms);
  }
  .card-refresh:hover:not(:disabled) {
    background: var(--bg-card-hover);
    color: var(--accent);
    border-color: var(--border);
  }
  .card-refresh:disabled {
    cursor: default;
    opacity: 0.6;
  }
  .card-refresh.spinning {
    color: var(--accent);
    animation: card-refresh-spin 700ms linear infinite;
  }
  @keyframes card-refresh-spin {
    from { transform: rotate(0deg); }
    to   { transform: rotate(360deg); }
  }

  /* Per-scene "remove from session" affordance — only rendered in edit mode.
     Sits at the end of the card-top row next to refresh. */
  .card-del {
    background: transparent;
    border: 1px solid transparent;
    color: var(--text-muted);
    padding: 0;
    width: 18px;
    height: 18px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    border-radius: 4px;
    cursor: pointer;
    transition: background var(--dur-2, 120ms), color var(--dur-2, 120ms),
                border-color var(--dur-2, 120ms);
  }
  .card-del:hover {
    background: var(--bg-card-hover);
    color: var(--danger);
    border-color: var(--danger);
  }

  .card-title {
    /* It's a button now (links to the scene) — reset chrome, keep typography. */
    background: none;
    border: none;
    padding: 0;
    font-family: inherit;
    text-align: left;
    cursor: pointer;
    transition: color var(--dur-2, 120ms);
    font-size: 12.5px;
    font-weight: 600;
    color: var(--text);
    line-height: 1.25;
    display: -webkit-box;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    -webkit-box-orient: vertical;
    overflow: hidden;
    word-break: break-word;
  }
  .card-title:hover { color: var(--accent); }
  /* ---------- Performer / studio pill row ---------- */
  .card-pills {
    display: flex;
    align-items: center;
    flex-wrap: wrap;            /* never crop a name; a long row wraps to a new line */
    gap: 4px;
    margin-top: 1px;
  }

  /* All pills share base shape, vary by role. */
  .pill {
    display: inline-flex;
    align-items: center;
    gap: 3px;
    padding: 2px 7px;
    border-radius: var(--radius-pill, 999px);
    font-family: inherit;
    font-size: 10.5px;
    font-weight: 500;
    line-height: 1.3;
    white-space: nowrap;
    cursor: pointer;
    background: var(--bg-card-hover, var(--bg));
    border: 1px solid var(--border, transparent);
    color: var(--text);
    transition: background var(--dur-2, 120ms),
                border-color var(--dur-2, 120ms),
                color var(--dur-2, 120ms);
  }
  .pill:hover {
    background: var(--bg);
    border-color: var(--border-strong, var(--accent));
  }

  /* Performer — neutral; base .pill styling is enough, only hover differs. */
  .performer-pill:hover { color: var(--accent); border-color: var(--accent); }

  /* Studio — highlighted with cream tint + a small clapperboard glyph,
     so it reads visually distinct from the performers above it. */
  .studio-pill {
    background: var(--highlight-soft, var(--bg-card-hover));
    border-color: var(--highlight, var(--border));
    color: var(--highlight, var(--text));
  }
  .studio-row { margin-top: 1px; }
  .studio-pill:hover {
    background: var(--highlight, transparent);
    color: var(--bg);
    border-color: var(--highlight);
  }

  /* +N overflow chip. Dashed border signals "there's more behind this". */
  .overflow-pill {
    border-style: dashed;
    color: var(--text-muted);
    font-weight: 600;
    font-variant-numeric: tabular-nums;
  }
  .overflow-pill:hover {
    color: var(--accent);
    border-color: var(--accent);
    border-style: solid;
  }

  .card-cumshots {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
    margin-top: auto;
  }
  .cumshot-pill {
    display: inline-flex;
    align-items: center;
    gap: 3px;
    padding: 2px 6px 2px 5px;
    background: var(--accent-soft, rgba(239, 107, 122, 0.16));
    color: var(--accent);
    border-radius: var(--radius-pill, 999px);
    font-size: 9.5px;
    font-weight: 600;
    font-variant-numeric: tabular-nums;
    line-height: 1;
  }
  .cumshot-pill.editing { padding-right: 2px; }
  .cumshot-del {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 14px;
    height: 14px;
    margin-left: 1px;
    padding: 0;
    border: none;
    border-radius: 50%;
    background: transparent;
    color: var(--accent);
    cursor: pointer;
    transition: background var(--dur-2, 120ms), color var(--dur-2, 120ms);
  }
  .cumshot-del:hover { background: var(--danger); color: #fff; }
  /* Edit-mode "+Cumshot" affordance per scene (logs at end-of-watch, syncs to Stash). */
  .cumshot-add {
    display: inline-flex;
    align-items: center;
    gap: 1px;
    padding: 2px 6px 2px 5px;
    border: 1px dashed var(--accent);
    border-radius: var(--radius-pill, 999px);
    background: transparent;
    color: var(--accent);
    cursor: pointer;
    line-height: 1;
    transition: background var(--dur-2, 120ms);
  }
  .cumshot-add:hover:not(:disabled) { background: var(--accent-soft, rgba(239, 107, 122, 0.16)); }
  .cumshot-add:disabled { opacity: 0.45; cursor: default; }

  /* ---------- "Stash logged activity here" review panel ---------- */
  .stash-pending {
    margin: 0 0 4px;
    padding: 10px 12px;
    border: 1px solid var(--border);
    border-left: 3px solid var(--accent);
    border-radius: var(--radius-md, 8px);
    background: var(--accent-soft, rgba(239, 107, 122, 0.08));
  }
  .sp-head { display: flex; align-items: center; gap: 7px; }
  /* The title row doubles as the collapse toggle (chevron hugs the title). */
  .sp-fold {
    flex: 1;
    min-width: 0;
    display: flex;
    align-items: center;
    gap: 7px;
    background: transparent;
    border: none;
    padding: 0;
    font-family: inherit;
    text-align: left;
    cursor: pointer;
  }
  .sp-fold:hover .sp-title { color: var(--accent); }
  .sp-title { font-size: 12px; font-weight: 600; color: var(--fg); min-width: 0; transition: color var(--dur-2, 120ms); }
  .sp-count { font-size: 11px; color: var(--fg-muted); font-variant-numeric: tabular-nums; flex-shrink: 0; }
  .sp-sub { font-size: 11px; color: var(--fg-muted); margin: 2px 0 8px 21px; }
  /* Cap the list so a long review scrolls inside the panel instead of
     swallowing the card: uncapped, a flex child can't shrink below its
     content, so past ~15 rows the list bled beyond the card border and the
     spine below lost its scroll. Cross-axis pinned per the overflow gotcha. */
  .sp-list {
    display: flex;
    flex-direction: column;
    gap: 6px;
    max-height: 38vh;
    overflow-x: hidden;
    overflow-y: auto;
    scrollbar-width: thin;
    scrollbar-color: var(--border-strong, var(--border)) transparent;
  }
  .sp-list::-webkit-scrollbar { width: 10px; }
  .sp-list::-webkit-scrollbar-thumb {
    background: var(--border-strong, var(--border));
    border-radius: 5px;
    border: 2px solid var(--bg-card);
  }
  .sp-row { display: flex; align-items: center; gap: 10px; }
  .sp-thumb {
    width: 56px; height: 32px; border-radius: 4px; object-fit: cover; flex-shrink: 0;
    background: var(--bg-input, var(--bg-elevated));
  }
  .sp-thumb.ph { display: flex; align-items: center; justify-content: center; }
  .sp-meta { display: flex; flex-direction: column; min-width: 0; flex: 1; }
  .sp-name { font-size: 12px; color: var(--fg); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .sp-info { font-size: 11px; color: var(--fg-muted); font-variant-numeric: tabular-nums; }
  .sp-acts { display: flex; gap: 6px; flex-shrink: 0; }
  .sp-add, .sp-skip, .sp-addall, .sp-skipall {
    padding: 4px 11px; border-radius: var(--radius-sm, 6px); font-family: inherit; font-size: 11px;
    font-weight: 500; cursor: pointer; border: 1px solid var(--border); background: transparent;
    color: var(--fg-muted); transition: background var(--dur-2, 120ms), color var(--dur-2, 120ms), border-color var(--dur-2, 120ms);
    flex-shrink: 0;
  }
  .sp-add { color: var(--accent); border-color: var(--accent); }
  .sp-add:hover:not(:disabled) { background: var(--accent); color: var(--accent-fg, #fff); }
  .sp-skip:hover:not(:disabled), .sp-skipall:hover:not(:disabled) { background: var(--bg-card-hover); color: var(--fg); border-color: var(--border-strong, var(--border)); }
  .sp-addall { color: var(--accent); border-color: var(--accent); font-weight: 600; }
  .sp-addall:hover:not(:disabled) { background: var(--accent); color: var(--accent-fg, #fff); }
  .sp-add:disabled, .sp-skip:disabled, .sp-addall:disabled, .sp-skipall:disabled { opacity: 0.5; cursor: default; }

  /* ---------- Overflow-performers popover ---------- */
  .performer-popover {
    position: absolute;
    z-index: 20;
    background: var(--bg-elevated);
    border: 1px solid var(--border-strong, var(--border));
    box-shadow: var(--shadow-md, 0 4px 12px rgba(0, 0, 0, 0.35));
    border-radius: var(--radius-md, 8px);
    padding: 8px 10px 10px;
    width: 280px;
    max-height: 60vh;
    overflow-y: auto;
    animation: pop-in 140ms var(--ease-out, ease-out);
  }
  /* Anchor to the same side of the spine as the card it belongs to. */
  .performer-popover.left {
    right: calc(50% + 16px);
  }
  .performer-popover.right {
    left: calc(50% + 16px);
  }
  @keyframes pop-in {
    from { transform: translateY(-4px); opacity: 0; }
    to   { transform: none; opacity: 1; }
  }
  .popover-head {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 6px;
    padding-bottom: 6px;
    border-bottom: 1px solid var(--border);
  }
  .popover-title {
    font-size: 10px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: var(--text-muted);
  }
  .popover-close {
    background: transparent;
    border: 1px solid transparent;
    color: var(--text-muted);
    width: 20px;
    height: 20px;
    border-radius: var(--radius-sm, 4px);
    cursor: pointer;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    transition: background var(--dur-2, 120ms), color var(--dur-2, 120ms);
  }
  .popover-close:hover {
    background: var(--bg-card-hover);
    color: var(--text);
  }
  .popover-body {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
  }

  /* ---------- session start/end editor (edit mode) ---------- */
  .time-edit {
    margin: 0 0 14px;
    padding: 12px 14px;
    background: var(--bg-elevated, #101218);
    border: 1px solid var(--border, #252934);
    border-radius: var(--radius-md, 8px);
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .te-row {
    display: flex;
    align-items: flex-end;
    gap: 12px;
    flex-wrap: wrap;
  }
  .te-field {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .te-label {
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.12em;
    font-weight: 600;
    color: var(--fg-muted, #8a909e);
  }
  .te-unlinked {
    display: flex;
    flex-direction: column;
    gap: 6px;
    margin-top: 10px;
    padding-top: 10px;
    border-top: 1px solid var(--border-subtle, #1c1f26);
  }
  .unlinked-items {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .unlinked-item {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .te-add-unlinked {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    align-self: flex-start;
    padding: 6px 11px;
    font-size: 12px;
    font-weight: 500;
    color: var(--fg);
    background: transparent;
    border: 1px solid var(--border);
    border-radius: 7px;
    cursor: pointer;
    font-family: inherit;
    transition: background 120ms, border-color 120ms;
  }
  .te-add-unlinked:hover:not(:disabled) { background: var(--accent-soft); border-color: var(--accent); }
  .te-add-unlinked:disabled { opacity: 0.5; cursor: default; }
  .te-notes {
    display: flex;
    flex-direction: column;
    gap: 6px;
    margin-top: 10px;
    padding-top: 10px;
    border-top: 1px solid var(--border-subtle, #1c1f26);
  }
  .te-notes-input {
    width: 100%;
    box-sizing: border-box;
    resize: vertical;
    min-height: 48px;
    padding: 8px 10px;
    background: var(--bg-input, #101218);
    border: 1px solid var(--border, #252934);
    border-radius: var(--radius-sm, 6px);
    color: var(--fg, #eceef3);
    font-family: inherit;
    font-size: 13px;
    line-height: 1.5;
    transition: border-color 120ms;
  }
  .te-notes-input:focus { outline: none; border-color: var(--accent, #ef6b7a); }
  .te-notes-input::placeholder { color: var(--fg-subtle, #5c6273); }

  /* Read-only note shown in view mode, styled like the .time-edit block. */
  .notes-view {
    display: flex;
    flex-direction: column;
    gap: 5px;
    margin: 0 0 14px;
    padding: 12px 14px;
    background: var(--bg-elevated, #101218);
    border: 1px solid var(--border, #252934);
    border-radius: var(--radius-md, 8px);
  }
  .notes-view-label {
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.12em;
    font-weight: 600;
    color: var(--fg-muted, #8a909e);
  }
  .notes-view-text {
    margin: 0;
    font-size: 13px;
    line-height: 1.55;
    color: var(--fg, #eceef3);
    white-space: pre-wrap;
    word-break: break-word;
  }
  .te-save {
    padding: 8px 16px;
    border-radius: var(--radius-sm, 6px);
    background: var(--accent, #ef6b7a);
    border: 1px solid var(--accent, #ef6b7a);
    color: var(--accent-fg, #fff);
    font-family: inherit;
    font-size: 13px;
    font-weight: 500;
    cursor: pointer;
    white-space: nowrap;
    transition: background 120ms, border-color 120ms, transform 80ms;
  }
  .te-save:hover:not(:disabled) {
    background: var(--accent-hover, #f58895);
    border-color: var(--accent-hover, #f58895);
  }
  .te-save:active:not(:disabled) {
    transform: translateY(1px);
  }
  .te-save:disabled {
    opacity: 0.5;
    cursor: default;
  }
  .te-msg {
    font-size: 12px;
    line-height: 1.4;
  }
  .te-msg.hint {
    color: var(--fg-subtle, #5c6273);
  }
  .te-msg.warn {
    color: var(--accent, #ef6b7a);
  }
  .te-msg.err {
    color: var(--danger, #f26b6b);
  }

  /* transient "nothing found" note after a manual Check Stash */
  .stash-note {
    margin: 0 0 12px;
    padding: 8px 12px;
    border: 1px solid var(--border, #252934);
    background: var(--bg-elevated, #101218);
    color: var(--fg-muted, #8a909e);
    border-radius: var(--radius-md, 8px);
    font-size: 12px;
  }
</style>
