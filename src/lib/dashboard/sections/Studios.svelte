<!--
  Studios section (Phase 5) — browse every studio you've watched. Search + sort,
  grid of 16:9 logo cards (brand-palette hue), inline detail panel.
-->
<script lang="ts">
  import { w } from "$lib/wording.svelte";
  import { onMount, onDestroy } from "svelte";
  import { api, type BrowseStudio } from "$lib/api";
  import BrowsePage from "$lib/dashboard/browse/BrowsePage.svelte";
  import StudioTile from "$lib/dashboard/browse/StudioTile.svelte";

  let items = $state<BrowseStudio[]>([]);
  let loading = $state(true);
  let poll: ReturnType<typeof setInterval> | null = null;

  async function load() {
    try {
      items = await api.catalogBrowseStudios();
    } catch (e) {
      console.error("browse studios load failed", e);
    } finally {
      loading = false;
    }
  }
  onMount(() => {
    load();
    api.catalogEnrichEntities("studio").catch(() => {});
    poll = setInterval(load, 10_000);
  });
  onDestroy(() => {
    if (poll) clearInterval(poll);
  });

  const sortOptions = [
    { value: "cumshots", get label() { return w("Most cumshots"); } },
    { value: "play_count", label: "Play count" },
    { value: "watch_time", label: "Watch time" },
    { value: "recent", label: "Recently watched" },
    { value: "name", label: "Name (A-Z)" },
  ];
  const defaultFilters = { sort: "cumshots" };

  // Tile trophy: the #1 studio by cumshots, independent of the current
  // search/sort. EVERY studio tied at the top gets one (competition ranking -
  // they are all genuinely #1); empty when nothing has a cumshot. The tooltip
  // mirrors the detail panel's rank pill wording.
  const topCumshotIds = $derived.by(() => {
    let best = 0;
    for (const s of items) if (s.cumshots > best) best = s.cumshots;
    return new Set(best > 0 ? items.filter((s) => s.cumshots === best).map((s) => s.id) : []);
  });
  const trophyTitle = $derived(`Ranked #1 of ${items.length} studios by ${w("cumshots")}`);

  function applyFilters(list: BrowseStudio[], f: Record<string, string>, q: string): BrowseStudio[] {
    let r = list;
    if (q) {
      const ql = q.toLowerCase();
      r = r.filter((s) => s.name.toLowerCase().includes(ql));
    }
    const cmp: Record<string, (a: BrowseStudio, b: BrowseStudio) => number> = {
      watch_time: (a, b) => b.watch_time_ms - a.watch_time_ms,
      play_count: (a, b) => b.play_count - a.play_count,
      recent: (a, b) => (b.last_watched_at ?? 0) - (a.last_watched_at ?? 0),
      cumshots: (a, b) => b.cumshots - a.cumshots,
      name: (a, b) => a.name.localeCompare(b.name),
    };
    return [...r].sort(cmp[f.sort] ?? cmp.cumshots);
  }
</script>

{#snippet tile(item: BrowseStudio, selected: boolean, onclick: () => void)}
  <StudioTile studio={item} {selected} {onclick} trophy={topCumshotIds.has(item.id) ? trophyTitle : null} />
{/snippet}

<BrowsePage
  kind="studio"
  {items}
  {loading}
  searchPlaceholder="Search studios..."
  chipDefs={[]}
  {sortOptions}
  {defaultFilters}
  {applyFilters}
  idOf={(s) => s.id}
  gridClass="cx-grid-studio"
  {tile}
/>
