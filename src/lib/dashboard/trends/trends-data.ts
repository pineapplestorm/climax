// Trends client-side helpers — Phase 6.
//
// Typed port of the design's data-trends.js. The Trends page fetches ONE wide
// daily series (full history) + per-range hour-histogram / entity-breakdown
// from the backend; everything in the Overview tab and the time-grouped report
// rows are derived here by slicing / bucketing that series. Keeps the chart
// math in one place and matches the rest of the app's metric vocabulary.

import { w } from "$lib/wording.svelte";
import {
  type DailyActivity,
  type EntityBreakdownRow,
  type HourHistogram,
  type RangeBucket,
  formatWatchShort,
} from "$lib/api";
import { type Granularity, type DatePreset } from "$lib/filter-store.svelte";

export type MetricId = "cumshots" | "watch_time" | "sessions" | "active";

/** A daily series row enriched with fields the UI derives client-side. */
export interface DayPoint extends DailyActivity {
  /** Local-midnight epoch ms. */
  ts: number;
  /** 0 Sun ... 6 Sat. */
  dow: number;
  /** sessions > 0 — the streak / active-days definition (matches HeroStats). */
  active: boolean;
}

export interface MetricDef {
  id: MetricId;
  label: string;
  /** CHART colour — cumshots coral (the brand colour, used for cumshot data
   *  everywhere), watch time white, sessions bone, active green. */
  color: string;
  fmt: (v: number) => string;
}

export const METRICS: Record<MetricId, MetricDef> = {
  // A getter, not a value: the label follows the Wording setting, and reading it
  // from a template keeps that template reactive to the change. It also means
  // the CSV export's header and filename use the same word the screen shows.
  cumshots:   { id: "cumshots",   get label() { return w("Cumshots"); }, color: "var(--accent)", fmt: (v) => String(v) },
  watch_time: { id: "watch_time", label: "Watch time",  color: "var(--fg-strong)", fmt: (v) => formatWatchShort(v) },
  sessions:   { id: "sessions",   label: "Sessions",    color: "var(--highlight)", fmt: (v) => String(v) },
  active:     { id: "active",     label: "Active days", color: "var(--live)",      fmt: (v) => String(v) },
};

export const METRIC_ORDER: MetricId[] = ["cumshots", "watch_time", "sessions", "active"];

const WEEKDAYS_SHORT = ["SUN", "MON", "TUE", "WED", "THU", "FRI", "SAT"];
const MONTHS_SHORT = ["JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC"];

function pad(n: number): string {
  return n.toString().padStart(2, "0");
}
function parseYmd(s: string): Date {
  const [y, m, d] = s.split("-").map((n) => parseInt(n, 10));
  return new Date(y, m - 1, d);
}
function ymd(d: Date): string {
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
}

export function enrich(series: DailyActivity[]): DayPoint[] {
  return series.map((d) => {
    const dt = parseYmd(d.date);
    return { ...d, ts: dt.getTime(), dow: dt.getDay(), active: d.sessions > 0 };
  });
}

/** Value of one metric on one day. `active` → 0/1. */
export function valueOf(d: DayPoint, metric: MetricId): number {
  switch (metric) {
    case "cumshots": return d.cumshots;
    case "watch_time": return d.watch_time_ms;
    case "sessions": return d.sessions;
    case "active": return d.active ? 1 : 0;
  }
}

export function sumMetric(days: DayPoint[], metric: MetricId): number {
  return days.reduce((a, d) => a + valueOf(d, metric), 0);
}

export function sliceByDate(days: DayPoint[], start: string, end: string): DayPoint[] {
  return days.filter((d) => d.date >= start && d.date <= end);
}

/** The equal-length window immediately before [start, end] — for ▲/▼ deltas. */
export function prevWindow(start: string, end: string): { start: string; end: string } {
  const s = parseYmd(start);
  const e = parseYmd(end);
  const lenDays = Math.round((e.getTime() - s.getTime()) / 86400000) + 1;
  const prevEnd = new Date(s);
  prevEnd.setDate(prevEnd.getDate() - 1);
  const prevStart = new Date(prevEnd);
  prevStart.setDate(prevStart.getDate() - (lenDays - 1));
  return { start: ymd(prevStart), end: ymd(prevEnd) };
}

/** Mon-first day-of-week totals for a metric. */
export function byDayOfWeek(days: DayPoint[], metric: MetricId): { label: string; value: number }[] {
  const buckets = Array.from({ length: 7 }, () => 0);
  for (const d of days) buckets[d.dow] += valueOf(d, metric);
  return [1, 2, 3, 4, 5, 6, 0].map((dow) => ({ label: WEEKDAYS_SHORT[dow], value: buckets[dow] }));
}

/** N evenly-sized buckets of `metric` across `days` — the summary sparkline. */
export function sparkline(days: DayPoint[], metric: MetricId, n = 12): number[] {
  if (days.length === 0) return Array(n).fill(0);
  const size = Math.max(1, Math.floor(days.length / n));
  const pts: number[] = [];
  for (let i = 0; i < n; i++) {
    pts.push(sumMetric(days.slice(i * size, (i + 1) * size), metric));
  }
  return pts;
}

