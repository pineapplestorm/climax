<!--
  GitHub-style consistency heatmap (weeks × days). Intensity follows a 3-metric
  toggle (Cumshots / Watch time / Sessions — no Active days: any active day
  already gets the level-1 floor, so "active" IS the heatmap's baseline), with
  the cell tint matching the metric's chart colour (coral / white / bone) and
  the shading scaled to the visible window's own max. Has its own Default pill
  (chartPrefs scope "trends_heatmap"), like every other metric toggle here.

  The header stats follow the metric: CURRENT STREAK = consecutive days
  (ending today) with a nonzero value of the selected metric. The second stat
  is per-metric: cumshots/sessions show DAY BEST (the longest-ever run of such
  days — streaks are meaningful for counts); watch time shows BEST DAY (the
  visible window's highest single-day total, i.e. the brightest cell) since a
  "watch-time streak run" would just mirror the sessions one.

  Window responds to the date filter but never shrinks below a rolling 12
  months (a heatmap of "last 7 days" would be meaningless); the grid stretches
  to fill the card width via measured square cell sizing.
-->
<script lang="ts">
  import { w } from "$lib/wording.svelte";
  import { onMount } from "svelte";
  import { valueOf, METRICS, type DayPoint } from "./trends-data";
  import { formatWatchShort } from "$lib/api";
  import MetricDefaultPill from "$lib/dashboard/MetricDefaultPill.svelte";
  import { chartPrefs } from "$lib/chart-prefs.svelte";

  type Props = {
    series: DayPoint[];
    rangeStart: string;
    /** Click a day cell → drill into that day (Overview's session view). */
    onDayClick?: (day: string) => void;
  };
  let { series, rangeStart, onDayClick }: Props = $props();

  type HeatMetric = "cumshots" | "watch_time" | "sessions";
  let metric = $state<HeatMetric>("cumshots");

  // Seed from this chart's saved default (3-metric toggle — narrow "active"
  // away; only the area chart's scope can hold it).
  onMount(() => {
    chartPrefs.load("trends_heatmap").then(() => {
      const d = chartPrefs.defaultMetric("trends_heatmap");
      if (metric === "cumshots" && d !== "active") metric = d;
    });
  });

  const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
  const DOW_W = 28;
  const GAP = 3;
  /** Gutter between the Mon-Sun column and the cell grid. Shared with the CSS via
   *  --bodygap rather than restated there: it used to be a bare `gap: 6px` that
   *  the width budget below never subtracted, so every row rendered 6px wider
   *  than the space it was sized for. */
  const BODY_GAP = 6;

  let bodyW = $state(0);

  function pad(n: number): string {
    return String(n).padStart(2, "0");
  }
  function ymd(d: Date): string {
    return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
  }

  // Window start: the filter's start, but never less than a rolling ~year.
  const winStart = $derived.by(() => {
    const t = new Date();
    t.setHours(0, 0, 0, 0);
    const yearAgo = new Date(t);
    yearAgo.setDate(yearAgo.getDate() - 364);
    const ya = ymd(yearAgo);
    return rangeStart && rangeStart < ya ? rangeStart : ya;
  });

  const recent = $derived(series.filter((d) => d.date >= winStart));

  // Metric-gated streaks over the WHOLE history (not just the window):
  // current = trailing run ending today, best = longest run ever.
  const streaks = $derived.by(() => {
    let best = 0;
    let run = 0;
    let cur = 0;
    for (const d of series) {
      if (valueOf(d, metric) > 0) {
        run++;
        best = Math.max(best, run);
      } else run = 0;
    }
    // Trailing run ending today, WITH a grace for today: if today hasn't been
    // logged yet, measure the run up to yesterday instead of dropping to 0.
    // Matches the Overview streak's at-risk behaviour (compute_streak in
    // dashboard.rs) - otherwise the heatmap streak reads 0 every morning until
    // something is logged, disagreeing with the Overview card. The series is
    // ascending and zero-filled to today, so the last element is today.
    let i = series.length - 1;
    if (i >= 0 && valueOf(series[i], metric) === 0) i--; // skip an empty today
    for (; i >= 0; i--) {
      if (valueOf(series[i], metric) > 0) cur++;
      else break;
    }
    return { current: cur, best };
  });
  // Watch time's second stat: the VISIBLE window's highest single-day total —
  // by construction, the brightest cell on the grid. (A watch-time streak run
  // would just mirror the sessions one, so it shows this instead.)
  const bestDayLabel = $derived(
    formatWatchShort(recent.length === 0 ? 0 : Math.max(...recent.map((d) => valueOf(d, "watch_time")))),
  );
  const streakNoun = $derived(
    metric === "cumshots" ? w("≥1 cumshot") : metric === "sessions" ? "≥1 session" : "any watch time",
  );

  const weeks = $derived.by<(DayPoint | null)[][]>(() => {
    const days = recent;
    if (days.length === 0) return [];
    const padded: (DayPoint | null)[] = [];
    // Monday-first rows (Mon row 0 ... Sun row 6), matching the app's week
    // convention everywhere else. Pad the lead so day 1 lands on its weekday.
    const lead = (new Date(days[0].ts).getDay() + 6) % 7;
    for (let i = 0; i < lead; i++) padded.push(null);
    days.forEach((d) => padded.push(d));
    const w: (DayPoint | null)[][] = [];
    for (let i = 0; i < padded.length; i += 7) w.push(padded.slice(i, i + 7));
    return w;
  });
  const numWeeks = $derived(weeks.length);

  // Cell size to fill the measured width (square cells). The min keeps
  // multi-year windows legible (overflowing into a horizontal scroll); the
  // generous max lets a ~53-week window genuinely FILL wide screens instead of
  // stranding dead space on the right.
  const cell = $derived.by(() => {
    if (!bodyW || numWeeks === 0) return 14;
    const avail = bodyW - DOW_W - BODY_GAP - (numWeeks - 1) * GAP;
    return Math.max(9, Math.min(36, Math.floor(avail / numWeeks)));
  });

  // Shading is contextual to the visible window's own range for the SELECTED
  // metric — the brightest cell is your busiest day of this window.
  const maxV = $derived(Math.max(1, ...recent.map((d) => valueOf(d, metric))));
  function level(d: DayPoint | null | undefined): number {
    if (!d || !d.active) return 0;
    const r = valueOf(d, metric) / maxV;
    if (r > 0.66) return 4;
    if (r > 0.4) return 3;
    if (r > 0.15) return 2;
    return 1; // any active day shows at least the faint floor
  }

  const monthCols = $derived.by<{ wi: number; label: string }[]>(() => {
    const cols: { wi: number; label: string }[] = [];
    let lastMonth = -1;
    let lastYear = -1;
    weeks.forEach((wk, wi) => {
      const firstReal = wk.find((d) => d);
      if (firstReal) {
        const dt = new Date(firstReal.ts);
        const mo = dt.getMonth();
        const yr = dt.getFullYear();
        if (mo !== lastMonth) {
          // Year marker on the first label and at year transitions — the
          // app-wide date-label convention.
          const yearMark = lastYear === -1 || yr !== lastYear;
          const label = `${MONTHS[mo]}${yearMark ? ` '${String(yr).slice(-2)}` : ""}`;
          // Drop a label with no room left to render in. Its column sits above the
          // cells, but the TEXT is free to run past the last one, and the wrap's
          // min-width:max-content would turn that spill into a horizontal
          // scrollbar. Only ever bites the final week or two (GitHub's heatmap
          // drops its trailing label for the same reason). 9px mono ~= 5.5px/char.
          const room = (numWeeks - wi) * (cell + GAP) - GAP;
          if (room >= label.length * 5.5 + 2) {
            cols.push({ wi, label });
          }
          lastMonth = mo;
          lastYear = yr;
        }
      }
    });
    return cols;
  });

  function title(d: DayPoint | null | undefined): string {
    if (!d) return "";
    return `${d.date} · ${d.cumshots} ${w(d.cumshots === 1 ? "cumshot" : "cumshots")} · ${formatWatchShort(d.watch_time_ms)} · ${d.sessions} session${d.sessions === 1 ? "" : "s"}`;
  }
</script>

<section
  class="cx-card cx-heatmap"
  style={`--hm-color: ${METRICS[metric].color}`}
>
  <header class="cx-heatmap-head">
    <div class="cx-heatmap-titles">
      <h3>Consistency</h3>
      <span class="cx-heatmap-sub">{METRICS[metric].label} per day</span>
    </div>
    <div class="cx-heatmap-controls">
      <div class="cx-trend-chart-controls">
        <MetricDefaultPill scope="trends_heatmap" current={metric} />
        <div class="cx-toggle">
          <button class:active={metric === "cumshots"} onclick={() => (metric = "cumshots")}>{w("Cumshots")}</button>
          <button class:active={metric === "watch_time"} onclick={() => (metric = "watch_time")}>Watch time</button>
          <button class:active={metric === "sessions"} onclick={() => (metric = "sessions")}>Sessions</button>
        </div>
      </div>
      <div class="cx-heatmap-streaks">
        <span title={`Days in a row with ${streakNoun}, up to today.`}>
          <b>{streaks.current}</b> day current streak
        </span>
        <span class="cx-tile-sep">·</span>
        {#if metric === "watch_time"}
          <span title="The highest single-day watch time in the period shown - the most strongly shaded square below.">
            <b>{bestDayLabel}</b> best day
          </span>
        {:else}
          <span title={`The longest run of days with ${streakNoun}.`}>
            <b>{streaks.best}</b> day best
          </span>
        {/if}
      </div>
    </div>
  </header>
  <div
    class="cx-heatmap-scroll"
    bind:clientWidth={bodyW}
    style={`--cell:${cell}px; --gap:${GAP}px; --doww:${DOW_W}px; --bodygap:${BODY_GAP}px`}
  >
    <div class="cx-heatmap-grid-wrap">
      <div class="cx-heatmap-months">
        {#each monthCols as mc (mc.wi)}
          <span class="cx-heatmap-month" style={`grid-column: ${mc.wi + 1}`}>{mc.label}</span>
        {/each}
      </div>
      <div class="cx-heatmap-body">
        <!-- One slot per row (Mon-first), every weekday labelled. -->
        <div class="cx-heatmap-dow">
          <span>Mon</span><span>Tue</span><span>Wed</span><span>Thu</span><span>Fri</span><span>Sat</span><span>Sun</span>
        </div>
        <div class="cx-heatmap-grid">
          {#each weeks as wk, wi (wi)}
            <div class="cx-heatmap-col">
              {#each Array.from({ length: 7 }) as _, di (di)}
                {@const d = wk[di]}
                {#if d && onDayClick}
                  <button
                    class="cx-hm-cell lv{level(d)}"
                    title={title(d)}
                    aria-label={`Open ${d.date}`}
                    onclick={() => onDayClick(d.date)}
                  ></button>
                {:else}
                  <span class="cx-hm-cell lv{level(d)}" title={title(d)}></span>
                {/if}
              {/each}
            </div>
          {/each}
        </div>
      </div>
    </div>
  </div>
  <div class="cx-heatmap-legend">
    <span>Less</span>
    <span class="cx-hm-cell lv0"></span>
    <span class="cx-hm-cell lv1"></span>
    <span class="cx-hm-cell lv2"></span>
    <span class="cx-hm-cell lv3"></span>
    <span class="cx-hm-cell lv4"></span>
    <span>More</span>
  </div>
</section>
