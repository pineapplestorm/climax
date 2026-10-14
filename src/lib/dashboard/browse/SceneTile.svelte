<!-- Scene grid tile — 16:9 thumbnail (real Stash screenshot when enriched).
     The whole tile drills into the scene; the studio chip + performer pills are
     nested links to those entities (so the root is a role="button" div, not a
     <button>, to keep nested buttons valid).

     FIT LADDER (measured - scrollWidth vs clientWidth, re-checked on resize),
     the same machinery as PerformerTile but tuned for this row, which is the
     app's most crowded: FOUR segments, and two filler words the other tiles
     don't carry ("watched", "ago"). Degradation order, cheapest first:
       1. Hard format rule, always on: 100h+ drops minutes ("347h"), so the
          longest watch string is "99h 59m" (shared watchLabel).
       2. Row overflows -> drop "ago" ("28d ago" -> "28d"). Cheapest possible
          loss: it's the last segment and the d/h/w unit still reads as elapsed.
       3. Drop "watched" ("7m watched" -> "7m") - matches the other three tiles,
          which already show watch time bare.
       4. Drop the watch minutes, floored ("5h 6m" -> "5h").
       5. Collapse "cumshots" to icon + count. Last resort.
     Below the ladder the ago segment is .cx-truncate, so anything that still
     doesn't fit ellipsises ("28d...") instead of being sliced mid-word by the
     row's overflow:hidden - which is what produced "24d ag".
     Because that segment CAN shrink, it absorbs overflow before the row reports
     any, so `squeezed()` watches it directly (same trick as StudioTile's parent
     studio). -->
<script lang="ts">
  import { w, wording } from "$lib/wording.svelte";
  import Icon from "$lib/Icon.svelte";
  import { formatDurationShort, formatAgo, type BrowseScene, type NamedEntity } from "$lib/api";
  import { watchLabel, runFitLadder } from "./tile-fit";

  type Props = {
    scene: BrowseScene;
    selected: boolean;
    onclick: () => void;
    /** #1 scene by cumshots across the whole catalogue - gold trophy badge.
     *  The string is the tooltip (same wording as the detail panel's rank
     *  pill); null = no trophy. */
    trophy?: string | null;
    onStudioSelect?: (studio: NamedEntity) => void;
    onPerformerSelect?: (performer: NamedEntity) => void;
  };
  let { scene, selected, onclick, trophy = null, onStudioSelect, onPerformerSelect }: Props = $props();

  // Match SceneCard / SessionDetail: up to 2 performer pills + a "+N" overflow.
  const MAX_VISIBLE_PERFORMERS = 2;
  const visiblePerformers = $derived(scene.performers.slice(0, MAX_VISIBLE_PERFORMERS));
  const overflowPerformers = $derived(scene.performers.slice(MAX_VISIBLE_PERFORMERS));
  const initial = $derived((scene.title ?? "?").trim()[0]?.toUpperCase() ?? "?");

  // ---- fit ladder (see the header comment) ----
  let rowEl = $state<HTMLElement | null>(null);
  let agoEl = $state<HTMLElement | null>(null);
  let dropAgoWord = $state(false);
  let dropWatchedWord = $state(false);
  let dropMinutes = $state(false);
  let dropCumshotWord = $state(false);
  // Guards overlapping refits (each awaits DOM flushes; a resize mid-flight
  // starts a fresh pass and the stale one must not write its conclusions).
  let refitSeq = 0;

  const watch = $derived(watchLabel(scene.watch_time_ms, dropMinutes));
  // "- watched" reads like a bug, so an unwatched scene never takes the word.
  const watchText = $derived(
    watch === "-" || dropWatchedWord ? watch : `${watch} watched`,
  );
  const agoText = $derived.by(() => {
    const a = formatAgo(scene.last_watched_at);
    return dropAgoWord ? a.replace(/\s+ago$/, "") : a;
  });

  // Overflow means EITHER the rigid row overflows OR the shrinkable ago segment
  // is being ellipsised (it absorbs overflow before the row reports any).
  function squeezed(): boolean {
    if (rowEl && rowEl.scrollWidth > rowEl.clientWidth) return true;
    if (agoEl && agoEl.scrollWidth > agoEl.clientWidth) return true;
    return false;
  }

  // Rungs (cheapest loss first); runFitLadder drives the measure/degrade cycle.
  // No title check here - scene titles wrap to two lines rather than degrade.
  async function refit() {
    const seq = ++refitSeq;
    await runFitLadder({
      current: () => seq === refitSeq,
      reset: () => {
        dropAgoWord = false;
        dropWatchedWord = false;
        dropMinutes = false;
        dropCumshotWord = false;
      },
      squeezed,
      steps: [
        () => (dropAgoWord = true),
        () => (dropWatchedWord = true),
        () => (dropMinutes = true),
        () => (dropCumshotWord = true),
      ],
    });
  }

  $effect(() => {
    const el = rowEl;
    // Data deps: a tile re-used with different stats must re-measure.
    void scene.cumshots;
    // The word itself changes width when the Wording setting flips.
    void wording.value;
    void scene.watch_time_ms;
    void scene.play_count;
    void scene.last_watched_at;
    if (!el) return;
    const ro = new ResizeObserver(() => void refit());
    ro.observe(el);
    void refit();
    return () => ro.disconnect();
  });
