<!--
  Trends page chassis — Phase 6. Two modes (Overview / Build report) via a
  segmented control, sharing ONE period control: the app's reused date dropdown
  (chip + DateRangePicker), backed by the trends-scoped FilterStore.

  Data flow: fetch the full-history daily series ONCE on mount; the period just
  slices it client-side (summary cards, area chart, busiest days, heatmap). The
  hour histogram + per-kind entity breakdowns are range-scoped and refetched
  whenever the period changes (they power Peak hours + leaderboards + the report
  builder's entity / time-of-day groupings).
-->
<script lang="ts">
  import { w } from "$lib/wording.svelte";
  import "./trends.css";
  import { onMount } from "svelte";
  import Icon from "$lib/Icon.svelte";
  import DateRangePicker from "$lib/dashboard/DateRangePicker.svelte";
  import MetricDefaultPill from "$lib/dashboard/MetricDefaultPill.svelte";
  import DefaultPill from "$lib/dashboard/DefaultPill.svelte";
  import { trendsFilter, fmtDay } from "$lib/filter-store.svelte";
  import { chartPrefs } from "$lib/chart-prefs.svelte";
  import { nav, type BrowseKind } from "$lib/dashboard/nav.svelte";
  import { api, formatWatchShort, competitionRanks, type HourHistogram, type EntityBreakdownRow } from "$lib/api";
  import {
    enrich,
    sliceByDate,
    prevWindow,
    byDayOfWeek,
    hourCompact,
    hourFull,
    vsLabelFor,
    METRICS,
    type DayPoint,
    type MetricId,
  } from "./trends-data";
  import TrendSummaryCard from "./TrendSummaryCard.svelte";
  import AreaTrendChart from "./AreaTrendChart.svelte";
  import ConsistencyHeatmap from "./ConsistencyHeatmap.svelte";
  import PatternChart from "./PatternChart.svelte";
  import Leaderboard from "./Leaderboard.svelte";
  import ReportBuilder from "./ReportBuilder.svelte";

  let mode = $state<"overview" | "report">("overview");
  let dateOpen = $state(false);
  let ready = $state(false);

  let fullSeries = $state<DayPoint[]>([]);
  let hist = $state<HourHistogram | null>(null);
  let perfRows = $state<EntityBreakdownRow[]>([]);
  let studioRows = $state<EntityBreakdownRow[]>([]);
  let tagRows = $state<EntityBreakdownRow[]>([]);
  // Scene breakdown (report-builder only — no scene leaderboard).
  let sceneRows = $state<EntityBreakdownRow[]>([]);
  // Previous equal-length window's breakdowns — for the rankings' ▲/▼ movement
  // badges. Empty for All time (no prior window to compare against).
  let prevPerfRows = $state<EntityBreakdownRow[]>([]);
  let prevStudioRows = $state<EntityBreakdownRow[]>([]);
  let prevTagRows = $state<EntityBreakdownRow[]>([]);

  const range = $derived(trendsFilter.range);
  const granularity = $derived(trendsFilter.granularity);
  const presetLabel = $derived(trendsFilter.presetLabel);
  const vsLabel = $derived(vsLabelFor(trendsFilter.preset));

  const cur = $derived(sliceByDate(fullSeries, range.start, range.end));
  const prev = $derived.by(() => {
    const w = prevWindow(range.start, range.end);
    return sliceByDate(fullSeries, w.start, w.end);
  });

  // The trend chart's plotted metric, lifted so the summary cards can drive it
  // (clicking a card = clicking that metric on the chart's toggle). The chart
  // still seeds this from its own saved default on mount.
  let areaMetric = $state<MetricId>("cumshots");

  // Leaderboards: one shared metric drives all three boards (clearer than each
  // board ranking by a different, invisible metric). Entity metric vocabulary is
  // the canonical 3 — active days can't apply to an entity.
  type LeaderMetric = "cumshots" | "watch_time" | "sessions";
  let leaderMetric = $state<LeaderMetric>("cumshots");
  function entityVal(r: EntityBreakdownRow, m: LeaderMetric): number {
    return m === "watch_time" ? r.watch_time_ms : m === "sessions" ? r.sessions : r.cumshots;
  }
  /** Top 10 by the shared metric, each row carrying a rank-movement delta vs the
   *  previous equal-length window (sports-table style): prevRank − curRank, so
   *  positive = climbed. "new" = had no ranking (zero on this metric) last
   *  period. Deltas are omitted entirely for All time (vsLabel === null). */
  function rankTop(rows: EntityBreakdownRow[], prevRows: EntityBreakdownRow[]) {
    const withDeltas = vsLabel !== null;
    const prevRank = new Map<string, number>();
    if (withDeltas) {
      const prevSorted = [...prevRows].sort(
        (a, b) => entityVal(b, leaderMetric) - entityVal(a, leaderMetric),
      );
      const prevRanks = competitionRanks(prevSorted.map((r) => entityVal(r, leaderMetric)));
      prevSorted.forEach((r, i) => {
        // Only a non-zero value counts as "ranked" last period - breakdown
        // rows can carry 0 on the selected metric (e.g. watch-only scenes
        // when ranking by cumshots).
        if (entityVal(r, leaderMetric) > 0) prevRank.set(r.id, prevRanks[i]);
      });
    }
    // Rank is competition-style, so entities tied on the metric SHARE a rank
    // (two tied at the top are both #1, the next is #3) - the row's position in
    // the list is no longer its rank, so it's carried on the item. Ranked over
    // the whole sorted list before the top-10 slice; the slice is a prefix, so
    // the indices still line up.
    const sorted = [...rows].sort(
      (a, b) => entityVal(b, leaderMetric) - entityVal(a, leaderMetric),
    );
    const ranks = competitionRanks(sorted.map((r) => entityVal(r, leaderMetric)));
    return sorted.slice(0, 10).map((r, i) => {
      const item: { id: string; name: string; value: number; rank: number; delta?: number | "new" } = {
        id: r.id,
        name: r.name,
        value: entityVal(r, leaderMetric),
        rank: ranks[i],
      };
      if (withDeltas) {
        const pr = prevRank.get(r.id);
        item.delta = pr === undefined ? "new" : pr - ranks[i];
      }
      return item;
    });
  }
  // ---- Performers female/male split ----
  // The Top-performers board never mixes genders: a slide toggle picks the
  // side. Male side = Stash gender MALE / TRANSGENDER_MALE; everyone else
  // (FEMALE, TRANSGENDER_FEMALE, NON_BINARY, INTERSEX, not-yet-enriched)
  // shows on the Female side. Default side is user-settable via the toggle's
  // Default pill (persisted in default_performer_gender).
  type Gender = "female" | "male";
  let perfGender = $state<Gender>("female");
  let genderDefault = $state<Gender>("female");

  function isMaleSide(g: string | null): boolean {
    return g === "MALE" || g === "TRANSGENDER_MALE";
  }
  function genderFilter(rows: EntityBreakdownRow[]): EntityBreakdownRow[] {
    return rows.filter((r) => isMaleSide(r.gender) === (perfGender === "male"));
  }
  async function pinGenderDefault() {
    genderDefault = perfGender;
    try {
      await api.defaultPerformerGenderSet(perfGender);
    } catch (e) {
      console.warn("save default performer gender failed", e);
    }
  }

  // Prev rows filtered by the SAME side, so movement badges rank within the
  // visible gender's list (a climb past someone on the other side isn't real).
  const topPerformers = $derived(rankTop(genderFilter(perfRows), genderFilter(prevPerfRows)));
  const topStudios = $derived(rankTop(studioRows, prevStudioRows));
  const topTags = $derived(rankTop(tagRows, prevTagRows));
  const leaderFmt = $derived((v: number) => (leaderMetric === "watch_time" ? formatWatchShort(v) : String(v)));

  // ---- Pattern charts (Busiest days / Peak hours) ----
  // Each has its own metric toggle + Default pill. Busiest days supports all 3
  // (totals per weekday from the daily series); Peak hours supports cumshots +
  // sessions only — no honest per-hour watch durations exist (honesty matrix),
  // and its sessions variant buckets by START hour.
  type BusiestMetric = "cumshots" | "watch_time" | "sessions";
  type PeakMetric = "cumshots" | "sessions";
  let busiestMetric = $state<BusiestMetric>("cumshots");
  let peakMetric = $state<PeakMetric>("cumshots");

  const busiestDays = $derived(byDayOfWeek(cur, busiestMetric));
  const peakHours = $derived(
    ((peakMetric === "sessions" ? hist?.sessions : hist?.cumshots) ?? []).map(
      (value, hour) => ({ label: hourCompact(hour), tip: hourFull(hour), value }),
    ),
  );

  const busiestSub = $derived(
    busiestMetric === "cumshots" ? w("Cumshots by day of week")
    : busiestMetric === "watch_time" ? "Watch time by day of week"
    : "Sessions by day of week",
  );
  const peakSub = $derived(
    peakMetric === "cumshots" ? w("Cumshots by hour of day") : "Session starts by hour of day",
  );

  function countPhrase(v: number, noun: string): string {
    return w(`${v} ${noun}${v === 1 ? "" : "s"}`);
  }
  const busiestFmt = $derived((v: number) =>
    busiestMetric === "watch_time"
      ? `${formatWatchShort(v)} watched`
      : countPhrase(v, busiestMetric === "sessions" ? "session" : "cumshot"),
  );
  const busiestFmtShort = $derived((v: number) =>
    busiestMetric === "watch_time" ? formatWatchShort(v) : String(v),
  );
  const peakFmt = $derived((v: number) =>
    peakMetric === "sessions" ? countPhrase(v, "session") : countPhrase(v, "cumshot"),
  );

  function gotoEntity(kind: BrowseKind, id: string) {
    nav.goto(kind, id, { section: "trends" });
  }

  // Date popover: click-outside / Escape close (mirrors FilterBar).
  $effect(() => {
    if (!dateOpen) return;
    function onDoc(e: MouseEvent) {
      // composedPath(), not target.closest() - see FilterBar: a node removed by
      // its own click handler is detached by the time this runs, so closest()
      // returns null and an inside click reads as outside.
      const inside = e.composedPath().some(
        (n) => n instanceof Element && (n.classList.contains("cx-trends-date")),
      );
      if (inside) return;
      dateOpen = false;
    }
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") dateOpen = false;
    }
    document.addEventListener("click", onDoc);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("click", onDoc);
      document.removeEventListener("keydown", onKey);
    };
  });

  // Range-scoped fetches (hour histogram + entity breakdowns, current AND
  // previous window — the latter powers the rankings' movement badges).
  // Monotonic token guards against an earlier slow response clobbering a
  // newer one.
  let reqToken = 0;
  async function loadRangeData(start: string, end: string, withPrev: boolean) {
    const token = ++reqToken;
    const pw = prevWindow(start, end);
    const none = Promise.resolve([] as EntityBreakdownRow[]);
    try {
      const [h, p, s, t, sc, pp, ps, pt] = await Promise.all([
        api.trendsHourHistogram(start, end),
        api.trendsEntityBreakdown("performer", start, end),
        api.trendsEntityBreakdown("studio", start, end),
        api.trendsEntityBreakdown("tag", start, end),
        api.trendsEntityBreakdown("scene", start, end),
        withPrev ? api.trendsEntityBreakdown("performer", pw.start, pw.end) : none,
        withPrev ? api.trendsEntityBreakdown("studio", pw.start, pw.end) : none,
        withPrev ? api.trendsEntityBreakdown("tag", pw.start, pw.end) : none,
      ]);
      if (token !== reqToken) return;
      hist = h;
      perfRows = p;
      studioRows = s;
      tagRows = t;
      sceneRows = sc;
      prevPerfRows = pp;
      prevStudioRows = ps;
      prevTagRows = pt;
    } catch (e) {
      if (token === reqToken) console.error("trends range fetch failed", e);
    }
  }
  $effect(() => {
    if (!ready) return;
    loadRangeData(range.start, range.end, vsLabel !== null);
  });

  function monthsAgo(n: number): string {
    const d = new Date();
    d.setHours(0, 0, 0, 0);
    d.setMonth(d.getMonth() - n);
    return fmtDay(d);
  }

  onMount(async () => {
    // Fresh sidebar arrival → saved default period; a Back-restore (claimed
    // target) keeps the period as-left. Same rule as Sessions/Overview/browse.
    if (!nav.claimTarget("trends")) trendsFilter.reset();
    // Seed the rankings metric from its own saved default (set via the
    // "Default" pill — independent from the Overview + area charts), unless
    // the user already toggled while the load was in flight.
    chartPrefs.load("trends_rankings").then(() => {
      const d = chartPrefs.defaultMetric("trends_rankings");
      // Rankings are 3-metric (active days can't apply to an entity) — narrow
      // away an "active" value, which only the area chart's scope can hold.
      if (leaderMetric === "cumshots" && d !== "active") leaderMetric = d;
    });
    // Seed the pattern charts from their own saved defaults (3- and 2-metric
    // toggles — narrow away values their toggles can't show).
    chartPrefs.load("trends_busiest").then(() => {
      const d = chartPrefs.defaultMetric("trends_busiest");
      if (busiestMetric === "cumshots" && d !== "active") busiestMetric = d;
    });
    chartPrefs.load("trends_peak").then(() => {
      const d = chartPrefs.defaultMetric("trends_peak");
      if (peakMetric === "cumshots" && d === "sessions") peakMetric = d;
    });
    // Seed the performers' gender side from its saved default (same race
    // guard: don't clobber a toggle the user already made).
    api.defaultPerformerGenderGet().then((g) => {
      if (g === "male" || g === "female") {
        genderDefault = g;
        if (perfGender === "female") perfGender = g;
      }
    }).catch(() => {});
    // Background-backfill performer gender (+ images etc) from Stash — rows
    // enriched before the gender column existed re-fetch automatically.
    api.catalogEnrichEntities("performer").catch(() => {});
    await trendsFilter.loadDefaults();
    // Full-history series: from the earliest data (or 14 months back, whichever
    // is earlier, so the heatmap always has a full year of cells) → today.
    const todayStr = fmtDay(new Date());
    const fm = monthsAgo(14);
    const fs = trendsFilter.firstSessionDay;
    const seriesStart = fs && fs < fm ? fs : fm;
    try {
      const raw = await api.trendsDailySeries(seriesStart, todayStr);
      fullSeries = enrich(raw);
    } catch (e) {
      console.error("trends daily series failed", e);
    }
    ready = true;
  });
