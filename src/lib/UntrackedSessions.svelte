<!--
  Untracked sessions review (Phase 7 component 2). Scans Stash history for
  sittings Climax didn't track, clusters them into candidate sessions, and lets
  you STAGE a decision per row - Add (import as an estimated session), Remove
  (dismiss so it stops being offered), or leave it (the default: stays for next
  time). Nothing happens until you hit Apply, so a stray click is harmless.

  Renders either as a standalone modal (default) or `embedded` inside the paged
  Settings window (no overlay/header chrome; Settings provides those).

  Estimates, not gospel: Stash logs WHEN you watched, not how long each time, so
  bounds + per-scene minutes are approximate. Accepted sessions are tagged
  "estimated" and stay fully editable/deletable in the Sessions tab.
-->
<script lang="ts">
  import { w } from "$lib/wording.svelte";
  import { onMount } from "svelte";
  import Icon from "$lib/Icon.svelte";
  import ConfirmDialog from "$lib/ConfirmDialog.svelte";
  import DateTimePicker from "$lib/DateTimePicker.svelte";
  import {
    api,
    formatDuration,
    formatTimeOnly,
    formatDateOnly,
    dayKey,
    today,
    type CandidateSession,
    type CandidateScene,
  } from "$lib/api";

  let {
    onClose,
    embedded = false,
    onApplied,
    awayMode = false,
  }: {
    onClose?: () => void;
    embedded?: boolean;
    onApplied?: () => void;
    // "While you were away" mode: scan only since the last logged session (the
    // backend floor), force dismissed-respecting, and hide the heavy controls
    // (gap knob / show-removed / refresh / Continue) - it's a quick launch triage.
    awayMode?: boolean;
  } = $props();

  const RANGE_START = "2000-01-01";

  let loaded = $state(false);
  let refreshing = $state(false);
  let applying = $state(false);
  let errorMsg = $state<string | null>(null);
  let gapMinutes = $state(45);
  let showRemoved = $state(false);
  // Candidates get a stable `uid` at load time. Editing a candidate's start/end
  // must NOT change its key - a content-derived key (start_ms) re-keyed the row
  // on every edit, collapsing the open editor and forcing one digit at a time.
  type Candidate = CandidateSession & { uid: string };

  let candidates = $state<Candidate[]>([]);
  let expanded = $state<Set<string>>(new Set());
  // Staged decisions, keyed by candidate uid. Absence = "leave it".
  let decisions = $state<Record<string, "add" | "remove">>({});

  // Extra cumshots the user adds per scene in the expand view (ones Stash doesn't
  // know about). Keyed `${uid}:${content_item_id}`. Reset on every load.
  let extraCumshots = $state<Record<string, number>>({});
  const scKey = (c: Candidate, s: CandidateScene) => `${c.uid}:${s.content_item_id}`;
  const extraFor = (c: Candidate, s: CandidateScene) => extraCumshots[scKey(c, s)] ?? 0;
  const totalCumshots = (c: Candidate) =>
    c.cumshot_count + c.scenes.reduce((n, s) => n + extraFor(c, s), 0);
  function bumpCumshot(c: Candidate, s: CandidateScene, delta: number) {
    const k = scKey(c, s);
    const next = { ...extraCumshots };
    const v = Math.max(0, (next[k] ?? 0) + delta);
    if (v === 0) delete next[k];
    else next[k] = v;
    extraCumshots = next;
  }

  // Below-threshold handling. `s.below_threshold` is baked by the backend (a scene
  // watched under the play threshold with NO cumshot; cumshot scenes are never
  // flagged), so it's the single source of truth. These scenes are collapsed
  // behind a per-candidate toggle and EXCLUDED from a default accept - the same as
  // the wrap-up modal. The user can expand and opt individual ones back in.
  let showBelowSet = $state<Set<string>>(new Set());
  // Below-threshold scenes the user opted back IN. Keyed `${uid}:${content_item_id}`.
  let includedBelow = $state<Record<string, boolean>>({});

  const belowScenes = (c: Candidate) => c.scenes.filter((s) => s.below_threshold);
  // A scene is brought into the session when it's above threshold, OR the user
  // explicitly included it, OR it carries a staged cumshot (which counts it -
  // excluding it would silently drop that cumshot).
  const isBroughtIn = (c: Candidate, s: CandidateScene) =>
    !s.below_threshold || (includedBelow[scKey(c, s)] ?? false) || extraFor(c, s) > 0;
  const broughtInScenes = (c: Candidate) => c.scenes.filter((s) => isBroughtIn(c, s));

  function toggleBelow(uid: string) {
    const next = new Set(showBelowSet);
    next.has(uid) ? next.delete(uid) : next.add(uid);
    showBelowSet = next;
  }
  function toggleInclude(c: Candidate, s: CandidateScene) {
    const k = scKey(c, s);
    const next = { ...includedBelow };
    if (next[k]) delete next[k];
    else next[k] = true;
    includedBelow = next;
  }

  // Immediate (non-staged) action behind a themed confirm: deleting a detected
  // cumshot removes it from Climax AND pushes the removal to Stash.
  let confirmDeleteO = $state<{ cand: Candidate; scene: CandidateScene; oId: number } | null>(null);

  const TIER: Record<string, { label: string; cls: string }> = {
    boundable: { label: "Session", cls: "t-conf" },
    lone_boundable: { label: "Single play", cls: "t-mid" },
    lone_fuzzy: { label: "Uncertain", cls: "t-low" },
  };

  const key = (c: Candidate) => c.uid;
  const addCount = $derived(candidates.filter((c) => decisions[key(c)] === "add").length);
  const removeCount = $derived(candidates.filter((c) => decisions[key(c)] === "remove").length);
  const allAdded = $derived(
    candidates.length > 0 && candidates.every((c) => decisions[key(c)] === "add"),
  );

  // Re-run the clustering scan over the (already-imported) history. Does NOT
  // toggle `refreshing` - the callers (load / refresh) own that.
  async function scan() {
    // Away mode scans only since the last logged session (the backend applies the
    // floor + always respects dismissed); the full review scans all of history.
    const raw = awayMode
      ? (await api.reconstructAwayCandidates()).candidates
      : await api.reconstructCandidates(RANGE_START, dayKey(today()), {
          gapMinutes,
          respectDismissed: !showRemoved,
        });
    // Stable per-load uid so editing bounds doesn't re-key rows.
    candidates = raw.map((c, i) => ({ ...c, uid: `c${i}` }));
    expanded = new Set();
    decisions = {};
    extraCumshots = {};
    showBelowSet = new Set();
    includedBelow = {};
  }

  // Backend rejections are a mix: designed sentences ("That cumshot no longer
  // exists.") and raw driver chains. Pass a designed sentence through as-is;
  // swap anything raw for the fallback (the raw text goes to the console).
  function humanError(e: unknown, fallback: string): string {
    const s = String(e).trim();
    const looksDesigned =
      /^[A-Z]/.test(s) && s.endsWith(".") && !s.includes(": ") && s.length <= 200;
    return looksDesigned ? s : fallback;
  }

  async function load() {
    refreshing = true;
    errorMsg = null;
    try {
      await scan();
    } catch (e) {
      console.error("untracked sessions scan failed", e);
      errorMsg = "Couldn't scan your history. Try again in a moment.";
    } finally {
      refreshing = false;
    }
  }

  // Refresh: pull the latest play / o history from Stash, wait for that
  // background sync to finish (capped so a no-op or Stash-off refresh can't hang),
  // then re-scan - so newly-watched activity (or anything the last sync missed)
  // surfaces without leaving the page. Falls back to a plain local re-scan if
  // Stash is off/unreachable.
  async function refresh() {
    if (refreshing || applying) return;
    refreshing = true;
    errorMsg = null;
    try {
      try {
        await api.mirrorSyncNow();
        let sawRunning = false;
        for (let i = 0; i < 16; i++) {
          await new Promise((r) => setTimeout(r, 700));
          const status = await api.mirrorStatusGet().catch(() => null);
          if (!status) break;
          if (status.phase !== "idle") sawRunning = true;
          else if (sawRunning || i >= 2) break; // finished, or never had work to do
        }
      } catch {
        /* Stash off / unreachable - fall through to a local re-scan */
      }
      await scan();
    } catch (e) {
      console.error("untracked sessions refresh failed", e);
      errorMsg = "Couldn't refresh from Stash. Try again in a moment.";
    } finally {
      refreshing = false;
    }
  }

  onMount(async () => {
    try {
      gapMinutes = (await api.sessionGapGet()).gap_minutes;
    } catch {
      /* keep default */
    }
    await load();
    loaded = true;
  });

  async function applyGap() {
    const g = Math.max(1, Math.floor(gapMinutes || 45));
    gapMinutes = g;
    try {
      await api.sessionGapSet(g);
    } catch {
      /* non-fatal */
    }
    decisions = {};
    await load();
  }

  async function toggleShowRemoved() {
    showRemoved = !showRemoved;
    decisions = {};
    await load();
  }

  function toggleExpand(k: string) {
    const next = new Set(expanded);
    next.has(k) ? next.delete(k) : next.add(k);
    expanded = next;
  }

  function decide(c: Candidate, choice: "add" | "remove") {
    const k = key(c);
    const next = { ...decisions };
    if (next[k] === choice) delete next[k];
    else next[k] = choice;
    decisions = next;
  }

  // "Add all" / "Clear all" toggle: stage every candidate as add, or if they
  // already all are, clear back to none. Deliberately forgets any prior custom
  // selection - it's a one-shot select-all, not a merge.
  function toggleAddAll() {
    if (allAdded) {
      decisions = {};
      return;
    }
    const next: Record<string, "add" | "remove"> = {};
    for (const c of candidates) next[key(c)] = "add";
    decisions = next;
  }

  function clearStaged() {
    decisions = {};
  }

  // Only scenes that are brought in (above threshold, or a below-threshold one the
  // user opted back in / put a cumshot on) go into the session. Below-threshold
  // scenes carry no detected cumshots, so their o_event_ids are empty - taking the
  // kept scenes' o_event_ids is equivalent to all, just scoped for clarity.
  function acceptArgs(c: Candidate) {
    const kept = broughtInScenes(c);
    return {
      startMs: c.start_ms,
      endMs: c.end_ms,
      assignedDay: c.assigned_day,
      oEventIds: kept.flatMap((s) => s.o_event_ids),
      scenes: kept.map((s) => {
        const times = [...s.play_times, ...s.cumshot_times].sort((a, b) => a - b);
        return {
          content_item_id: s.content_item_id,
          est_seconds: s.est_seconds,
          first_seen_ms: times[0] ?? c.start_ms,
          last_seen_ms: times[times.length - 1] ?? c.end_ms,
          extra_cumshots: extraFor(c, s),
        };
      }),
    };
  }

  async function apply() {
    if (applying || addCount + removeCount === 0) return;
    applying = true;
    errorMsg = null;
    try {
      const adds = candidates.filter((c) => decisions[key(c)] === "add");
      const removes = candidates.filter((c) => decisions[key(c)] === "remove");
      for (const c of adds) {
        const args = acceptArgs(c);
        // Everything was below threshold and nothing was opted in -> nothing to
        // add; don't create an empty session (the candidate stays for next time).
        if (args.scenes.length === 0 && args.oEventIds.length === 0) continue;
        await api.reconstructAccept(args);
      }
      if (removes.length) {
        await api.reconstructDismiss(
          removes.flatMap((c) => c.scenes.flatMap((s) => s.play_import_ids)),
          removes.flatMap((c) => c.scenes.flatMap((s) => s.o_event_ids)),
        );
      }
      decisions = {};
      await load();
      onApplied?.();
      // In the launch triage, once everything's handled, get out of the way.
      if (awayMode && candidates.length === 0) onClose?.();
    } catch (e) {
      console.error("untracked sessions apply failed", e);
      errorMsg = humanError(e, "Couldn't apply your changes. Some may have gone through - please try again in a moment.");
    } finally {
      applying = false;
    }
  }

  // ---- Delete a detected cumshot (removes from Climax + Stash) ----
  function startDeleteO(c: Candidate, s: CandidateScene) {
    const ids = s.o_event_ids;
    if (!ids || ids.length === 0) return;
    confirmDeleteO = { cand: c, scene: s, oId: ids[ids.length - 1] };
  }
  async function doDeleteO() {
    const d = confirmDeleteO;
    if (!d || applying) return;
    applying = true;
    errorMsg = null;
    try {
      await api.oDelete(d.oId);
      confirmDeleteO = null;
      // Update the candidate IN PLACE (no full reload, so the open row + every
      // staged decision/extra survive): drop the deleted o_event + one cumshot
      // time and decrement the candidate's total.
      d.scene.o_event_ids = d.scene.o_event_ids.filter((id) => id !== d.oId);
      if (d.scene.cumshot_times.length > 0) {
        d.scene.cumshot_times = d.scene.cumshot_times.slice(0, -1);
      }
      d.cand.cumshot_count = Math.max(0, d.cand.cumshot_count - 1);
      onApplied?.();
    } catch (e) {
      console.error("untracked sessions cumshot delete failed", e);
      errorMsg = w(humanError(e, "Couldn't remove that cumshot. Please try again in a moment."));
      confirmDeleteO = null;
    } finally {
      applying = false;
    }
  }
  // The cumshot stepper's "-": peel a staged extra first (reversible), then delete
  // a real detected cumshot (confirm + Stash rollback).
  function minusCumshot(c: Candidate, s: CandidateScene) {
    if (extraFor(c, s) > 0) bumpCumshot(c, s, -1);
    else if (s.o_event_ids.length > 0) startDeleteO(c, s);
  }

  function setStart(c: CandidateSession, ms: number) {
    c.start_ms = ms;
  }
  function setEnd(c: CandidateSession, ms: number) {
    c.end_ms = ms;
  }
