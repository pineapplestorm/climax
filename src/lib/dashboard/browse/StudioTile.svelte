<!-- Studio grid tile — 16:9 card; the studio's hue radiates from the corner.
     Cumshots render in the stat line exactly like the scene/performer tiles
     (coral droplet + weight-600 count + the word, first segment, "·"
     dividers) - NOT a thumbnail pill - and the row is JUSTIFIED
     (space-between) so it fills the card width.

     Same measured FIT LADDER as PerformerTile (see its header for the full
     order), with one studio twist: the parent-studio segment is the row's
     only shrinkable child (.cx-truncate), so instead of the row overflowing
     it ellipsizes the parent - the ladder watches for that squeeze and
     shaves stat detail (minutes, then the word "cumshots") to give the
     parent its room back before letting the ellipsis stand. -->
<script lang="ts">
  import { w, wording } from "$lib/wording.svelte";
  import Icon from "$lib/Icon.svelte";
  import { type BrowseStudio } from "$lib/api";
  import { colorForKey } from "./palette";
  import { watchLabel, withoutSecondWord, runFitLadder } from "./tile-fit";

  type Props = {
    studio: BrowseStudio;
    selected: boolean;
    onclick: () => void;
    /** #1 studio by cumshots - gold trophy badge. The string is the tooltip
     *  (same wording as the detail panel's rank pill); null = no trophy. */
    trophy?: string | null;
  };
  let { studio, selected, onclick, trophy = null }: Props = $props();

  const color = $derived(colorForKey(studio.id));
  const initial = $derived(studio.name.trim()[0]?.toUpperCase() ?? "?");

  // ---- fit ladder (see PerformerTile) ----
  let rowEl = $state<HTMLElement | null>(null);
  let titleEl = $state<HTMLElement | null>(null);
  let parentEl = $state<HTMLElement | null>(null);
  let dropMinutes = $state(false);
  let dropCumshotWord = $state(false);
  let dropSecondWord = $state(false);
  let refitSeq = 0;

  const watch = $derived(watchLabel(studio.watch_time_ms, dropMinutes));
  const displayName = $derived(dropSecondWord ? withoutSecondWord(studio.name) : studio.name);

  // Overflow here means EITHER the rigid row overflows (no parent segment) OR
  // the shrinkable parent segment is being ellipsized (it absorbs overflow
  // before the row reports any).
  function squeezed(): boolean {
    if (rowEl && rowEl.scrollWidth > rowEl.clientWidth) return true;
    if (parentEl && parentEl.scrollWidth > parentEl.clientWidth) return true;
    return false;
  }

  async function refit() {
    const seq = ++refitSeq;
    await runFitLadder({
      current: () => seq === refitSeq,
      reset: () => {
        dropMinutes = false;
        dropCumshotWord = false;
        dropSecondWord = false;
      },
      squeezed,
      steps: [() => (dropMinutes = true), () => (dropCumshotWord = true)],
      after: () => {
        if (titleEl && titleEl.scrollWidth > titleEl.clientWidth) dropSecondWord = true;
      },
    });
  }

  $effect(() => {
    const el = rowEl;
    void studio.name;
    void studio.parent_studio;
    void studio.watch_time_ms;
    void studio.play_count;
    void studio.cumshots;
    // The word itself changes width when the Wording setting flips.
    void wording.value;
    if (!el) return;
    const ro = new ResizeObserver(() => void refit());
    ro.observe(el);
    void refit();
    return () => ro.disconnect();
  });
</script>

<button class="cx-tile cx-tile-studio" class:is-selected={selected} {onclick}>
  <div class="cx-tile-thumb cx-thumb-16x9 cx-studio-thumb" style={`--studio-color: ${color};`}>
    {#if studio.image_url}
      <img class="cx-thumb-img cx-contain-img" src={studio.image_url} alt="" />
    {:else}
      <span class="cx-studio-initial">{initial}</span>
    {/if}
    {#if trophy}
      <span class="cx-thumb-trophy" title={trophy}><Icon name="trophy" size={12} color="var(--ink-950)" /></span>
    {/if}
  </div>
  <div class="cx-tile-body">
    <div class="cx-tile-title cx-tile-title-one" bind:this={titleEl} title={studio.name}>{displayName}</div>
    <div class="cx-tile-stats cx-tile-stats-fill" bind:this={rowEl}>
      {#if studio.cumshots > 0}
        <span class="cx-tile-cumshots" title={w(`${studio.cumshots} cumshot${studio.cumshots === 1 ? "" : "s"}`)}>
          <Icon name="cumshot" size={14} color="var(--accent)" filled />
          {#if dropCumshotWord}
            <span class="cx-tile-cumshots-n">{studio.cumshots}</span>
          {:else}
            <span><span class="cx-tile-cumshots-n">{studio.cumshots}</span> {w(studio.cumshots === 1 ? "cumshot" : "cumshots")}</span>
          {/if}
        </span>
        <span class="cx-tile-sep">·</span>
      {/if}
      {#if studio.parent_studio}
        <span class="cx-tile-studio cx-truncate" bind:this={parentEl} title={studio.parent_studio}>{studio.parent_studio}</span>
        <span class="cx-tile-sep">·</span>
      {/if}
      <span>{studio.play_count} play{studio.play_count === 1 ? "" : "s"}</span>
      <span class="cx-tile-sep">·</span>
      <span>{watch}</span>
    </div>
  </div>
</button>
