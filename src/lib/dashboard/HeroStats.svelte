<!--
  Hero stats top row — six primary lifestyle metrics. These are
  ALL-TIME / TODAY-scoped by design and DO NOT respond to the
  dashboard filter; they are anchored above the FilterBar in the
  Overview layout. The "this range" sub-row (formerly the bottom of
  this component) is now its own `HeroStatsRange` component, lives
  BELOW the filter bar, and reads filter-aware range stats.

  Per the climax-design system:
   - Non-milestone, non-warn stats: accent (coral) icon, fg-strong number.
   - Milestone stats (Record day, Record streak, Longest session): highlight
     (bone) on both icon and number. They alternate with the accent ones, so
     the row reads coral / cream / coral / cream / coral / cream - keep that
     if anything is ever inserted.
   - Warn stats (Streak at-risk): amber on both icon and number, plus a
     small at-risk sub-label.
   - Icons are Lucide (1.75 stroke), except the cumshot glyph (custom
     three-droplet, used filled).
-->
<script lang="ts">
  import { w } from "$lib/wording.svelte";
  import { onMount, onDestroy } from "svelte";
  import Icon from "$lib/Icon.svelte";
  import { api, formatDuration, type HeroStats } from "$lib/api";

  type Props = {
    /** Click the Record-day card -> open that day in the Overview drill-down. */
    onRecordDay?: (date: string) => void;
    /** Click the Longest-session card -> open that session in the Overview drill-down. */
    onLongestSession?: (date: string, sessionId: number) => void;
    /** Click the Record-streak card -> scope the Overview filter to that run.
     *  A range, not a day, so it drives the date filter rather than the
     *  drill-down below (which only ever holds one day). */
    onRecordStreak?: (start: string, end: string) => void;
  };
  let { onRecordDay, onLongestSession, onRecordStreak }: Props = $props();

  let stats = $state<HeroStats | null>(null);
  let loading = $state(true);
  let poll: ReturnType<typeof setInterval> | null = null;

  // The two milestone cards are clickable only when there's a real day/session
  // behind them (and a handler is wired). No data -> plain, non-interactive card.
  const recordClickable = $derived(
    !!onRecordDay && !!stats?.record_day_date && (stats?.record_day_count ?? 0) > 0,
  );
  const longestClickable = $derived(
    !!onLongestSession &&
      stats?.longest_session_id != null &&
      !!stats?.longest_session_date &&
      (stats?.longest_session_ms ?? 0) > 0,
  );
  const streakClickable = $derived(
    !!onRecordStreak &&
      !!stats?.record_streak_start &&
      !!stats?.record_streak_end &&
      (stats?.record_streak_days ?? 0) > 0,
  );

  async function refresh() {
    try {
      stats = await api.dashboardHeroStats();
    } catch (e) {
      console.error("hero stats refresh failed", e);
    } finally {
      loading = false;
    }
  }

  onMount(() => {
    refresh();
    poll = setInterval(refresh, 10_000);
  });
  onDestroy(() => { if (poll) clearInterval(poll); });

  function formatRecordDate(d: string | null): string {
    if (!d) return "";
    const [y, m, day] = d.split("-").map((s) => parseInt(s, 10));
    const dt = new Date(y, m - 1, day);
    return dt.toLocaleDateString([], { day: "numeric", month: "short", year: "2-digit" });
  }

  function formatDaysSince(d: number | null): string {
    if (d === null) return "-";
    if (d === 0) return "today";
    if (d === 1) return "1 day";
    return `${d} days`;
  }

  function formatStreak(days: number): string {
    if (days === 0) return "0";
    if (days === 1) return "1 day";
    return `${days} days`;
  }

  /** The record streak's span, as one short line under the value. Collapses the
   *  repeated parts: a run inside one month reads "2 - 31 July 26", one that
   *  crosses reads "28 June - 27 July 26". A single day is just the date. The
   *  day COUNT is deliberately absent - the value above already says it. */
  function formatStreakRange(start: string | null, end: string | null): string {
    if (!start || !end) return "";
    if (start === end) return formatRecordDate(start);
    const [sy, sm, sd] = start.split("-").map((s) => parseInt(s, 10));
    const [ey, em] = end.split("-").map((s) => parseInt(s, 10));
    const tail = formatRecordDate(end);
    if (sy === ey && sm === em) return `${sd} - ${tail}`;
    const startDt = new Date(sy, sm - 1, sd);
    const sameYear = sy === ey;
    return `${startDt.toLocaleDateString([], {
      day: "numeric",
      month: "short",
      // Keep the year on the start only when the run crosses one, so
      // "28 Dec 25 - 3 Jan 26" stays unambiguous.
      ...(sameYear ? {} : { year: "2-digit" }),
    })} - ${tail}`;
  }
</script>

