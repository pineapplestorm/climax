<!--
  Compact scene card used in the dashboard day view + tracker scene rows.
  Thumbnail + title + studio + performers + watched time + cumshot badge.

  When metadata hasn't been enriched yet (just created), shows placeholders;
  the dashboard polls every 2s so this auto-fills in.
-->
<script lang="ts">
  import { w } from "$lib/wording.svelte";
  import { formatDuration, parseSceneMetadata, type PlayedScene } from "$lib/api";
  import { stashConfig } from "$lib/stash-config.svelte";
  import Icon from "$lib/Icon.svelte";

  // Local mirrors of the entity shapes the metadata JSON carries. Matches
  // SessionDetail's inline types so the pill click contract is identical.
  type Performer = { id: string; name: string };
  type Studio = { id: string; name: string };

  type Props = {
    scene: PlayedScene;
    pendingCumshots?: number;
    compact?: boolean;
    /** Phase 5 wiring point — clicking a performer pill. No-op fallback for now;
     *  Performers sidebar section will pass a real handler. */
    onPerformerSelect?: (performer: Performer) => void;
    /** Phase 5 wiring point — clicking the studio pill. Same as above. */
    onStudioSelect?: (studio: Studio) => void;
    /** Clicking the thumbnail or title opens the scene's page (same contract as
     *  SessionDetail's scene cards). Omitted = thumb/title aren't interactive. */
    onSelect?: () => void;
  };

  let {
    scene,
    pendingCumshots = 0,
    compact = false,
    onPerformerSelect,
    onStudioSelect,
    onSelect,
  }: Props = $props();

  const metadata = $derived(parseSceneMetadata(scene.metadata_json));
  const totalCumshots = $derived(scene.cumshot_count + pendingCumshots);

  // Match SessionDetail's threshold so the pill rows behave the same way.
  const MAX_VISIBLE_PERFORMERS = 2;
  const performers = $derived(metadata?.performers ?? []);
  const visiblePerformers = $derived(performers.slice(0, MAX_VISIBLE_PERFORMERS));
  const overflowPerformers = $derived(performers.slice(MAX_VISIBLE_PERFORMERS));
  const studio = $derived(metadata?.studio ?? null);

  // "Open in Stash" link. For Stash scenes, build it live from the configured
  // base URL + external_id so every Stash scene gets one (and it tracks the
  // current Stash URL). Non-Stash scenes fall back to any stored `url`.
  const openHref = $derived.by(() => {
    if (scene.source_key === "stash" && scene.external_id && stashConfig.baseUrl) {
      return `${stashConfig.baseUrl}/scenes/${scene.external_id}`;
    }
    return scene.url ?? null;
  });
  const openTitle = $derived(scene.source_key === "stash" ? "Open in Stash" : "Open link");

  function selectPerformer(p: Performer) {
    if (onPerformerSelect) onPerformerSelect(p);
  }
  function selectStudio(s: Studio) {
    if (onStudioSelect) onStudioSelect(s);
  }
</script>

