<!--
  Sessions section — the dedicated "go deeper" space for sessions (Phase 4).

  A sortable, paginated table of every tracked session. Sort + pagination are
  done in the backend (SQLite ORDER BY/LIMIT) so a column sort holds across ALL
  rows, not just the page in view. The date range is the global sessionsFilter's
  window (shared with Overview); entity drill-down lives in the Phase 5 section
  pages, so this section only exposes the date chip.

  Clicking a row expands SessionDetail inline beneath it (same component the
  Overview day-drilldown uses). Checkboxes + an action bar allow multi-select
  delete; deletion rolls each session's footprint back out of Stash (see the
  backend delete_session/cascade rollback).
-->
<script lang="ts">
  import { pulse } from "$lib/pulse";
  import { w } from "$lib/wording.svelte";
  import { onMount, onDestroy, tick } from "svelte";
  import Select from "$lib/Select.svelte";
  import {
    api,
    formatDuration,
    formatTimeOnly,
    formatDateOnly,
    type Session,
    type SessionSort,
    type MergeCheck,
    type ExtendSessionCheck,
    type ReopenCheck,
  } from "$lib/api";
  import { sessionsFilter } from "$lib/filter-store.svelte";
  import { nav } from "$lib/dashboard/nav.svelte";
  import DateRangePicker from "$lib/dashboard/DateRangePicker.svelte";
  import SessionDetail from "$lib/dashboard/SessionDetail.svelte";
  import ConfirmDialog from "$lib/ConfirmDialog.svelte";
  import Icon from "$lib/Icon.svelte";

  // Rows per page. "fit" auto-sizes to the viewport so the table fills the
  // screen without a scrollbar; explicit numbers let the user expand/shrink
  // (the larger sizes intentionally introduce a scrollbar).
  type RowsMode = "fit" | "10" | "25" | "50" | "100";
  const ROWS_OPTIONS: RowsMode[] = ["fit", "10", "25", "50", "100"];

  let rows = $state<Session[]>([]);
  let total = $state(0);
  let loading = $state(true);
  let page = $state(0);
  let sortCol = $state<SessionSort>("start");
  let sortDesc = $state(true);
  let rowsMode = $state<RowsMode>("fit");
  let fitRows = $state(12); // recomputed from viewport height on mount/resize

  let selectedSessionId = $state<number | null>(null); // expanded row
  let selected = $state<Set<number>>(new Set()); // checkbox selection
  let dateOpen = $state(false);
  let confirmDeleteOpen = $state(false);
  // Merge: exactly 2 selected + consecutive. mergeCheck previews allow + gap.
  let mergeCheck = $state<MergeCheck | null>(null);
  let mergeOpen = $state(false);
  let merging = $state(false);
  let mergeGapAsPause = $state(true);
  let mergeCheckToken = 0;

  // Extend: exactly 1 selected + untracked activity exists right after it.
  // extendCheck previews whether it's possible + the gap.
  let extendCheck = $state<ExtendSessionCheck | null>(null);
  let extendOpen = $state(false);
  let extending = $state(false);
  let extendGapAsPause = $state(true);
  let extendCheckToken = 0;

  // Reopen: exactly 1 selected + it's the most recent ended session + nothing is
  // running. Makes it live again ("I stopped that by accident"); the backend
  // owns the rules, reopenCheck just previews them + the gap.
  let reopenCheck = $state<ReopenCheck | null>(null);
  let reopenOpen = $state(false);
  let reopening = $state(false);
  let reopenGapAsPause = $state(true);
  let reopenCheckToken = 0;

  let headerEl: HTMLElement | null = null; // toolbar + meta-row; measured for the fit calc
  let poll: ReturnType<typeof setInterval> | null = null;

  const pageSize = $derived(rowsMode === "fit" ? fitRows : parseInt(rowsMode, 10));
  const offset = $derived(page * pageSize);

  // Monotonic token: at mount two loads can be in flight (the effect's first
  // run + the post-claim rerun) — an older response must not overwrite a
  // newer one.
  let loadToken = 0;
  async function load() {
    const token = ++loadToken;
    try {
      const res = await api.sessionsPage({
        startDay: sessionsFilter.startDay,
        endDay: sessionsFilter.endDay,
        sort: sortCol,
        desc: sortDesc,
        limit: pageSize,
        offset: page * pageSize,
      });
      if (token !== loadToken) return;
      rows = res.rows;
      total = res.total;
    } catch (e) {
      console.error("sessions page load failed", e);
    } finally {
      if (token === loadToken) loading = false;
    }
  }

  // Reset to the first page whenever the filter window changes (a later page
  // would likely be out of range for the new, smaller result set). Gated on
  // the store being ready so the fallback→saved-default transition at first
  // load doesn't count as a "change".
  let prevWindow = "";
  $effect(() => {
    if (!sessionsFilter.ready) return;
    const w = `${sessionsFilter.startDay}|${sessionsFilter.endDay}`;
    if (prevWindow && prevWindow !== w) page = 0;
    prevWindow = w;
  });

  // Load whenever window / sort / page / page-size changes. Waits for the
  // store's saved default to be applied — fetching against the fallback
  // window briefly showed wrong-window rows (e.g. a page of 12-month data
  // under an empty "Last 7 days" default).
  $effect(() => {
    if (!sessionsFilter.ready) return;
    sessionsFilter.startDay;
    sessionsFilter.endDay;
    sortCol;
    sortDesc;
    page;
    pageSize;
    load();
  });

  // ----- Page-size fit -----
  // Approximate rendered heights; conservative so "Fit" never overflows into a
  // scrollbar (a couple px of slack beats a stray scrollbar).
  const ROW_H = 44; // one data row
  const HEADER_H = 40; // table header row
  const RESERVE = 84; // bottom pager + section paddings under the table
  const MIN_FIT = 5;

  // The dashboard shell scrolls .content internally (not the window), so size
  // the table against the scroll container's visible height, not innerHeight.
  function scrollContainer(): HTMLElement | null {
    let el: HTMLElement | null = headerEl;
    while (el) {
      const oy = getComputedStyle(el).overflowY;
      if (oy === "auto" || oy === "scroll") return el;
      el = el.parentElement;
    }
    return null;
  }

  function computeFit() {
    if (!headerEl) return;
    // Table content starts just below the header (toolbar + meta-row) plus the
    // .sessions flex gap.
    const tableTop = headerEl.getBoundingClientRect().bottom + 12;
    const sc = scrollContainer();
    const visibleBottom = sc
      ? sc.getBoundingClientRect().top + sc.clientHeight
      : window.innerHeight;
    const avail = visibleBottom - tableTop - RESERVE;
    fitRows = Math.max(MIN_FIT, Math.floor((avail - HEADER_H) / ROW_H));
  }

  // Reset to the first page when the page size changes (the current offset may
  // be past the end for the new size). Skips the initial mount.
  let prevPageSize = -1;
  $effect(() => {
    if (prevPageSize !== -1 && pageSize !== prevPageSize) page = 0;
    prevPageSize = pageSize;
  });

  // Selection is per-page (like "select all on this page"): clear it whenever
  // the page changes so the action-bar count + delete confirm always reflect
  // what's actually in view. Skips the initial mount.
  let selPage = -1;
  $effect(() => {
    if (page !== selPage) {
      if (selPage !== -1) clearSelection();
      selPage = page;
    }
  });

  function onResize() {
    if (rowsMode === "fit") computeFit();
  }

  onMount(() => {
    // Returning here via Back? Restore the page + re-open the session that was
    // expanded when we navigated away (and keep the filter as-left, so the
    // restored view makes sense). A FRESH sidebar arrival instead resets the
    // filter to the saved default — same rule as the browse pages, so the
    // Default pill means the same thing everywhere.
    const t = nav.claimTarget("sessions");
    if (t?.openSession && typeof t.sessionId === "number") {
      // Forward nav from elsewhere (e.g. Overview's SessionDetail "Open in
      // Sessions"): open this session fresh. It may be outside the saved filter
      // or on another page, so land on its own day to guarantee the row is in
      // range, then expand it. (openSessionFromNav loads defaults itself.)
      openSessionFromNav(t.sessionId);
    } else if (t) {
      if (typeof t.page === "number") page = t.page;
      if (typeof t.sessionId === "number") selectedSessionId = t.sessionId;
      restoredDetailScroll = t.detailScroll ?? null;
      // Back-restore keeps the filter as-left; just ensure the store is ready.
      sessionsFilter.loadDefaults();
    } else {
      sessionsFilter.reset();
      // Respect the user's saved default date preset even if Sessions is the
      // first section visited (idempotent — no-op once loaded).
      sessionsFilter.loadDefaults();
    }
    computeFit();
    // Sync the page-size baseline to the JUST-MEASURED fit. The pagesize
    // $effect's first run happens BEFORE onMount and baselines on the
    // provisional fitRows (12); without this sync, the provisional→measured
    // transition reads as a user page-size change and resets page to 0 —
    // which was silently wiping the Back-restored page moments after every
    // cross-nav return (it never showed on fresh visits, where page is
    // already 0).
    prevPageSize = pageSize;
    window.addEventListener("resize", onResize);
    // Keep an active session's live duration / counts fresh.
    poll = setInterval(load, 5_000);
  });
  onDestroy(() => {
    if (poll) clearInterval(poll);
    window.removeEventListener("resize", onResize);
  });

  // Close the date popover on outside click / Escape.
  $effect(() => {
    if (!dateOpen) return;
    function onDocClick(e: MouseEvent) {
      // composedPath(), not target.closest() - see FilterBar: a node removed by
      // its own click handler is detached by the time this runs, so closest()
      // returns null and an inside click reads as outside.
      const inside = e.composedPath().some(
        (n) => n instanceof Element && (n.classList.contains("date-pop") || n.classList.contains("date-chip")),
      );
      if (inside) return;
      dateOpen = false;
    }
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") dateOpen = false;
    }
    document.addEventListener("click", onDocClick);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("click", onDocClick);
      document.removeEventListener("keydown", onKey);
    };
  });

  function setSort(col: SessionSort) {
    if (sortCol === col) {
      sortDesc = !sortDesc;
    } else {
      sortCol = col;
      // Sensible default direction: newest/biggest first for every column.
      sortDesc = true;
    }
    page = 0;
  }

  // ----- Expand / collapse + scroll behaviour -----
  // Opening a session scrolls its ROW to the very top of the viewport (the row
  // you clicked stays visible as the first thing, detail right below it —
  // scrolling the row rather than the detail also keeps the anchor stable while
  // the detail's content loads in). Closing restores the scroll position from
  // before the expand. Mirrors the Overview day-drilldown's scroll-into-view.
  let rowEls: Record<number, HTMLTableRowElement | undefined> = {};
  let savedScroll = 0;
  /** Back-restored inner-scroll position for the expanded session's detail
   *  (null on user-initiated expands, which start at the detail's top). */
  let restoredDetailScroll = $state<number | null>(null);

  function collapseDetail() {
    selectedSessionId = null;
    tick().then(() =>
      scrollContainer()?.scrollTo({ top: savedScroll, behavior: "smooth" }),
    );
  }

  function toggleExpand(id: number) {
    restoredDetailScroll = null;
    if (selectedSessionId === id) {
      collapseDetail();
      return;
    }
    // Save the pre-open position only when coming from fully closed, so
    // switching row→row still restores the ORIGINAL spot on close.
    if (selectedSessionId === null) savedScroll = scrollContainer()?.scrollTop ?? 0;
    selectedSessionId = id;
    // tick() so the detail row is in the DOM; small timeout so its initial
    // layout settles before we align the clicked row to the viewport top
    // (same pattern as the Overview session-detail scroll).
    tick().then(() =>
      setTimeout(() => rowEls[id]?.scrollIntoView({ behavior: "smooth", block: "start" }), 60),
    );
  }

  // Origin snapshot pushed when navigating away from an expanded session's
  // detail (a performer/studio pill), so Back returns here with it re-opened.
  function sessionsOrigin() {
    return { section: "sessions" as const, sessionId: selectedSessionId, page };
  }

  // Forward-nav entry: open a specific session that may be outside the current
  // filter window or on another page. Land on the session's own day (so it's in
  // the result set and on page 0), then expand it. Used by Overview's "Open in
  // Sessions" link via nav.gotoSession.
  async function openSessionFromNav(id: number) {
    selectedSessionId = id;
    restoredDetailScroll = null;
    page = 0;
    try {
      const s = await api.sessionGet(id);
      if (s?.assigned_day) sessionsFilter.setCustomRange(s.assigned_day, s.assigned_day);
    } catch (e) {
      console.error("open session from nav failed", e);
    }
    // Ready-gate the load effect (first visit may not have loaded defaults).
    // Idempotent and won't clobber the custom range we just set (preset='custom').
    sessionsFilter.loadDefaults();
    await load();
    // Busy day (e.g. many reconstructed sittings): if the row landed on a later
    // page, walk forward until it's in view (bounded; most days fit on page 0).
    let guard = 0;
    while (
      selectedSessionId === id &&
      !rows.some((r) => r.id === id) &&
      (page + 1) * pageSize < total &&
      guard < 100
    ) {
      page = page + 1;
      await load();
      guard++;
    }
    tick().then(() =>
      setTimeout(() => rowEls[id]?.scrollIntoView({ behavior: "smooth", block: "start" }), 80),
    );
  }

  function toggleSelect(id: number) {
    const next = new Set(selected);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    selected = next;
  }

  const allOnPageSelected = $derived(
    rows.length > 0 && rows.every((r) => selected.has(r.id)),
  );

  function toggleSelectAll() {
    const next = new Set(selected);
    if (allOnPageSelected) {
      for (const r of rows) next.delete(r.id);
    } else {
      for (const r of rows) next.add(r.id);
    }
    selected = next;
  }

  function clearSelection() {
    selected = new Set();
  }

  // Sum of cumshots across the selected rows currently in view — concrete
  // number for the confirm copy. (Selection can't span pages here since
  // selecting is per-page, so the in-view rows cover the whole selection.)
  const selectedCumshots = $derived(
    rows.filter((r) => selected.has(r.id)).reduce((s, r) => s + r.o_count, 0),
  );

  // Preview the merge whenever exactly two sessions are selected (else clear it).
  $effect(() => {
    const ids = [...selected];
    if (ids.length !== 2) {
      mergeCheck = null;
      return;
    }
    const my = ++mergeCheckToken;
    api
      .sessionMergeCheck(ids[0], ids[1])
      .then((c) => {
        if (my === mergeCheckToken) mergeCheck = c;
      })
      .catch(() => {
        if (my === mergeCheckToken) mergeCheck = null;
      });
  });

  // Preview "extend" whenever exactly one session is selected (else clear it).
  $effect(() => {
    const ids = [...selected];
    if (ids.length !== 1) {
      extendCheck = null;
      return;
    }
    const my = ++extendCheckToken;
    api
      .sessionExtendCheck(ids[0])
      .then((c) => {
        if (my === extendCheckToken) extendCheck = c;
      })
      .catch(() => {
        if (my === extendCheckToken) extendCheck = null;
      });
  });

  // Preview "reopen" whenever exactly one session is selected (else clear it).
  $effect(() => {
    const ids = [...selected];
    if (ids.length !== 1) {
      reopenCheck = null;
      return;
    }
    const my = ++reopenCheckToken;
    api
      .sessionReopenCheck(ids[0])
      .then((c) => {
        if (my === reopenCheckToken) reopenCheck = c;
      })
      .catch(() => {
        if (my === reopenCheckToken) reopenCheck = null;
      });
  });

  function fmtDateTime(ms: number): string {
    return `${formatDateOnly(ms)} ${formatTimeOnly(ms)}`;
  }

  async function doMerge() {
    if (merging || !mergeCheck?.ok) return;
    const ids = [...selected];
    if (ids.length !== 2) return;
    merging = true;
    try {
      await api.sessionMerge(ids[0], ids[1], mergeGapAsPause);
      mergeOpen = false;
      // The merged-away session's detail (if open) no longer exists.
      if (selectedSessionId !== null && ids.includes(selectedSessionId)) selectedSessionId = null;
      clearSelection();
      await load();
    } catch (e) {
      console.error("merge failed", e);
    } finally {
      merging = false;
    }
  }

  async function doExtend() {
    if (extending) return;
    const ids = [...selected];
    if (ids.length !== 1) return;
    extending = true;
    try {
      await api.sessionExtend(ids[0], extendGapAsPause);
      extendOpen = false;
      clearSelection();
      await load();
    } catch (e) {
      console.error("extend failed", e);
    } finally {
      extending = false;
    }
  }

  async function doReopen() {
    if (reopening) return;
    const ids = [...selected];
    if (ids.length !== 1) return;
    reopening = true;
    try {
      await api.sessionReopen(ids[0], reopenGapAsPause);
      reopenOpen = false;
      clearSelection();
      await load();
    } catch (e) {
      console.error("reopen failed", e);
    } finally {
      reopening = false;
    }
  }

  async function confirmDelete() {
    confirmDeleteOpen = false;
    const ids = [...selected];
    try {
      await api.sessionDeleteMany(ids);
    } catch (e) {
      console.error("bulk delete failed", e);
    }
    if (selectedSessionId !== null && selected.has(selectedSessionId)) {
      selectedSessionId = null;
    }
    clearSelection();
    page = 0;
    await load();
  }

  const deleteMessage = $derived(
    `Permanently delete ${selected.size} session${selected.size === 1 ? "" : "s"}` +
      `${selectedCumshots > 0 ? ` and ${selectedCumshots} ${w(selectedCumshots === 1 ? "cumshot" : "cumshots")}` : ""}.\n\n` +
      w(`The watch time, play counts, and cumshots will also be deleted in Stash. This cannot be undone.`),
  );

  function startLabel(s: Session): { date: string; time: string } {
    return { date: formatDateOnly(s.started_at), time: formatTimeOnly(s.started_at) };
  }

  type Col = { key: SessionSort; label: string; align: "left" | "right" };
  const columns: Col[] = [
    { key: "start", label: "Start", align: "left" },
    { key: "duration", label: "Duration", align: "right" },
    { key: "scenes", label: "Scenes", align: "right" },
    { key: "cumshots", label: "Cumshots", align: "right" },
  ];

  // The rendered range summary, e.g. "1–50 of 214".
  const rangeStart = $derived(total === 0 ? 0 : offset + 1);
  const rangeEnd = $derived(Math.min(offset + pageSize, total));
  const canPrev = $derived(page > 0);
  const canNext = $derived(offset + pageSize < total);
