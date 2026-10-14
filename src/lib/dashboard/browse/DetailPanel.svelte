<!--
  Browse detail panel — drill-down stats for the selected scene / performer /
  studio / tag. Header (identity + Open-in-Stash + close) · stat strip with
  #-rank pills · period toggle · BarChart (reused, fed by the existing
  dashboard_filtered_buckets command) · Top scenes (performer/studio/tag).

  Rank is computed client-side from the full peer list the page already loaded.
-->
<script lang="ts">
  import { w } from "$lib/wording.svelte";
  import { onMount } from "svelte";
  import Icon from "$lib/Icon.svelte";
  import BarChart, { type ChartBucket } from "$lib/dashboard/BarChart.svelte";
  import DateRangePicker from "$lib/dashboard/DateRangePicker.svelte";
  import {
    detailSceneFilter,
    detailPerformerFilter,
    detailStudioFilter,
    detailTagFilter,
    dayCount,
  } from "$lib/filter-store.svelte";
  import { chartPrefs } from "$lib/chart-prefs.svelte";
  import { stashConfig } from "$lib/stash-config.svelte";
  import { nav, KIND_SECTION, type BrowseKind } from "$lib/dashboard/nav.svelte";
  import {
    api,
    formatWatchShort,
    formatAgo,
    formatDurationShort,
    competitionRank,
    type BrowseScene,
    type BrowsePerformer,
    type BrowseStudio,
    type BrowseTag,
    type RangeBucket,
  } from "$lib/api";
  import { colorForKey } from "./palette";
  import { detailLabel } from "./detail-buckets";

  type Item = BrowseScene | BrowsePerformer | BrowseStudio | BrowseTag;
  type Props = {
    kind: BrowseKind;
    item: Item;
    /** Full peer list (same kind) for #-rank computation. */
    peers: Item[];
    onClose: () => void;
  };
  let { kind, item, peers, onClose }: Props = $props();

  // Narrowed views for kind-specific fields.
  const scene = $derived(kind === "scene" ? (item as BrowseScene) : null);

  const eyebrow = $derived(kind.toUpperCase());
  const name = $derived(kind === "scene" ? (scene!.title ?? "Untitled scene") : (item as any).name);
  const initial = $derived((name ?? "?").trim()[0]?.toUpperCase() ?? "?");
  const performerInitials = $derived(
    String(name ?? "")
      .split(" ")
      .map((s) => s[0])
      .join("")
      .slice(0, 2)
      .toUpperCase(),
  );
  const color = $derived(colorForKey(String((item as any).id)));
  // Enriched image (performer/studio/tag); null until background enrichment lands.
  const imageUrl = $derived(((item as any).image_url ?? null) as string | null);
  const favorite = $derived(kind === "performer" && !!(item as BrowsePerformer).favorite);

  // ---- Open in Stash ----
  const openHref = $derived.by(() => {
    const base = stashConfig.baseUrl;
    if (!base) return null;
    if (kind === "scene") return scene!.external_id ? `${base}/scenes/${scene!.external_id}` : null;
    const seg = kind === "performer" ? "performers" : kind === "studio" ? "studios" : "tags";
    return `${base}/${seg}/${(item as any).id}`;
  });

  // ---- Rank ----
  // Performers never rank across genders: the peer group is this performer's
  // own side only. Same bucketing as the Trends toggle + the Performers gender
  // chip: male side = Stash MALE / TRANSGENDER_MALE; everyone else (FEMALE,
  // TRANSGENDER_FEMALE, NON_BINARY, INTERSEX, not-yet-enriched) counts female.
  function isMaleSide(g: string | null): boolean {
    return g === "MALE" || g === "TRANSGENDER_MALE";
  }
  const maleSide = $derived(kind === "performer" && isMaleSide((item as BrowsePerformer).gender));
  const rankPeers = $derived.by(() =>
    kind === "performer"
      ? peers.filter((p) => isMaleSide((p as BrowsePerformer).gender) === maleSide)
      : peers,
  );
  const peerCount = $derived(rankPeers.length);
  /** Tooltip noun: "female performers" / "male performers" / "scenes" / ... */
  const peerNoun = $derived(kind === "performer" ? `${maleSide ? "male" : "female"} performers` : `${kind}s`);
  // Competition ranking, so a TIE for the top shows #1 on every tied entity
  // (the old sort+findIndex arbitrarily made one of them #2). Counting how many
  // peers beat this one is the same scheme without needing the sort at all.
  function rankOf(key: "watch_time_ms" | "cumshots" | "scene_count" | "play_count"): number | null {
    const id = String((item as any).id);
    if (!rankPeers.some((x) => String((x as any).id) === id)) return null;
    return competitionRank(
      rankPeers.map((x) => ((x as any)[key] ?? 0) as number),
      ((item as any)[key] ?? 0) as number,
    );
  }

  type StatTile = { label: string; value: string; accent?: boolean; rank?: number | null; sub?: string };
  const tiles = $derived.by<StatTile[]>(() => {
    // Cumshots is the MIRROR total (incl. Stash-history imports). When some of
    // those weren't logged in a Climax session, show how many WERE tracked live.
    const trackedSub =
      item.cumshots_tracked < item.cumshots ? `${item.cumshots_tracked} tracked live` : undefined;
    const t: StatTile[] = [
      { label: "Watch time", value: formatWatchShort(item.watch_time_ms), rank: rankOf("watch_time_ms") },
      { label: w("Cumshots"), value: String(item.cumshots), accent: true, rank: rankOf("cumshots"), sub: trackedSub },
      { label: "Play count", value: String(item.play_count), rank: rankOf("play_count") },
      { label: "Sessions", value: String(item.sessions) },
      { label: "Last watched", value: formatAgo(item.last_watched_at) },
    ];
    if (kind === "performer") t.push({ label: "Scenes watched", value: String((item as BrowsePerformer).scene_count) });
    if (kind === "studio") t.push({ label: "Scenes watched", value: String((item as BrowseStudio).scene_count), rank: rankOf("scene_count") });
    if (kind === "tag") t.push({ label: "Scenes watched", value: String((item as BrowseTag).scene_count), rank: rankOf("scene_count") });
    return t;
  });

  // ---- Sub-line ----
  // The scene sub-line is rendered directly in the template (studio + performer
  // PILLS + plain duration/resolution); `sub` covers the non-scene kinds.
  const sub = $derived.by<string[]>(() => {
    if (kind === "scene") return [];
    const sc = (item as any).scene_count as number;
    if (kind === "performer") {
      const p = item as BrowsePerformer;
      const parts: string[] = [];
      if (p.aliases?.length) parts.push(`aka ${p.aliases.join(", ")}`);
      if (p.ethnicity) parts.push(p.ethnicity);
      if (p.country) parts.push(p.country);
      if (p.height_cm) parts.push(`${p.height_cm}cm`);
      if (p.hair_color) parts.push(`${p.hair_color} hair`);
      // Fall back to the scene count when no demographics are enriched yet.
      if (parts.length === 0) parts.push(`${sc} scene${sc === 1 ? "" : "s"} watched`);
      return parts;
    }
    if (kind === "studio") {
      const parts: string[] = [];
      if ((item as BrowseStudio).parent_studio) parts.push((item as BrowseStudio).parent_studio!);
      parts.push(`${sc} scene${sc === 1 ? "" : "s"} watched`);
      return parts;
    }
    return [`${sc} scene${sc === 1 ? "" : "s"} watched`, `${item.sessions} session${item.sessions === 1 ? "" : "s"}`];
  });

  // ---- Chart: the app-wide DateRangePicker (per-kind detail filter) + the
  // existing filtered-buckets command + BarChart. The date range (with its
  // per-preset "Default" badge) and the metric "Default" pill are per ENTITY
  // KIND, so a default set on one performer applies to every performer detail. ----
  const detailFilter = $derived(
    {
      scene: detailSceneFilter,
      performer: detailPerformerFilter,
      studio: detailStudioFilter,
      tag: detailTagFilter,
    }[kind],
  );
  const metricScope = $derived(`detail_${kind}`);
  let dateOpen = $state(false);
  let chartMetric = $state<"watch_time" | "cumshots" | "sessions">("cumshots");
  let buckets = $state<RangeBucket[]>([]);
  let chartLoading = $state(true);

  // Close the date popover on outside click / Escape (same as the Overview chip).
  $effect(() => {
    if (!dateOpen) return;
    function onDocClick(e: MouseEvent) {
      // composedPath(), not target.closest() - see FilterBar: a node removed by
      // its own click handler is detached by the time this runs, so closest()
      // returns null and an inside click reads as outside.
      const inside = e.composedPath().some(
        (n) => n instanceof Element && (n.classList.contains("cx-date-pop") || n.classList.contains("cx-date-chip")),
      );
      if (inside) return;
      dateOpen = false;
    }
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") dateOpen = false;
    }
    document.addEventListener("click", onDocClick);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("click", onDocClick);
      document.removeEventListener("keydown", onKey);
    };
  });

  // Snapshot of THIS detail view, pushed as the origin when navigating away via
  // a pill / top-scene row — so Back returns here.
  function originSnap() {
    return { section: KIND_SECTION[kind], entityKind: kind, entityId: (item as any).id };
  }

  function facetArgs() {
    const id = (item as any).id;
    switch (kind) {
      case "scene": return { sceneContentItemIds: [id as number] };
      case "performer": return { performerIds: [String(id)] };
      case "studio": return { studioIds: [String(id)] };
      case "tag": return { tagIds: [String(id)] };
    }
  }

  // Monotonic request token: Tauri's invoke() has no AbortController, so we tag
  // each request and only apply the latest — a slow earlier period switch can't
  // clobber a newer one's results.
  let reqToken = 0;
  async function loadBuckets() {
    const token = ++reqToken;
    chartLoading = true;
    try {
      const result = await api.dashboardFilteredBuckets({
        startDay: detailFilter.startDay,
        endDay: detailFilter.endDay,
        granularity: detailFilter.granularity,
        ...facetArgs(),
      });
      if (token !== reqToken) return;
      buckets = result;
    } catch (e) {
      if (token !== reqToken) return;
      console.error("detail buckets failed", e);
      buckets = [];
    } finally {
      if (token === reqToken) chartLoading = false;
    }
  }

  // Refetch whenever the item or the (per-kind) date window changes. Gated on the
  // store being ready so the first fetch uses the saved default, not the fallback.
  $effect(() => {
    if (!detailFilter.ready) return;
    (item as any).id;
    detailFilter.startDay;
    detailFilter.endDay;
    loadBuckets();
  });

  // Weekday x-axis labels for a ~week-wide window; day-numbers otherwise.
  const weekdayLabels = $derived(dayCount(detailFilter.startDay, detailFilter.endDay) <= 8);
  const chartBuckets = $derived.by<ChartBucket[]>(() =>
    buckets.map((b) => ({
      key: b.key,
      label: detailLabel(b, weekdayLabels),
      watch_time_ms: b.watch_time_ms,
      cumshots: b.cumshots,
      sessions: b.sessions,
    })),
  );

  // ---- Related "Top scenes" (performer / studio / tag) ----
  let allScenes = $state<BrowseScene[]>([]);
  const related = $derived.by<BrowseScene[]>(() => {
    if (kind === "scene") return [];
    const id = String((item as any).id);
    const match = (s: BrowseScene) =>
      kind === "performer" ? s.performers.some((p) => p.id === id)
      : kind === "studio" ? s.studio?.id === id
      : s.tags.some((t) => t.id === id);
    return [...allScenes]
      .filter(match)
      .sort((a, b) => b.cumshots - a.cumshots || b.watch_time_ms - a.watch_time_ms)
      .slice(0, 6);
  });

  onMount(async () => {
    // Apply this kind's saved date default (+ first-session-day for "All time").
    detailFilter.loadDefaults();
    // Seed the chart metric from this kind's saved default (3-metric chart has no
    // "active" option, so narrow it away).
    try {
      await chartPrefs.load(metricScope);
      const d = chartPrefs.defaultMetric(metricScope);
      // Seed only if the user hasn't already picked a metric during the load.
      if (chartMetric === "cumshots" && d !== "active") chartMetric = d;
    } catch (e) {
      console.error("detail metric default load failed", e);
    }
    if (kind !== "scene") {
      try {
        allScenes = await api.catalogBrowseScenes();
      } catch (e) {
        console.error("related scenes load failed", e);
      }
    }
  });
