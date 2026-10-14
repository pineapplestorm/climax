<!--
  Overview section — filter-driven drill-down dashboard.

  Layout (top to bottom):
   - Active session card (immutable, always visible)
   - Hero stats top row (immutable lifestyle metrics)
   - FilterBar (sticky, controls everything below)
   - Hero stats range row (responds to filter)
   - Breadcrumb
   - Bar chart (range buckets, drills into days when granularity=month)
   - Session strip + detail (when drilled into a day)

  The filter store is the single source of truth for what's shown
  below the filter bar.
-->
<script lang="ts">
  import { w } from "$lib/wording.svelte";
  import { onMount, onDestroy, tick } from "svelte";
  import OPromptModal from "$lib/OPromptModal.svelte";
  import UnlinkedQuickLog from "$lib/dashboard/UnlinkedQuickLog.svelte";
  import SceneCard from "$lib/SceneCard.svelte";
  import Icon from "$lib/Icon.svelte";
  import HeroStats from "$lib/dashboard/HeroStats.svelte";
  import HeroStatsRange from "$lib/dashboard/HeroStatsRange.svelte";
  import FilterBar from "$lib/dashboard/FilterBar.svelte";
  import BarChart, { type ChartBucket } from "$lib/dashboard/BarChart.svelte";
  import SessionStrip from "$lib/dashboard/SessionStrip.svelte";
  import SessionDetail from "$lib/dashboard/SessionDetail.svelte";
  import { filterStore } from "$lib/filter-store.svelte";
  import { activeSession } from "$lib/active-session.svelte";
  import { chartPrefs } from "$lib/chart-prefs.svelte";
  import { nav } from "$lib/dashboard/nav.svelte";
  import {
    api,
    formatDuration,
    type Session,
    type PlayedScene,
    type RangeBucket,
  } from "$lib/api";

  // ---------- Active session state ----------

  // Active session comes from the shared store (one poll for the whole
  // dashboard); scenes for the active-session card are still fetched here
  // since only this section needs them.
  const active = $derived(activeSession.current);
  let activeSceneList = $state<PlayedScene[]>([]);
  // Active-session card shows newest scene at top — matches the tracker.
  // Only this card; SessionDetail's spine layout below stays chronological.
  // Same visibility rule as the tracker (see +page.svelte for the canonical
  // comment): a scene appears while actively being played, held for
  // PLAYING_GRACE_MS after its last credited second, plus always if it has a
  // cumshot. "Actively played" reads the backend's `last_advance_at` directly,
  // so the list is correct the instant this section mounts (no client-side
  // seconds-delta baseline to rebuild on every navigation).
  const PLAYING_GRACE_MS = 60_000;
  const isPlaying = (s: PlayedScene) =>
    s.last_advance_at != null && now - s.last_advance_at <= PLAYING_GRACE_MS;
  const activeSceneListVisible = $derived.by(() =>
    [...activeSceneList].reverse().filter((s) => s.cumshot_count > 0 || isPlaying(s)),
  );
  // Three live stats: "playing" = scenes advancing now (live, from
  // last_advance_at; paused never counts, even with a cumshot). "watched" uses
  // the backend COUNTED footprint (active.scene_count: threshold-crossed or
  // cumshot, same as SessionDetail) so sub-threshold tab-opens don't inflate it.
  // Mirrors the tracker — see +page.svelte.
  const playingCount = $derived(activeSceneList.filter(isPlaying).length);
  let now = $state(Date.now());
  // From the store — when the active session was last polled — so the live
  // timer stays smooth between the 1s `now` ticks.
  const lastRefreshAt = $derived(activeSession.lastRefreshAt);
  let unsub: (() => void) | null = null;

  let oPromptOpen = $state(false);
  let oPromptSessionId = $state<number | null>(null);
  let oPromptScenes = $state<PlayedScene[]>([]);

  let timer: ReturnType<typeof setInterval> | null = null;
  let poll: ReturnType<typeof setInterval> | null = null;

  const livenessElapsed = $derived.by(() => {
    if (!active) return 0;
    if (active.status !== "active") return active.effective_duration_ms;
    return active.effective_duration_ms + Math.max(0, now - lastRefreshAt);
  });

  // Fetch the active session's scene list (card content). Reads the current
  // active session from the shared store; clears the list when idle.
  async function refreshScenes() {
    const a = activeSession.current;
    try {
      activeSceneList = a ? await api.sessionScenes(a.id) : [];
    } catch (e) { console.error("active scenes refresh failed", e); }
  }

  // The dashboard window's timers throttle while it's hidden, so the
  // active-session card can read stale the instant the window is shown.
  // Refresh + re-anchor `now` on becoming visible.
  function onVisible() {
    if (document.visibilityState === "visible") {
      now = Date.now();
      refreshScenes();
    }
  }

  // After an action that changes session state (start/stop/pause/log/remove),
  // refresh the shared store immediately AND re-pull this section's scenes,
  // so the card updates without waiting for the next poll tick.
  async function refreshActiveAndScenes() {
    await activeSession.refresh();
    await refreshScenes();
  }

  async function startSession() {
    await api.sessionStart();
    await refreshActiveAndScenes();
  }
  async function stopSession() {
    // Same rule as the tracker's stop(): wrap up whenever the session will be
    // kept or has rows to review - any scene, any cumshot (a scene-less one keeps
    // a session on its own), or "Keep empty sessions" being on.
    const wrapUp =
      !!active &&
      (activeSceneList.length > 0 ||
        active.o_count > 0 ||
        (await api.keepEmptySessionsGet().catch(() => false)));
    if (active && wrapUp) {
      oPromptSessionId = active.id;
      oPromptScenes = activeSceneList;
      oPromptOpen = true;
      return;
    }
    await api.sessionStop();
    await refreshActiveAndScenes();
  }
  async function pauseOrResume() {
    if (!active) return;
    if (active.status === "paused") await api.sessionResume();
    else await api.sessionPause();
    await refreshActiveAndScenes();
  }
  // Counts come straight from the backend after each mutation — no local
  // optimistic layer. The oLog/delete + refresh round trip is local IPC
  // (single-digit ms), so the badge updates effectively instantly while
  // staying authoritative. An earlier optimistic-guess approach could leave
  // a stale +1 sitting next to the now-updated backend count, showing one
  // tap as "2"; reading the backend directly removes that whole class of bug.
  async function logCumshotInline(scene: PlayedScene) {
    if (!active) return;
    try {
      await api.oLog({ sessionId: active.id, contentItemId: scene.content_item_id });
    } catch (e) {
      console.error(e);
    }
    await refreshActiveAndScenes();
  }
  async function removeCumshotInline(scene: PlayedScene) {
    if (!active) return;
    try {
      await api.oDeleteRecentForScene(active.id, scene.content_item_id, 1);
    } catch (e) {
      console.error(e);
    }
    await refreshActiveAndScenes();
  }
  function cumshotModalClose() { oPromptOpen = false; }
  async function cumshotModalCommitted() {
    oPromptOpen = false;
    await refreshActiveAndScenes();
  }

  // ---------- Drill state ----------
  //
  // "range"     — chart shows the filter's range at filter's granularity
  //               (day / month / year, picked automatically by the store).
  // "months"    — drilled into a year (only meaningful when the filter's
  //               granularity was "year"). Chart shows that year's months,
  //               clipped to filter range.
  // "days"      — drilled into a month. Chart shows that month's days,
  //               clipped to filter range.
  // "sessions"  — drilled into a day; SessionStrip + detail panel.

  type Level = "range" | "months" | "days" | "sessions";
  let level = $state<Level>("range");
  /** YYYY when drilled into a year (granularity="year" only). */
  let selectedYear = $state<string | null>(null);
  /** YYYY-MM when drilled into a month. */
  let selectedMonth = $state<string | null>(null);
  /** YYYY-MM-DD (Monday) when drilled into a week. Mutex with selectedMonth
   *  at the "days" level — only one is set at a time. */
  let selectedWeek = $state<string | null>(null);
  /** YYYY-MM-DD when drilled into a day. */
  let selectedDay = $state<string | null>(null);
  /** Session id when expanded for detail. */
  let selectedSessionId = $state<number | null>(null);

  let rangeBuckets = $state<RangeBucket[]>([]);
  let monthsBuckets = $state<RangeBucket[]>([]);
  let dailyBuckets = $state<RangeBucket[]>([]);
  let rangeLoading = $state(true);
  let monthsLoading = $state(false);
  let dailyLoading = $state(false);

  /** Which metric the bar chart is plotting. Lifted out of BarChart so
   *  it persists across drill levels — clicking a bar mounts a new
   *  BarChart instance, and without this hoist the selection would
   *  reset to "watch_time" every time. */
  let chartMetric = $state<"watch_time" | "cumshots" | "sessions">("cumshots");

  /** Filter signature — joined string of every facet that affects what's
   *  shown below the filter bar. Plain `let` (NOT `$state`) so writes
   *  inside the effect don't re-trigger it; we only want the effect to
   *  re-fire when one of the filter facets it reads changes. */
  let prevFilterSig = "";
  let filterEffectInitialised = false;

  function filterSignature(): string {
    return [
      filterStore.startDay,
      filterStore.endDay,
      filterStore.sceneIds.join(","),
      filterStore.performerIds.join(","),
      filterStore.studioIds.join(","),
      filterStore.tagIds.join(","),
    ].join("|");
  }

  /** When the filter changes, reset the drill (a "May 2026" drill makes
   *  no sense if the filter is now "this week") and refetch the chart. */
  $effect(() => {
    const sig = filterSignature();
    if (filterEffectInitialised && sig !== prevFilterSig) {
      level = "range";
      selectedYear = null;
      selectedMonth = null;
      selectedWeek = null;
      selectedDay = null;
      selectedSessionId = null;
    }
    prevFilterSig = sig;
    filterEffectInitialised = true;
    refreshRange();
  });

  async function refreshRange() {
    rangeLoading = true;
    try {
      rangeBuckets = await api.dashboardFilteredBuckets({
        startDay: filterStore.startDay,
        endDay: filterStore.endDay,
        granularity: filterStore.granularity,
        sceneContentItemIds: filterStore.sceneIds,
        performerIds: filterStore.performerIds,
        studioIds: filterStore.studioIds,
        tagIds: filterStore.tagIds,
      });
    } catch (e) {
      console.error("range buckets failed", e);
    } finally {
      rangeLoading = false;
    }
  }

  async function refreshMonths(yearKey: string) {
    monthsLoading = true;
    try {
      const y = parseInt(yearKey, 10);
      const yearStart = `${yearKey}-01-01`;
      const yearEnd = `${yearKey}-12-31`;
      const start = yearStart > filterStore.startDay ? yearStart : filterStore.startDay;
      const end = yearEnd < filterStore.endDay ? yearEnd : filterStore.endDay;
      monthsBuckets = await api.dashboardFilteredBuckets({
        startDay: start,
        endDay: end,
        granularity: "month",
        sceneContentItemIds: filterStore.sceneIds,
        performerIds: filterStore.performerIds,
        studioIds: filterStore.studioIds,
        tagIds: filterStore.tagIds,
      });
    } catch (e) {
      console.error("months buckets failed", e);
    } finally {
      monthsLoading = false;
    }
  }

  async function refreshDaily() {
    dailyLoading = true;
    try {
      let rawStart: string;
      let rawEnd: string;
      if (selectedMonth) {
        const [y, m] = selectedMonth.split("-").map((s) => parseInt(s, 10));
        rawStart = `${selectedMonth}-01`;
        rawEnd = `${String(y).padStart(4, "0")}-${String(m).padStart(2, "0")}-${new Date(y, m, 0).getDate().toString().padStart(2, "0")}`;
      } else if (selectedWeek) {
        // Monday → Sunday (6 days later)
        const parts = selectedWeek.split("-").map((s) => parseInt(s, 10));
        const monday = new Date(parts[0], parts[1] - 1, parts[2]);
        const sunday = new Date(monday);
        sunday.setDate(monday.getDate() + 6);
        rawStart = selectedWeek;
        rawEnd = `${sunday.getFullYear()}-${String(sunday.getMonth() + 1).padStart(2, "0")}-${String(sunday.getDate()).padStart(2, "0")}`;
      } else {
        return;
      }
      const start = rawStart > filterStore.startDay ? rawStart : filterStore.startDay;
      const end = rawEnd < filterStore.endDay ? rawEnd : filterStore.endDay;
      dailyBuckets = await api.dashboardFilteredBuckets({
        startDay: start,
        endDay: end,
        granularity: "day",
        sceneContentItemIds: filterStore.sceneIds,
        performerIds: filterStore.performerIds,
        studioIds: filterStore.studioIds,
        tagIds: filterStore.tagIds,
      });
    } catch (e) {
      console.error("daily buckets failed", e);
    } finally {
      dailyLoading = false;
    }
  }

  $effect(() => {
    if (selectedYear && level === "months") refreshMonths(selectedYear);
  });
  $effect(() => {
    if (level === "days" && (selectedMonth || selectedWeek)) refreshDaily();
  });

  function clickRangeBar(key: string) {
    selectedSessionId = null;
    if (filterStore.granularity === "year") {
      selectedYear = key;
      selectedMonth = null;
      selectedWeek = null;
      selectedDay = null;
      level = "months";
    } else if (filterStore.granularity === "month") {
      selectedMonth = key;
      selectedWeek = null;
      selectedDay = null;
      level = "days";
    } else if (filterStore.granularity === "week") {
      selectedWeek = key;          // key = Monday YYYY-MM-DD
      selectedMonth = null;
      selectedDay = null;
      level = "days";
    } else {
      selectedDay = key;
      level = "sessions";
    }
  }

  function clickMonthsBar(key: string) {
    selectedMonth = key;
    selectedWeek = null;
    selectedDay = null;
    selectedSessionId = null;
    level = "days";
  }

  function clickDayBar(key: string) {
    selectedDay = key;
    selectedSessionId = null;
    level = "sessions";
  }

  /** Back-restored inner-scroll position for the session detail (null on
   *  user-initiated opens, which start the detail at its top as usual). */
  let restoredDetailScroll = $state<number | null>(null);

  function selectSession(id: number) {
    restoredDetailScroll = null;
    selectedSessionId = selectedSessionId === id ? null : id;
  }

  // ---------- Open a day / session from the hero stat cards ----------
  // The Record-day and Longest-session hero cards (above the filter bar) jump
  // the bottom drill-down straight to that day (and, for the longest session,
  // expand its detail), scrolling it into view.
  let daySectionEl = $state<HTMLDivElement | undefined>();
  function scrollDaySectionIntoView() {
    tick().then(() =>
      setTimeout(() => daySectionEl?.scrollIntoView({ behavior: "smooth", block: "start" }), 60),
    );
  }
  /** Record streak -> scope the filter to the run's dates. Unlike the other two
   *  milestone cards this drives the FILTER rather than the drill-down, because
   *  the drill-down holds a single day and a streak is a range. Everything under
   *  the filter bar (chart, range stats, day list) re-scopes to those dates.
   *  Any open drill-down is cleared first: it was selected under the old range
   *  and would be showing a day outside the new one.
   *
   *  `level` MUST land on "range" here, and the reset cannot be left to the
   *  filter effect below. Clicking the card a SECOND time re-sets the same
   *  dates, so the filter signature doesn't change and that effect never runs -
   *  any other level would stick with no year/month/day beside it, and every
   *  branch of the drill-down would fall through to nothing. */
  function openRecordStreak(start: string, end: string) {
    selectedYear = null;
    selectedMonth = null;
    selectedWeek = null;
    selectedDay = null;
    selectedSessionId = null;
    level = "range";
    filterStore.setCustomRange(start, end);
  }
  function openRecordDay(date: string) {
    selectedYear = null;
    selectedMonth = null;
    selectedWeek = null;
    selectedDay = date;
    selectedSessionId = null;
    level = "sessions";
    scrollDaySectionIntoView();
  }
  function openLongestSession(date: string, sessionId: number) {
    selectedYear = null;
    selectedMonth = null;
    selectedWeek = null;
    selectedDay = date;
    restoredDetailScroll = null;
    selectedSessionId = sessionId;
    level = "sessions";
    // The selectedSessionId $effect scrolls the opened SessionDetail into view.
  }

  // Origin snapshot pushed when navigating away (a performer/studio pill on a
  // scene card or session detail), so Back returns to this exact drill state.
  function overviewOrigin() {
    return {
      section: "overview" as const,
      overview: {
        level,
        selectedYear,
        selectedMonth,
        selectedWeek,
        selectedDay,
        selectedSessionId,
      },
    };
  }

  // Scroll the loaded session detail into view when a session is selected from
  // the day's session strip (it renders below the strip, so it can be off-screen).
  let sessionDetailEl = $state<HTMLDivElement | undefined>();
  let lastScrolledSession: number | null = null;
  $effect(() => {
    const sid = selectedSessionId;
    if (sid !== null && sid !== lastScrolledSession) {
      // tick() so the detail is in the DOM; small timeout so its initial layout
      // settles before we scroll its top to the top of the content viewport.
      tick().then(() =>
        setTimeout(() => sessionDetailEl?.scrollIntoView({ behavior: "smooth", block: "start" }), 60),
      );
    }
    lastScrolledSession = sid;
  });

  function goLevel(lvl: Level) {
    level = lvl;
    if (lvl === "range") {
      selectedYear = null;
      selectedMonth = null;
      selectedWeek = null;
      selectedDay = null;
      selectedSessionId = null;
    } else if (lvl === "months") {
      selectedMonth = null;
      selectedWeek = null;
      selectedDay = null;
      selectedSessionId = null;
    } else if (lvl === "days") {
      selectedDay = null;
      selectedSessionId = null;
    }
  }

  /** Pretty crumb label when at the "days" level — either a month
   *  ("May 2026") or a week ("Week of 20 May"). */
  const daysCrumbLabel = $derived.by(() => {
    if (selectedMonth) return monthLongLabel(selectedMonth);
    if (selectedWeek) {
      const [y, m, d] = selectedWeek.split("-").map((s) => parseInt(s, 10));
      const dt = new Date(y, m - 1, d);
      return `Week of ${dt.toLocaleDateString([], { day: "numeric", month: "short" })}`;
    }
    return "";
  });

  // ---------- Bucket → ChartBucket transforms ----------

  function monthShortLabel(yyyymm: string): string {
    const [y, m] = yyyymm.split("-").map((s) => parseInt(s, 10));
    return new Date(y, m - 1, 1).toLocaleDateString([], { month: "short", year: "2-digit" });
  }
  function monthLongLabel(yyyymm: string): string {
    const [y, m] = yyyymm.split("-").map((s) => parseInt(s, 10));
    return new Date(y, m - 1, 1).toLocaleDateString([], { month: "long", year: "numeric" });
  }
  function dayShortLabel(yyyymmdd: string): string {
    const day = parseInt(yyyymmdd.split("-")[2], 10);
    return String(day);
  }
  function dayLongLabel(yyyymmdd: string): string {
    const [y, m, d] = yyyymmdd.split("-").map((s) => parseInt(s, 10));
    return new Date(y, m - 1, d).toLocaleDateString([], { weekday: "short", month: "short", day: "numeric" });
  }

  /** Pull the year/month/day numbers out of a YYYY-MM-DD or YYYY-MM key. */
  function partsOf(key: string): { y: number; m: number; d: number } {
    const parts = key.split("-").map((p) => parseInt(p, 10));
    return { y: parts[0] ?? 0, m: parts[1] ?? 0, d: parts[2] ?? 1 };
  }
  function monthAbbrev(y: number, m: number): string {
    return new Date(y, m - 1, 1).toLocaleDateString([], { month: "short" });
  }

  /** First Monday on or after January 1st of `year`. W1 of the user's
   *  week-numbering system. Toggl uses this convention. Days in Dec or
   *  early Jan that fall BEFORE this Monday belong to the previous
   *  year's last week. */
  function firstMondayOfYear(year: number): Date {
    const jan1 = new Date(year, 0, 1);
    const daysToMon = (8 - jan1.getDay()) % 7; // 0..6 (Mon..Sun → 0..6)
    return new Date(year, 0, 1 + daysToMon);
  }

  /** Returns {year, week} for the week whose Monday is `mondayKey`
   *  (YYYY-MM-DD). Weeks always count from the first Monday of their
   *  year (W1) onwards. A Monday in late December whose week extends
   *  into the next year is still labelled as the LAST week of its
   *  Monday's year. */
  function weekOfYear(mondayKey: string): { year: number; week: number } {
    const { y, m, d } = partsOf(mondayKey);
    const monday = new Date(y, m - 1, d);
    let weekYear = y;
    let firstMon = firstMondayOfYear(weekYear);
    if (monday < firstMon) {
      weekYear -= 1;
      firstMon = firstMondayOfYear(weekYear);
    }
    const diffDays = Math.round(
      (monday.getTime() - firstMon.getTime()) / 86400000,
    );
    return { year: weekYear, week: Math.floor(diffDays / 7) + 1 };
  }

  /** Bucket → x-axis label generation, per granularity:
   *   - year:  full year ("2024")
   *   - month: full "Mon Yy" on every bar (e.g. "MAY 24"); chart never
   *            has so many monthly bars that this gets cramped, and the
   *            year on each label removes any ambiguity.
   *   - week:  "W{N}", with year suffix at the first bar and at year
   *            transitions. W1 = first Monday of January.
   *   - day:   just the day number; first bar and any month transition
   *            includes the month abbreviation. Year is omitted (the
   *            breadcrumb supplies context). */
  function smartLabels(buckets: RangeBucket[]): string[] {
    return buckets.map((b, i) => {
      const prev = i > 0 ? buckets[i - 1] : null;
      const isFirst = i === 0;
      if (b.granularity === "year") return b.key;

      if (b.granularity === "month") {
        return monthShortLabel(b.key);
      }

      if (b.granularity === "week") {
        const { year, week } = weekOfYear(b.key);
        const prevYear = prev ? weekOfYear(prev.key).year : null;
        const yearChanged = prev != null && prevYear !== year;
        if (isFirst || yearChanged) {
          return `W${week} '${String(year).slice(-2)}`;
        }
        return `W${week}`;
      }

      // day
      const { y, m, d } = partsOf(b.key);
      const monthChanged = prev
        ? partsOf(prev.key).m !== m || partsOf(prev.key).y !== y
        : false;
      if (isFirst || monthChanged) {
        return `${d} ${monthAbbrev(y, m)}`;
      }
      return String(d);
    });
  }

  /** Tooltip detail string for week buckets — the Mon-Sun date range
   *  the W{N} label glosses over. Only emitted for weeks; other
   *  granularities have enough context in their own labels. */
  function tooltipDetail(b: RangeBucket): string | undefined {
    if (b.granularity !== "week") return undefined;
    const { y, m, d } = partsOf(b.key);
    const monday = new Date(y, m - 1, d);
    const sunday = new Date(monday);
    sunday.setDate(monday.getDate() + 6);
    const fmt = (dt: Date) =>
      dt.toLocaleDateString([], { day: "numeric", month: "short" });
    return `${fmt(monday)} - ${fmt(sunday)}`;
  }

  function toChartBucket(b: RangeBucket, label: string): ChartBucket {
    return {
      key: b.key,
      label,
      tooltipDetail: tooltipDetail(b),
      watch_time_ms: b.watch_time_ms,
      cumshots: b.cumshots,
      sessions: b.sessions,
    };
  }

  const rangeChartBuckets = $derived.by<ChartBucket[]>(() => {
    const labels = smartLabels(rangeBuckets);
    return rangeBuckets.map((b, i) => toChartBucket(b, labels[i]));
  });
  const monthsChartBuckets = $derived.by<ChartBucket[]>(() => {
    const labels = smartLabels(monthsBuckets);
    return monthsBuckets.map((b, i) => toChartBucket(b, labels[i]));
  });
  const dailyChartBuckets = $derived.by<ChartBucket[]>(() => {
    const labels = smartLabels(dailyBuckets);
    return dailyBuckets.map((b, i) => toChartBucket(b, labels[i]));
  });

  // ---------- Lifecycle ----------

  onMount(() => {
    // Shared session_active poll (also drives the sidebar status dot).
    unsub = activeSession.subscribe();
    refreshScenes();
    // Arrived from the bridge's stop click on a headless server (?wrapup=1):
    // the server has no window to raise, so the bridge opens this page instead.
    // Opens the same wrap-up the Stop button does.
    //
    // The flag is consumed ONLY after an authenticated read succeeds. On a
    // token-protected server the first read 401s and WebTokenGate (a sibling
    // overlay, so this still mounts and runs) prompts, then reloads - and a
    // reload keeps the query string. Stripping the flag before that read
    // deleted the intent while unauthenticated, so the post-token reload landed
    // on a plain dashboard. Leaving it put means the second pass picks it up.
    if (typeof window !== "undefined" && new URLSearchParams(window.location.search).has("wrapup")) {
      (async () => {
        let session;
        try {
          session = await api.sessionActive();
        } catch {
          return; // not authenticated (or server unreachable) - retry next load
        }
        // Authenticated: consume the flag so a later manual refresh can't wrap
        // up a DIFFERENT session.
        const url = new URL(window.location.href);
        url.searchParams.delete("wrapup");
        window.history.replaceState({}, "", url);
        if (!session) return;
        // stopSession() reads the store + scene list to choose between the
        // wrap-up modal and a bare stop, so both must be loaded first.
        await activeSession.refresh();
        await refreshScenes();
        if (active) await stopSession();
      })();
    }
    // Load the user's saved default date preset BEFORE the first chart
    // fetch so the chart starts on their preferred range, not the
    // module-level fallback. loadDefaults is idempotent and async-safe.
    filterStore.loadDefaults();
    // Seed the chart's metric from the user's saved default (set via the
    // "Default" pill on the toggle). Only if the user hasn't already toggled
    // the metric while the load was in flight (same race-defence as the
    // filter store's loadDefaults).
    chartPrefs.load("overview").then(() => {
      const d = chartPrefs.defaultMetric("overview");
      // "active" is only a valid default for the Trends area chart's 4-metric
      // toggle; this chart is 3-metric, so narrow it away.
      if (chartMetric === "cumshots" && d !== "active") chartMetric = d;
    });
    // Returning here via Back? Restore the drill state we navigated away from
    // (the filter-change effect's first run won't clobber it — it only resets on
    // subsequent filter changes).
    const restoreT = nav.claimTarget("overview");
    if (restoreT?.overview) {
      const o = restoreT.overview;
      level = o.level;
      selectedYear = o.selectedYear;
      selectedMonth = o.selectedMonth;
      selectedWeek = o.selectedWeek;
      selectedDay = o.selectedDay;
      selectedSessionId = o.selectedSessionId;
      // Suppress the scroll-detail-into-view effect for the restored id — the
      // dashboard shell re-establishes the exact scroll position instead.
      lastScrolledSession = o.selectedSessionId;
      restoredDetailScroll = restoreT.detailScroll ?? null;
    } else if (!restoreT) {
      // Fresh sidebar arrival: back to the saved default view (date preset +
      // cleared chips) — Back-restores above keep the filter as-left instead.
      filterStore.reset();
    }
    timer = setInterval(() => { now = Date.now(); }, 1000);
    poll = setInterval(() => {
      refreshScenes();
      refreshRange();
      if (selectedYear && level === "months") refreshMonths(selectedYear);
      if (level === "days" && (selectedMonth || selectedWeek)) refreshDaily();
    }, 5000);
    document.addEventListener("visibilitychange", onVisible);
  });
  onDestroy(() => {
    if (timer) clearInterval(timer);
    if (poll) clearInterval(poll);
    document.removeEventListener("visibilitychange", onVisible);
    unsub?.();
  });