</script>

<!-- One scene row inside a candidate's expand view. Shared by the counted list and
     the below-threshold list. Below-threshold rows dim their thumbnail + title
     until brought in, and carry an "Include" checkbox to opt the scene back into
     the session (a staged cumshot forces inclusion, so the box locks checked). -->
{#snippet sceneRow(c: Candidate, s: CandidateScene)}
  {@const brought = isBroughtIn(c, s)}
  <div class="scene" class:dim={s.below_threshold && !brought}>
    {#if s.thumbnail_url}
      <img src={s.thumbnail_url} alt="" />
    {:else}
      <div class="thumb-fallback">▦</div>
    {/if}
    <div class="meta">
      <span class="title">{s.title ?? `Scene ${s.external_id ?? s.content_item_id}`}</span>
      <span class="sub2">{s.play_times.length} {s.play_times.length === 1 ? 'play' : 'plays'}</span>
    </div>
    {#if s.below_threshold}
      <label class="incl" title={extraFor(c, s) > 0 ? w('This scene has a cumshot, so it stays. Remove the cumshot to leave it out.') : 'Add this scene to the session anyway.'}>
        <input type="checkbox" checked={brought} disabled={extraFor(c, s) > 0 || applying} onchange={() => toggleInclude(c, s)} />
        Include
      </label>
    {/if}
    <div class="cstep" title={w("Add or remove cumshots for this scene - updates the total cumshot count in both Stash and Climax.")}>
      <button class="cs-btn"
        disabled={(extraFor(c, s) === 0 && s.o_event_ids.length === 0) || applying}
        onclick={() => minusCumshot(c, s)}
        title={w("Remove a cumshot - affects both Stash and Climax.")}
        aria-label={w("Remove cumshot")}>-</button>
      <span class="cs-n">
        <Icon name="cumshot" size={13} color="var(--accent)" filled />
        {s.cumshot_times.length + extraFor(c, s)}
      </span>
      <button class="cs-btn" onclick={() => bumpCumshot(c, s, 1)} aria-label={w("Add cumshot")}>+</button>
    </div>
  </div>
{/snippet}

{#snippet inner()}
  <p class="intro">
    {#if awayMode}
      Stash logged this activity since your last session, but Climax wasn't tracking it. The times are estimates - add the ones worth keeping. Anything you remove won't be offered again.
    {:else}
      Sessions Climax didn't record, rebuilt from Stash's play history. The times are estimates - add the ones worth keeping. Anything you leave stays here for next time.
    {/if}
  </p>

  <div class="controls">
    {#if !awayMode}
      <label class="gap">
        New session after a break of
        <input type="number" min="1" step="1" bind:value={gapMinutes} onchange={applyGap} />
        min
      </label>
      <label class="chk">
        <input type="checkbox" checked={showRemoved} onchange={toggleShowRemoved} />
        Show previously rejected sessions
      </label>
      <button class="ghost refresh" onclick={refresh} disabled={refreshing || applying}
        title="Re-check Stash for new activity and re-scan your history.">
        {refreshing ? 'Checking Stash...' : 'Refresh'}
      </button>
    {/if}
    {#if candidates.length > 0}
      <button class="ghost" class:pushright={awayMode} onclick={toggleAddAll} disabled={applying}>
        {allAdded ? 'Clear all' : 'Add all'}
      </button>
    {/if}
  </div>

  <div class="body" class:dim={refreshing || applying}>
    {#if !loaded}
      <div class="empty">Scanning your Stash history...</div>
    {:else if errorMsg}
      <div class="error">{errorMsg}</div>
    {:else if candidates.length === 0}
      <div class="empty">
        {#if awayMode}
          All caught up - nothing untracked since your last session
        {:else if showRemoved}
          No untracked sessions anywhere in your history
        {:else}
          All caught up - no untracked Stash activity found
        {/if}
      </div>
    {:else}
      {#each candidates as c (key(c))}
        {@const k = key(c)}
        {@const d = decisions[k]}
        {@const keptCount = broughtInScenes(c).length}
        {@const below = belowScenes(c)}
        <div class="cand" class:s-add={d === 'add'} class:s-remove={d === 'remove'}>
          <div class="head" role="button" tabindex="0"
            onclick={() => toggleExpand(k)}
            onkeydown={(e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); toggleExpand(k); } }}>
            <span class="chev" class:open={expanded.has(k)}>
              <Icon name="chevron-right" size={15} />
            </span>
            <div class="when">
              <span class="date">{formatDateOnly(c.start_ms)}</span>
              <span class="span">{formatTimeOnly(c.start_ms)}-{formatTimeOnly(c.end_ms)} · {formatDuration(c.end_ms - c.start_ms)}</span>
            </div>
            <span class="tier {TIER[c.tier]?.cls ?? ''}">{TIER[c.tier]?.label ?? c.tier}</span>
            <div class="stats">
              <span>{keptCount} {keptCount === 1 ? 'scene' : 'scenes'}</span>
              {#if totalCumshots(c) > 0}<span class="cum"><Icon name="cumshot" size={12} color="var(--accent)" filled /> {totalCumshots(c)}</span>{/if}
            </div>
            <div class="acts">
              <button class="add" class:on={d === 'add'} onclick={(e) => { e.stopPropagation(); decide(c, 'add'); }}>Add</button>
              <button class="rem" class:on={d === 'remove'} onclick={(e) => { e.stopPropagation(); decide(c, 'remove'); }}>Remove</button>
            </div>
          </div>

          {#if expanded.has(k)}
            <div class="detail">
              <div class="edit">
                <div class="field"><span>Start</span>
                  <DateTimePicker value={c.start_ms} onChange={(ms) => setStart(c, ms)} />
                </div>
                <div class="field"><span>End</span>
                  <DateTimePicker value={c.end_ms} onChange={(ms) => setEnd(c, ms)} />
                </div>
              </div>
              <div class="scenes">
                {#each c.scenes.filter((s) => !s.below_threshold) as s}
                  {@render sceneRow(c, s)}
                {/each}
              </div>

              <!-- Scenes watched below the play threshold (no cumshot) are hidden by
                   default and left out of a default Add, so brief opens don't get
                   logged. Reveal to opt any back in, or log a cumshot on one. -->
              {#if below.length > 0}
                <button class="below-toggle" onclick={() => toggleBelow(k)}>
                  <Icon name={showBelowSet.has(k) ? 'chevron-up' : 'chevron-down'} size={14} />
                  <span>{showBelowSet.has(k) ? 'Hide' : 'Show'} {below.length} scene{below.length === 1 ? '' : 's'} below the threshold</span>
                </button>
                {#if showBelowSet.has(k)}
                  <div class="scenes below-list">
                    {#each below as s}
                      {@render sceneRow(c, s)}
                    {/each}
                  </div>
                {/if}
              {/if}
            </div>
          {/if}
        </div>
      {/each}
    {/if}
  </div>

  <footer>
    <span class="hint">
      {#if addCount + removeCount > 0}
        Pending: {addCount} to add{removeCount > 0 ? `, ${removeCount} to remove` : ''}. Nothing happens until you Apply.
      {:else}
        No changes pending. Select Add or Remove on the rows you want.
      {/if}
    </span>
    <div class="foot-btns">
      {#if addCount + removeCount > 0}
        <button class="clear" onclick={clearStaged} disabled={applying}>Clear</button>
      {/if}
      {#if !embedded || awayMode}
        <button class="close" onclick={() => onClose?.()} disabled={applying}>{awayMode ? 'Not now' : 'Close'}</button>
      {/if}
      <button class="apply" onclick={apply} disabled={applying || addCount + removeCount === 0}>
        {applying ? 'Applying...' : `Apply${addCount + removeCount > 0 ? ` (${addCount} add${removeCount > 0 ? ` · ${removeCount} remove` : ''})` : ''}`}
      </button>
    </div>
  </footer>
{/snippet}

{#if embedded}
  <div class="embedded">
    {@render inner()}
  </div>
{:else}
  <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_noninteractive_element_interactions -->
  <div class="overlay" role="dialog" aria-modal="true" tabindex="-1"
    onkeydown={(e) => { if (e.key === 'Escape') onClose?.(); }}
    onclick={() => onClose?.()}>
    <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
    <div class="modal" role="document" onclick={(e) => e.stopPropagation()}>
      <header><h2>Untracked sessions</h2></header>
      {@render inner()}
    </div>
  </div>
{/if}

{#if confirmDeleteO}
  <ConfirmDialog
    title={w("Remove this cumshot?")}
    message={w("This removes the cumshot from both Climax and Stash. This can't be undone from here.")}
    confirmLabel="Remove"
    cancelLabel="Cancel"
    variant="danger"
    onConfirm={doDeleteO}
    onCancel={() => (confirmDeleteO = null)}
  />
{/if}

<style>
/* =====================================================================
   UntrackedSessions styling.
   ---------------------------------------------------------------------
   The chevron is an inline Icon (chevron-right) rather than a literal
   character, rotated 90deg when the row is expanded.

   Tier coding stays meaningful: Session = coral tint, Single play =
   neutral fill, Uncertain = subtle. All Title case, weight 500,
   matching the rest of Climax's chip language.
   ===================================================================== */

/* ---------- standalone modal (overlay mode) ---------- */
.overlay {
  position: fixed; inset: 0;
  background: rgba(7, 8, 11, 0.72);
  display: flex; align-items: center; justify-content: center;
  z-index: 1200;
  animation: fade-in 140ms cubic-bezier(0.16, 1, 0.3, 1);
}
@keyframes fade-in { from { opacity: 0 } to { opacity: 1 } }

.modal {
  background: var(--bg-card, #15171E);
  border: 1px solid var(--border, #252934);
  border-radius: var(--radius-xl, 14px);
  width: min(900px, 94vw);
  max-height: 86vh;
  display: flex; flex-direction: column;
  box-shadow: var(--shadow-lg, 0 12px 32px rgba(0,0,0,0.45));
  overflow: hidden;
  animation: pop-in 220ms cubic-bezier(0.16, 1, 0.3, 1);
}
@keyframes pop-in {
  from { transform: translateY(8px) scale(0.98); opacity: 0 }
  to   { transform: none; opacity: 1 }
}

/* Embedded inside the Settings paged window: fill the pane, let .body scroll. */
.embedded { display: flex; flex-direction: column; flex: 1; min-height: 0; }

header {
  padding: 18px 28px 16px;
  border-bottom: 1px solid var(--border-subtle, #1C1F26);
}
header h2 {
  margin: 0; font-family: var(--font-display, system-ui);
  font-size: 22px; font-weight: 600; letter-spacing: -0.01em;
  color: var(--fg-strong, #F7F8FA);
}

/* ---------- intro paragraph ---------- */
.intro {
  margin: 0;
  padding: 22px 28px 8px;
  font-size: 14px; line-height: 1.55;
  color: var(--fg-muted, #8A909E);
  max-width: 64ch;
}

/* ---------- toolbar (break-of, show-removed, add-all) ---------- */
.controls {
  display: flex; align-items: center; gap: 16px; flex-wrap: wrap;
  padding: 4px 28px 16px;
}
.gap {
  display: inline-flex; align-items: center; gap: 8px;
  font-size: 14px; color: var(--fg-muted, #8A909E);
  white-space: nowrap;
}
.gap input {
  width: 92px; padding: 8px 11px;
  background: var(--bg-input, #101218); border: 1px solid var(--border, #252934);
  border-radius: var(--radius-sm, 6px); color: var(--fg, #ECEEF3);
  font-size: 13px; font-family: var(--font-mono, monospace); text-align: left;
  font-variant-numeric: tabular-nums;
  transition: border-color 120ms, box-shadow 120ms;
}
.gap input:focus {
  outline: none; border-color: var(--accent, #EF6B7A);
  box-shadow: 0 0 0 3px var(--coral-a-24, rgba(239,107,122,0.24));
}
.chk {
  display: inline-flex; align-items: center; gap: 8px;
  font-size: 14px; color: var(--fg-muted, #8A909E); cursor: pointer;
  white-space: nowrap;
}
.chk input {
  width: 17px; height: 17px;
  accent-color: var(--accent, #EF6B7A);
  cursor: pointer;
}
.ghost {
  padding: 7px 15px;
  background: var(--bg-card-hover, #1B1E26); color: var(--fg, #ECEEF3);
  border: 1px solid var(--border, #252934); border-radius: var(--radius-sm, 6px);
  font-family: inherit; font-size: 12px; font-weight: 500; cursor: pointer;
  white-space: nowrap;
  transition: background 120ms, border-color 120ms, transform 80ms;
}
/* Refresh is the first of the right-aligned group; it consumes the free space so
   it + Add all sit together on the right. */
.refresh { margin-left: auto; }
/* In away mode the refresh button is hidden, so push Add all to the right itself. */
.ghost.pushright { margin-left: auto; }
.ghost:hover:not(:disabled) { background: var(--bg-elevated, #101218); border-color: var(--border-strong, #353A47); }
.ghost:active { transform: translateY(1px); }
.ghost:disabled { opacity: 0.5; cursor: default; }

/* ---------- scroll body + states ---------- */
.body {
  flex: 1; min-height: 0;
  padding: 4px 28px 28px;
  overflow-y: auto;
  display: flex; flex-direction: column; gap: 6px;
  transition: opacity 200ms;
}
.body.dim { opacity: 0.5; pointer-events: none; }
.empty {
  padding: 40px 16px; text-align: center;
  color: var(--fg-muted, #8A909E); font-size: 14px;
}
.error {
  padding: 8px 12px;
  border: 1px solid rgba(242,107,107,0.25);
  background: rgba(242,107,107,0.07);
  color: var(--danger, #F26B6B);
  border-radius: var(--radius-md, 8px);
  font-size: 12px;
}

/* ---------- candidate row card ---------- */
.cand {
  background: var(--bg-elevated, #101218);
  border: 1px solid var(--border, #252934);
  border-radius: var(--radius-md, 8px);
  border-left: 3px solid transparent;
  overflow: hidden;
  flex-shrink: 0; /* don't let many rows squeeze to slivers; let .body scroll */
  transition: background 120ms, border-color 120ms;
}
.cand.s-add { border-left-color: var(--live, #4ADE80); background: rgba(74,222,128,0.07); }
.cand.s-remove { border-left-color: var(--coral-700, #B83A4A); background: rgba(184,58,74,0.08); }

.head {
  display: flex; align-items: center; gap: 14px;
  padding: 11px 14px 11px 11px;
  cursor: pointer;
  transition: background 120ms;
}
.head:hover { background: var(--bg-card-hover, #1B1E26); }
.cand.s-add .head:hover { background: rgba(74,222,128,0.14); }
.cand.s-remove .head:hover { background: rgba(184,58,74,0.16); }

.chev {
  color: var(--fg-subtle, #5C6273);
  flex-shrink: 0; display: inline-flex; align-items: center;
  width: 16px;
  transition: transform 200ms cubic-bezier(0.16,1,0.3,1), color 120ms;
}
.chev.open { transform: rotate(90deg); color: var(--fg, #ECEEF3); }

.when {
  display: flex; flex-direction: column;
  width: 188px; flex-shrink: 0;
}
.when .date { font-size: 13px; font-weight: 600; color: var(--fg, #ECEEF3); }
.when .span {
  font-size: 11px; color: var(--fg-muted, #8A909E);
  font-family: var(--font-mono, monospace); font-variant-numeric: tabular-nums;
  margin-top: 1px;
}

/* ---------- tier pill (session / single play / uncertain) ----------
   Title case, weight 500, soft fill - matches Climax's performer/studio
   chip language. Fixed column width via inline-flex + min-width so the
   three tiers line up. */
.tier {
  font-size: 11px; font-weight: 500; letter-spacing: 0;
  text-transform: none;
  padding: 3px 10px; border-radius: 999px;
  border: none;
  white-space: nowrap;
  display: inline-flex; align-items: center; justify-content: center; text-align: center;
  min-width: 104px;
}
.t-conf { background: var(--coral-a-16, rgba(239,107,122,0.16)); color: var(--coral-300, #FAA8B1); }
.t-mid  { background: var(--white-a-08, rgba(255,255,255,0.08)); color: var(--fg-muted, #8A909E); }
.t-low  { background: transparent; color: var(--fg-subtle, #5C6273); border: 1px dashed var(--border, #252934); }

/* ---------- stats (N scenes · N cumshots) ---------- */
.stats {
  display: flex; gap: 14px; margin-left: auto;
  font-size: 12px; color: var(--fg-muted, #8A909E);
  font-variant-numeric: tabular-nums;
  white-space: nowrap;
}
.stats .cum { color: var(--accent, #EF6B7A); font-weight: 600; }

/* ---------- staging buttons (Add / Remove) ----------
   GREEN for add (--live), DEEPER RED for remove (--coral-700) - so the
   two staged states are unmistakably different from each other AND from
   the brand-coral Apply button (commit). */
.acts { display: flex; gap: 6px; flex-shrink: 0; }
.acts button {
  padding: 6px 14px;
  border-radius: var(--radius-sm, 6px);
  font-family: inherit; font-size: 12px; font-weight: 500;
  border: 1px solid var(--border, #252934);
  background: transparent; color: var(--fg-muted, #8A909E);
  cursor: pointer;
  transition: background 120ms, color 120ms, border-color 120ms, transform 80ms;
}
.acts button:active { transform: translateY(1px); }
.acts .add:hover     { border-color: var(--live, #4ADE80); color: var(--live, #4ADE80); }
.acts .add.on        { background: var(--live, #4ADE80); border-color: var(--live, #4ADE80); color: var(--ink-950, #0A0B0F); }
.acts .rem:hover     { border-color: var(--coral-700, #B83A4A); color: var(--coral-400, #F58895); }
.acts .rem.on        { background: var(--coral-700, #B83A4A); border-color: var(--coral-700, #B83A4A); color: #fff; }

/* ---------- expanded detail ---------- */
.detail {
  padding: 4px 16px 16px 28px;
  display: flex; flex-direction: column; gap: 14px;
  border-top: 1px solid var(--border-subtle, #1C1F26);
  margin-top: -1px;
}
.edit { display: flex; gap: 16px; flex-wrap: wrap; padding-top: 12px; }
.edit .field {
  display: inline-flex; flex-direction: column; gap: 5px;
  font-size: 10px; text-transform: uppercase; letter-spacing: 0.14em;
  font-weight: 600; color: var(--fg-muted, #8A909E);
}

/* ---------- scene list inside detail ---------- */
.scenes { display: flex; flex-direction: column; gap: 8px; }
.scenes.below-list { margin-top: 8px; }
.scene { display: flex; align-items: center; gap: 12px; }
/* Below-threshold rows not (yet) brought in: dim the thumbnail + title so it's
   clear they won't be logged; the controls stay fully readable. */
.scene.dim img, .scene.dim .thumb-fallback, .scene.dim .meta {
  opacity: 0.42;
  transition: opacity 160ms ease;
}
.scene.dim:hover img, .scene.dim:hover .thumb-fallback, .scene.dim:hover .meta { opacity: 0.7; }

/* Below-threshold reveal toggle: a quiet full-width dashed ghost between the
   counted scenes and the hidden below-threshold ones (matches the wrap-up modal). */
.below-toggle {
  display: flex; align-items: center; justify-content: center; gap: 7px;
  width: 100%; margin-top: 10px; padding: 8px;
  background: transparent;
  border: 1px dashed var(--border, #252934); border-radius: var(--radius-md, 8px);
  color: var(--fg-muted, #8A909E);
  font-family: inherit; font-size: 12px; font-weight: 500; cursor: pointer;
  transition: background 120ms, border-color 120ms, color 120ms;
}
.below-toggle:hover { background: var(--bg-card-hover, #1B1E26); border-color: var(--border-strong, #353A47); color: var(--fg, #ECEEF3); }

/* "Include" checkbox on a below-threshold row: opt the scene back into the session. */
.incl {
  display: inline-flex; align-items: center; gap: 6px; margin-left: auto;
  flex-shrink: 0; white-space: nowrap;
  font-size: 11px; font-weight: 500; color: var(--fg-muted, #8A909E); cursor: pointer;
}
.incl input { width: 15px; height: 15px; accent-color: var(--accent, #EF6B7A); cursor: pointer; }
.incl input:disabled { cursor: default; }
.scene img, .thumb-fallback {
  flex-shrink: 0;
  width: 72px; aspect-ratio: 16/9; height: auto;
  border-radius: var(--radius-sm, 6px);
  border: 1px solid var(--border, #252934);
  object-fit: cover;
  background: linear-gradient(135deg, #1d1218 0%, #14101a 100%);
}
.thumb-fallback {
  display: grid; place-items: center;
  color: rgba(239, 107, 122, 0.22);
  font-family: var(--font-mono, monospace);
  font-size: 16px; font-weight: 700;
}
.scene .meta { display: flex; flex-direction: column; gap: 2px; min-width: 0; }
.scene .title {
  font-size: 13px; color: var(--fg, #ECEEF3);
  white-space: nowrap; overflow: hidden; text-overflow: ellipsis;
  max-width: 440px;
}
.scene .sub2 {
  font-size: 11px; color: var(--fg-muted, #8A909E);
  font-family: var(--font-mono, monospace); font-variant-numeric: tabular-nums;
}

/* ---------- per-scene cumshot stepper ---------- */
.cstep { margin-left: auto; display: inline-flex; align-items: center; gap: 2px; flex-shrink: 0; }
.cs-btn {
  width: 22px; height: 22px; display: inline-flex; align-items: center; justify-content: center;
  padding: 0; border-radius: var(--radius-sm, 6px); font-family: inherit; font-size: 15px; line-height: 1;
  border: 1px solid var(--border, #252934); background: transparent; color: var(--fg-muted, #8A909E);
  cursor: pointer; transition: background 120ms, color 120ms, border-color 120ms;
}
.cs-btn:hover:not(:disabled) { border-color: var(--accent, #EF6B7A); color: var(--accent, #EF6B7A); }
.cs-btn:disabled { opacity: 0.4; cursor: default; }
.cs-n {
  display: inline-flex; align-items: center; gap: 3px; min-width: 36px; justify-content: center;
  font-size: 12px; font-weight: 600; color: var(--accent, #EF6B7A); font-variant-numeric: tabular-nums;
}

/* ---------- footer (owned by this component, embedded or not) ---------- */
footer {
  padding: 16px 28px;
  border-top: 1px solid var(--border-subtle, #1C1F26);
  background: var(--bg-sunken, #07080B);
  display: flex; align-items: center; gap: 10px;
  flex-shrink: 0;
}
footer .hint {
  font-size: 12px; color: var(--fg-subtle, #5C6273);
  margin-right: auto; flex: 1; min-width: 0;
  line-height: 1.4;
}
.foot-btns { display: flex; gap: 8px; flex-shrink: 0; }
footer button {
  padding: 9px 18px;
  border-radius: var(--radius-md, 8px);
  font-size: 13px; font-weight: 500;
  border: 1px solid var(--border, #252934);
  background: transparent; color: var(--fg, #ECEEF3);
  cursor: pointer; font-family: inherit;
  white-space: nowrap;
  transition: background 120ms, border-color 120ms, transform 80ms;
}
footer button:active { transform: translateY(1px); }
footer .clear:hover:not(:disabled),
footer .close:hover:not(:disabled) {
  background: var(--bg-card-hover, #1B1E26);
  border-color: var(--border-strong, #353A47);
}
footer .apply {
  background: var(--accent, #EF6B7A);
  border-color: var(--accent, #EF6B7A);
  color: var(--accent-fg, #fff);
}
footer .apply:hover:not(:disabled) {
  background: var(--accent-hover, #F58895);
  border-color: var(--accent-hover, #F58895);
}
footer .apply:focus-visible {
  outline: none;
  box-shadow: 0 0 0 3px var(--coral-a-24, rgba(239,107,122,0.24));
}
footer button:disabled { opacity: 0.5; cursor: default; }
</style>
