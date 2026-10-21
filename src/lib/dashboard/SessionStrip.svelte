<!--
  Horizontal time-of-day strip showing every session on the selected day,
  positioned by start time + width = duration.

  X-axis: 00:00 (local midnight) to 24:00. Hour ticks every 4 hours.

  Each session row contains, layered:
    - background gridlines
    - the start-time label sitting ABOVE the bar (anchored to bar's
      left edge; flips to right-anchored if bar is near 24:00)
    - the bar itself — a pure coloured rectangle, no text inside, so
      narrow bars never get truncated
    - a cumshot chip floating just OUTSIDE the bar's right edge (flips
      to the left edge if the bar is near the strip's right end)
    - a hover popover above the bar with the full breakdown
      (start–end, duration, scenes, cumshots)

  Active session (still running) uses --live and pulses, with `now`
  as its right edge.
-->
<script lang="ts">
  import { pulse } from "$lib/pulse";
  import { w } from "$lib/wording.svelte";
  import { onMount, onDestroy } from "svelte";
  import { api, formatDuration, formatTimeOnly, type Session } from "$lib/api";
  import Icon from "$lib/Icon.svelte";

  type Props = {
    /** YYYY-MM-DD. */
    day: string;
    selectedSessionId: number | null;
    onSelect: (sessionId: number) => void;
  };
  let { day, selectedSessionId, onSelect }: Props = $props();

  let sessions = $state<Session[]>([]);
  let loading = $state(true);
  let now = $state(Date.now());
  let hoveredId = $state<number | null>(null);
  let poll: ReturnType<typeof setInterval> | null = null;
  let tick: ReturnType<typeof setInterval> | null = null;

  async function refresh() {
    try {
      sessions = await api.sessionsForDay(day);
    } catch (e) {
      console.error("sessions for day failed", e);
    } finally {
      loading = false;
    }
  }

  // When the `day` prop changes (user drilled into a different day),
  // refetch immediately.
  $effect(() => {
    day; // dependency
    loading = true;
    refresh();
  });

  onMount(() => {
    poll = setInterval(refresh, 5_000);
    tick = setInterval(() => { now = Date.now(); }, 1000);
  });
  onDestroy(() => {
    if (poll) clearInterval(poll);
    if (tick) clearInterval(tick);
  });

  // Day boundaries (local time).
  const dayStartMs = $derived.by(() => {
    const [y, m, d] = day.split("-").map((s) => parseInt(s, 10));
    return new Date(y, m - 1, d, 0, 0, 0, 0).getTime();
  });
  const dayEndMs = $derived(dayStartMs + 24 * 60 * 60 * 1000);
  const dayMs = $derived(dayEndMs - dayStartMs);

  function leftPercent(startedAt: number): number {
    const clamped = Math.max(dayStartMs, Math.min(dayEndMs, startedAt));
    return ((clamped - dayStartMs) / dayMs) * 100;
  }

  function widthPercent(startedAt: number, endedAt: number | null): number {
    const start = Math.max(dayStartMs, Math.min(dayEndMs, startedAt));
    const endRaw = endedAt ?? now;
    const end = Math.max(dayStartMs, Math.min(dayEndMs, endRaw));
    const w = ((end - start) / dayMs) * 100;
    return Math.max(0.4, w); // minimum visible width
  }

  const totalDurationMs = $derived(
    sessions.reduce((s, ss) => s + ss.effective_duration_ms, 0),
  );
  const totalCumshots = $derived(sessions.reduce((s, ss) => s + ss.o_count, 0));
  const totalScenes = $derived(sessions.reduce((s, ss) => s + ss.scene_count, 0));

  const hourTicks = [0, 4, 8, 12, 16, 20, 24];
</script>

