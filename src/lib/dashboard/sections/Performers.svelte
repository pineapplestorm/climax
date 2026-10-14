<!--
  Performers section (Phase 5) — browse every performer you've watched. Search +
  sort, grid of portrait cards, inline detail panel. (Favorite/Ethnicity chips
  from the mock are deferred — that metadata isn't enriched yet.)
-->
<script lang="ts">
  import { w } from "$lib/wording.svelte";
  import { onMount, onDestroy } from "svelte";
  import { api, type BrowsePerformer } from "$lib/api";
  import type { ChipDef } from "$lib/dashboard/browse/BrowseFilterBar.svelte";
  import BrowsePage from "$lib/dashboard/browse/BrowsePage.svelte";
  import PerformerTile from "$lib/dashboard/browse/PerformerTile.svelte";

  let items = $state<BrowsePerformer[]>([]);
  let loading = $state(true);
  let poll: ReturnType<typeof setInterval> | null = null;

  async function load() {
    try {
      items = await api.catalogBrowsePerformers();
    } catch (e) {
      console.error("browse performers load failed", e);
    } finally {
      loading = false;
    }
  }
  onMount(() => {
    load();
    // Background-fill performer images/demographics/favorite from Stash; the
    // poll below picks them up as they land.
    api.catalogEnrichEntities("performer").catch(() => {});
    poll = setInterval(load, 10_000);
  });
  onDestroy(() => {
    if (poll) clearInterval(poll);
  });

  const chipDefs: ChipDef[] = [
    {
      id: "gender",
      label: "Gender",
      icon: "user",
      options: [
        { value: "any", label: "Any" },
        { value: "female", label: "Female" },
        { value: "male", label: "Male" },
      ],
    },
    {
      id: "favorite",
      label: "Favourite",
      icon: "heart",
      options: [
        { value: "any", label: "Any" },
        { value: "fav", label: "Favourites" },
      ],
    },
  ];

  const sortOptions = [
    { value: "cumshots", get label() { return w("Most cumshots"); } },
    { value: "play_count", label: "Play count" },
    { value: "watch_time", label: "Watch time" },
    { value: "recent", label: "Recently watched" },
    { value: "name", label: "Name (A-Z)" },
  ];
  const defaultFilters = { sort: "cumshots", favorite: "any", gender: "any" };

  /** Same gender bucketing as the Trends Top-performers toggle: male side =
   *  Stash MALE / TRANSGENDER_MALE; everyone else (FEMALE, TRANSGENDER_FEMALE,
   *  NON_BINARY, INTERSEX, not-yet-enriched) counts as the female side. */
  function isMaleSide(g: string | null): boolean {
    return g === "MALE" || g === "TRANSGENDER_MALE";
  }

  // Tile trophies: the #1 by cumshots on EACH gender side (the sides rank
  // separately), independent of the current search/filter/sort. EVERY performer
  // tied at the top of their own side gets one (competition ranking - they are
  // all genuinely #1), so there are at least two trophies whenever both sides
  // have a cumshot, and more when a side ties. A side with no cumshots gets
  // none. Tooltips mirror the detail panel's rank pill wording, counting only
  // the performer's own side.
  function topCumshotIds(list: BrowsePerformer[]): Set<string> {
    let best = 0;
    for (const p of list) if (p.cumshots > best) best = p.cumshots;
    return new Set(best > 0 ? list.filter((p) => p.cumshots === best).map((p) => p.id) : []);
  }
  const females = $derived(items.filter((p) => !isMaleSide(p.gender)));
  const males = $derived(items.filter((p) => isMaleSide(p.gender)));
  const topFemaleIds = $derived(topCumshotIds(females));
  const topMaleIds = $derived(topCumshotIds(males));
  const femaleTrophyTitle = $derived(`Ranked #1 of ${females.length} female performers by ${w("cumshots")}`);
  const maleTrophyTitle = $derived(`Ranked #1 of ${males.length} male performers by ${w("cumshots")}`);

  function applyFilters(list: BrowsePerformer[], f: Record<string, string>, q: string): BrowsePerformer[] {
    let r = list;
    if (q) {
      const ql = q.toLowerCase();
      r = r.filter(
        (p) =>
          p.name.toLowerCase().includes(ql) ||
          p.aliases.some((a) => a.toLowerCase().includes(ql)),
      );
    }
    if (f.gender === "female") r = r.filter((p) => !isMaleSide(p.gender));
    else if (f.gender === "male") r = r.filter((p) => isMaleSide(p.gender));
    if (f.favorite === "fav") r = r.filter((p) => p.favorite);
    const cmp: Record<string, (a: BrowsePerformer, b: BrowsePerformer) => number> = {
      recent: (a, b) => (b.last_watched_at ?? 0) - (a.last_watched_at ?? 0),
      watch_time: (a, b) => b.watch_time_ms - a.watch_time_ms,
      play_count: (a, b) => b.play_count - a.play_count,
      cumshots: (a, b) => b.cumshots - a.cumshots,
      name: (a, b) => a.name.localeCompare(b.name),
    };
    return [...r].sort(cmp[f.sort] ?? cmp.cumshots);
  }
</script>

{#snippet tile(item: BrowsePerformer, selected: boolean, onclick: () => void)}
  <PerformerTile
    performer={item}
    {selected}
    {onclick}
    trophy={topFemaleIds.has(item.id) ? femaleTrophyTitle : topMaleIds.has(item.id) ? maleTrophyTitle : null}
  />
{/snippet}

<BrowsePage
  kind="performer"
  {items}
  {loading}
  searchPlaceholder="Search performers..."
  {chipDefs}
  {sortOptions}
  {defaultFilters}
  {applyFilters}
  idOf={(p) => p.id}
  gridClass="cx-grid-performer"
  {tile}
/>
