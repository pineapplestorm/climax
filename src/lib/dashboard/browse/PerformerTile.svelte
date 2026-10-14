<!-- Performer grid tile — 3:4 portrait card with initials placeholder.
     Cumshots render in the stat line exactly like the scene tiles (coral
     droplet + weight-600 count + the word, first segment, "·" dividers) - NOT
     a thumbnail pill. The row is JUSTIFIED (space-between) so the three stats
     fill the card width like the scene row fills its wider card naturally.

     FIT LADDER (measured, not guessed - scrollWidth vs clientWidth, re-checked
     on every card resize). Degradation order per the user's spec:
       1. Hard format rule, always on: at 100h+ the watch time drops its
          minutes ("347h"), so the longest possible string is "99h 59m".
       2. Stat row overflows -> drop the minutes, floored ("5h 6m" -> "5h";
          sub-hour times keep their "45m" - there's no hour to floor to).
       3. Row STILL overflows (extreme aggregates: "99 cumshots · 999 plays ·
          99h" needs ~202px vs ~196 available at the 200px card minimum) ->
          the word "cumshots" collapses to icon + count. Last resort only;
          typical tiles keep the full word.
       4. Name overflows its single line -> a 3+ word name loses its SECOND
          word (any second word, not just entity middle names); still too
          long -> the CSS ellipsis takes it. Hover shows the full name.
     Names are one line here (cx-tile-title-one), never wrapping, so the
     portrait rows stay aligned. -->
<script lang="ts">
  import { w, wording } from "$lib/wording.svelte";
  import Icon from "$lib/Icon.svelte";
  import { type BrowsePerformer } from "$lib/api";
  import { watchLabel, withoutSecondWord, runFitLadder } from "./tile-fit";

  type Props = {
    performer: BrowsePerformer;
    selected: boolean;
    onclick: () => void;
    /** #1 by cumshots within this performer's gender side - gold trophy badge.
     *  The string is the tooltip (same wording as the detail panel's rank
     *  pill); null = no trophy. */
    trophy?: string | null;
  };
  let { performer, selected, onclick, trophy = null }: Props = $props();

  const initials = $derived(
    performer.name
      .split(" ")
      .map((s) => s[0])
      .join("")
      .slice(0, 2)
      .toUpperCase(),
  );

  // ---- fit ladder ----
  let rowEl = $state<HTMLElement | null>(null);
  let titleEl = $state<HTMLElement | null>(null);
  let dropMinutes = $state(false);
  let dropCumshotWord = $state(false);
  let dropSecondWord = $state(false);
  // Guards overlapping refits (each awaits two DOM flushes; a resize mid-flight
  // starts a fresh pass and the stale one must not write its conclusions).
  let refitSeq = 0;

  const watch = $derived(watchLabel(performer.watch_time_ms, dropMinutes));
  const displayName = $derived(dropSecondWord ? withoutSecondWord(performer.name) : performer.name);

  // Rungs (cheapest loss first); runFitLadder drives the measure/degrade cycle.
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
      // Independent of the stat row: a name too long for its single line drops
      // its second word before the CSS ellipsis takes over.
      after: () => {
        if (titleEl && titleEl.scrollWidth > titleEl.clientWidth) dropSecondWord = true;
      },
    });
  }

  $effect(() => {
    const el = rowEl;
    // Data deps: a tile re-used with different stats must re-measure.
    void performer.name;
    void performer.watch_time_ms;
    void performer.play_count;
    void performer.cumshots;
    // The word itself changes width when the Wording setting flips.
    void wording.value;
    if (!el) return;
    const ro = new ResizeObserver(() => void refit());
    ro.observe(el);
    void refit();
    return () => ro.disconnect();
  });
</script>

<button class="cx-tile cx-tile-performer" class:is-selected={selected} {onclick}>
  <div class="cx-tile-thumb cx-thumb-3x4">
    {#if performer.image_url}
      <img class="cx-thumb-img" src={performer.image_url} alt="" />
    {:else}
      <span class="cx-thumb-ph cx-thumb-ph-large" aria-hidden="true">{initials}</span>
    {/if}
    {#if performer.favorite}
      <span class="cx-thumb-fav" title="favourite"><Icon name="heart" size={12} color="var(--accent)" filled /></span>
    {/if}
    {#if trophy}
      <span class="cx-thumb-trophy" title={trophy}><Icon name="trophy" size={12} color="var(--ink-950)" /></span>
    {/if}
  </div>
  <div class="cx-tile-body">
    <div class="cx-tile-title cx-tile-title-one" bind:this={titleEl} title={performer.name}>{displayName}</div>
    <div class="cx-tile-stats cx-tile-stats-fill" bind:this={rowEl}>
      {#if performer.cumshots > 0}
        <span class="cx-tile-cumshots" title={w(`${performer.cumshots} cumshot${performer.cumshots === 1 ? "" : "s"}`)}>
          <Icon name="cumshot" size={14} color="var(--accent)" filled />
          {#if dropCumshotWord}
            <span class="cx-tile-cumshots-n">{performer.cumshots}</span>
          {:else}
            <span><span class="cx-tile-cumshots-n">{performer.cumshots}</span> {w(performer.cumshots === 1 ? "cumshot" : "cumshots")}</span>
          {/if}
        </span>
        <span class="cx-tile-sep">·</span>
      {/if}
      <span>{performer.play_count} play{performer.play_count === 1 ? "" : "s"}</span>
      <span class="cx-tile-sep">·</span>
      <span>{watch}</span>
    </div>
  </div>
</button>