<div class="strip-card">
  <div class="strip-head">
    <h2>Sessions on this day</h2>
    <div class="totals">
      <span><strong>{formatDuration(totalDurationMs)}</strong></span>
      <span>{sessions.length} session{sessions.length === 1 ? "" : "s"}</span>
      <span>{totalScenes} scene{totalScenes === 1 ? "" : "s"}</span>
      <span class="cumshot-count">{totalCumshots} {w(totalCumshots === 1 ? "cumshot" : "cumshots")}</span>
    </div>
  </div>

  {#if loading && sessions.length === 0}
    <div class="empty">loading...</div>
  {:else if sessions.length === 0}
    <div class="empty">Nothing logged on this day.</div>
  {:else}
    <div class="strip">
      <!-- Hour axis at top -->
      <div class="hour-axis">
        {#each hourTicks as h (h)}
          <div class="hour-tick" style={`left: ${(h / 24) * 100}%;`}>
            <span>{h.toString().padStart(2, "0")}:00</span>
          </div>
        {/each}
      </div>

      <!-- Vertical gridlines at each hour tick -->
      <div class="grid-rows">
        {#each sessions as s (s.id)}
          {@const isActive = s.status === "active"}
          {@const isSelected = selectedSessionId === s.id}
          {@const left = leftPercent(s.started_at)}
          {@const width = widthPercent(s.started_at, s.ended_at)}
          {@const rightEdge = left + width}
          {@const labelFlipRight = left > 70}
          {@const chipFlipLeft = rightEdge > 84}
          {@const popoverFlipRight = left > 60}
          <div class="row">
            <div class="row-gridlines">
              {#each hourTicks as h (h)}
                <div class="vgridline" style={`left: ${(h / 24) * 100}%;`}></div>
              {/each}
            </div>

            <!-- Start-time label above the bar -->
            <span
              class="bar-label"
              class:flip-right={labelFlipRight}
              style={labelFlipRight
                ? `right: ${Math.max(0, 100 - rightEdge)}%;`
                : `left: ${left}%;`}
            >{formatTimeOnly(s.started_at)}</span>

            <!-- The bar itself — no text, just a coloured rect -->
            <button
              class="session-bar"
              class:active={isActive} use:pulse={isActive}
              class:selected={isSelected}
              style={`left: ${left}%; width: ${width}%;`}
              onmouseenter={() => (hoveredId = s.id)}
              onmouseleave={() => (hoveredId = null)}
              onclick={() => onSelect(s.id)}
              aria-label={`Session starting ${formatTimeOnly(s.started_at)}, ${formatDuration(s.effective_duration_ms)}, ${s.o_count} ${w(s.o_count === 1 ? "cumshot" : "cumshots")}`}
            ></button>

            <!-- Cumshot chip floating outside the bar -->
            {#if s.o_count > 0}
              <span
                class="bar-chip"
                class:active-chip={isActive}
                style={chipFlipLeft
                  ? `right: ${Math.max(0, 100 - left) + 0.4}%;`
                  : `left: ${rightEdge + 0.4}%;`}
              >
                <Icon name="cumshot" size={10} filled />
                <span>{s.o_count}</span>
              </span>
            {/if}

            <!-- Themed hover popover with full breakdown -->
            {#if hoveredId === s.id}
              <div
                class="bar-popover"
                class:flip-right={popoverFlipRight}
                style={popoverFlipRight
                  ? `right: ${Math.max(0, 100 - rightEdge)}%;`
                  : `left: ${left}%;`}
              >
                <div class="pop-time">
                  {formatTimeOnly(s.started_at)} - {s.ended_at ? formatTimeOnly(s.ended_at) : "now"}
                </div>
                <div class="pop-stats">
                  <span class="pop-dur">{formatDuration(s.effective_duration_ms)}</span>
                  <span class="pop-sep">•</span>
                  <span>{s.scene_count} scene{s.scene_count === 1 ? "" : "s"}</span>
                  <span class="pop-sep">•</span>
                  <span class="pop-cumshots">
                    <Icon name="cumshot" size={10} filled />
                    {s.o_count} {w(s.o_count === 1 ? "cumshot" : "cumshots")}
                  </span>
                </div>
                {#if isActive}
                  <div class="pop-active">● live</div>
                {/if}
              </div>
            {/if}
          </div>
        {/each}
      </div>
    </div>
  {/if}
</div>

<style>
  .strip-card {
    background: var(--bg-card);
    border: 1px solid var(--border);
    border-radius: var(--radius-lg, 10px);
    padding: 18px 22px 16px;
  }

  .strip-head {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 14px;
    gap: 12px;
  }
  .strip-head h2 {
    margin: 0;
    font-size: 13px;
    color: var(--text);
    font-weight: 600;
    letter-spacing: -0.01em;
  }
  .totals {
    display: flex;
    gap: 12px;
    font-size: 12px;
    color: var(--text-muted);
    font-variant-numeric: tabular-nums;
    align-items: center;
  }
  .totals strong { color: var(--text); font-size: 14px; }
  .totals .cumshot-count { color: var(--accent); font-weight: 600; }

  .empty {
    text-align: center;
    padding: 32px 0;
    color: var(--text-muted);
    font-size: 13px;
  }

  .strip {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .hour-axis {
    position: relative;
    height: 18px;
    margin-bottom: 6px;
  }
  .hour-tick {
    position: absolute;
    top: 0;
    transform: translateX(-50%);
    font-size: 10px;
    color: var(--text-muted);
    font-variant-numeric: tabular-nums;
    letter-spacing: 0.05em;
  }
  .hour-tick:first-child { transform: translateX(0); }
  .hour-tick:last-child { transform: translateX(-100%); }

  .grid-rows {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  /* Each row holds: gridlines (back), start-time label (above bar),
     bar (mid), cumshot chip (floating next to bar), popover (overlay). */
  .row {
    position: relative;
    height: 44px;
    background: var(--bg);
    border-radius: 6px;
  }
  .row-gridlines {
    position: absolute;
    inset: 0;
    pointer-events: none;
  }
  .vgridline {
    position: absolute;
    top: 0;
    bottom: 0;
    width: 1px;
    background: var(--border-subtle, #1C1F26);
  }
  .vgridline:first-child, .vgridline:last-child { background: transparent; }

  /* Start-time label — sits above the bar, anchored to bar's left edge
     (flips to right-anchored when bar is near 24:00). Never gets clipped. */
  .bar-label {
    position: absolute;
    top: 5px;
    font-size: 10px;
    font-weight: 500;
    color: var(--text-muted);
    font-variant-numeric: tabular-nums;
    letter-spacing: 0.04em;
    line-height: 1;
    white-space: nowrap;
    pointer-events: none;
  }
  .bar-label.flip-right { text-align: right; }

  /* The bar — pure visual, no text inside. */
  .session-bar {
    position: absolute;
    top: 20px;
    height: 20px;
    background: var(--accent);
    border: 1px solid var(--accent);
    border-radius: 4px;
    padding: 0;
    cursor: pointer;
    transition: filter var(--dur-2, 120ms) var(--ease-out),
                box-shadow var(--dur-2, 120ms) var(--ease-out);
  }
  .session-bar:hover {
    filter: brightness(1.08);
    box-shadow: 0 0 0 3px var(--accent-glow, rgba(239, 107, 122, 0.24));
  }
  .session-bar.selected {
    outline: 2px solid var(--fg-strong, #fff);
    outline-offset: -1px;
  }
  .session-bar.active {
    background: var(--live, #4ade80);
    border-color: var(--live, #4ade80);
  }
  /* A soft halo that breathes around the live bar. Opacity only, so the GPU
     composites it without repainting. (A bar is not round, so the dots' grow-
     and-fade cannot be copied with a scale - it would spread far wider than
     tall - hence a fixed 4px halo fading in and out instead.) */
  .session-bar.active::after {
    content: "";
    position: absolute;
    inset: -4px;
    border-radius: 8px;
    background: var(--live, #4ade80);
    pointer-events: none;
    will-change: opacity;
    opacity: calc(0.22 * var(--cx-pulse, 0));
  }

  /* Cumshot chip — sits just outside the bar's right edge (or left,
     if bar is near the strip's right end). Vertically centred on bar. */
  .bar-chip {
    position: absolute;
    /* The bar's vertical CENTRE is 30px (.session-bar is top 20px, height 20px),
       and with the -50% translate below `top` is where the chip's own centre
       lands. One pixel above that by eye: the chip's pill border and the bar's
       flat block don't read as level at the same maths-centre. (It was 21px,
       which rode 9px high and collided with the start-time label.) */
    top: 29px;
    transform: translateY(-50%);
    display: inline-flex;
    align-items: center;
    gap: 3px;
    height: 18px;
    padding: 0 6px 0 5px;
    background: var(--bg-card);
    border: 1px solid var(--accent);
    color: var(--accent);
    border-radius: var(--radius-pill, 999px);
    font-size: 10px;
    font-weight: 700;
    font-variant-numeric: tabular-nums;
    line-height: 1;
    pointer-events: none;
    z-index: 3;
    white-space: nowrap;
  }
  .bar-chip.active-chip {
    border-color: var(--live, #4ade80);
    color: var(--live, #4ade80);
  }

  /* Hover popover — themed card with full session breakdown.
     Sits above the bar; flips to right-anchored if bar starts past 60%. */
  .bar-popover {
    position: absolute;
    bottom: calc(100% + 8px);
    z-index: 10;
    background: var(--bg-elevated);
    border: 1px solid var(--border-strong, var(--border));
    box-shadow: var(--shadow-md, 0 4px 12px rgba(0, 0, 0, 0.35));
    border-radius: var(--radius-md, 8px);
    padding: 8px 11px 9px;
    pointer-events: none;
    white-space: nowrap;
    animation: pop-in 120ms var(--ease-out, ease-out);
  }
  @keyframes pop-in {
    from { transform: translateY(2px); opacity: 0; }
    to   { transform: none; opacity: 1; }
  }
  .pop-time {
    font-size: 12px;
    font-weight: 600;
    color: var(--text);
    font-variant-numeric: tabular-nums;
    letter-spacing: -0.005em;
  }
  .pop-stats {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-top: 5px;
    font-size: 11px;
    color: var(--text-muted);
    font-variant-numeric: tabular-nums;
  }
  .pop-dur { color: var(--text); font-weight: 600; }
  .pop-sep { color: var(--border-strong, var(--border)); }
  .pop-cumshots {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    color: var(--accent);
    font-weight: 600;
  }
  .pop-active {
    margin-top: 5px;
    font-size: 10px;
    font-weight: 700;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: var(--live, #4ade80);
  }
</style>