</script>

<div class="overview">
  <!-- Cross-nav return (e.g. arrived from the Trends heatmap). Only shows when
       there's a back-stack entry; a plain sidebar arrival clears it. The Overview
       breadcrumb below handles drill-UP within the day view; this returns to
       wherever you came from. -->
  {#if nav.canGoBack}
    <button class="cx-back-btn" style="margin-bottom: 6px;" onclick={() => nav.back()}>
      <Icon name="chevron-left" size={14} /> Back
    </button>
  {/if}
  <!-- ACTIVE SESSION (compact) -->
  {#if active}
    <section class="card session-card">
      <div class="session-main">
        <div class="session-status">
          <div class="dot" class:active={active.status === "active"} class:paused={active.status === "paused"}></div>
          <span>{active.status === "paused" ? "Session paused" : "Session running"}</span>
        </div>
        <div class="session-timer">{formatDuration(livenessElapsed)}</div>
        <div class="session-stats">
          <span><strong class="live">{playingCount}</strong> scene{playingCount === 1 ? "" : "s"} playing</span>
          <span><strong>{active.scene_count}</strong> scene{active.scene_count === 1 ? "" : "s"} watched</span>
          <span><strong class="accent">{active.o_count}</strong> {w(active.o_count === 1 ? "cumshot" : "cumshots")}</span>
        </div>
        <div class="session-actions">
          <UnlinkedQuickLog session={active} refreshKey={lastRefreshAt} onChanged={refreshActiveAndScenes} />
          <button class="secondary" onclick={pauseOrResume}>{active.status === "paused" ? "Resume" : "Pause"}</button>
          <button class="danger" onclick={stopSession}>Stop</button>
        </div>
      </div>

      {#if activeSceneListVisible.length > 0}
        <div class="active-scenes">
          {#each activeSceneListVisible as sc (sc.play_id)}
            {@const removable = sc.cumshot_count}
            <div class="active-scene-row">
              <div class="asc-card">
                <SceneCard
                  scene={sc}
                  compact={true}
                  onSelect={() => nav.goto("scene", sc.content_item_id, overviewOrigin())}
                  onPerformerSelect={(p) => nav.goto("performer", p.id, overviewOrigin())}
                  onStudioSelect={(s) => nav.goto("studio", s.id, overviewOrigin())}
                />
              </div>
              <div class="asc-actions">
                {#if removable > 0}
                  <button class="cumshot-minus" onclick={() => removeCumshotInline(sc)} aria-label={w("Remove cumshot")} title={w("Remove the most recent cumshot from this scene, in both Climax and Stash.")}>
                    <Icon name="minus" size={14} />
                  </button>
                {/if}
                <button class="cumshot-plus" onclick={() => logCumshotInline(sc)} aria-label={w("Add cumshot")} title={w("Log cumshot")}>
                  <Icon name="cumshot" size={16} color="var(--accent)" filled />
                </button>
              </div>
            </div>
          {/each}
        </div>
      {/if}
    </section>
  {:else}
    <section class="card idle-card">
      <div class="idle-msg">No active session</div>
      <div class="idle-actions">
        <UnlinkedQuickLog session={null} refreshKey={lastRefreshAt} />
        <button class="primary" onclick={startSession}>Start session</button>
      </div>
    </section>
  {/if}

  <!-- HERO STATS — top row (immutable, ignores filter). Record day + longest
       session jump the drill-down below to that day / session. -->
  <HeroStats
    onRecordDay={openRecordDay}
    onLongestSession={openLongestSession}
    onRecordStreak={openRecordStreak}
  />

  <!-- FILTER BAR — sticky; everything below responds to it -->
  <FilterBar />

  <!-- HERO STATS — range row (responds to filter) -->
  <HeroStatsRange />

  <!-- BREADCRUMB -->
  <nav class="breadcrumb" aria-label="Drill-down trail">
    <button
      class="crumb"
      class:active={level === "range"}
      onclick={() => goLevel("range")}
    >{filterStore.presetLabel}</button>
    {#if selectedYear}
      <span class="sep">/</span>
      <button
        class="crumb"
        class:active={level === "months"}
        onclick={() => goLevel("months")}
      >{selectedYear}</button>
    {/if}
    {#if selectedMonth || selectedWeek}
      <span class="sep">/</span>
      <button
        class="crumb"
        class:active={level === "days"}
        onclick={() => goLevel("days")}
      >{daysCrumbLabel}</button>
    {/if}
    {#if selectedDay}
      <span class="sep">/</span>
      <button
        class="crumb"
        class:active={level === "sessions"}
        onclick={() => goLevel("sessions")}
      >{dayLongLabel(selectedDay)}</button>
    {/if}
  </nav>

  <!-- DRILL CONTENT -->
  {#if level === "range"}
    <BarChart
      buckets={rangeChartBuckets}
      titleSuffix={
        filterStore.granularity === "year" ? "by year"
        : filterStore.granularity === "month" ? "by month"
        : filterStore.granularity === "week" ? "by week"
        : "by day"
      }
      loading={rangeLoading}
      onBarClick={clickRangeBar}
      bind:metric={chartMetric}
      defaultPill
    />
  {:else if level === "months" && selectedYear}
    <BarChart
      buckets={monthsChartBuckets}
      titleSuffix={`in ${selectedYear}`}
      loading={monthsLoading}
      onBarClick={clickMonthsBar}
      bind:metric={chartMetric}
      defaultPill
    />
  {:else if level === "days" && (selectedMonth || selectedWeek)}
    <BarChart
      buckets={dailyChartBuckets}
      titleSuffix={`in ${daysCrumbLabel}`}
      loading={dailyLoading}
      onBarClick={clickDayBar}
      bind:metric={chartMetric}
      defaultPill
    />
  {:else if level === "sessions" && selectedDay}
    <div bind:this={daySectionEl}>
      <SessionStrip day={selectedDay} {selectedSessionId} onSelect={selectSession} />
      {#if selectedSessionId !== null}
        <!-- Keyed so switching sessions MOUNTS A FRESH detail rather than
             feeding a new id to the existing one. Without this the component
             keeps its per-session state across the switch: edit mode stays on,
             and the start/end pickers still hold the PREVIOUS session's times,
             which instantly fail validation against the new session's bounds.
             (showSkipped and the pending-history list leaked the same way.)
             The Sessions page never had this because its detail lives inside
             the row loop, so selecting another row destroys and remounts it -
             this makes Overview behave identically. selectSession() already
             clears restoredDetailScroll, so the remount can't re-apply a stale
             scroll position. -->
        {#key selectedSessionId}
          <div bind:this={sessionDetailEl}>
            <SessionDetail
              sessionId={selectedSessionId}
              initialScroll={restoredDetailScroll ?? undefined}
              onClose={() => selectedSessionId = null}
              onDeleted={() => selectedSessionId = null}
              onOpenInSessions={() => selectedSessionId !== null && nav.gotoSession(selectedSessionId, overviewOrigin())}
              onPerformerSelect={(p) => nav.goto("performer", p.id, overviewOrigin())}
              onStudioSelect={(s) => nav.goto("studio", s.id, overviewOrigin())}
              onSceneSelect={(id) => nav.goto("scene", id, overviewOrigin())}
            />
          </div>
        {/key}
      {/if}
    </div>
  {/if}
</div>

{#if oPromptOpen && oPromptSessionId !== null && active}
  <OPromptModal sessionId={oPromptSessionId} scenes={oPromptScenes} session={active} onClose={cumshotModalClose} onCommitted={cumshotModalCommitted} />
{/if}

<style>
  .overview {
    display: flex;
    flex-direction: column;
    /* 12px (was 16) — empty-state Overview at 16:9 was overflowing by a
       few pixels; trimming each inter-section gap by 4px sums to ~20px
       saved, enough to fit on load without the scrollbar appearing. The
       layout still has plenty of vertical breathing room. */
    gap: 12px;
  }

  .card {
    background: var(--bg-card);
    border: 1px solid var(--border);
    border-radius: var(--radius, 10px);
    padding: 14px 18px;
  }

  /* ---------- ACTIVE SESSION (compact) ---------- */
  .session-card { border-left: 3px solid var(--accent); }
  .session-main {
    display: flex;
    align-items: center;
    gap: 22px;
    flex-wrap: wrap;
  }
  .session-status {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 11px;
    color: var(--text-muted);
    text-transform: uppercase;
    letter-spacing: 0.12em;
    min-width: 130px;
  }
  .dot { width: 8px; height: 8px; border-radius: 50%; background: var(--text-muted); transition: background 200ms; }
  .dot.active { background: var(--green); animation: pulse 1.6s infinite ease-in-out; }
  .dot.paused { background: var(--amber); }
  @keyframes pulse {
    0%, 100% { box-shadow: 0 0 0 0 #4ade8055; }
    50%      { box-shadow: 0 0 0 6px #4ade8000; }
  }
  .session-timer {
    font-family: "JetBrains Mono", "SF Mono", monospace;
    font-size: 32px; font-weight: 600; line-height: 1;
    letter-spacing: -0.02em; font-variant-numeric: tabular-nums;
  }
  .session-stats {
    display: flex; gap: 16px;
    font-size: 12px; color: var(--text-muted);
    font-variant-numeric: tabular-nums;
  }
  .session-stats strong {
    color: var(--text); font-weight: 600; font-size: 14px; margin-right: 4px;
  }
  .session-stats strong.accent { color: var(--accent); }
  .session-stats strong.live { color: var(--live); }
  .session-actions { margin-left: auto; display: flex; gap: 6px; }

  .active-scenes {
    margin-top: 12px;
    display: flex; flex-direction: column; gap: 4px;
    padding-top: 12px; border-top: 1px solid var(--border);
    max-height: 200px; overflow-y: auto;
  }
  .active-scene-row { display: flex; align-items: center; gap: 6px; }
  .asc-card { flex: 1; min-width: 0; }
  .asc-actions { display: flex; gap: 4px; flex-shrink: 0; }

  .idle-card {
    display: flex; align-items: center; justify-content: space-between;
    padding: 18px 22px;
  }
  .idle-actions { display: flex; align-items: center; gap: 6px; }
  .idle-msg {
    font-size: 12px; color: var(--text-muted);
    text-transform: uppercase; letter-spacing: 0.15em;
  }

  /* ---------- BREADCRUMB ---------- */
  .breadcrumb {
    display: flex;
    align-items: center;
    gap: 6px;
    flex-wrap: wrap;
    padding: 2px 0;
  }
  .crumb {
    background: transparent;
    border: 1px solid transparent;
    color: var(--text-muted);
    padding: 4px 10px;
    border-radius: 6px;
    font-size: 12px;
    font-weight: 500;
    cursor: pointer;
    font-family: inherit;
    transition: color 120ms, background 120ms, border-color 120ms;
  }
  .crumb:hover { color: var(--text); background: var(--bg-card-hover); border-color: var(--border); }
  .crumb.active {
    color: var(--text);
    font-weight: 600;
    cursor: default;
  }
  .crumb.active:hover { background: transparent; border-color: transparent; }
  .sep { color: var(--text-muted); font-size: 12px; }

  /* ---------- BUTTONS ---------- */
  button {
    padding: 7px 16px;
    border-radius: 7px;
    font-size: 13px;
    font-weight: 500;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--text);
    cursor: pointer;
    transition: background 120ms, border-color 120ms, transform 80ms;
    font-family: inherit;
  }
  button:hover:not(:disabled) { border-color: #3a3e4a; background: var(--bg-card-hover); }
  button:active:not(:disabled) { transform: translateY(1px); }
  button.primary { background: var(--accent); border-color: var(--accent); color: #fff; }
  button.primary:hover { background: #ff7280; border-color: #ff7280; }
  button.danger { background: var(--red); border-color: var(--red); color: #fff; }
  button.danger:hover { background: #fa8585; border-color: #fa8585; }
  button.secondary { background: transparent; color: var(--text); }

  .cumshot-plus, .cumshot-minus {
    width: 32px; height: 32px; padding: 0;
    border-radius: 8px; font-size: 16px;
    display: flex; align-items: center; justify-content: center; line-height: 1;
  }
  .cumshot-plus { background: var(--bg-card-hover); border-color: var(--border); }
  .cumshot-plus:hover { background: var(--accent-soft); border-color: var(--accent); }
  .cumshot-minus { color: var(--text-muted); background: var(--bg-card-hover); border-color: var(--border); }
  .cumshot-minus:hover { color: var(--red); border-color: var(--red); background: #f8717118; }
</style>
