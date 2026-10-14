<!--
  Generic vertical bar chart used by Overview for both months view and
  drilled-down days view. Presentation-only: parent supplies the buckets,
  parent owns the click drill-down.

  Each bucket has all three metrics (watch_time_ms / cumshots / sessions)
  so the chart's own metric toggle can swap which one is plotted without
  going back to the parent for new data.

  Visual: Toggl Reports-style. Vertical bars, rotated value labels on top,
  dashed Y-axis gridlines, horizontal X-axis labels in a dedicated row
  BELOW the chart's baseline (not inside the plot area — that previously
  caused them to clip into the border line).

  Bar colour follows the selected metric:
   - Cumshots    → coral (--accent)   (brand colour; cumshot data is coral everywhere)
   - Watch time  → white (--fg-strong)
   - Sessions    → bone (--highlight)
-->
<script lang="ts">
  import { w } from "$lib/wording.svelte";
  import { formatDuration } from "$lib/api";
  import MetricDefaultPill from "$lib/dashboard/MetricDefaultPill.svelte";

  type Metric = "watch_time" | "cumshots" | "sessions";

  export type ChartBucket = {
    /** Unique key (e.g. "2026-04" or "2026-04-15"). */
    key: string;
    /** Short label rendered under the bar (e.g. "Apr '26" or "15"). */
    label: string;
    /** Optional extra context appended to the hover tooltip after the
     *  label — used by week bars to surface the Mon-Sun date range
     *  alongside the "W21" label. Leave undefined for other granularities. */
    tooltipDetail?: string;
    watch_time_ms: number;
    cumshots: number;
    sessions: number;
  };

  type Props = {
    buckets: ChartBucket[];
    /** Suffix appended to the metric name to form the chart title.
     *  e.g. "by month" → "Watch time by month";
     *       "in April 2026" → "Watch time in April 2026". */
    titleSuffix: string;
    loading?: boolean;
    onBarClick?: (key: string) => void;
    /** Which metric to plot. Bindable so the parent can persist the
     *  user's selection across drill levels — without this prop the
     *  chart would reset to "watch_time" every time a new BarChart
     *  mounts after a drill. */
    metric?: Metric;
    /** Show the "Default" pill next to the toggle (lets the user pin their
     *  default chart metric). Off for the per-entity detail charts. */
    defaultPill?: boolean;
    /** chartPrefs scope for the Default pill — each chart's default is
     *  independent. */
    defaultScope?: string;
  };
  let {
    buckets,
    titleSuffix,
    loading = false,
    onBarClick,
    metric = $bindable<Metric>("watch_time"),
    defaultPill = false,
    defaultScope = "overview",
  }: Props = $props();

  function valueFor(b: ChartBucket): number {
    switch (metric) {
      case "watch_time": return b.watch_time_ms;
      case "cumshots":   return b.cumshots;
      case "sessions":   return b.sessions;
    }
  }

  function labelFor(b: ChartBucket): string {
    const v = valueFor(b);
    if (v === 0) return "";
    if (metric === "watch_time") return formatDuration(v);
    return String(v);
  }

  function metricUnitLabel(m: Metric): string {
    switch (m) {
      case "watch_time": return "Watch time (hh:mm)";
      case "cumshots":   return w("Cumshots");
      case "sessions":   return "Sessions";
    }
  }

  function metricName(m: Metric): string {
    switch (m) {
      case "watch_time": return "Watch time";
      case "cumshots":   return w("Cumshots");
      case "sessions":   return "Sessions";
    }
  }

  const maxValue = $derived(Math.max(1, ...buckets.map(valueFor)));

  function niceCeil(v: number): number {
    if (v <= 0) return 1;
    if (metric === "watch_time") {
      const halfHourMs = 30 * 60 * 1000;
      if (v < halfHourMs) return halfHourMs;
      return Math.ceil(v / (5 * halfHourMs)) * (5 * halfHourMs);
    } else {
      const order = Math.pow(10, Math.floor(Math.log10(v)));
      return Math.ceil(v / order) * order;
    }
  }

  const yTicks = $derived.by(() => {
    const niceMax = niceCeil(maxValue);
    const step = niceMax / 5;
    return [0, step, step * 2, step * 3, step * 4, niceMax].reverse();
  });

  function tickLabel(v: number): string {
    if (metric === "watch_time") {
      if (v === 0) return "0h";
      const hours = Math.round(v / (60 * 60 * 1000));
      return `${hours}h`;
    }
    return String(Math.round(v));
  }

  function tooltipFor(b: ChartBucket): string {
    const wt = formatDuration(b.watch_time_ms);
    const detail = b.tooltipDetail ? ` (${b.tooltipDetail})` : "";
    return `${b.label}${detail} - ${wt} • ${b.sessions} session${b.sessions === 1 ? "" : "s"} • ${b.cumshots} ${w(b.cumshots === 1 ? "cumshot" : "cumshots")}`;
  }

  function handleClick(b: ChartBucket) {
    if (onBarClick) onBarClick(b.key);
  }