</script>

<div class="cx-trends">
  <div class="cx-trends-toolbar">
    <div class="cx-segmented">
      <button class:active={mode === "overview"} onclick={() => (mode = "overview")}>
        <Icon name="trending-up" size={14} /> Overview
      </button>
      <button class:active={mode === "report"} onclick={() => (mode = "report")}>
        <Icon name="bar-chart-3" size={14} /> Build report
      </button>
    </div>

    <div class="cx-trends-date">
      <button
        class="cx-trends-date-btn"
        class:active={dateOpen}
        onclick={(e) => {
          e.stopPropagation();
          dateOpen = !dateOpen;
        }}
        aria-expanded={dateOpen}
      >
        <Icon name="calendar" size={13} />
        <span>{presetLabel}</span>
        <Icon name="chevron-down" size={11} color="var(--fg-subtle)" />
      </button>
      {#if dateOpen}
        <div class="cx-trends-date-pop">
          <DateRangePicker store={trendsFilter} onClose={() => (dateOpen = false)} />
        </div>
      {/if}
    </div>
  </div>

  {#if mode === "overview"}
    <div class="cx-trends-summary">
      <TrendSummaryCard metric="cumshots" {cur} {prev} {granularity} {vsLabel} primary
        active={areaMetric === "cumshots"} onclick={() => (areaMetric = "cumshots")} />
      <TrendSummaryCard metric="watch_time" {cur} {prev} {granularity} {vsLabel}
        active={areaMetric === "watch_time"} onclick={() => (areaMetric = "watch_time")} />
      <TrendSummaryCard metric="sessions" {cur} {prev} {granularity} {vsLabel}
        active={areaMetric === "sessions"} onclick={() => (areaMetric = "sessions")} />
      <TrendSummaryCard metric="active" {cur} {prev} {granularity} {vsLabel}
        active={areaMetric === "active"} onclick={() => (areaMetric = "active")} />
    </div>

    <AreaTrendChart {cur} {granularity} periodLabel={presetLabel} bind:metric={areaMetric} />

    <ConsistencyHeatmap
      series={fullSeries}
      rangeStart={range.start}
      onDayClick={(day) => nav.gotoOverviewDay(day, { section: "trends" })}
    />

    {#snippet busiestControls()}
      <div class="cx-trend-chart-controls">
        <MetricDefaultPill scope="trends_busiest" current={busiestMetric} />
        <div class="cx-toggle">
          <button class:active={busiestMetric === "cumshots"} onclick={() => (busiestMetric = "cumshots")}>{w("Cumshots")}</button>
          <button class:active={busiestMetric === "watch_time"} onclick={() => (busiestMetric = "watch_time")}>Watch time</button>
          <button class:active={busiestMetric === "sessions"} onclick={() => (busiestMetric = "sessions")}>Sessions</button>
        </div>
      </div>
    {/snippet}
    {#snippet peakControls()}
      <div class="cx-trend-chart-controls">
        <MetricDefaultPill scope="trends_peak" current={peakMetric} />
        <div class="cx-toggle">
          <button class:active={peakMetric === "cumshots"} onclick={() => (peakMetric = "cumshots")}>{w("Cumshots")}</button>
          <button class:active={peakMetric === "sessions"} onclick={() => (peakMetric = "sessions")}>Sessions</button>
        </div>
      </div>
    {/snippet}

    <div class="cx-trends-2col">
      <PatternChart
        title="Busiest days"
        subtitle={busiestSub}
        period={presetLabel}
        data={busiestDays}
        formatVal={busiestFmt}
        formatShort={busiestFmtShort}
        color={METRICS[busiestMetric].color}
        controls={busiestControls}
      />
      <PatternChart
        title="Peak hours"
        subtitle={peakSub}
        period={presetLabel}
        data={peakHours}
        formatVal={peakFmt}
        color={METRICS[peakMetric].color}
        controls={peakControls}
      />
    </div>

    <div class="cx-leaders-head">
      <span class="cx-leaders-title">Rankings <span class="cx-leaders-period">· {presetLabel}</span></span>
      <div class="cx-trend-chart-controls">
        <MetricDefaultPill scope="trends_rankings" current={leaderMetric} />
        <div class="cx-toggle">
          <button class:active={leaderMetric === "cumshots"} onclick={() => (leaderMetric = "cumshots")}>{w("Cumshots")}</button>
          <button class:active={leaderMetric === "watch_time"} onclick={() => (leaderMetric = "watch_time")}>Watch time</button>
          <button class:active={leaderMetric === "sessions"} onclick={() => (leaderMetric = "sessions")}>Sessions</button>
        </div>
      </div>
    </div>

    {#snippet genderControls()}
      <div class="cx-lb-head-controls">
        <DefaultPill
          isDefault={perfGender === genderDefault}
          onSet={pinGenderDefault}
          isTitle="This side is your default."
          setTitle="Pin this side as your default"
        />
        <div class="cx-toggle">
          <button class:active={perfGender === "female"} onclick={() => (perfGender = "female")}>Female</button>
          <button class:active={perfGender === "male"} onclick={() => (perfGender = "male")}>Male</button>
        </div>
      </div>
    {/snippet}

    <div class="cx-trends-3col">
      <Leaderboard
        title="Top performers"
        items={topPerformers}
        metric={leaderMetric}
        formatVal={leaderFmt}
        onSelect={(id) => gotoEntity("performer", id)}
        headRight={genderControls}
      />
      <Leaderboard
        title="Top studios"
        items={topStudios}
        metric={leaderMetric}
        formatVal={leaderFmt}
        onSelect={(id) => gotoEntity("studio", id)}
      />
      <Leaderboard
        title="Top tags"
        items={topTags}
        metric={leaderMetric}
        formatVal={leaderFmt}
        onSelect={(id) => gotoEntity("tag", id)}
      />
    </div>
  {:else}
    <ReportBuilder {cur} {hist} {sceneRows} {perfRows} {studioRows} {tagRows} {range} />
  {/if}
</div>