</script>

<section class="cx-detail" class:cx-detail-performer={kind === "performer"}>
  <header class="cx-detail-head">
    <div class="cx-detail-id">
      {#if kind === "scene"}
        <div class="cx-detail-thumb cx-thumb-16x9">
          {#if scene?.thumbnail_url}
            <img class="cx-thumb-img" src={scene.thumbnail_url} alt="" />
          {:else}
            <span class="cx-thumb-ph">{initial}</span>
          {/if}
        </div>
      {:else if kind === "performer"}
        <div class="cx-detail-thumb cx-thumb-3x4">
          {#if imageUrl}
            <img class="cx-thumb-img" src={imageUrl} alt="" />
          {:else}
            <span class="cx-thumb-ph cx-thumb-ph-large">{performerInitials}</span>
          {/if}
        </div>
      {:else if kind === "studio"}
        <div class="cx-studio-logo cx-detail-studio-logo cx-studio-thumb" style={`--studio-color: ${color};`}>
          {#if imageUrl}
            <img class="cx-thumb-img cx-contain-img" src={imageUrl} alt="" />
          {:else}
            <span class="cx-studio-initial">{initial}</span>
          {/if}
        </div>
      {:else}
        <div class="cx-detail-tag-glyph" style={`--tag-color: ${color};`}>
          {#if imageUrl}
            <img class="cx-thumb-img cx-contain-img" src={imageUrl} alt="" />
          {:else}
            <Icon name="tag" size={36} color="var(--tag-color)" />
          {/if}
        </div>
      {/if}

      <div class="cx-detail-meta">
        <div class="cx-detail-eyebrow">{eyebrow}</div>
        <div class="cx-detail-namerow">
          <h2 class="cx-detail-name">{name}</h2>
          {#if favorite}
            <Icon name="heart" size={16} color="var(--accent)" filled />
          {/if}
        </div>
        <div class="cx-detail-sub">
          {#if kind === "scene" && scene}
            {#if scene.studio}
              <button
                class="cx-pill cx-pill-studio"
                onclick={() => nav.goto("studio", scene.studio!.id, originSnap())}
                title={`Studio: ${scene.studio.name}`}
              >
                <Icon name="clapperboard" size={10} /><span>{scene.studio.name}</span>
              </button>
            {/if}
            {#each scene.performers as p (p.id)}
              <button
                class="cx-pill cx-pill-performer"
                onclick={() => nav.goto("performer", p.id, originSnap())}
                title={p.name}
              >{p.name}</button>
            {/each}
            {#if scene.duration_seconds}
              {#if scene.studio || scene.performers.length}<span class="cx-tile-sep">·</span>{/if}
              <span>{formatDurationShort(scene.duration_seconds)}</span>
            {/if}
            {#if scene.resolution}
              {#if scene.studio || scene.performers.length || scene.duration_seconds}<span class="cx-tile-sep">·</span>{/if}
              <span>{scene.resolution}</span>
            {/if}
          {:else}
            {#each sub as part, i (i)}
              {#if i > 0}<span class="cx-tile-sep">·</span>{/if}
              <span>{part}</span>
            {/each}
          {/if}
        </div>
      </div>
    </div>

    <div class="cx-detail-actions">
      {#if openHref}
        <a class="cx-btn-ghost" href={openHref} target="_blank" rel="noopener" title="Open in Stash">
          <Icon name="external-link" size={14} /> Open in Stash
        </a>
      {/if}
      <button class="cx-btn-ghost cx-btn-icon" onclick={onClose} aria-label="Close">
        <Icon name="x" size={16} />
      </button>
    </div>
  </header>

  <div class="cx-detail-stats">
    {#each tiles as t (t.label)}
      {@const isTop = t.rank === 1}
      <div class="cx-detail-stat" class:is-top={isTop}>
        <div class="cx-detail-stat-top">
          <div class="cx-detail-stat-value" class:accent={t.accent}>{t.value}</div>
          {#if t.rank}
            <span
              class="cx-detail-rank"
              class:top={isTop}
              title={`Ranked #${t.rank} of ${peerCount} ${peerNoun} by ${t.label.toLowerCase()}`}
            >
              {#if isTop}<Icon name="trophy" size={10} />{/if}
              <span>#{t.rank}</span>
            </span>
          {/if}
        </div>
        <div class="cx-detail-stat-label">{t.label}</div>
        {#if t.sub}<div class="cx-detail-stat-sub">{t.sub}</div>{/if}
      </div>
    {/each}
  </div>

  <div class="cx-detail-daterow">
    <div class="cx-detail-date">
      <button
        class="cx-date-chip"
        class:active={dateOpen}
        onclick={(e) => { e.stopPropagation(); dateOpen = !dateOpen; }}
        aria-expanded={dateOpen}
      >
        <Icon name="calendar" size={13} />
        <span>{detailFilter.presetLabel}</span>
        <Icon name="chevron-down" size={11} color="var(--fg-subtle)" />
      </button>
      {#if dateOpen}
        <div class="cx-date-pop">
          <DateRangePicker store={detailFilter} onClose={() => (dateOpen = false)} />
        </div>
      {/if}
    </div>
  </div>

  <BarChart
    buckets={chartBuckets}
    titleSuffix=""
    loading={chartLoading}
    bind:metric={chartMetric}
    defaultPill
    defaultScope={metricScope}
  />

  {#if related.length > 0}
    <div class="cx-detail-related">
      <h3 class="cx-detail-related-title">Top scenes</h3>
      <div class="cx-detail-related-grid">
        {#each related as s (s.id)}
          <button class="cx-detail-related-row" onclick={() => nav.goto("scene", s.id, originSnap())}>
            <div class="cx-tile-thumb cx-thumb-16x9 cx-related-thumb">
              {#if s.thumbnail_url}
                <img class="cx-thumb-img" src={s.thumbnail_url} alt="" />
              {:else}
                <span class="cx-thumb-ph">{(s.title ?? "?").trim()[0]?.toUpperCase() ?? "?"}</span>
              {/if}
              {#if s.cumshots > 0}
                <span class="cx-thumb-badge"><Icon name="cumshot" size={10} filled /><span>{s.cumshots}</span></span>
              {/if}
            </div>
            <div class="cx-related-info">
              <div class="cx-related-title">{s.title ?? `Scene ${s.external_id ?? "?"}`}</div>
              <div class="cx-related-meta">
                {formatWatchShort(s.watch_time_ms)} · {s.sessions} session{s.sessions === 1 ? "" : "s"}
              </div>
            </div>
          </button>
        {/each}
      </div>
    </div>
  {/if}
</section>

<style>
  /* Date control for the detail chart — same chip + popover as the Overview
     filter bar, so the entity drill-down date selector matches the rest of the app. */
  .cx-detail-daterow { display: flex; align-items: center; margin: 4px 0 2px; }
  .cx-detail-date { position: relative; }
  .cx-date-chip {
    background: var(--bg-card-hover);
    border: 1px solid var(--border);
    color: var(--fg);
    border-radius: 7px;
    padding: 5px 11px;
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-family: inherit;
    font-size: 12px;
    cursor: pointer;
    white-space: nowrap;
    transition: background var(--dur-2), border-color var(--dur-2), color var(--dur-2);
  }
  .cx-date-chip:hover { border-color: var(--border-strong); }
  .cx-date-chip.active { border-color: var(--accent); }
  .cx-date-pop {
    position: absolute;
    top: calc(100% + 6px);
    left: 0;
    z-index: 50;
  }
</style>