</script>

<div class="chart-card metric-{metric}">
  <div class="chart-head">
    <h2>{metricName(metric)} {titleSuffix}</h2>
    <div class="metric-controls">
      {#if defaultPill}<MetricDefaultPill scope={defaultScope} current={metric} />{/if}
      <div class="metric-toggle">
        <button class:active={metric === "cumshots"}   onclick={() => metric = "cumshots"}>{w("Cumshots")}</button>
        <button class:active={metric === "watch_time"} onclick={() => metric = "watch_time"}>Watch time</button>
        <button class:active={metric === "sessions"}   onclick={() => metric = "sessions"}>Sessions</button>
      </div>
    </div>
  </div>

  {#if loading && buckets.length === 0}
    <div class="loading">loading...</div>
  {:else}
    <div class="chart">
      <div class="y-axis">
        {#each yTicks as t (t)}
          <div class="y-tick"><span>{tickLabel(t)}</span></div>
        {/each}
      </div>

      <div class="plot-wrap">
        <div class="plot">
          <div class="gridlines">
            {#each yTicks as _ (_)}
              <div class="gridline"></div>
            {/each}
          </div>

          <div class="bars">
            {#each buckets as b (b.key)}
              {@const v = valueFor(b)}
              {@const ratio = yTicks[0] > 0 ? v / yTicks[0] : 0}
              <button
                class="bar-col"
                class:empty={v === 0}
                onclick={() => handleClick(b)}
                title={tooltipFor(b)}
              >
                <div class="bar-wrap">
                  {#if v > 0}
                    <span class="bar-value">{labelFor(b)}</span>
                  {/if}
                  <div class="bar" style={`height: ${Math.max(2, ratio * 100)}%;`}></div>
                </div>
              </button>
            {/each}
          </div>
        </div>

        <div class="x-labels">
          {#each buckets as b (b.key)}
            <div class="x-label">{b.label}</div>
          {/each}
        </div>
      </div>
    </div>

    <div class="legend">
      <span class="legend-swatch"></span>
      <span>{metricUnitLabel(metric)}</span>
    </div>
  {/if}
</div>

<style>
  .chart-card {
    background: var(--bg-card);
    border: 1px solid var(--border);
    border-radius: var(--radius, 10px);
    padding: 20px 24px 20px;
  }

  .chart-head {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 24px;
    gap: 12px;
  }
  .chart-head h2 {
    margin: 0;
    font-size: 13px;
    color: var(--text);
    font-weight: 600;
    letter-spacing: -0.01em;
  }
  .metric-controls {
    display: flex;
    align-items: center;
    gap: 10px;
  }

  .metric-toggle {
    display: flex;
    gap: 2px;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 7px;
    padding: 2px;
  }
  .metric-toggle button {
    background: transparent;
    border: none;
    color: var(--text-muted);
    padding: 4px 10px;
    font-size: 11px;
    font-weight: 500;
    border-radius: 5px;
    cursor: pointer;
    font-family: inherit;
    transition: background 120ms, color 120ms;
  }
  .metric-toggle button:hover { color: var(--text); }
  .metric-toggle button.active { background: var(--accent); color: #fff; }

  .loading { text-align: center; padding: 80px; color: var(--text-muted); font-size: 13px; }

  .chart {
    display: flex;
    gap: 10px;
    height: 380px;
    margin-bottom: 16px;
  }

  /* ---------- Y-axis ---------- */
  .y-axis {
    width: 40px;
    display: flex;
    flex-direction: column;
    justify-content: space-between;
    flex-shrink: 0;
    font-size: 10px;
    color: var(--fg-muted);
    font-variant-numeric: tabular-nums;
    font-family: var(--font-mono);
    text-align: right;
    padding: 80px 0 0;
  }
  .y-tick { height: 0; display: flex; align-items: center; justify-content: flex-end; }
  .y-tick span { transform: translateY(-50%); display: inline-block; }

  /* ---------- plot + x labels ---------- */
  .plot-wrap {
    flex: 1;
    display: flex;
    flex-direction: column;
    min-width: 0;
  }
  .plot {
    flex: 1;
    position: relative;
    border-bottom: 1px solid var(--border);
    /* 80px top reserve so value labels (rotated for watch time, horizontal
       for cumshots/sessions) never clip into the topmost gridline or into
       the bars themselves. Tuned for the worst case: a maximum-height bar
       (ratio ≈ 1.0) with a wide watch-time string like "147:32:00". */
    padding: 80px 0 0;
  }
  .gridlines {
    position: absolute;
    inset: 80px 0 0 0;
    display: flex;
    flex-direction: column;
    justify-content: space-between;
    pointer-events: none;
  }
  .gridline { border-top: 1px dashed var(--chart-grid, #1A1D25); height: 0; }

  .bars {
    position: relative;
    height: 100%;
    display: grid;
    grid-auto-flow: column;
    grid-auto-columns: 1fr;
    gap: 6px;
  }

  .bar-col {
    background: transparent;
    border: none;
    padding: 0;
    cursor: pointer;
    display: flex;
    flex-direction: column;
    align-items: center;
    font-family: inherit;
    min-width: 0;
    height: 100%;
    transition: opacity 120ms;
  }
  .bar-col.empty { cursor: default; }

  .bar-wrap {
    flex: 1;
    width: 80%;
    max-width: 40px;
    min-width: 8px;
    display: flex;
    flex-direction: column;
    justify-content: flex-end;
    align-items: center;
    position: relative;
  }

  /* ---------- bar (metric-coloured via parent .chart-card class) ---------- */
  .bar {
    width: 100%;
    border-radius: 3px 3px 0 0;
    min-height: 2px;
    transition: height 200ms ease, background 120ms, filter 120ms;
  }
  /* default (overridden per metric below) */
  .bar {
    background: linear-gradient(180deg, var(--accent) 0%, color-mix(in srgb, var(--accent) 80%, transparent) 100%);
  }
  /* Per-metric bar colour. Cumshots is coral (the brand colour — cumshot data
     is coral everywhere); watch time reads as bright white; sessions uses the
     warm bone/cream so it differs cleanly from both. */
  .chart-card.metric-watch_time .bar {
    background: linear-gradient(180deg, var(--fg-strong) 0%, color-mix(in srgb, var(--fg-strong) 80%, transparent) 100%);
  }
  .chart-card.metric-cumshots .bar {
    background: linear-gradient(180deg, var(--accent) 0%, color-mix(in srgb, var(--accent) 80%, transparent) 100%);
  }
  .chart-card.metric-sessions .bar {
    background: linear-gradient(180deg, var(--highlight) 0%, color-mix(in srgb, var(--highlight) 80%, transparent) 100%);
  }
  .bar-col:hover:not(.empty) .bar { filter: brightness(1.12); }

  .bar-col.empty .bar {
    background: var(--bg-card-hover);
    border: 1px dashed var(--border);
  }

  /* ---------- value labels on top of bars ----------
     Default: horizontal text (used for cumshots + sessions where values
     are single- or double-digit integers and fit naturally above the bar).
     Watch time overrides below to rotate -90° (the format is long enough
     that horizontal text would collide with neighbouring columns). */
  .bar-value {
    position: absolute;
    bottom: calc(100% + 14px);
    left: 50%;
    transform: translateX(-50%);
    font-family: var(--font-mono);
    font-size: 11px;
    font-weight: 600;
    color: var(--fg-muted);
    white-space: nowrap;
    font-variant-numeric: tabular-nums;
    letter-spacing: 0.02em;
    pointer-events: none;
  }
  /* Watch time: rotate -90°. With transform-origin: center, the rotated
     visual extends ±text_width/2 above and below the layout's centre.
     Pushing `bottom` to +40px lifts the layout high enough that even the
     longest watch-time strings (~56px wide) keep their visual bottom edge
     clear of the tallest bar (and clear of the topmost gridline). */
  .chart-card.metric-watch_time .bar-value {
    bottom: calc(100% + 40px);
    transform: translateX(-50%) rotate(-90deg);
    transform-origin: center;
  }

  /* ---------- x-axis labels (separate row, below chart border) ---------- */
  .x-labels {
    margin-top: 14px;
    display: grid;
    grid-auto-flow: column;
    grid-auto-columns: 1fr;
    gap: 6px;
  }
  .x-label {
    font-family: var(--font-mono);
    font-size: 11px;
    color: var(--fg-muted);
    font-weight: 700;
    text-align: center;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  /* ---------- legend ---------- */
  .legend {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 11px;
    color: var(--fg-muted);
    justify-content: center;
    padding-top: 6px;
  }
  .legend-swatch {
    width: 10px;
    height: 10px;
    border-radius: 2px;
    display: inline-block;
    background: var(--accent);
  }
  .chart-card.metric-watch_time .legend-swatch { background: var(--fg-strong); }
  .chart-card.metric-cumshots   .legend-swatch { background: var(--accent); }
  .chart-card.metric-sessions   .legend-swatch { background: var(--highlight); }
</style>