</script>

<div class="sessions">
  {#snippet pager(bottom = false)}
    <div class="pager" class:pager-bottom={bottom}>
      <span class="pager-range">{rangeStart}-{rangeEnd} of {total}</span>
      <div class="pager-btns">
        <button class="ghost" disabled={!canPrev} onclick={() => (page = page - 1)} aria-label="Previous page">
          <Icon name="chevron-left" size={14} /> Prev
        </button>
        <button class="ghost" disabled={!canNext} onclick={() => (page = page + 1)} aria-label="Next page">
          Next <Icon name="chevron-right" size={14} />
        </button>
      </div>
    </div>
  {/snippet}

  <!-- Back to wherever we came from (e.g. Overview's day-drill via "Open in
       Sessions"). Only shows when there's a back-stack entry; a plain sidebar
       arrival clears it. -->
  {#if nav.canGoBack}
    <button class="cx-back-btn" onclick={() => nav.back()}>
      <Icon name="chevron-left" size={14} /> Back
    </button>
  {/if}

  <!-- Header: toolbar (date chip + rows/pager) above a meta-row (count · range · Reset) -->
  <div class="header" bind:this={headerEl}>
    <div class="toolbar">
      <div class="date-wrap">
        <button
          class="date-chip"
          class:active={dateOpen}
          onclick={(e) => { e.stopPropagation(); dateOpen = !dateOpen; }}
          aria-expanded={dateOpen}
        >
          <Icon name="calendar" size={13} />
          <span>{sessionsFilter.presetLabel}</span>
          <Icon name="chevron-down" size={11} color="var(--fg-subtle)" />
        </button>
        {#if dateOpen}
          <div class="date-pop">
            <DateRangePicker store={sessionsFilter} onClose={() => (dateOpen = false)} />
          </div>
        {/if}
      </div>

      <div class="top-controls">
        <div class="rows-select" title="Rows per page">
          <span>Rows</span>
          <Select
            value={rowsMode}
            options={ROWS_OPTIONS.map((o) => ({ value: o, label: o === "fit" ? "Fit" : o }))}
            onChange={(v) => { rowsMode = v as RowsMode; if (rowsMode === "fit") computeFit(); }}
            ariaLabel="Rows per page"
          />
        </div>
        {#if total > 0}
          {@render pager()}
        {/if}
      </div>
    </div>

    <div class="meta-row">
      <span class="count">{total} session{total === 1 ? " matches" : "s match"}</span>
      <span class="sep">·</span>
      <span class="range">{sessionsFilter.rangeLabel}</span>
      {#if !sessionsFilter.isDefault}
        <span class="sep">·</span>
        <button class="reset" onclick={() => sessionsFilter.reset()}>Reset</button>
      {/if}
    </div>
  </div>

  {#if selected.size > 0}
    <div class="action-bar">
      <span class="sel-count">{selected.size} selected</span>
      <button class="ghost" onclick={clearSelection}>Clear</button>
      {#if selected.size === 2}
        <button
          class="ghost"
          disabled={!mergeCheck?.ok}
          title={mergeCheck?.ok ? "Merge these two sessions into one" : (mergeCheck?.reason ?? "Checking...")}
          onclick={() => (mergeOpen = true)}
        >
          <Icon name="arrow-left-to-line" size={14} /> Merge
        </button>
      {/if}
      {#if selected.size === 1 && extendCheck?.can_extend}
        <button
          class="ghost"
          title="Extend this session - fold in the untracked activity right after it (as if it never stopped)."
          onclick={() => (extendOpen = true)}
        >
          <Icon name="arrow-left-to-line" size={14} /> Extend
        </button>
      {/if}
      {#if selected.size === 1 && reopenCheck?.can_reopen}
        <button
          class="ghost"
          title="Reopen this session so it starts tracking again, for one you stopped by accident."
          onclick={() => (reopenOpen = true)}
        >
          <Icon name="play" size={14} /> Reopen
        </button>
      {/if}
      <button class="danger" onclick={() => (confirmDeleteOpen = true)}>
        <Icon name="trash-2" size={14} />
        Delete {selected.size} session{selected.size === 1 ? "" : "s"}
      </button>
    </div>
  {/if}

  {#if loading && rows.length === 0}
    <div class="empty">loading...</div>
  {:else if total === 0}
    <div class="empty">
      <Icon name="list" size={30} color="var(--fg-muted)" />
      <p>No sessions in this range.</p>
    </div>
  {:else}
    <div class="table-card">
      <table>
        <thead>
          <tr>
            <th class="check-col">
              <input
                type="checkbox"
                checked={allOnPageSelected}
                onchange={toggleSelectAll}
                aria-label="Select all on this page"
              />
            </th>
            {#each columns as col (col.key)}
              <th
                class={`sortable ${col.align}`}
                class:active={sortCol === col.key}
                onclick={() => setSort(col.key)}
              >
                <span class="th-inner">
                  {w(col.label)}
                  {#if sortCol === col.key}
                    <Icon name={sortDesc ? "chevron-down" : "chevron-up"} size={12} />
                  {/if}
                </span>
              </th>
            {/each}
          </tr>
        </thead>
        <tbody>
          {#each rows as s (s.id)}
            {@const isActive = s.status === "active" || s.status === "paused"}
            {@const sl = startLabel(s)}
            <tr
              class="data-row"
              class:expanded={selectedSessionId === s.id}
              class:selected={selected.has(s.id)}
              onclick={() => toggleExpand(s.id)}
              bind:this={rowEls[s.id]}
            >
              <td class="check-col" onclick={(e) => e.stopPropagation()}>
                <input
                  type="checkbox"
                  checked={selected.has(s.id)}
                  onchange={() => toggleSelect(s.id)}
                  aria-label={`Select session from ${sl.date}`}
                />
              </td>
              <td class="start-col">
                <span class="start-date">{sl.date}</span>
                <span class="start-time">{sl.time}</span>
                {#if isActive}
                  <span class="live-dot" class:paused={s.status === "paused"} use:pulse={s.status !== "paused"} title={s.status === "paused" ? "paused" : "running"}></span>
                {/if}
                {#if s.estimated}
                  <span class="est-badge" title="Reconstructed from Stash's history rather than tracked live, so the times are approximate.">est</span>
                {/if}
              </td>
              <td class="right num">{formatDuration(s.effective_duration_ms)}</td>
              <td class="right num">{s.scene_count}</td>
              <td class="right num cumshots">{s.o_count}</td>
            </tr>
            {#if selectedSessionId === s.id}
              <tr class="detail-row">
                <td colspan="5">
                  <SessionDetail
                    sessionId={s.id}
                    initialScroll={restoredDetailScroll ?? undefined}
                    onClose={collapseDetail}
                    onDeleted={() => { collapseDetail(); clearSelection(); load(); }}
                    onPerformerSelect={(p) => nav.goto("performer", p.id, sessionsOrigin())}
                    onStudioSelect={(st) => nav.goto("studio", st.id, sessionsOrigin())}
                    onSceneSelect={(id) => nav.goto("scene", id, sessionsOrigin())}
                  />
                </td>
              </tr>
            {/if}
          {/each}
        </tbody>
      </table>
    </div>

    {@render pager(true)}
  {/if}
</div>

{#if confirmDeleteOpen}
  <ConfirmDialog
    title={`Delete ${selected.size} session${selected.size === 1 ? "" : "s"}?`}
    message={deleteMessage}
    confirmLabel="Delete"
    variant="danger"
    onConfirm={confirmDelete}
    onCancel={() => (confirmDeleteOpen = false)}
  />
{/if}

{#if mergeOpen && mergeCheck?.ok}
  <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_noninteractive_element_interactions -->
  <div class="merge-overlay" role="dialog" aria-modal="true" tabindex="-1"
    onkeydown={(e) => { if (e.key === 'Escape') mergeOpen = false; }}
    onclick={() => (mergeOpen = false)}>
    <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
    <div class="merge-modal" role="document" onclick={(e) => e.stopPropagation()}>
      <h3>Merge these two sessions?</h3>
      <p class="merge-span">
        One session, <strong>{fmtDateTime(mergeCheck.merged_start)}</strong> to
        <strong>{fmtDateTime(mergeCheck.merged_end)}</strong>. Scenes and {w("cumshots")} combine; this cannot be undone.
      </p>
      {#if mergeCheck.gap_ms > 0}
        <label class="merge-gap">
          <input type="checkbox" bind:checked={mergeGapAsPause} />
          <span>
            Insert a pause for the {formatDuration(mergeCheck.gap_ms)} gap between them
            <span class="merge-gap-note">{mergeGapAsPause ? "Gap counts as a break, left out of the session's time." : "Gap is kept, counts towards the session's time."}</span>
          </span>
        </label>
      {/if}
      <div class="merge-actions">
        <button class="ghost" disabled={merging} onclick={() => (mergeOpen = false)}>Cancel</button>
        <button class="primary" disabled={merging} onclick={doMerge}>{merging ? "Merging..." : "Merge"}</button>
      </div>
    </div>
  </div>
{/if}

{#if extendOpen && extendCheck?.can_extend}
  <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_noninteractive_element_interactions -->
  <div class="merge-overlay" role="dialog" aria-modal="true" tabindex="-1"
    onkeydown={(e) => { if (e.key === 'Escape') extendOpen = false; }}
    onclick={() => (extendOpen = false)}>
    <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
    <div class="merge-modal" role="document" onclick={(e) => e.stopPropagation()}>
      <h3>Extend this session?</h3>
      <p class="merge-span">
        Fold in {extendCheck.scene_count} scene{extendCheck.scene_count === 1 ? '' : 's'}{extendCheck.cumshot_count > 0 ? ` and ${extendCheck.cumshot_count} ${w(extendCheck.cumshot_count === 1 ? 'cumshot' : 'cumshots')}` : ''} that Climax saw after this session, as if it never stopped.
      </p>
      {#if extendCheck.gap_ms > 0}
        <label class="merge-gap">
          <input type="checkbox" bind:checked={extendGapAsPause} />
          <span>
            Insert a pause for the {formatDuration(extendCheck.gap_ms)} gap
            <span class="merge-gap-note">{extendGapAsPause ? "Gap counts as a break, left out of the session's time." : "Gap is kept, counts towards the session's time."}</span>
          </span>
        </label>
      {/if}
      <div class="merge-actions">
        <button class="ghost" disabled={extending} onclick={() => (extendOpen = false)}>Cancel</button>
        <button class="primary" disabled={extending} onclick={doExtend}>{extending ? "Extending..." : "Extend"}</button>
      </div>
    </div>
  </div>
{/if}

{#if reopenOpen && reopenCheck?.can_reopen}
  <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_noninteractive_element_interactions -->
  <div class="merge-overlay" role="dialog" aria-modal="true" tabindex="-1"
    onkeydown={(e) => { if (e.key === 'Escape') reopenOpen = false; }}
    onclick={() => (reopenOpen = false)}>
    <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
    <div class="merge-modal" role="document" onclick={(e) => e.stopPropagation()}>
      <h3>Reopen this session?</h3>
      <p class="merge-span">
        It goes live again and keeps tracking from now.
      </p>
      {#if reopenCheck.gap_ms > 0}
        <label class="merge-gap">
          <input type="checkbox" bind:checked={reopenGapAsPause} />
          <span>
            Insert a pause for the {formatDuration(reopenCheck.gap_ms)} since it ended
            <span class="merge-gap-note">{reopenGapAsPause ? "Gap counts as a break, left out of the session's time." : "Gap is kept, counts towards the session's time."}</span>
          </span>
        </label>
      {/if}
      <div class="merge-actions">
        <button class="ghost" disabled={reopening} onclick={() => (reopenOpen = false)}>Cancel</button>
        <button class="primary" disabled={reopening} onclick={doReopen}>{reopening ? "Reopening..." : "Reopen"}</button>
      </div>
    </div>
  </div>
{/if}

<style>
  .sessions {
    display: flex;
    flex-direction: column;
    gap: 12px;
    /* At least fill the content area (so the bottom pager sits at the page
       bottom via margin-top:auto), but GROW + let .content scroll when a session
       detail expands. flex-shrink:0 stops .content from clamping us to its own
       height — clamping is what killed scrolling. */
    flex-shrink: 0;
    min-height: 100%;
  }
  /* Only the BOTTOM pager gets pushed down (flagged via the snippet); the top
     one lives inside .top-controls and is untouched. */
  .pager-bottom {
    margin-top: auto;
  }

  /* ---------- Toolbar ---------- */
  .toolbar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    min-height: 34px;
  }
  .date-wrap {
    position: relative;
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .date-chip {
    background: var(--bg-card-hover);
    border: 1px solid var(--border);
    color: var(--fg);
    border-radius: 7px;
    padding: 5px 11px;
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-family: inherit;
    font-size: 12px;
    cursor: pointer;
    white-space: nowrap;
    transition: background var(--dur-2), border-color var(--dur-2), color var(--dur-2);
  }
  .date-chip:hover { border-color: var(--border-strong); }
  .date-chip.active { border-color: var(--accent); }
  .date-pop {
    position: absolute;
    top: calc(100% + 6px);
    left: 0;
    z-index: 50;
  }
  /* Header wraps the toolbar + meta-row so the fit calc measures both. */
  .header {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  /* Meta-row mirrors the Overview FilterBar: match count · range · Reset. */
  .meta-row {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 11px;
    color: var(--fg-muted);
    font-variant-numeric: tabular-nums;
  }
  .meta-row .count { font-weight: 600; color: var(--fg); }
  .meta-row .sep { color: var(--border-strong, var(--border)); }
  .meta-row .reset {
    background: transparent;
    border: 1px solid transparent;
    color: var(--fg-muted);
    font-family: inherit;
    font-size: 11px;
    padding: 1px 7px;
    border-radius: var(--radius-sm, 6px);
    cursor: pointer;
    transition: color var(--dur-2, 120ms), border-color var(--dur-2, 120ms);
  }
  .meta-row .reset:hover {
    color: var(--accent);
    border-color: var(--accent);
  }

  /* Right-side controls: rows-per-page selector + top pager. */
  .top-controls {
    display: flex;
    align-items: center;
    gap: 14px;
  }
  .rows-select {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-size: 11px;
    color: var(--fg-muted);
    white-space: nowrap;
  }

  .action-bar {
    display: flex;
    align-items: center;
    gap: 8px;
    animation: slide-in 140ms var(--ease-out, ease-out);
  }
  @keyframes slide-in { from { opacity: 0; transform: translateY(-2px); } to { opacity: 1; transform: none; } }
  .sel-count {
    font-size: 12px;
    color: var(--fg-muted);
    font-variant-numeric: tabular-nums;
  }
  .ghost {
    background: transparent;
    border: 1px solid var(--border);
    color: var(--fg-muted);
    font-family: inherit;
    font-size: 12px;
    padding: 5px 10px;
    border-radius: 7px;
    cursor: pointer;
    display: inline-flex;
    align-items: center;
    gap: 5px;
    transition: background var(--dur-2), border-color var(--dur-2), color var(--dur-2);
  }
  .ghost:hover:not(:disabled) { background: var(--bg-card-hover); color: var(--fg); border-color: var(--border-strong); }
  .ghost:disabled { opacity: 0.4; cursor: default; }
  .danger {
    background: var(--danger);
    border: 1px solid var(--danger);
    color: #fff;
    font-family: inherit;
    font-size: 12px;
    font-weight: 600;
    padding: 5px 11px;
    border-radius: 7px;
    cursor: pointer;
    display: inline-flex;
    align-items: center;
    gap: 6px;
    transition: filter var(--dur-2);
  }
  .danger:hover { filter: brightness(1.08); }

  /* ---------- Empty ---------- */
  .empty {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 10px;
    padding: 64px 0;
    color: var(--fg-muted);
    font-size: 13px;
  }
  .empty p { margin: 0; }

  /* ---------- Table ---------- */
  .table-card {
    background: var(--bg-card);
    border: 1px solid var(--border);
    border-radius: var(--radius-lg, 10px);
    overflow: hidden;
  }
  table {
    width: 100%;
    border-collapse: collapse;
    font-variant-numeric: tabular-nums;
  }
  thead th {
    text-align: left;
    font-size: 11px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: var(--ls-eyebrow, 0.08em);
    color: var(--fg-subtle);
    padding: 10px 14px;
    border-bottom: 1px solid var(--border);
    background: var(--bg-elevated);
    user-select: none;
  }
  th.right { text-align: right; }
  th.sortable { cursor: pointer; transition: color var(--dur-2); }
  th.sortable:hover { color: var(--fg-muted); }
  th.sortable.active { color: var(--fg); }
  .th-inner { display: inline-flex; align-items: center; gap: 4px; }
  th.right .th-inner { flex-direction: row-reverse; }
  .check-col { width: 38px; text-align: center; padding-left: 14px; padding-right: 0; }

  tbody td {
    padding: 11px 14px;
    border-bottom: 1px solid var(--border-subtle);
    font-size: 13px;
    color: var(--fg);
  }
  .data-row { cursor: pointer; transition: background var(--dur-2); }
  .data-row:hover { background: var(--bg-card-hover); }
  .data-row.selected { background: var(--coral-a-08); }
  .data-row.expanded { background: var(--bg-card-hover); }
  .data-row.expanded td { border-bottom-color: transparent; }

  td.right { text-align: right; }
  td.num { color: var(--fg); }
  td.cumshots { color: var(--accent); font-weight: 600; }

  .start-col {
    display: flex;
    align-items: baseline;
    gap: 8px;
  }
  .start-date { font-weight: 500; color: var(--fg-strong); }
  .start-time { color: var(--fg-muted); font-size: 12px; }
  .live-dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--live);
    align-self: center;
    position: relative;
  }
  /* Low-rate pulse driven by --cx-pulse from src/lib/pulse.ts (about 10 frames a
     second, not 60), changing only transform + opacity, so a running session does not
     repaint the window every frame. 7px dot + 5px halo = scale 17/7. */
  .live-dot::after {
    content: "";
    position: absolute;
    inset: 0;
    border-radius: 50%;
    background: inherit;
    pointer-events: none;
    will-change: transform, opacity;
    transform: scale(calc(1 + 1.429 * var(--cx-pulse, 0)));
    opacity: calc(0.45 * (1 - var(--cx-pulse, 0)));
  }
  .live-dot.paused { background: var(--warn); }
  .live-dot.paused::after { animation: none; opacity: 0; }
  .est-badge {
    align-self: center;
    font-size: 9px;
    font-weight: 600;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    color: var(--fg-muted);
    border: 1px solid var(--border);
    border-radius: 5px;
    padding: 1px 5px;
    line-height: 1.5;
  }

  input[type="checkbox"] {
    width: 15px;
    height: 15px;
    accent-color: var(--accent);
    cursor: pointer;
    vertical-align: middle;
  }

  .detail-row td {
    padding: 0 8px 14px;
    background: var(--bg-card-hover);
    border-bottom: 1px solid var(--border);
  }

  /* ---------- Pager (rendered at top inside .top-controls and at bottom) ---------- */
  .pager {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 2px 2px 4px;
  }
  .pager-range {
    font-size: 12px;
    color: var(--fg-muted);
    font-variant-numeric: tabular-nums;
  }
  .pager-btns { display: flex; gap: 6px; }

  /* ---------- Merge dialog ---------- */
  .merge-overlay {
    position: fixed; inset: 0; z-index: 1200;
    background: rgba(0, 0, 0, 0.6); backdrop-filter: blur(4px);
    display: flex; align-items: center; justify-content: center;
    animation: merge-fade 140ms ease-out;
  }
  @keyframes merge-fade { from { opacity: 0 } to { opacity: 1 } }
  .merge-modal {
    width: min(440px, 92vw);
    background: var(--bg-card); border: 1px solid var(--border);
    border-radius: var(--radius-lg, 12px); padding: 20px 22px;
    box-shadow: 0 24px 80px rgba(0, 0, 0, 0.55);
  }
  .merge-modal h3 { margin: 0 0 8px; font-size: 16px; font-weight: 600; color: var(--fg-strong); }
  .merge-span { margin: 0 0 14px; font-size: 13px; line-height: 1.5; color: var(--fg-muted); }
  .merge-span strong { color: var(--fg); font-variant-numeric: tabular-nums; }
  .merge-gap {
    display: flex; align-items: flex-start; gap: 8px; margin: 0 0 16px;
    font-size: 12px; color: var(--fg); cursor: pointer; line-height: 1.4;
  }
  .merge-gap input { margin-top: 1px; accent-color: var(--accent); cursor: pointer; flex-shrink: 0; }
  .merge-gap-note { display: block; color: var(--fg-muted); font-size: 11px; margin-top: 2px; }
  .merge-actions { display: flex; justify-content: flex-end; gap: 8px; }
  .merge-actions button {
    padding: 7px 16px; border-radius: 8px; font-family: inherit; font-size: 13px; font-weight: 500;
    border: 1px solid var(--border); background: transparent; color: var(--fg); cursor: pointer;
    transition: background var(--dur-2), border-color var(--dur-2), filter var(--dur-2);
  }
  .merge-actions .ghost:hover:not(:disabled) { background: var(--bg-card-hover); border-color: var(--border-strong); }
  .merge-actions .primary { background: var(--accent); border-color: var(--accent); color: var(--accent-fg); }
  .merge-actions .primary:hover:not(:disabled) { filter: brightness(1.08); }
  .merge-actions button:disabled { opacity: 0.55; cursor: default; }
</style>