{#snippet thumbInner()}
  {#if scene.thumbnail_url}
    <img src={scene.thumbnail_url} alt="" />
  {:else}
    <div class="thumb-placeholder">{(scene.source_key || "?")[0].toUpperCase()}</div>
  {/if}
{/snippet}

<div class="scene-card" class:compact>
  <div class="thumb">
    {#if onSelect}
      <button class="thumb-link" onclick={onSelect} title="Open this scene in Climax" aria-label="Open this scene in Climax">
        {@render thumbInner()}
      </button>
    {:else}
      {@render thumbInner()}
    {/if}
    {#if totalCumshots > 0}
      <span class="thumb-badge" title={w("Cumshots logged on this scene in this session")}>
        <Icon name="cumshot" size={11} filled />
        <span>{totalCumshots}</span>
      </span>
    {/if}
  </div>
  <div class="info">
    <div class="title-row">
      {#if onSelect}
        <button class="title title-link" onclick={onSelect} title="Open this scene in Climax">
          {scene.title ?? `Scene ${scene.external_id ?? "?"}`}
        </button>
      {:else}
        <span class="title">{scene.title ?? `Scene ${scene.external_id ?? "?"}`}</span>
      {/if}
      {#if openHref}
        <a href={openHref} target="_blank" rel="noopener" class="open-link" title={openTitle}>
          <Icon name="external-link" size={12} />
        </a>
      {/if}
    </div>
    {#if studio}
      <div class="card-pills studio-row">
        <button
          class="pill studio-pill"
          onclick={() => selectStudio(studio)}
          title={`Studio: ${studio.name}`}
        >
          <Icon name="clapperboard" size={10} />
          <span>{studio.name}</span>
        </button>
      </div>
    {/if}
    {#if visiblePerformers.length > 0}
      <div class="card-pills">
        {#each visiblePerformers as p (p.id)}
          <button
            class="pill performer-pill"
            onclick={() => selectPerformer(p)}
            title={p.name}
          >{p.name}</button>
        {/each}
        {#if overflowPerformers.length > 0}
          <button
            class="pill overflow-pill"
            title={`${overflowPerformers.length} more performer${overflowPerformers.length === 1 ? "" : "s"}: ${overflowPerformers.map((p) => p.name).join(", ")}`}
            aria-label="More performers"
          >+{overflowPerformers.length}</button>
        {/if}
      </div>
    {/if}
    <div class="bottom">
      <span class="watched">{formatDuration(scene.seconds_tracked * 1000)} watched</span>
    </div>
  </div>
</div>

<style>
  .scene-card {
    display: flex;
    gap: 12px;
    align-items: center;            /* keep thumb visually centred when info column is taller */
    padding: 8px;
    border-radius: 8px;
    transition: background var(--dur-2, 120ms);
    background: transparent;
    min-width: 0;
  }
  .scene-card.compact {
    padding: 4px 6px;
  }

  /* Explicit width × height (16:9) — matches SessionDetail's thumb. Using
     aspect-ratio alone breaks when the flex parent's cross-axis stretches. */
  .thumb {
    flex-shrink: 0;
    width: 160px;
    height: 90px;
    background: linear-gradient(135deg, #1d1218 0%, #14101a 100%);
    border: 1px solid var(--border);
    border-radius: 6px;
    overflow: hidden;
    position: relative;
    display: flex;
    align-items: center;
    justify-content: center;
  }
  .scene-card.compact .thumb {
    width: 120px;
    height: 68px;                   /* 16:9 */
  }
  /* Clickable thumbnail (when onSelect is wired) — a bare button that fills
     the thumb box and centres the placeholder, mirroring SessionDetail. */
  .thumb-link {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 100%;
    height: 100%;
    padding: 0;
    border: none;
    background: transparent;
    cursor: pointer;
    transition: filter var(--dur-2, 120ms);
  }
  .thumb-link:hover { filter: brightness(1.12); }
  .thumb img {
    width: 100%;
    height: 100%;
    object-fit: cover;
    display: block;
  }
  .thumb-placeholder {
    font-family: var(--font-mono);
    font-size: 24px;
    font-weight: 700;
    color: rgba(239, 107, 122, 0.20);
  }
  .scene-card.compact .thumb-placeholder { font-size: 18px; }

  .thumb-badge {
    position: absolute;
    bottom: 4px;
    right: 4px;
    background: rgba(239, 107, 122, 0.92);
    color: white;
    padding: 2px 7px;
    border-radius: 12px;
    font-size: 10px;
    font-weight: 700;
    font-variant-numeric: tabular-nums;
    font-family: var(--font-mono);
    line-height: 1.4;
    box-shadow: var(--shadow-sm, 0 1px 2px rgba(0, 0, 0, 0.30));
    display: inline-flex;
    align-items: center;
    gap: 3px;
  }

  .info {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 3px;
    justify-content: center;
  }
  .title-row {
    display: flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
  }
  .title {
    font-size: 13px;
    font-weight: 600;
    color: var(--fg);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    flex: 1;
    min-width: 0;
  }
  /* Clickable title (when onSelect is wired) — a button reset to read exactly
     like the .title span, with a coral hover for affordance. */
  .title-link {
    font-family: inherit;
    border: none;
    background: transparent;
    padding: 0;
    margin: 0;
    text-align: left;
    cursor: pointer;
    transition: color var(--dur-2, 120ms);
  }
  .title-link:hover { color: var(--accent); }
  .open-link {
    color: var(--fg-muted);
    text-decoration: none;
    flex-shrink: 0;
    padding: 0 2px;
    transition: color var(--dur-2, 120ms);
    display: inline-flex;
    align-items: center;
  }
  .open-link:hover { color: var(--accent); }

  /* ---------- Pills (performer / studio / overflow) ----------
     Mirrored from SessionDetail.svelte so both surfaces share the exact
     same visual language. If these grow further, lift into a shared
     Pill.svelte component or a global stylesheet. */
  .card-pills {
    display: flex;
    align-items: center;
    flex-wrap: nowrap;          /* one line; overflow goes into +N chip */
    gap: 4px;
    overflow: hidden;
    margin-top: 1px;
    min-width: 0;
  }
  .pill {
    display: inline-flex;
    align-items: center;
    gap: 3px;
    padding: 2px 7px;
    border-radius: var(--radius-pill, 999px);
    font-family: inherit;
    font-size: 10.5px;
    font-weight: 500;
    line-height: 1.3;
    white-space: nowrap;
    cursor: pointer;
    background: var(--bg-card-hover, var(--bg));
    border: 1px solid var(--border, transparent);
    color: var(--text);
    max-width: 140px;
    overflow: hidden;
    text-overflow: ellipsis;
    transition: background var(--dur-2, 120ms),
                border-color var(--dur-2, 120ms),
                color var(--dur-2, 120ms);
  }
  .pill:hover {
    background: var(--bg);
    border-color: var(--border-strong, var(--accent));
  }
  /* Performer — neutral. */
  .performer-pill:hover { color: var(--accent); border-color: var(--accent); }
  /* Studio — cream-tinted, sits below the performer row. */
  .studio-pill {
    background: var(--highlight-soft, var(--bg-card-hover));
    border-color: var(--highlight, var(--border));
    color: var(--highlight, var(--text));
  }
  .studio-row { margin-top: 1px; }
  .studio-pill:hover {
    background: var(--highlight, transparent);
    color: var(--bg);
    border-color: var(--highlight);
  }
  /* +N overflow chip. Dashed border signals "there's more behind this". */
  .overflow-pill {
    border-style: dashed;
    color: var(--text-muted);
    font-weight: 600;
    font-variant-numeric: tabular-nums;
  }
  .overflow-pill:hover {
    color: var(--accent);
    border-color: var(--accent);
    border-style: solid;
  }

  .bottom {
    display: flex;
    gap: 8px;
    font-size: 11px;
    color: var(--fg-muted);
    margin-top: 2px;
    font-variant-numeric: tabular-nums;
    font-family: var(--font-mono);
  }
  .watched { white-space: nowrap; }
</style>
