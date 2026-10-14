<!--
  Hero stats range row — four stats that respond to the active filter.
  Sits BELOW the FilterBar in the Overview layout. The label suffix
  ("this week" / "in May 2026" / etc) tracks the filter's preset.

  This is the row formerly known as `.sub-row` on HeroStats; pulled
  out so the filter bar can sit between it and the immutable top row,
  visually separating "what changes with the filter" from "what doesn't".
-->
<script lang="ts">
  import { w } from "$lib/wording.svelte";
  import { onMount, onDestroy } from "svelte";
  import { api, formatDuration, type RangeStats } from "$lib/api";
  import { filterStore, PRESET_LABELS } from "$lib/filter-store.svelte";

  let stats = $state<RangeStats | null>(null);
  let loading = $state(true);
  let poll: ReturnType<typeof setInterval> | null = null;

  /** Re-fetch whenever the filter facets change. Bus-stop debounced
   *  via the natural rune dependency tracking — each unique signature
   *  triggers exactly one refetch. */
  $effect(() => {
    refresh({
      startDay: filterStore.startDay,
      endDay: filterStore.endDay,
      sceneContentItemIds: filterStore.sceneIds,
      performerIds: filterStore.performerIds,
      studioIds: filterStore.studioIds,
      tagIds: filterStore.tagIds,
    });
  });

  async function refresh(args: {
    startDay: string;
    endDay: string;
    sceneContentItemIds: number[];
    performerIds: string[];
    studioIds: string[];
    tagIds: string[];
  }) {
    try {
      stats = await api.dashboardRangeStats(args);
    } catch (e) {
      console.error("range stats refresh failed", e);
    } finally {
      loading = false;
    }
  }

  onMount(() => {
    // Slow background poll so an active session updating doesn't go
    // unnoticed even when the user hasn't touched the filter bar.
    poll = setInterval(() => {
      refresh({
        startDay: filterStore.startDay,
        endDay: filterStore.endDay,
        sceneContentItemIds: filterStore.sceneIds,
        performerIds: filterStore.performerIds,
        studioIds: filterStore.studioIds,
        tagIds: filterStore.tagIds,
      });
    }, 10_000);
  });
  onDestroy(() => { if (poll) clearInterval(poll); });

  /** "this week" / "in May 2026" / "in last 30 days" — used as a suffix
   *  on the stat labels so the user sees what range these are scoped to. */
  const rangeSuffix = $derived.by(() => {
    if (filterStore.preset === "today") return "today";
    if (filterStore.preset === "yesterday") return "yesterday";
    if (filterStore.preset === "all_time") return "all-time";
    if (filterStore.preset === "custom") return "in range";
    return PRESET_LABELS[filterStore.preset].toLowerCase();
  });
</script>

<section class="range-row">
  <div class="sub-stat">
    <div class="sub-value">{loading ? "-" : formatDuration(stats?.watch_time_ms ?? 0)}</div>
    <div class="sub-label">Watch time {rangeSuffix}</div>
  </div>
  <div class="sub-stat">
    <div class="sub-value accent">{loading ? "-" : stats?.cumshots ?? 0}</div>
    <div class="sub-label">{w("Cumshots")} {rangeSuffix}</div>
  </div>
  <div class="sub-stat">
    <div class="sub-value">{loading ? "-" : stats?.sessions ?? 0}</div>
    <div class="sub-label">Sessions {rangeSuffix}</div>
  </div>
  <div class="sub-stat">
    <div class="sub-value">{loading ? "-" : stats?.active_days ?? 0}</div>
    <div class="sub-label">Active days {rangeSuffix}</div>
  </div>
</section>

<style>
  .range-row {
    background: var(--bg-card);
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    padding: 14px 22px;
    display: grid;
    grid-template-columns: repeat(4, 1fr);
    gap: 12px;
  }

  .sub-stat {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 4px 6px;
  }
  .sub-value {
    font-family: var(--font-display);
    font-size: 18px;
    font-weight: 600;
    color: var(--fg);
    font-variant-numeric: tabular-nums;
    line-height: 1.2;
    letter-spacing: -0.01em;
  }
  .sub-value.accent { color: var(--accent); }
  .sub-label {
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: var(--ls-eyebrow, 0.14em);
    color: var(--fg-muted);
    font-weight: 600;
  }
</style>
