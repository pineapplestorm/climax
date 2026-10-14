<!--
  Wrap-up modal shown on Stop when scenes were played.

  Per scene we display the count as: `current + pending_add - pending_delete`.
  - `+` increments pending_add
  - `-` first cancels a pending_add, then queues a pending_delete (which removes
    the scene's most recent existing cumshot when committed)
  - Can't go below 0

  On commit: for each scene, run any pending deletes (which sync to Stash via
  the delete path) then any pending adds (which sync via the add path).
  Then stop the session.
-->
<script lang="ts">
  import { w } from "$lib/wording.svelte";
  import { onDestroy, onMount } from "svelte";
  import { api, dayKey, dayKeyLabel, type PlayedScene, type Session, type OEvent } from "$lib/api";
  import Icon from "$lib/Icon.svelte";
  import SceneCard from "$lib/SceneCard.svelte";

  type Props = {
    sessionId: number;
    scenes: PlayedScene[];
    /** The active session being wrapped up. Passed so we can detect cross-midnight. */
    session: Session;
    /** Quit-guard mode: relabel commit to "Save and close", hide Discard, and
     *  make the secondary action "Go back" (onClose). commit() still stops the
     *  session; the caller exits the app in onCommitted afterwards. */
    quitMode?: boolean;
    onClose: () => void;
    onCommitted: () => void;
  };

  let { sessionId, scenes, session, quitMode = false, onClose, onCommitted }: Props = $props();

  // Wrap-up list mirrors the tracker / Overview active-session card: newest
  // scene at top. The `scenes` prop arrives ASC by first_seen_at (backend
  // order); reverse a copy without mutating the caller's array.
  const scenesNewestFirst = $derived([...scenes].reverse());

  // Split the list: "real" plays that crossed the play threshold (or already
  // carry a cumshot) always show; scenes opened but never watched long enough to
  // count AND with no cumshot are tucked behind a toggle (default hidden) - a
  // long session otherwise buries the real plays under a pile of brief opens.
  // A revealed below-threshold row still takes a cumshot, which counts it. The
  // partition is by AT-MOUNT state (cumshot_count, not the live staged `after`)
  // so a row doesn't jump groups mid-edit when you stage a cumshot on it.
  const wasBelow = (sc: PlayedScene) => sc.counted_at === null && sc.cumshot_count === 0;
  const countedRows = $derived(scenesNewestFirst.filter((sc) => !wasBelow(sc)));
  const belowRows = $derived(scenesNewestFirst.filter(wasBelow));
  let showBelow = $state(false);

  // Per-scene staged change: positive = adds, negative = deletes.
  let delta = $state<Record<number, number>>({});
  let submitting = $state(false);

  // Optional freeform note for this session, saved on commit.
  let notes = $state("");
  let notesOpen = $state(false);

  // Cumshots with no scene (content_item_id null) already on this session, plus a
  // staged +/- delta applied on commit - mirrors the per-scene counter. These are
  // never pushed to Stash (nothing to attach to).
  let unlinkedOEvents = $state<OEvent[]>([]);
  let unlinkedDelta = $state(0);
  const unlinkedCurrent = $derived(unlinkedOEvents.length);
  const unlinkedAfter = $derived(unlinkedCurrent + unlinkedDelta);
  function adjustUnlinked(step: number) {
    const next = unlinkedDelta + step;
    if (unlinkedCurrent + next < 0) return;
    unlinkedDelta = next;
  }
  async function loadUnlinked() {
    try {
      unlinkedOEvents = (await api.oListForSession(sessionId)).filter((o) => o.content_item_id == null);
    } catch {
      unlinkedOEvents = [];
    }
  }
  onMount(loadUnlinked);

  // Cross-midnight day picker.
  // Compute the two candidate day-keys: start day (session.assigned_day) and end day (now).
  const startDayKey = $derived(session.assigned_day);
  const endDayKey = $derived(dayKey(new Date()));
  const spansMidnight = $derived(startDayKey !== endDayKey);
  // User's chosen day for this session. Defaults to start (seeded once; the
  // $effect below keeps it synced, so the initial-value capture is intended).
  // svelte-ignore state_referenced_locally
  let chosenDayKey = $state<string>(session.assigned_day);
  // Keep chosenDayKey in sync if session updates.
  $effect(() => { chosenDayKey = session.assigned_day; });

  function adjust(contentItemId: number, currentCount: number, step: number) {
    const cur = delta[contentItemId] ?? 0;
    const next = cur + step;
    // Total can't go below zero. Stash-origin cumshots ARE removable now —
    // removing them on commit pushes a decrement back to Stash.
    if (currentCount + next < 0) return;
    delta = { ...delta, [contentItemId]: next };
  }

  const totalCurrent = $derived(scenes.reduce((sum, sc) => sum + sc.cumshot_count, 0) + unlinkedCurrent);
  const totalDelta = $derived(Object.values(delta).reduce((sum, n) => sum + n, 0) + unlinkedDelta);
  const totalAfter = $derived(totalCurrent + totalDelta);
  const totalAdds = $derived(Object.values(delta).reduce((s, n) => s + Math.max(0, n), 0) + Math.max(0, unlinkedDelta));
  const totalDeletes = $derived(Object.values(delta).reduce((s, n) => s + Math.max(0, -n), 0) + Math.max(0, -unlinkedDelta));

  async function commit() {
    if (submitting) return;
    submitting = true;
    try {
      // Day reassignment (if user chose a different day in the picker).
      if (chosenDayKey !== session.assigned_day) {
        await api.sessionSetAssignedDay(sessionId, chosenDayKey);
      }
      // Process deletes first (so total doesn't accidentally hit a constraint),
      // then adds.
      for (const sc of scenes) {
        const d = delta[sc.content_item_id] ?? 0;
        if (d < 0) {
          await api.oDeleteRecentForScene(sessionId, sc.content_item_id, -d);
        }
      }
      for (const sc of scenes) {
        const d = delta[sc.content_item_id] ?? 0;
        if (d > 0) {
          for (let i = 0; i < d; i++) {
            await api.oLog({
              sessionId,
              contentItemId: sc.content_item_id,
              occurredAt: Date.now() - (d - 1 - i) * 1000,
            });
          }
        }
      }
      // Unlinked cumshots (no scene): deletes peel the most-recent ones; adds log
      // new session-attached scene-less O's. No Stash push (content_item_id null).
      if (unlinkedDelta < 0) {
        for (const o of unlinkedOEvents.slice(unlinkedDelta)) {
          await api.oDelete(o.id);
        }
      } else if (unlinkedDelta > 0) {
        for (let i = 0; i < unlinkedDelta; i++) {
          await api.oLog({ sessionId, occurredAt: Date.now() - (unlinkedDelta - 1 - i) * 1000 });
        }
      }
      // Optional session note.
      const noteTrim = notes.trim();
      if (noteTrim) {
        try { await api.sessionUpdate(sessionId, { notes: noteTrim }); }
        catch (e) { console.error("save note failed", e); }
      }
      await api.sessionStop();
      onCommitted();
    } catch (e) {
      console.error("commit failed", e);
      submitting = false;
    }
  }

  // ---- Discard: replaces the old "skip" path ----
  // Two-tap confirmation. First tap arms; second tap within 3s commits.
  // After 3s without a confirm the button reverts. The morph happens in
  // CSS (`.discard.armed`) — no extra modal stacking.
  let discardArmed = $state(false);
  let discardArmTimer: ReturnType<typeof setTimeout> | null = null;

  function discardTap() {
    if (submitting) return;
    if (!discardArmed) {
      discardArmed = true;
      discardArmTimer = setTimeout(() => {
        discardArmed = false;
        discardArmTimer = null;
      }, 3000);
      return;
    }
    // Armed + tapped again → commit.
    if (discardArmTimer) { clearTimeout(discardArmTimer); discardArmTimer = null; }
    commitDiscard();
  }

  async function commitDiscard() {
    if (submitting) return;
    submitting = true;
    try {
      // Day reassignment is irrelevant on discard — the session is going to
      // be filtered out of every analytic query anyway. Skip it.
      await api.sessionDiscard();
      onCommitted();
    } catch (e) {
      console.error("discard failed", e);
      submitting = false;
      discardArmed = false;
    }
  }

  // Modal can be dismissed (Esc / overlay click) mid-arm. Clear the timer
  // so we don't fire a state update on a torn-down component.
  onDestroy(() => {
    if (discardArmTimer) { clearTimeout(discardArmTimer); discardArmTimer = null; }
  });
</script>

<!-- svelte-ignore a11y_click_events_have_key_events a11y_no_noninteractive_element_interactions -->
<div class="overlay" role="dialog" aria-modal="true" tabindex="-1" onkeydown={(e) => { if (e.key === 'Escape') onClose(); }} onclick={onClose}>
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <div class="modal" role="document" onclick={(e) => e.stopPropagation()}>
    <header>
      <h2>Wrap up this session</h2>
      <p class="subtitle">
        {#if totalCurrent === 0}
          {w("No cumshots logged yet. Add one with the cumshot button, or just stop if none happened.")}
        {:else}
          {totalCurrent} {w(totalCurrent === 1 ? "cumshot" : "cumshots")} already logged. Adjust counts below if needed.
        {/if}
      </p>
    </header>

    <div class="body">
    {#if spansMidnight}
      <div class="day-picker">
        <div class="day-picker-label">This session crossed midnight. Which day should it count towards?</div>
        <div class="day-picker-options">
          <label class="day-option" class:selected={chosenDayKey === startDayKey}>
            <input type="radio" name="assigned-day" value={startDayKey} bind:group={chosenDayKey} />
            <span class="day-option-main">{dayKeyLabel(startDayKey)}</span>
            <span class="day-option-meta">started</span>
          </label>
          <label class="day-option" class:selected={chosenDayKey === endDayKey}>
            <input type="radio" name="assigned-day" value={endDayKey} bind:group={chosenDayKey} />
            <span class="day-option-main">{dayKeyLabel(endDayKey)}</span>
            <span class="day-option-meta">ended</span>
          </label>
        </div>
      </div>
    {/if}

    {#snippet sceneRow(sc: PlayedScene)}
      {@const change = delta[sc.content_item_id] ?? 0}
      {@const after = sc.cumshot_count + change}
      {@const stashOrigin = sc.cumshot_count - sc.climax_cumshot_count}
      {@const canRemove = after > 0}
      <!-- Removals peel Climax-origin cumshots off first, then Stash-origin
           (matches the backend's `(origin='climax') DESC` ordering), so the
           "via Stash" label below counts down only once staged deletes exceed
           the Climax-origin count. -->
      {@const stagedDeletes = Math.max(0, -change)}
      {@const stashRemaining = stashOrigin - Math.max(0, stagedDeletes - sc.climax_cumshot_count)}
      <!-- "below threshold" = scene didn't watch long enough to count AND no
           cumshot is logged on it (live or staged). Tapping + un-dims it live. -->
      {@const belowThreshold = sc.counted_at === null && after === 0}
      <li class="scene" class:below-threshold={belowThreshold}>
        <div class="scene-card-wrap">
          <SceneCard scene={sc} pendingCumshots={change} compact={true} />
        </div>
        <div class="counter-side">
          <div class="counter" class:has-count={after > 0} class:has-delta={change !== 0}>
            <button
              class="counter-btn"
              disabled={!canRemove || submitting}
              onclick={() => adjust(sc.content_item_id, sc.cumshot_count, -1)}
              aria-label={w("Remove a cumshot from this scene")}
            >-</button>
            <span class="count">
              {after}
              {#if change !== 0}
                <span class="delta" class:positive={change > 0} class:negative={change < 0}>
                  {change > 0 ? `+${change}` : change}
                </span>
              {/if}
            </span>
            <button
              class="counter-btn add"
              disabled={submitting}
              onclick={() => adjust(sc.content_item_id, sc.cumshot_count, 1)}
              aria-label={w("Add a cumshot to this scene")}
            >
              <Icon name="cumshot" size={16} color="var(--accent)" filled />
            </button>
          </div>
          {#if sc.off_bridge}
            <span
              class="offbridge-note"
              title="Tracked from Stash's play history, not the browser bridge - e.g. watched on a TV or phone app."
            >off-bridge</span>
          {/if}
          {#if stashRemaining > 0}
            <span class="stash-note" title="{stashRemaining} of these came from Stash's O button. Removing them here removes them from Stash too.">
              {stashRemaining} from Stash
            </span>
          {/if}
          {#if belowThreshold}
            <span
              class="threshold-note"
              title={w("This scene was open less than your play threshold. It won't be counted as a play unless you log a cumshot on it.")}
            >below threshold</span>
          {/if}
        </div>
      </li>
    {/snippet}

    {#if countedRows.length > 0}
      <ul class="scene-list">
        {#each countedRows as sc (sc.content_item_id)}
          {@render sceneRow(sc)}
        {/each}
      </ul>
    {/if}

    <!-- Below-threshold scenes (opened but not watched long enough to count, no
         cumshot) are hidden by default so a long session's real plays aren't
         buried. Reveal to log a cumshot on any of them. -->
    {#if belowRows.length > 0}
      <button class="below-toggle" onclick={() => (showBelow = !showBelow)}>
        <Icon name={showBelow ? "chevron-up" : "chevron-down"} size={14} />
        <span>{showBelow ? "Hide" : "Show"} {belowRows.length} scene{belowRows.length === 1 ? "" : "s"} below the threshold</span>
      </button>
      {#if showBelow}
        <ul class="scene-list below-list">
          {#each belowRows as sc (sc.content_item_id)}
            {@render sceneRow(sc)}
          {/each}
        </ul>
      {/if}
    {/if}

    <div class="unlinked-wrap">
      <div class="unlinked-meta">
        <Icon name="cumshot" size={15} filled color="var(--accent)" />
        <div class="unlinked-text">
          <span class="unlinked-title">{w("Cumshots with no scene")}</span>
          <span class="unlinked-sub">Not tied to anything in Stash. Never synced.</span>
        </div>
      </div>
      <div class="counter" class:has-count={unlinkedAfter > 0} class:has-delta={unlinkedDelta !== 0}>
        <button
          class="counter-btn"
          disabled={unlinkedAfter === 0 || submitting}
          onclick={() => adjustUnlinked(-1)}
          aria-label={w("Remove an unlinked cumshot")}
        >-</button>
        <span class="count">
          {unlinkedAfter}
          {#if unlinkedDelta !== 0}
            <span class="delta" class:positive={unlinkedDelta > 0} class:negative={unlinkedDelta < 0}>
              {unlinkedDelta > 0 ? `+${unlinkedDelta}` : unlinkedDelta}
            </span>
          {/if}
        </span>
        <button
          class="counter-btn add"
          disabled={submitting}
          onclick={() => adjustUnlinked(1)}
          aria-label={w("Add an unlinked cumshot")}
        >
          <Icon name="cumshot" size={16} color="var(--accent)" filled />
        </button>
      </div>
    </div>

    <div class="notes-row">
      {#if notesOpen || notes}
        <span class="notes-label">Session note</span>
        <textarea
          class="notes-input"
          bind:value={notes}
          placeholder="Anything worth remembering about this session (optional)"
          rows="2"
        ></textarea>
      {:else}
        <button class="notes-toggle" onclick={() => (notesOpen = true)}>
          <Icon name="edit" size={13} />
          <span>Add a note</span>
        </button>
      {/if}
    </div>
    </div>

    <footer>
      <div class="summary">
        {#if totalDelta === 0}
          {totalCurrent > 0
            ? w(`${totalCurrent} cumshot${totalCurrent === 1 ? "" : "s"} this session`)
            : w("No cumshots to log")}
        {:else}
          <strong class:positive={totalDelta > 0} class:negative={totalDelta < 0}>
            {totalAdds > 0 && totalDeletes > 0
              ? `+${totalAdds} / -${totalDeletes}`
              : totalDelta > 0 ? `+${totalDelta}` : `-${-totalDelta}`}
          </strong>
          <span class="muted">({totalAfter} total)</span>
        {/if}
      </div>
      <div class="actions">
        {#if quitMode}
          <!-- Quit guard: no discard. "Go back" abandons the quit (session keeps
               running); "Save and close" stops the session then exits the app. -->
          <button disabled={submitting} onclick={onClose}>Go back</button>
          <button class="commit" disabled={submitting} onclick={commit}>
            {submitting ? "Saving..." : "Save and close"}
          </button>
        {:else}
          <!-- Discard replaces the old "Skip" path. Two taps required: first
               arms (.armed style), second commits within 3s. Reverts on its
               own after the timeout. Deliberately styled scarier on the
               second tap so accidental discards are hard to fire. -->
          <button
            class="discard"
            class:armed={discardArmed}
            disabled={submitting}
            onclick={discardTap}
            title="Discard this session - kept in the database but excluded from every dashboard view."
          >
            {discardArmed ? "Discard? Click again" : "Discard session"}
          </button>
          <button class="commit" disabled={submitting} onclick={commit}>
            {submitting ? "Saving..." : totalDelta !== 0 ? "Apply and stop" : "Stop session"}
          </button>
        {/if}
      </div>
    </footer>
  </div>
</div>

<style>
  .overlay {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.6);
    backdrop-filter: blur(4px);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 1000;
    animation: fade-in 160ms ease-out;
  }
  @keyframes fade-in { from { opacity: 0 } to { opacity: 1 } }

  .modal {
    background: var(--bg-card);
    border: 1px solid var(--border);
    border-radius: 14px;
    width: min(560px, 92vw);
    max-height: 86vh;
    display: flex;
    flex-direction: column;
    box-shadow: 0 24px 80px rgba(0, 0, 0, 0.55);
    overflow: hidden;
    animation: pop-in 220ms cubic-bezier(0.2, 0.8, 0.2, 1);
  }
  @keyframes pop-in {
    from { transform: translateY(8px) scale(0.97); opacity: 0; }
    to   { transform: none;                        opacity: 1; }
  }

  /* Everything between the pinned header and footer scrolls as one region, so the
     Stop/commit buttons are ALWAYS reachable no matter how short the window is. */
  .body {
    flex: 1 1 auto;
    min-height: 0;
    overflow-y: auto;
  }

  header {
    flex-shrink: 0;
    padding: 24px 24px 12px;
    border-bottom: 1px solid var(--border);
  }
  header h2 {
    margin: 0 0 6px;
    font-size: 18px;
    font-weight: 600;
    color: var(--fg-strong);
  }
  .subtitle {
    margin: 0;
    font-size: 13px;
    color: var(--fg-muted);
    line-height: 1.5;
  }

  .day-picker {
    padding: 14px 24px;
    background: var(--bg-elevated);
    border-bottom: 1px solid var(--border);
  }
  .day-picker-label {
    font-size: 12px;
    color: #b8bcc4;
    margin-bottom: 10px;
  }
  .day-picker-options {
    display: flex;
    gap: 8px;
  }
  .day-option {
    flex: 1;
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px 14px;
    border: 1px solid var(--border);
    border-radius: 8px;
    cursor: pointer;
    background: var(--bg-card-hover);
    transition: border-color 120ms, background 120ms;
  }
  .day-option:hover { border-color: var(--border-strong); }
  .day-option.selected {
    border-color: var(--accent);
    background: var(--accent-soft);
  }
  .day-option input { accent-color: var(--accent); cursor: pointer; }
  .day-option-main {
    flex: 1;
    font-size: 13px;
    font-weight: 600;
    color: var(--fg);
  }
  .day-option-meta {
    font-size: 10px;
    color: var(--fg-muted);
    text-transform: uppercase;
    letter-spacing: 0.1em;
  }

  /* Below-threshold reveal toggle: a quiet full-width ghost button between the
     counted list and the hidden below-threshold rows. Matches the modal's
     ghost-button language (like "Add a note"). */
  .below-toggle {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 7px;
    width: calc(100% - 32px);
    margin: 2px 16px 4px;
    padding: 8px;
    background: transparent;
    border: 1px dashed var(--border);
    border-radius: 8px;
    color: var(--fg-muted);
    font-family: inherit;
    font-size: 12px;
    font-weight: 500;
    cursor: pointer;
    transition: background 120ms, border-color 120ms, color 120ms;
  }
  .below-toggle:hover { background: var(--bg-card-hover); border-color: var(--border-strong); color: var(--fg); }
  .scene-list.below-list { padding-top: 0; }

  .scene-list {
    list-style: none;
    margin: 0;
    padding: 8px 16px;
  }
  .scene {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 10px 8px;
    border-bottom: 1px solid var(--border-subtle);
    gap: 12px;
  }
  .scene:last-child { border-bottom: none; }
  /* Left column hosts a compact SceneCard (thumbnail + title + pills).
     min-width: 0 so the inner card's flex children can ellipsis instead
     of forcing the row to grow. */
  .scene-card-wrap {
    flex: 1;
    min-width: 0;
  }
  /* Right column stacks the counter on top of an optional "via Stash"
     footnote showing how many of the scene's cumshots came from Stash's O
     button. These are removable from here now — doing so also decrements
     Stash — and the count ticks down as you stage those removals. */
  .counter-side {
    display: flex;
    flex-direction: column;
    align-items: flex-end;
    gap: 4px;
    flex-shrink: 0;
  }
  .stash-note {
    background: var(--accent-soft, #ff647014);
    border: 1px solid var(--accent, #ff647040);
    color: var(--accent, #ff6470);
    padding: 1px 7px;
    border-radius: 8px;
    font-size: 10px;
    font-weight: 600;
    white-space: nowrap;
    cursor: help;
  }
  /* Off-bridge source tag: soft-coral pill (same "origin" language as the
     "N from Stash" note), marking a row tracked via the poll path, not the
     bridge - a TV / phone app. Calm metadata, so the soft tint, not loud. */
  .offbridge-note {
    background: var(--accent-soft, #ff647014);
    border: 1px solid var(--coral-a-24, rgba(239, 107, 122, 0.24));
    color: var(--coral-300, #faa8b1);
    padding: 1px 7px;
    border-radius: 8px;
    font-size: 10px;
    font-weight: 600;
    white-space: nowrap;
    cursor: help;
  }
  /* Below-threshold rows: SceneCard side dims to make clear "we won't
     log this", counter side stays fully readable so the user can still
     log a cumshot if they want. Tapping + un-dims live (the row's
     belowThreshold = false once `after` > 0). */
  .scene.below-threshold .scene-card-wrap {
    opacity: 0.45;
    transition: opacity 160ms ease;
  }
  .scene.below-threshold .scene-card-wrap:hover {
    opacity: 0.7;
  }
  .threshold-note {
    color: var(--text-muted, #8a909a);
    font-size: 10px;
    font-style: italic;
    font-weight: 500;
    white-space: nowrap;
    cursor: help;
    letter-spacing: 0.01em;
  }

  /* Unlinked-cumshot row: a session footer item below the scenes, same counter
     widget as the scene rows. Lets you add/remove scene-less cumshots at wrap-up. */
  .unlinked-wrap {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    margin: 0 16px 8px;
    padding: 10px 8px;
    border-top: 1px solid var(--border);
  }
  .unlinked-meta { display: flex; align-items: center; gap: 10px; min-width: 0; }
  .unlinked-text { display: flex; flex-direction: column; min-width: 0; }
  .unlinked-title { font-size: 13px; font-weight: 600; color: var(--fg); }
  .unlinked-sub { font-size: 11px; color: var(--fg-muted); }

  /* Optional session note. Collapsed to a button until opened/typed. */
  .notes-row {
    margin: 0 16px 8px;
    padding: 4px 8px 10px;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .notes-toggle {
    align-self: flex-start;
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 6px 12px;
    background: transparent;
    border: 1px solid var(--border);
    border-radius: 8px;
    color: var(--fg-muted);
    font-family: inherit;
    font-size: 12px;
    font-weight: 500;
    cursor: pointer;
    transition: background 120ms, border-color 120ms, color 120ms;
  }
  .notes-toggle:hover { background: var(--bg-card-hover); border-color: var(--border-strong); color: var(--fg); }
  .notes-label {
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.12em;
    font-weight: 600;
    color: var(--fg-muted);
  }
  .notes-input {
    width: 100%;
    box-sizing: border-box;
    resize: vertical;
    min-height: 48px;
    padding: 8px 10px;
    background: var(--bg-card-hover);
    border: 1px solid var(--border);
    border-radius: 8px;
    color: var(--fg);
    font-family: inherit;
    font-size: 13px;
    line-height: 1.5;
    transition: border-color 120ms;
  }
  .notes-input:focus { outline: none; border-color: var(--accent); }
  .notes-input::placeholder { color: var(--fg-subtle); }

  .counter {
    display: flex;
    align-items: center;
    gap: 4px;
    background: var(--bg-card-hover);
    border: 1px solid var(--border);
    border-radius: 10px;
    padding: 3px;
    transition: border-color 140ms, background 140ms;
  }
  .counter.has-count {
    border-color: #f0f2f588;
    background: #f0f2f514;
  }
  .counter.has-delta {
    border-color: var(--cumshot, #f0f2f5);
    background: #f0f2f51c;
  }
  .counter-btn {
    width: 32px;
    height: 32px;
    border: none;
    background: transparent;
    color: var(--fg);
    border-radius: 7px;
    font-size: 16px;
    font-weight: 600;
    cursor: pointer;
    transition: background 100ms, transform 80ms;
    display: flex;
    align-items: center;
    justify-content: center;
    font-family: inherit;
  }
  .counter-btn:hover:not(:disabled) { background: var(--border); }
  .counter-btn:active:not(:disabled) { transform: scale(0.92); }
  .counter-btn:disabled { color: #4a4e58; cursor: default; }
  .counter-btn.add {
    font-size: 16px;
  }
  .counter-btn.add:hover:not(:disabled) { background: #f0f2f518; }
  .count {
    min-width: 42px;
    text-align: center;
    font-variant-numeric: tabular-nums;
    font-weight: 600;
    font-size: 14px;
    color: var(--fg);
    padding: 0 4px;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 4px;
  }
  .delta {
    font-size: 10px;
    font-weight: 700;
  }
  .delta.positive { color: var(--cumshot, #f0f2f5); }
  .delta.negative { color: var(--warn); }

  footer {
    flex-shrink: 0;
    padding: 16px 24px;
    border-top: 1px solid var(--border);
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 16px;
    background: var(--bg-elevated);
  }
  .summary {
    font-size: 13px;
    color: var(--fg-muted);
  }
  .summary strong {
    font-variant-numeric: tabular-nums;
    font-size: 14px;
  }
  .summary strong.positive { color: var(--cumshot, #f0f2f5); }
  .summary strong.negative { color: var(--warn); }
  .summary .muted { color: var(--fg-subtle); font-size: 12px; margin-left: 4px; }
  .actions { display: flex; gap: 8px; }
  .actions button {
    padding: 8px 16px;
    border-radius: 8px;
    font-size: 13px;
    font-weight: 500;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg);
    cursor: pointer;
    transition: background 120ms, border-color 120ms, color 120ms;
    font-family: inherit;
  }
  .actions button:hover:not(:disabled) { border-color: var(--border-strong); background: var(--bg-card-hover); }
  /* Discard — armed state turns it scary red so the second click feels
     committed. Width is fixed so the label change ("Discard session" →
     "Discard? Click again") doesn't jiggle the layout. */
  .actions .discard {
    color: #f87171;
    border-color: #f8717140;
    min-width: 168px;
  }
  .actions .discard:hover:not(:disabled) {
    background: #f8717110;
    border-color: #f87171;
    color: #fca5a5;
  }
  .actions .discard.armed {
    background: #ef4444;
    border-color: #ef4444;
    color: #fff;
  }
  .actions .discard.armed:hover { background: #dc2626; border-color: #dc2626; }
  .actions .commit {
    background: var(--accent);
    border-color: var(--accent);
    color: var(--accent-fg);
  }
  .actions .commit:hover:not(:disabled) { background: var(--accent-hover); border-color: var(--accent-hover); }
  .actions button:disabled { opacity: 0.6; cursor: default; }
</style>
