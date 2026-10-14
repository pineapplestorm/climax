<!--
  UnlinkedQuickLog - the "log a cumshot with no scene" button for the Overview
  session card. The dashboard's counterpart of the droplet button in the tracker
  window's header, and deliberately the same control: a button with a count
  badge that opens a small minus / count / plus popover.

  It exists because a browser has no tracker window. Without this, someone
  using only the server could add a scene-less cumshot at the wrap-up or by
  editing a session afterwards, but never at the moment it happened - and the
  wrap-up stamps the stop time, not the real one.

  Works in both states, like the tracker's:
    - session running -> counts and logs that session's scene-less cumshots
    - no session      -> counts and logs TODAY's session-less ones

  Each press logs at the current time (the backend stamps it), and nothing is
  ever sent to Stash, since there is no scene to attach it to.
-->
<script lang="ts">
  import { w } from "$lib/wording.svelte";
  import { api, type OEvent, type Session } from "$lib/api";
  import Icon from "$lib/Icon.svelte";

  type Props = {
    /** The live session, or null when none is running. */
    session: Session | null;
    /** Changes whenever the parent's poll runs, so the count stays right when a
     *  cumshot is logged somewhere else (the tracker window, another device). */
    refreshKey?: number;
    /** Called after a log or a removal, so the parent can refresh its own
     *  session totals straight away. */
    onChanged?: () => void | Promise<void>;
  };
  let { session, refreshKey = 0, onChanged }: Props = $props();

  let list = $state<OEvent[]>([]);
  let open = $state(false);
  let wrapEl = $state<HTMLElement | undefined>(undefined);
  const count = $derived(list.length);

  // A slow answer for a session that has since ended must not overwrite the
  // idle list (or the other way round).
  let loadSeq = 0;
  async function fetchList(id: number | null): Promise<OEvent[]> {
    return id !== null
      ? (await api.oListForSession(id)).filter((o) => o.content_item_id == null)
      : await api.oSessionlessUnlinkedToday();
  }
  async function load() {
    const seq = ++loadSeq;
    try {
      const next = await fetchList(session?.id ?? null);
      if (seq === loadSeq) list = next;
    } catch (e) {
      console.error("unlinked cumshots refresh failed", e);
    }
  }

  $effect(() => {
    void session?.id;
    void refreshKey;
    void load();
  });

  // Every press counts. Presses are queued and applied one after another
  // instead of being ignored while an earlier one is still in flight: three
  // quick taps on + must log three, never one. Each job reads the list back
  // itself (and cancels any poll read still in flight) so the next job in the
  // queue always acts on what the backend actually holds.
  let queue: Promise<void> = Promise.resolve();
  function enqueue(id: number | null, job: () => Promise<unknown>) {
    queue = queue.then(async () => {
      try {
        await job();
      } catch (e) {
        console.error("unlinked cumshot change failed", e);
      }
      try {
        const fresh = await fetchList(id);
        ++loadSeq;
        list = fresh;
      } catch (e) {
        console.error("unlinked cumshots refresh failed", e);
      }
      // Not awaited: the parent's own refresh must never hold up the next press.
      void onChanged?.();
    });
  }

  // The session is captured when the button is PRESSED, so a press made during
  // a session still belongs to it if the queue runs a moment later.
  function add() {
    const id = session?.id ?? null;
    enqueue(id, () => api.oLog({ sessionId: id }));
  }

  function removeLast() {
    const id = session?.id ?? null;
    enqueue(id, async () => {
      const last = list[list.length - 1];
      if (last) await api.oDelete(last.id);
    });
  }

  // pointerdown, not click: it fires before any re-render, so the pressed node
  // still has its ancestors when we test whether the press landed inside.
  $effect(() => {
    if (!open) return;
    const onDown = (e: PointerEvent) => {
      if (wrapEl && !e.composedPath().includes(wrapEl)) open = false;
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") open = false;
    };
    document.addEventListener("pointerdown", onDown);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("pointerdown", onDown);
      document.removeEventListener("keydown", onKey);
    };
  });
</script>