// ---------- area chart / report time bucketing ----------

export interface AreaBucket {
  key: string;
  label: string;
  cumshots: number;
  watch_time_ms: number;
  sessions: number;
  /** Count of active days in the bucket. */
  active: number;
}

function bucketKey(d: DayPoint, g: Granularity): string {
  const dt = new Date(d.ts);
  if (g === "year") return String(dt.getFullYear());
  if (g === "month") return `${dt.getFullYear()}-${pad(dt.getMonth() + 1)}`;
  if (g === "week") {
    const r = new Date(dt);
    r.setDate(r.getDate() - ((r.getDay() + 6) % 7)); // back to Monday
    return ymd(r);
  }
  return d.date;
}

function bucketLabel(key: string, g: Granularity, prevKey: string | null): string {
  if (g === "year") return key;
  if (g === "month") {
    const [y, m] = key.split("-").map((n) => parseInt(n, 10));
    return `${MONTHS_SHORT[m - 1]} '${String(y).slice(-2)}`;
  }
  const [y, m, d] = key.split("-").map((n) => parseInt(n, 10));
  // Year marker on the first label and at year changes — computed against the
  // previous label in DISPLAY order (ascending chart axes, newest-first report
  // rows), so a span crossing New Year is never ambiguous. Same convention as
  // the Overview chart's axis labels.
  const prevY = prevKey ? parseInt(prevKey.split("-")[0], 10) : null;
  const yearChanged = prevY === null || prevY !== y;
  const yy = ` '${String(y).slice(-2)}`;
  if (g === "week") return `${d} ${MONTHS_SHORT[m - 1]}${yearChanged ? yy : ""}`;
  // day — month abbrev on the first label and at month changes.
  const prevM = prevKey ? parseInt(prevKey.split("-")[1], 10) : null;
  const monthChanged = prevM === null || prevM !== m || yearChanged;
  if (!monthChanged) return String(d);
  return `${d} ${MONTHS_SHORT[m - 1]}${yearChanged ? yy : ""}`;
}

/** Group days into contiguous buckets (day/week/month/year), pre-summing all
 *  metrics so the area chart can re-plot on metric toggle without recompute. */
export function bucketDays(days: DayPoint[], g: Granularity): AreaBucket[] {
  const map = new Map<string, AreaBucket>();
  for (const d of days) {
    const key = bucketKey(d, g);
    let b = map.get(key);
    if (!b) {
      b = { key, label: "", cumshots: 0, watch_time_ms: 0, sessions: 0, active: 0 };
      map.set(key, b);
    }
    b.cumshots += d.cumshots;
    b.watch_time_ms += d.watch_time_ms;
    b.sessions += d.sessions;
    b.active += d.active ? 1 : 0;
  }
  const buckets = [...map.values()];
  buckets.forEach((b, i) => {
    b.label = bucketLabel(b.key, g, i === 0 ? null : buckets[i - 1].key);
  });
  return buckets;
}

export function areaValue(b: AreaBucket, metric: MetricId): number {
  switch (metric) {
    case "cumshots": return b.cumshots;
    case "watch_time": return b.watch_time_ms;
    case "sessions": return b.sessions;
    case "active": return b.active;
  }
}

/** 24h hour → "12am" / "1am" / ... / "12pm" / "11pm". */
export function hour12(h: number): string {
  const ampm = h < 12 ? "am" : "pm";
  const hr = h % 12 === 0 ? 12 : h % 12;
  return `${hr}${ampm}`;
}

/** Compact hour axis label: only midnight/noon carry the am/pm anchor, the rest
 *  are bare numbers — "12am, 1, 2 ... 11, 12pm, 1, 2 ... 11" — so all 24 fit. */
export function hourCompact(h: number): string {
  if (h === 0) return "12am";
  if (h === 12) return "12pm";
  return String(h % 12);
}

/** Full, unambiguous hour label for a TOOLTIP (which has room the axis doesn't):
 *  "12am, 1am ... 11am, 12pm, 1pm ... 11pm". The compact axis shows bare numbers,
 *  so the hover carries the am/pm every hour instead of only at midnight/noon. */
export function hourFull(h: number): string {
  if (h === 0) return "12am";
  if (h === 12) return "12pm";
  return h < 12 ? `${h}am` : `${h - 12}pm`;
}

/** Contextual "vs ..." label for the summary-card delta. Fixed presets get an
 *  exact phrase; custom range falls back to "previous period"; all-time has no
 *  prior window → null (the card then shows no comparison). */
export function vsLabelFor(preset: DatePreset): string | null {
  switch (preset) {
    case "today": return "yesterday";
    case "yesterday": return "the day before";
    case "this_week": return "last week";
    case "last_week": return "the week before";
    case "this_month": return "last month";
    case "last_month": return "the month before";
    case "last_7_days": return "previous 7 days";
    case "last_30_days": return "previous 30 days";
    case "last_90_days": return "previous 90 days";
    case "last_12_months": return "previous 12 months";
    case "this_year": return "last year";
    case "all_time": return null;
    case "custom": return "previous period";
  }
}