</script>

<div
  class="cx-tile cx-tile-scene"
  class:is-selected={selected}
  role="button"
  tabindex="0"
  {onclick}
  onkeydown={(e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); onclick(); } }}
>
  <div class="cx-tile-thumb cx-thumb-16x9" class:has-trophy={trophy}>
    {#if scene.thumbnail_url}
      <img class="cx-thumb-img" src={scene.thumbnail_url} alt="" />
    {:else}
      <span class="cx-thumb-ph" aria-hidden="true">{initial}</span>
    {/if}
    {#if trophy}
      <span class="cx-thumb-trophy" title={trophy}><Icon name="trophy" size={12} color="var(--ink-950)" /></span>
    {/if}
    {#if scene.duration_seconds}
      <span class="cx-thumb-duration">{formatDurationShort(scene.duration_seconds)}</span>
    {/if}
    {#if scene.resolution}
      <span class="cx-thumb-res">{scene.resolution}</span>
    {/if}
    {#if scene.studio}
      <button
        class="cx-thumb-studio"
        title={`Studio: ${scene.studio.name}`}
        onclick={(e) => { e.stopPropagation(); if (scene.studio) onStudioSelect?.(scene.studio); }}
      >
        <Icon name="clapperboard" size={10} />
        <span>{scene.studio.name}</span>
      </button>
    {/if}
  </div>
  <div class="cx-tile-body">
    <div class="cx-tile-title">{scene.title ?? `Scene ${scene.external_id ?? "?"}`}</div>
    <div class="cx-tile-stats" bind:this={rowEl}>
      {#if scene.cumshots > 0}
        <span class="cx-tile-cumshots" title={w(`${scene.cumshots} cumshot${scene.cumshots === 1 ? "" : "s"}`)}>
          <Icon name="cumshot" size={14} color="var(--accent)" filled />
          {#if dropCumshotWord}
            <span class="cx-tile-cumshots-n">{scene.cumshots}</span>
          {:else}
            <span><span class="cx-tile-cumshots-n">{scene.cumshots}</span> {w(scene.cumshots === 1 ? "cumshot" : "cumshots")}</span>
          {/if}
        </span>
        <span class="cx-tile-sep">·</span>
      {/if}
      <span>{watchText}</span>
      <span class="cx-tile-sep">·</span>
      <span>{scene.play_count} play{scene.play_count === 1 ? "" : "s"}</span>
      <span class="cx-tile-sep">·</span>
      <span class="cx-tile-ago cx-truncate" bind:this={agoEl} title={formatAgo(scene.last_watched_at)}>{agoText}</span>
    </div>
    {#if visiblePerformers.length > 0}
      <div class="cx-tile-pills">
        {#each visiblePerformers as p (p.id)}
          <button
            class="cx-pill cx-pill-performer"
            title={p.name}
            onclick={(e) => { e.stopPropagation(); onPerformerSelect?.(p); }}
          >{p.name}</button>
        {/each}
        {#if overflowPerformers.length > 0}
          <span class="cx-pill cx-pill-more" title={overflowPerformers.map((p) => p.name).join(", ")}>+{overflowPerformers.length}</span>
        {/if}
      </div>
    {/if}
  </div>
</div>