<div class="uql" bind:this={wrapEl}>
  <button
    class="uql-btn"
    class:on={open}
    onclick={() => (open = !open)}
    title={w("Log a cumshot with no scene")}
    aria-label={w("Log a cumshot with no scene")}
    aria-expanded={open}
  >
    <Icon name="cumshot" size={15} filled color="var(--accent)" />
    {#if count > 0}<span class="uql-badge">{count}</span>{/if}
  </button>
  {#if open}
    <div class="uql-popover" role="group" aria-label={w("Cumshots with no scene")}>
      <div class="uql-head">
        <span class="uql-title">{w("Cumshots with no scene")}</span>
        <span class="uql-scope">{session ? "this session" : "today"}</span>
      </div>
      <div class="uql-stepper">
        <button
          class="uql-step minus"
          disabled={count === 0}
          onclick={removeLast}
          title="Remove the last one"
          aria-label="Remove the last one"
        >
          <Icon name="minus" size={14} />
        </button>
        <span class="uql-count">{count}</span>
        <button
          class="uql-step"
          onclick={add}
          title={w("Log a cumshot not tied to any scene")}
          aria-label={w("Log a cumshot not tied to any scene")}
        >
          <Icon name="cumshot" size={16} color="var(--accent)" filled />
        </button>
      </div>
      <div class="uql-note">These have no scene attached, so they are never sent to Stash.</div>
    </div>
  {/if}
</div>

<style>
  .uql { position: relative; display: flex; }

  /* Sized to sit level with the card's Pause / Stop / Start buttons. */
  .uql-btn {
    position: relative;
    width: 32px;
    height: 32px;
    padding: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    border-radius: 7px;
    border: 1px solid var(--border);
    background: transparent;
    cursor: pointer;
    font-family: inherit;
    transition: background 120ms, border-color 120ms, transform 80ms;
  }
  .uql-btn:hover { background: var(--bg-card-hover); border-color: #3a3e4a; }
  .uql-btn:active { transform: translateY(1px); }
  .uql-btn.on { background: var(--accent-soft); border-color: var(--accent); }

  .uql-badge {
    position: absolute;
    top: -5px;
    right: -5px;
    min-width: 15px;
    height: 15px;
    padding: 0 3px;
    box-sizing: border-box;
    border-radius: 8px;
    background: var(--accent);
    color: #fff;
    font-size: 9px;
    font-weight: 700;
    line-height: 15px;
    text-align: center;
    font-variant-numeric: tabular-nums;
  }

  .uql-popover {
    position: absolute;
    top: calc(100% + 8px);
    right: 0;
    z-index: 50;
    display: flex;
    flex-direction: column;
    gap: 10px;
    width: max-content;
    padding: 12px 14px;
    background: var(--bg-card);
    border: 1px solid var(--border-strong);
    border-radius: 10px;
    box-shadow: 0 14px 36px rgba(0, 0, 0, 0.55);
    animation: uql-pop 140ms var(--ease-out);
  }
  @keyframes uql-pop {
    from { opacity: 0; transform: translateY(-4px); }
    to   { opacity: 1; transform: none; }
  }

  .uql-head {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 16px;
  }
  .uql-title { font-size: 12px; font-weight: 600; color: var(--text); }
  .uql-scope {
    font-size: 9px;
    color: var(--text-muted);
    text-transform: uppercase;
    letter-spacing: 0.12em;
  }

  .uql-stepper {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 10px;
  }
  .uql-count {
    min-width: 18px;
    text-align: center;
    font-weight: 600;
    font-size: 14px;
    color: var(--accent);
    font-variant-numeric: tabular-nums;
  }
  .uql-step {
    width: 32px;
    height: 32px;
    padding: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    background: var(--bg-card-hover);
    border: 1px solid var(--border);
    border-radius: 8px;
    color: var(--text-muted);
    cursor: pointer;
    line-height: 1;
    font-family: inherit;
    flex-shrink: 0;
    transition: background 120ms, border-color 120ms, transform 80ms, color 120ms;
  }
  .uql-step:hover:not(:disabled) { background: var(--accent-soft); border-color: var(--accent); }
  .uql-step.minus:hover:not(:disabled) { color: var(--red); border-color: var(--red); background: #f8717118; }
  .uql-step:active:not(:disabled) { transform: scale(0.9); }
  .uql-step:disabled { opacity: 0.4; cursor: default; }

  .uql-note { font-size: 10px; color: var(--text-muted); text-align: center; }
</style>