// ---------- report builder ----------

export type ReportDim =
  | "day" | "week" | "month" | "dow" | "hour"
  | "scene" | "performer" | "studio" | "tag";

export const REPORT_DIMS: { id: ReportDim; label: string }[] = [
  { id: "day", label: "Day" },
  { id: "week", label: "Week" },
  { id: "month", label: "Month" },
  { id: "dow", label: "Day of week" },
  { id: "hour", label: "Time of day" },
  { id: "scene", label: "Scene" },
  { id: "performer", label: "Performer" },
  { id: "studio", label: "Studio" },
  { id: "tag", label: "Tag" },
];

const ENTITY_DIMS: ReportDim[] = ["scene", "performer", "studio", "tag"];
export function isEntityDim(dim: ReportDim): boolean {
  return ENTITY_DIMS.includes(dim);
}

/** Group-by options valid for a metric — the honesty matrix:
 *   - Active days can't be grouped by an entity (a busy day and a quiet day
 *     both count once; ranking entities by it is meaningless) — the options
 *     are simply not offered.
 *   - Watch time can't be grouped by Time of day (no per-hour watch data;
 *     only per-day durations exist). No silent proxies. */
export function dimsForMetric(metric: MetricId): ReportDim[] {
  return REPORT_DIMS.map((d) => d.id).filter((id) => {
    if (metric === "active" && ENTITY_DIMS.includes(id)) return false;
    if (metric === "watch_time" && id === "hour") return false;
    return true;
  });
}

export interface ReportRow {
  label: string;
  value: number;
  /** Machine-friendly label for CSV export (ISO dates: YYYY-MM-DD for days /
   *  week-Mondays, YYYY-MM for months). Screen shows `label`; the CSV prefers
   *  this — same rows + values, formatted for Excel instead of eyes. */
  csvLabel?: string;
}

/** Time-grouped report rows (day/week/month/dow) from the daily series.
 *  Complete domain, zeros included — a gappy time series misleads in Excel
 *  (a missing day reads as "no data", not 0). Date-bucketed rows render
 *  NEWEST-FIRST (present at the top, scroll down into the past); labels are
 *  recomputed in display order so day rows carry their month marker at each
 *  visible month change. Day-of-week stays Monday-first (not dates). */
export function reportTimeRows(days: DayPoint[], dim: ReportDim, metric: MetricId): ReportRow[] {
  if (dim === "dow") return byDayOfWeek(days, metric);
  const g: Granularity | null =
    dim === "day" ? "day" : dim === "week" ? "week" : dim === "month" ? "month" : null;
  if (!g) return [];
  const desc = bucketDays(days, g).reverse();
  return desc.map((b, i) => ({
    label: bucketLabel(b.key, g, i === 0 ? null : desc[i - 1].key),
    value: areaValue(b, metric),
    csvLabel: b.key,
  }));
}

/** Hour-of-day report rows from the histogram (watch_time is disallowed here).
 *  All 24 hours, zeros included — same complete-domain rule as day-of-week.
 *  24h "HH:00" labels parse as times in Excel, so screen + CSV share them. */
export function reportHourRows(hist: HourHistogram, metric: MetricId): ReportRow[] {
  const arr =
    metric === "cumshots" ? hist.cumshots
    : metric === "sessions" ? hist.sessions
    : metric === "active" ? hist.active_days
    : hist.cumshots;
  return arr.map((value, hour) => ({ label: `${pad(hour)}:00`, value }));
}

/** Time-series rows for a SPECIFIC entity — built from `dashboard_filtered_
 *  buckets` results (the same chip-aware path the Overview filters and browse
 *  detail charts use, so the numbers match those views). NEWEST-FIRST like the
 *  other date-bucketed reports, zero-filled by the backend, ISO csvLabels. */
export function reportSeriesRows(buckets: RangeBucket[], metric: MetricId): ReportRow[] {
  const desc = [...buckets].reverse();
  return desc.map((b, i) => ({
    label: bucketLabel(b.key, b.granularity, i === 0 ? null : desc[i - 1].key),
    value:
      metric === "watch_time" ? b.watch_time_ms
      : metric === "sessions" ? b.sessions
      : b.cumshots,
    csvLabel: b.key,
  }));
}

/** Human label for a granularity, as the CSV's time-column header. */
export function granDimLabel(g: Granularity): string {
  return g === "day" ? "Day" : g === "week" ? "Week" : g === "month" ? "Month" : "Year";
}

/** Entity report rows from a range-scoped breakdown. */
export function reportEntityRows(rows: EntityBreakdownRow[], metric: MetricId): ReportRow[] {
  return rows
    .map((r) => ({
      label: r.name,
      value:
        metric === "watch_time" ? r.watch_time_ms
        : metric === "cumshots" ? r.cumshots
        : metric === "sessions" ? r.sessions
        : 0, // active not allowed for entities
    }))
    .sort((a, b) => b.value - a.value);
}
