<!-- Tag grid tile — square card with a colored tag glyph.
     Cumshots render in the stat line exactly like the scene/performer tiles
     (coral droplet + weight-600 count + the word, first segment, "·"
     dividers) - NOT a thumbnail pill - and the row is JUSTIFIED
     (space-between) so it fills the card width. Same measured FIT LADDER as
     PerformerTile (see its header comment for the full degradation order). -->
<script lang="ts">
  import { w, wording } from "$lib/wording.svelte";
  import Icon from "$lib/Icon.svelte";
  import { type BrowseTag } from "$lib/api";
  import { colorForKey } from "./palette";
  import { watchLabel, withoutSecondWord, runFitLadder } from "./tile-fit";

  type Props = {
    tag: BrowseTag;
    selected: boolean;
    onclick: () => void;
    /** #1 tag by cumshots - gold trophy badge. The string is the tooltip
     *  (same wording as the detail panel's rank pill); null = no trophy. */
    trophy?: string | null;
  };
  let { tag, selected, onclick, trophy = null }: Props = $props();

  const color = $derived(colorForKey(tag.id));

  // ---- fit ladder (see PerformerTile) ----
  let rowEl = $state<HTMLElement | null>(null);
  let titleEl = $state<HTMLElement | null>(null);
  let dropMinutes = $state(false);
  let dropCumshotWord = $state(false);
  let dropSecondWord = $state(false);
  let refitSeq = 0;

  const watch = $derived(watchLabel(tag.watch_time_ms, dropMinutes));
  const displayName = $derived(dropSecondWord ? withoutSecondWord(tag.name) : tag.name);

  async function refit() {
    const seq = ++refitSeq;
    await runFitLadder({
      current: () => seq === refitSeq,
      reset: () => {
        dropMinutes = false;
        dropCumshotWord = false;
        dropSecondWord = false;
      },
      squeezed: () => !!rowEl && rowEl.scrollWidth > rowEl.clientWidth,
      steps: [() => (dropMinutes = true), () => (dropCumshotWord = true)],
      after: () => {
        if (titleEl && titleEl.scrollWidth > titleEl.clientWidth) dropSecondWord = true;
      },
    });
  }

  $effect(() => {
    const el = rowEl;
    void tag.name;
    void tag.watch_time_ms;
    void tag.play_count;
    void tag.cumshots;
    // The word itself changes width when the Wording setting flips.
    void wording.value;
    if (!el) return;
    const ro = new ResizeObserver(() => void refit());
    ro.observe(el);
    void refit();
    return () => ro.disconnect();
  });
</script>

<button class="cx-tile cx-tile-tag" class:is-selected={selected} {onclick} style={`--tag-color: ${color};`}>
  <div class="cx-tile-thumb cx-thumb-1x1 cx-tag-thumb">
    {#if tag.image_url}
      <img class="cx-thumb-img cx-contain-img" src={tag.image_url} alt="" />
    {:else}
      <Icon name="tag" size={46} color="var(--tag-color)" />
    {/if}
    {#if trophy}
      <span class="cx-thumb-trophy" title={trophy}><Icon name="trophy" size={12} color="var(--ink-950)" /></span>
    {/if}
  </div>
  <div class="cx-tile-body">
    <div class="cx-tile-title cx-tile-title-one" bind:this={titleEl} title={tag.name}>{displayName}</div>
    <div class="cx-tile-stats cx-tile-stats-fill" bind:this={rowEl}>
      {#if tag.cumshots > 0}
        <span class="cx-tile-cumshots" title={w(`${tag.cumshots} cumshot${tag.cumshots === 1 ? "" : "s"}`)}>
          <Icon name="cumshot" size={14} color="var(--accent)" filled />
          {#if dropCumshotWord}
            <span class="cx-tile-cumshots-n">{tag.cumshots}</span>
          {:else}
            <span><span class="cx-tile-cumshots-n">{tag.cumshots}</span> {w(tag.cumshots === 1 ? "cumshot" : "cumshots")}</span>
          {/if}
        </span>
        <span class="cx-tile-sep">·</span>
      {/if}
      <span>{tag.play_count} play{tag.play_count === 1 ? "" : "s"}</span>
      <span class="cx-tile-sep">·</span>
      <span>{watch}</span>
    </div>
  </div>
</button>