<section class="hero-stats">
  <div class="primary">
    <!-- Cumshots today — accent (coral) icon -->
    <div class="stat glyph-droplet" title={w("Cumshots logged today.")}>
      <Icon name="cumshot" size={22} color="var(--accent)" filled />
      <div class="value">{loading ? "-" : stats?.cumshots_today ?? 0}</div>
      <div class="label">{w("Cumshots today")}</div>
    </div>

    <!-- Record day — milestone (bone). Clickable: opens that day below. -->
    <button
      type="button"
      class="stat milestone"
      class:clickable={recordClickable}
      disabled={!recordClickable}
      title={w("The most cumshots you have logged in one day.")}
      onclick={() => recordClickable && onRecordDay?.(stats!.record_day_date!)}
    >
      <Icon name="trophy" size={22} color="var(--highlight)" />
      <div class="value">{loading ? "-" : stats?.record_day_count ?? 0}</div>
      <div class="label">
        Record day
        {#if stats?.record_day_date}
          <span class="sub">{formatRecordDate(stats.record_day_date)}</span>
        {/if}
      </div>
    </button>

    <!-- Streak — accent unless at-risk (then amber) -->
    <div
      class="stat glyph-flame"
      class:warn={stats?.streak_at_risk}
      title="Your current unbroken run of days with at least one session."
    >
      <Icon
        name={stats?.streak_at_risk ? "alert-triangle" : "flame"}
        size={22}
        color={stats?.streak_at_risk ? "var(--warn)" : "var(--accent)"}
      />
      <div class="value">{loading ? "-" : formatStreak(stats?.current_streak_days ?? 0)}</div>
      <div class="label">
        Streak
        {#if stats?.streak_at_risk}
          <span class="sub sub-warn">at risk - log today</span>
        {/if}
      </div>
    </div>

    <!-- Record streak - milestone (bone). The longest run ever, so unlike Streak
         it can sit anywhere in history; if the run still going is the longest,
         this describes that one. Clickable: scopes the filter to the run's dates,
         since a range can't be shown by the single-day drill-down. -->
    <button
      type="button"
      class="stat milestone"
      class:clickable={streakClickable}
      disabled={!streakClickable}
      title="Your longest unbroken run of days with at least one session, anywhere in your history."
      onclick={() =>
        streakClickable &&
        onRecordStreak?.(stats!.record_streak_start!, stats!.record_streak_end!)}
    >
      <Icon name="award" size={22} color="var(--highlight)" />
      <div class="value">{loading ? "-" : formatStreak(stats?.record_streak_days ?? 0)}</div>
      <div class="label">
        Record streak
        {#if stats?.record_streak_start}
          <span class="sub">
            {formatStreakRange(stats.record_streak_start, stats.record_streak_end)}
          </span>
        {/if}
      </div>
    </button>

    <!-- Days since last cumshot — accent icon -->
    <div class="stat" title={w("Days since your most recent cumshot.")}>
      <Icon name="hourglass" size={22} color="var(--accent)" />
      <div class="value">{loading ? "-" : formatDaysSince(stats?.days_since_last_cumshot ?? null)}</div>
      <div class="label">{w("Since last cumshot")}</div>
    </div>

    <!-- Longest session — milestone (bone). Clickable: opens that session below. -->
    <button
      type="button"
      class="stat milestone"
      class:clickable={longestClickable}
      disabled={!longestClickable}
      title="Your longest single session, measured start to finish including any pauses."
      onclick={() => longestClickable && onLongestSession?.(stats!.longest_session_date!, stats!.longest_session_id!)}
    >
      <Icon name="timer" size={22} color="var(--highlight)" />
      <div class="value">{loading ? "-" : formatDuration(stats?.longest_session_ms ?? 0)}</div>
      <div class="label">
        Longest session
        {#if stats?.longest_session_date}
          <span class="sub">{formatRecordDate(stats.longest_session_date)}</span>
        {/if}
      </div>
    </button>
  </div>

</section>

<style>
  .hero-stats {
    background: var(--bg-card);
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    padding: 18px 22px;
    display: flex;
    flex-direction: column;
    gap: 16px;
  }

  .primary {
    display: grid;
    grid-template-columns: repeat(6, 1fr);
    gap: 14px;
  }

  .stat {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 4px;
    padding: 10px 6px;
    border-radius: 8px;
    transition: background var(--dur-2);
  }
  .stat:hover:not(:disabled) { background: var(--bg-card-hover); }
  /* The two milestone cards render as <button>; strip native chrome so they
     look identical to the non-clickable stat divs. */
  button.stat {
    border: none; background: transparent; font: inherit; color: inherit;
    text-align: left; width: 100%;
  }
  .stat.clickable { cursor: pointer; }
  button.stat:disabled { cursor: default; }

  .value {
    font-family: var(--font-display);
    font-size: 28px;
    font-weight: 700;
    letter-spacing: -0.02em;
    color: var(--fg-strong);
    font-variant-numeric: tabular-nums;
    line-height: 1.1;
    margin-top: 2px;
    white-space: nowrap;
  }
  /* Optical alignment, applied to EVERY icon in the row. Lucide draws inside a
     24 viewBox with roughly 2px of padding, so at 22px the visible ink starts a
     couple of pixels inboard of its box - while the value below it starts at its
     own left edge. Left-aligned boxes therefore read as right-shifted icons.
     Uniform on purpose: the padding is a property of the icon set, not of any
     one glyph, so singling icons out just makes the row uneven. */
  .stat :global(svg) { margin-left: -1.5px; }
  /* The droplet and the flame sit further inside their box than the Lucide four,
     and by different amounts, so each gets its own value. These override the
     baseline rather than stacking on it, so the total is readable at a glance.
     All three settled by eye against the numbers below them - there is no
     formula, the glyphs simply have different bearings. */
  .stat.glyph-droplet :global(svg) { margin-left: -2.5px; }
  .stat.glyph-flame :global(svg) { margin-left: -2px; }

  .stat.milestone .value { color: var(--highlight); }
  .stat.warn .value { color: var(--warn); }

  .label {
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: var(--ls-eyebrow, 0.14em);
    color: var(--fg-muted);
    font-weight: 600;
    line-height: 1.4;
  }
  .sub {
    display: block;
    text-transform: none;
    letter-spacing: 0;
    font-weight: 500;
    color: var(--fg-subtle);
    font-size: 10px;
    margin-top: 1px;
  }
  .sub-warn {
    color: var(--warn);
    font-weight: 600;
  }

</style>
