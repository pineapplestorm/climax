<!--
  Scenes section (Phase 5) — browse every watched scene. Search + Studio/Performer
  filter chips + sort, grid of 16:9 tiles, inline detail panel. Data + in-memory
  filtering wired through the shared BrowsePage chassis.
-->
<script lang="ts">
  import { w } from "$lib/wording.svelte";
  import { onMount, onDestroy } from "svelte";
  import { api, type BrowseScene } from "$lib/api";
  import type { ChipDef } from "$lib/dashboard/browse/BrowseFilterBar.svelte";
  import BrowsePage from "$lib/dashboard/browse/BrowsePage.svelte";
  import SceneTile from "$lib/dashboard/browse/SceneTile.svelte";
  import { nav } from "$lib/dashboard/nav.svelte";

  // Origin for cross-nav from a tile's studio/performer chip: the scenes grid
  // (no detail open), so Back returns here.
  const gridOrigin = () => ({ section: "scenes" as const, entityKind: "scene" as const, entityId: null });

  let items = $state<BrowseScene[]>([]);
  let loading = $state(true);
  let poll: ReturnType<typeof setInterval> | null = null;

  async function load() {
    try {
      items = await api.catalogBrowseScenes();
    } catch (e) {
      console.error("browse scenes load failed", e);
    } finally {
      loading = false;
    }
  }
  onMount(() => {
    load();
    poll = setInterval(load, 10_000);
  });
  onDestroy(() => {
    if (poll) clearInterval(poll);
  });

  // Filter chip options derived from the loaded catalog (only studios/performers
  // that actually appear). Matched by id; labelled by name.
  function dedupe(pairs: { id: string; name: string }[]) {
    const seen = new Map<string, string>();
    for (const p of pairs) if (!seen.has(p.id)) seen.set(p.id, p.name);
    return [...seen.entries()]
      .map(([id, name]) => ({ value: id, label: name }))
      .sort((a, b) => a.label.localeCompare(b.label));
  }
  const chipDefs = $derived.by<ChipDef[]>(() => [
    {
      id: "studio",
      label: "Studio",
      icon: "video",
      options: [
        { value: "any", label: "Any" },
        ...dedupe(items.filter((s) => s.studio).map((s) => s.studio!)),
      ],
    },
    {
      id: "performer",
      label: "Performer",
      icon: "user",
      options: [
        { value: "any", label: "Any" },
        ...dedupe(items.flatMap((s) => s.performers)),
      ],
    },
  ]);

  const sortOptions = [
    { value: "cumshots", get label() { return w("Most cumshots"); } },
    { value: "play_count", label: "Play count" },
    { value: "watch_time", label: "Watch time" },
    { value: "recent", label: "Recently watched" },
    { value: "title", label: "Title (A-Z)" },
  ];
  const defaultFilters = { sort: "cumshots", studio: "any", performer: "any" };

  // Tile trophy: the #1 scene by cumshots across the whole catalogue,
  // independent of the current search/filter/sort. EVERY scene tied at the top
  // gets one (competition ranking - they are all genuinely #1, so picking one
  // arbitrarily would be a lie); empty when nothing has a cumshot. The tooltip
  // mirrors the detail panel's rank pill wording.
  const topCumshotIds = $derived.by(() => {
    let best = 0;
    for (const s of items) if (s.cumshots > best) best = s.cumshots;
    return new Set(best > 0 ? items.filter((s) => s.cumshots === best).map((s) => s.id) : []);
  });
  const trophyTitle = $derived(`Ranked #1 of ${items.length} scenes by ${w("cumshots")}`);

  function applyFilters(list: BrowseScene[], f: Record<string, string>, q: string): BrowseScene[] {
    let r = list;
    if (q) {
      const ql = q.toLowerCase();
      r = r.filter(
        (s) =>
          (s.title ?? "").toLowerCase().includes(ql) ||
          (s.studio?.name ?? "").toLowerCase().includes(ql) ||
          s.performers.some((p) => p.name.toLowerCase().includes(ql)),
      );
    }
    if (f.studio !== "any") r = r.filter((s) => s.studio?.id === f.studio);
    if (f.performer !== "any") r = r.filter((s) => s.performers.some((p) => p.id === f.performer));
    const cmp: Record<string, (a: BrowseScene, b: BrowseScene) => number> = {
      recent: (a, b) => (b.last_watched_at ?? 0) - (a.last_watched_at ?? 0),
      watch_time: (a, b) => b.watch_time_ms - a.watch_time_ms,
      play_count: (a, b) => b.play_count - a.play_count,
      cumshots: (a, b) => b.cumshots - a.cumshots,
      title: (a, b) => (a.title ?? "").localeCompare(b.title ?? ""),
    };
    return [...r].sort(cmp[f.sort] ?? cmp.cumshots);
  }
</script>

{#snippet tile(item: BrowseScene, selected: boolean, onclick: () => void)}
  <SceneTile
    scene={item}
    {selected}
    {onclick}
    trophy={topCumshotIds.has(item.id) ? trophyTitle : null}
    onStudioSelect={(s) => nav.goto("studio", s.id, gridOrigin())}
    onPerformerSelect={(p) => nav.goto("performer", p.id, gridOrigin())}
  />
{/snippet}

<BrowsePage
  kind="scene"
  {items}
  {loading}
  searchPlaceholder="Search scenes, performers, studios..."
  {chipDefs}
  {sortOptions}
  {defaultFilters}
  {applyFilters}
  idOf={(s) => s.id}
  gridClass="cx-grid-scene"
  {tile}
/>
