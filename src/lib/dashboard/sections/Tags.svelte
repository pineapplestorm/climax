<!--
  Tags section (Phase 5) — browse every tag on scenes you've watched. Search +
  sort, grid of square glyph cards (brand-palette hue), inline detail panel.
-->
<script lang="ts">
  import { w } from "$lib/wording.svelte";
  import { onMount, onDestroy } from "svelte";
  import { api, type BrowseTag } from "$lib/api";
  import BrowsePage from "$lib/dashboard/browse/BrowsePage.svelte";
  import TagTile from "$lib/dashboard/browse/TagTile.svelte";

  let items = $state<BrowseTag[]>([]);
  let loading = $state(true);
  let poll: ReturnType<typeof setInterval> | null = null;

  async function load() {
    try {
      items = await api.catalogBrowseTags();
    } catch (e) {
      console.error("browse tags load failed", e);
    } finally {
      loading = false;
    }
  }
  onMount(() => {
    load();
    api.catalogEnrichEntities("tag").catch(() => {});
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

  // Tile trophy: the #1 tag by cumshots, independent of the current
  // search/sort. EVERY tag tied at the top gets one (competition ranking - they
  // are all genuinely #1); empty when nothing has a cumshot. The tooltip mirrors
  // the detail panel's rank pill wording.
  const topCumshotIds = $derived.by(() => {
    let best = 0;
    for (const t of items) if (t.cumshots > best) best = t.cumshots;
    return new Set(best > 0 ? items.filter((t) => t.cumshots === best).map((t) => t.id) : []);
  });
  const trophyTitle = $derived(`Ranked #1 of ${items.length} tags by ${w("cumshots")}`);

  function applyFilters(list: BrowseTag[], f: Record<string, string>, q: string): BrowseTag[] {
    let r = list;
    if (q) {
      const ql = q.toLowerCase();
      r = r.filter((t) => t.name.toLowerCase().includes(ql));
    }
    const cmp: Record<string, (a: BrowseTag, b: BrowseTag) => number> = {
      cumshots: (a, b) => b.cumshots - a.cumshots,
      play_count: (a, b) => b.play_count - a.play_count,
      watch_time: (a, b) => b.watch_time_ms - a.watch_time_ms,
      recent: (a, b) => (b.last_watched_at ?? 0) - (a.last_watched_at ?? 0),
      name: (a, b) => a.name.localeCompare(b.name),
    };
    return [...r].sort(cmp[f.sort] ?? cmp.cumshots);
  }
</script>

{#snippet tile(item: BrowseTag, selected: boolean, onclick: () => void)}
  <TagTile tag={item} {selected} {onclick} trophy={topCumshotIds.has(item.id) ? trophyTitle : null} />
{/snippet}

<BrowsePage
  kind="tag"
  {items}
  {loading}
  searchPlaceholder="Search tags..."
  chipDefs={[]}
  {sortOptions}
  {defaultFilters}
  {applyFilters}
  idOf={(t) => t.id}
  gridClass="cx-grid-tag"
  {tile}
/>
