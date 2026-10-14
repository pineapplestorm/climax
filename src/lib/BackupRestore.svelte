<!--
  Backup and restore (Settings page). Snapshots the whole climax.sqlite for the
  CURRENT profile to a folder you pick (timestamped, never overwriting) via
  SQLite VACUUM INTO, and restores one back. Restore stages the chosen file and
  RESTARTS the app so the swap happens before the DB pool reopens (you can't
  hot-swap an open SQLite file on Windows). A safety copy of the current library
  is taken before any restore.

  Renders embedded inside the paged Settings window; the modal supplies the page
  title + chrome, so this only draws the body.
-->
<script lang="ts">
  import { w } from "$lib/wording.svelte";
  import { onMount } from "svelte";
  import {
    api,
    formatDateOnly,
    formatTimeOnly,
    type BackupSetting,
  } from "$lib/api";
  import type { RestoreCandidate } from "$lib/api";
  import { getAuthToken, getClientServer, getServerBase, isTauri } from "$lib/transport";
  import ConfirmDialog from "./ConfirmDialog.svelte";

  // In client mode the real database lives on the external server; the back up /
  // restore / reset commands here are NATIVE (they run locally) and would operate
  // on this app's throwaway local cache, so we don't offer them - we show a notice
  // instead (the backend also rejects them). Client mode is fixed at boot.
  const clientMode = getClientServer() !== null;
  // Pure browser web client (Phase 5): no native dialogs or local files, but the
  // SERVER can snapshot its own database and stream it down as a normal browser
  // download (GET /backup, same-origin). Restore stays a desktop/server-side
  // operation for now.
  const webClient = !isTauri();

  let downloading = $state(false);
  async function downloadBackup() {
    if (downloading) return;
    downloading = true;
    msg = null;
    try {
      // Honour the transport's origin override (same-origin by default) and
      // send the auth token, like every data command does.
      const token = getAuthToken();
      const res = await fetch(`${getServerBase()}/backup`, {
        headers: token ? { "X-Climax-Token": token } : {},
      });
      if (!res.ok) {
        let m = "Couldn't create a backup. Try again in a moment.";
        try {
          const j = await res.json();
          if (j && typeof j.error === "string") m = j.error;
        } catch { /* non-JSON error body - keep the fallback */ }
        throw m;
      }
      // Guard against saving the SPA fallback (HTML) as a .sqlite - only a real
      // snapshot response carries the octet-stream type.
      const ct = res.headers.get("content-type") ?? "";
      if (!ct.includes("application/octet-stream")) {
        throw "Couldn't create a backup. Try again in a moment.";
      }
      const cd = res.headers.get("content-disposition") ?? "";
      const name = /filename="([^"]+)"/.exec(cd)?.[1] ?? "climax-backup.sqlite";
      const blob = await res.blob();
      const url = URL.createObjectURL(blob);
      const a = document.createElement("a");
      a.href = url;
      a.download = name;
      // In-document click + deferred revoke: revoking synchronously can abort
      // the download in stricter browsers.
      document.body.appendChild(a);
      a.click();
      a.remove();
      setTimeout(() => URL.revokeObjectURL(url), 10_000);
      msg = { text: "Backup downloaded. Keep it somewhere safe.", tone: "ok" };
    } catch (e) {
      console.error("web backup failed", e);
      msg = { text: humanError(e, "Couldn't create a backup. Try again in a moment."), tone: "err" };
    } finally {
      downloading = false;
    }
  }

  let settings = $state<BackupSetting | null>(null);
  let dataFolder = $state<string | null>(null);
  let backingUp = $state(false);
  let restoring = $state(false);
  let pendingRestore = $state<RestoreCandidate | null>(null);
  let msg = $state<{ text: string; tone: "ok" | "err" } | null>(null);

  // ---- Web client: restore-from-upload + reset (Phase 5 follow-on) ----
  // The browser can't hand the server a file path, so restore is an UPLOAD:
  // POST the backup's bytes to /restore; the server validates, saves a
  // pre-restore safety copy, stages the file, and exits - its supervisor
  // (Docker restart policy / systemd) brings it back with the backup applied.
  // Reset stages a wipe the same way. Both are token-gated server-side.
  let webRestoreFile = $state<File | null>(null);
  let fileInput = $state<HTMLInputElement | null>(null);

  function pickWebRestore() {
    if (downloading || restoring || resetting) return;
    msg = null;
    fileInput?.click();
  }
  function onWebFilePicked(e: Event) {
    const input = e.currentTarget as HTMLInputElement;
    const f = input.files?.[0] ?? null;
    input.value = ""; // so picking the same file again re-fires change
    if (f) webRestoreFile = f;
  }

  // The server exits ~1s after accepting a restore/reset and the supervisor
  // restarts it. Wait out the exit, then poll /health until it answers, then
  // reload so every view re-reads the swapped database.
  async function waitForServerBack(): Promise<void> {
    await new Promise((r) => setTimeout(r, 3000));
    const deadline = Date.now() + 120_000;
    while (Date.now() < deadline) {
      try {
        const r = await fetch(`${getServerBase()}/health`, { cache: "no-store" });
        if (r.ok) {
          location.reload();
          return;
        }
      } catch {
        /* still down - keep polling */
      }
      await new Promise((r) => setTimeout(r, 1500));
    }
    msg = {
      text: "The server hasn't come back yet. Check that it's running, then reload this page.",
      tone: "err",
    };
    restoring = false;
    resetting = false;
  }

  async function confirmWebRestore() {
    const f = webRestoreFile;
    if (!f || restoring || resetting) return;
    webRestoreFile = null;
    restoring = true;
    msg = { text: "Uploading the backup...", tone: "ok" };
    try {
      const token = getAuthToken();
      const res = await fetch(`${getServerBase()}/restore`, {
        method: "POST",
        headers: {
          // octet-stream is required server-side (its CSRF guard) and is the
          // honest type for raw sqlite bytes.
          "Content-Type": "application/octet-stream",
          ...(token ? { "X-Climax-Token": token } : {}),
        },
        body: f,
      });
      if (!res.ok) {
        let m = "Couldn't restore that backup. Nothing was changed.";
        try {
          const j = await res.json();
          if (j && typeof j.error === "string") m = j.error;
        } catch { /* non-JSON error body - keep the fallback */ }
        throw m;
      }
      msg = { text: "Backup accepted. The server is restarting...", tone: "ok" };
      await waitForServerBack();
    } catch (e) {
      console.error("web restore failed", e);
      if (e instanceof TypeError) {
        // The connection dropped mid-request, so we can't know whether the
        // server accepted it (a response lost AFTER staging looks identical to
        // a lost upload). Don't claim "nothing was changed" - poll instead: if
        // the server restarted, the restore applied; if it never went down,
        // the reload is a harmless no-op.
        msg = { text: "Connection interrupted. Checking the server...", tone: "ok" };
        await waitForServerBack();
        return;
      }
      msg = { text: humanError(e, "Couldn't restore that backup. Nothing was changed."), tone: "err" };
      restoring = false;
    }
  }

  async function webReset() {
    resetting = true;
    msg = { text: "Resetting...", tone: "ok" };
    try {
      const token = getAuthToken();
      const res = await fetch(`${getServerBase()}/reset`, {
        method: "POST",
        // The custom header is the server's CSRF guard - required.
        headers: { "X-Climax-Reset": "1", ...(token ? { "X-Climax-Token": token } : {}) },
      });
      if (!res.ok) {
        let m = "Reset failed - nothing was deleted.";
        try {
          const j = await res.json();
          if (j && typeof j.error === "string") m = j.error;
        } catch { /* non-JSON error body - keep the fallback */ }
        throw m;
      }
      msg = { text: "Reset accepted. The server is restarting...", tone: "ok" };
      await waitForServerBack();
    } catch (e) {
      console.error("web reset failed", e);
      if (e instanceof TypeError) {
        // Same uncertainty as the restore path: a dropped connection can't
        // distinguish "never arrived" from "accepted, response lost". Poll
        // rather than falsely claiming nothing was deleted.
        msg = { text: "Connection interrupted. Checking the server...", tone: "ok" };
        await waitForServerBack();
        return;
      }
      msg = { text: humanError(e, "Reset failed - nothing was deleted."), tone: "err" };
      resetting = false;
    }
  }

  // Backend backup/restore rejections are a mix: designed sentences ("That's
  // Climax's live database; restore a saved backup file instead.") and raw
  // io/driver chains. Pass a designed sentence through as-is; swap anything
  // raw for the fallback (the raw text still goes to the console).
  function humanError(e: unknown, fallback: string): string {
    const s = String(e).trim();
    const looksDesigned =
      /^[A-Z]/.test(s) && s.endsWith(".") && !s.includes(": ") && s.length <= 200;
    return looksDesigned ? s : fallback;
  }

  const lastBackupLabel = $derived(
    settings?.last_backup_at
      ? `${formatDateOnly(settings.last_backup_at)} · ${formatTimeOnly(settings.last_backup_at)}`
      : null,
  );

  onMount(async () => {
    if (clientMode || webClient) return; // no local-file flow to prime
    try {
      settings = await api.backupSettingsGet();
    } catch {
      settings = { dest_folder: null, last_backup_at: null };
    }
    dataFolder = await api.backupDataFolder().catch(() => null);
  });

  async function backupNow() {
    if (backingUp || restoring) return;
    backingUp = true;
    msg = null;
    try {
      const r = await api.backupNow();
      if (r) {
        settings = r.settings;
        msg = { text: `Backup saved to ${r.path}`, tone: "ok" };
      }
      // null => user cancelled the folder picker; leave the page quiet.
    } catch (e) {
      console.error("backup failed", e);
      msg = { text: humanError(e, "Backup failed. Check that the folder is writable and has free space."), tone: "err" };
    } finally {
      backingUp = false;
    }
  }

  async function pickRestore() {
    if (backingUp || restoring) return;
    msg = null;
    try {
      const c = await api.backupRestorePick();
      if (c) pendingRestore = c;
      // null => cancelled; an invalid file throws and is shown below.
    } catch (e) {
      console.error("restore pick failed", e);
      msg = { text: humanError(e, "Can't restore this file - it isn't a usable Climax backup."), tone: "err" };
    }
  }

  function confirmRestore() {
    const c = pendingRestore;
    if (!c) return;
    pendingRestore = null;
    restoring = true;
    msg = { text: "Restoring... Climax will restart to finish.", tone: "ok" };
    // On success the app restarts, so this never resolves; only catch matters.
    api.backupRestoreApply(c.path).catch((e) => {
      console.error("restore failed", e);
      restoring = false;
      msg = { text: humanError(e, "Restore failed. Your library hasn't been changed."), tone: "err" };
    });
  }

  function cancelRestore() {
    pendingRestore = null;
  }

  // ---- Danger zone: full reset (wipe everything + restart) ----
  // Two deliberate steps: a danger dialog, then type-to-confirm.
  let resetStage = $state<null | "confirm" | "type">(null);
  let resetTyped = $state("");
  let resetting = $state(false);

  function startReset() {
    if (backingUp || restoring || resetting || downloading) return;
    resetTyped = "";
    resetStage = "confirm";
  }
  function reallyReset() {
    if (resetTyped.trim().toUpperCase() !== "RESET") return;
    resetStage = null;
    if (webClient) {
      // Web client: the SERVER wipes itself and restarts (see webReset).
      webReset();
      return;
    }
    resetting = true;
    msg = { text: "Resetting Climax. The app will restart in a moment.", tone: "ok" };
    // On success the app restarts, so this never resolves; only catch matters.
    api.backupReset().catch((e) => {
      console.error("reset failed", e);
      resetting = false;
      msg = { text: humanError(e, "Reset failed - nothing was deleted."), tone: "err" };
    });
  }
</script>

<div class="backup-page">
  {#if webClient}
    <section class="setting-group">
      <div class="group-header"><h3>Back up your library</h3></div>
      <p class="group-help">
        Download a single-file snapshot of everything Climax holds: sessions,
        {w("cumshots")}, watch time, imported Stash history, and settings. The server
        creates the snapshot and your browser saves it like any download - so you
        can back up from whichever device you're viewing on.
      </p>
      <div class="actions">
        <button class="action primary" onclick={downloadBackup} disabled={downloading || restoring || resetting}>
          {downloading ? "Preparing backup..." : "Download a backup"}
        </button>
      </div>
    </section>

    <section class="setting-group">
      <div class="group-header"><h3>Restore from a backup</h3></div>
      <p class="group-help">
        Replace the server's whole library with a backup's contents. The server
        saves a safety copy of its current library first, then restarts to swap
        the backup in - this page reconnects on its own once it's back.
      </p>
      <div class="actions">
        <button class="action" onclick={pickWebRestore} disabled={downloading || restoring || resetting}>
          {restoring ? "Restoring..." : "Restore from a backup..."}
        </button>
      </div>
    </section>

    <section class="setting-group danger-zone">
      <div class="group-header"><h3>Danger zone</h3></div>
      <p class="group-help">
        Reset Climax to a clean slate. This wipes ALL data - every session,
        {w("cumshot")}, watch time, and imported Stash history, plus settings (including
        the Stash connection). The server then restarts empty (as if freshly
        installed) and the first-launch setup runs again. NOTE: This CANNOT be
        undone; restoring a backup is the ONLY way back, so download a backup
        first.
      </p>
      <div class="actions">
        <button class="action danger" onclick={startReset} disabled={downloading || restoring || resetting}>
          {resetting ? "Resetting..." : "Reset Climax..."}
        </button>
      </div>
    </section>

    <input type="file" accept=".sqlite" hidden bind:this={fileInput} onchange={onWebFilePicked} />

    {#if msg}
      <div class="status {msg.tone}">{msg.text}</div>
    {/if}
  {:else if clientMode}
    <section class="setting-group">
      <div class="group-header"><h3>Managed by the server</h3></div>
      <p class="group-help">
        This app is connected to an external Climax server, which owns the
        database. Backing up, restoring, and resetting are file operations that
        run on the server, not here, so they aren't available from this client. To
        manage this app's own local database instead, turn off "Connect to an
        external server" on the Server / connection page.
      </p>
    </section>
  {:else}
  <section class="setting-group">
    <div class="group-header"><h3>Back up your library</h3></div>
    <p class="group-help">
      Save a single-file snapshot of everything Climax holds: sessions, {w("cumshots")},
      watch time, imported Stash history, and settings. It does not include Stash
      itself or your video files. Each backup is timestamped, so a new one never
      overwrites an older one.
    </p>
    <div class="meta-line">
      {#if lastBackupLabel}
        <span class="meta">Last backup: <strong>{lastBackupLabel}</strong></span>
        {#if settings?.dest_folder}
          <span class="folder" title={settings.dest_folder}>{settings.dest_folder}</span>
        {/if}
      {:else}
        <span class="meta muted">No backups yet</span>
      {/if}
    </div>
    <div class="actions">
      <button class="action primary" onclick={backupNow} disabled={backingUp || restoring}>
        {backingUp ? "Backing up..." : "Back up now"}
      </button>
    </div>
  </section>

  <section class="setting-group">
    <div class="group-header"><h3>Restore from a backup</h3></div>
    <p class="group-help">
      Replace your whole library with a backup's contents. Climax saves your
      current library as a safety copy first, then restarts to swap the backup in.
      Use this to roll back a mistake or move your library to another computer.
    </p>
    <div class="actions">
      <button class="action" onclick={pickRestore} disabled={backingUp || restoring}>
        {restoring ? "Restoring..." : "Restore from a backup..."}
      </button>
    </div>
    {#if dataFolder}
      <p class="safety-note">
        Before each restore, your current library is saved here as a
        <code>climax-pre-restore-...</code> file. To roll back a restore, restore that file.
      </p>
      <div class="folder-path">{dataFolder}</div>
    {/if}
  </section>

  <section class="setting-group danger-zone">
    <div class="group-header"><h3>Danger zone</h3></div>
    <p class="group-help">
      Reset Climax to a clean slate. This wipes ALL data - every session, {w("cumshot")},
      watch time, and imported Stash history, plus your settings (including the
      Stash connection). Climax will then restart (as if freshly installed) and
      run the first-launch setup again. NOTE: This CANNOT be undone; restoring a
      backup is the ONLY way back, so make sure to save a backup first.
    </p>
    <div class="actions">
      <button class="action danger" onclick={startReset} disabled={backingUp || restoring || resetting}>
        {resetting ? "Resetting..." : "Reset Climax..."}
      </button>
    </div>
  </section>

  {#if msg}
    <div class="status {msg.tone}">{msg.text}</div>
  {/if}
  {/if}
</div>

{#if resetStage === "confirm"}
  <ConfirmDialog
    title="Reset everything?"
    message={"This permanently deletes your entire library and all settings, including the Stash connection. It can't be undone. The only way back is restoring from a backup."}
    confirmLabel="Continue"
    cancelLabel="Cancel"
    variant="danger"
    onConfirm={() => (resetStage = "type")}
    onCancel={() => (resetStage = null)}
  />
{/if}

<!-- svelte-ignore a11y_click_events_have_key_events a11y_no_noninteractive_element_interactions -->
{#if resetStage === "type"}
  <div class="overlay" role="dialog" aria-modal="true" tabindex="-1" onclick={() => (resetStage = null)}>
    <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
    <div class="dialog" role="document" onclick={(e) => e.stopPropagation()}>
      <h3>Type RESET to confirm</h3>
      <p>This is the last step. Typing <strong>RESET</strong> wipes everything and restarts {webClient ? "the server" : "Climax"}.</p>
      <!-- svelte-ignore a11y_autofocus -->
      <input
        class="reset-input"
        bind:value={resetTyped}
        placeholder="RESET"
        autocomplete="off"
        autocapitalize="off"
        spellcheck="false"
        autofocus
        onkeydown={(e) => { if (e.key === 'Enter') reallyReset(); }}
      />
      <div class="dialog-actions">
        <button class="action" onclick={() => (resetStage = null)}>Cancel</button>
        <button class="action danger" disabled={resetTyped.trim().toUpperCase() !== "RESET"} onclick={reallyReset}>
          Reset everything
        </button>
      </div>
    </div>
  </div>
{/if}

{#if pendingRestore}
  <ConfirmDialog
    title="Replace your library with this backup?"
    message={`${pendingRestore.file_name} will replace your entire Climax library. Climax will safely save a copy of what you have now in case you wish to revert back, then restart to apply the change.`}
    confirmLabel="Replace and restart"
    cancelLabel="Cancel"
    variant="danger"
    onConfirm={confirmRestore}
    onCancel={cancelRestore}
  />
{/if}

{#if webRestoreFile}
  <ConfirmDialog
    title="Replace the server's library with this backup?"
    message={`${webRestoreFile.name} will replace the entire library on the server. The server safely saves a copy of what it holds now in case you wish to revert back, then restarts to apply the change.`}
    confirmLabel="Replace and restart"
    cancelLabel="Cancel"
    variant="danger"
    onConfirm={confirmWebRestore}
    onCancel={() => (webRestoreFile = null)}
  />
{/if}

<style>
  .backup-page {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 24px 28px 28px;
    display: flex;
    flex-direction: column;
    gap: 40px;
  }

  .setting-group {
    display: flex;
    flex-direction: column;
  }
  .group-header h3 {
    margin: 0 0 5px;
    font-size: 18px;
    font-weight: 600;
    letter-spacing: -0.01em;
    color: var(--fg-strong, #f7f8fa);
  }
  .group-help {
    margin: 0 0 16px;
    font-size: 14px;
    line-height: 1.55;
    color: var(--fg-muted, #8a909e);
    max-width: 64ch;
    text-wrap: pretty;
  }

  /* Where each restore drops its pre-restore safety copy - so the user can find
     it and roll back by restoring it. */
  .safety-note {
    margin: 14px 0 0;
    font-size: 13px;
    line-height: 1.5;
    color: var(--fg-muted, #8a909e);
    max-width: 64ch;
  }
  .safety-note code {
    font-family: var(--font-mono, monospace);
    font-size: 12px;
    color: var(--fg, #eceef3);
  }
  .folder-path {
    margin-top: 6px;
    font-family: var(--font-mono, monospace);
    font-size: 12px;
    color: var(--fg-subtle, #5c6273);
    user-select: all;
    overflow-wrap: anywhere;
    max-width: 64ch;
  }

  .meta-line {
    display: flex;
    flex-direction: column;
    gap: 2px;
    margin-bottom: 14px;
    min-width: 0;
  }
  .meta {
    font-size: 13px;
    color: var(--fg-muted, #8a909e);
    font-variant-numeric: tabular-nums;
  }
  .meta strong {
    color: var(--fg, #eceef3);
    font-weight: 600;
  }
  .meta.muted {
    color: var(--fg-subtle, #5c6273);
  }
  .folder {
    font-size: 11px;
    color: var(--fg-subtle, #5c6273);
    font-family: var(--font-mono, monospace);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    max-width: 100%;
  }

  .actions {
    display: flex;
    gap: 8px;
  }
  .action {
    padding: 8px 16px;
    background: var(--bg-card-hover, #1b1e26);
    color: var(--fg, #eceef3);
    border: 1px solid var(--border, #252934);
    border-radius: var(--radius-sm, 6px);
    font-family: inherit;
    font-size: 13px;
    font-weight: 500;
    cursor: pointer;
    white-space: nowrap;
    transition: background 120ms, border-color 120ms, transform 80ms;
  }
  .action:hover:not(:disabled) {
    border-color: var(--border-strong, #353a47);
    background: var(--bg-elevated, #101218);
  }
  .action:active:not(:disabled) {
    transform: translateY(1px);
  }
  .action:disabled {
    opacity: 0.5;
    cursor: default;
  }
  .action.primary {
    background: var(--accent, #ef6b7a);
    border-color: var(--accent, #ef6b7a);
    color: var(--accent-fg, #fff);
  }
  .action.primary:hover:not(:disabled) {
    background: var(--accent-hover, #f58895);
    border-color: var(--accent-hover, #f58895);
  }
  .action.danger {
    background: var(--danger, #f26b6b);
    border-color: var(--danger, #f26b6b);
    color: #fff;
  }
  .action.danger:hover:not(:disabled) {
    background: #f47e7e;
    border-color: #f47e7e;
  }

  .danger-zone {
    border-top: 1px solid var(--border, #252934);
    padding-top: 24px;
  }
  .danger-zone .group-header h3 { color: var(--danger, #f26b6b); }

  /* Type-to-confirm modal (stage 2 of the reset). Mirrors ConfirmDialog. */
  .overlay {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.55);
    backdrop-filter: blur(4px);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 1500;
  }
  .dialog {
    background: #16181f;
    border: 1px solid #2a2d36;
    border-radius: 12px;
    padding: 22px 26px 18px;
    max-width: 420px;
    width: 92vw;
    box-shadow: 0 24px 60px rgba(0, 0, 0, 0.55);
  }
  .dialog h3 {
    margin: 0 0 8px;
    font-size: 16px;
    font-weight: 600;
    color: #f0f2f5;
  }
  .dialog p {
    margin: 0 0 14px;
    font-size: 13px;
    color: #b8bcc4;
    line-height: 1.55;
  }
  .dialog p strong { color: #f2e8d4; letter-spacing: 0.06em; }
  .reset-input {
    width: 100%;
    box-sizing: border-box;
    padding: 9px 12px;
    margin-bottom: 16px;
    background: #0f1116;
    border: 1px solid #2a2d36;
    border-radius: 8px;
    color: #f0f2f5;
    font-family: inherit;
    font-size: 14px;
    letter-spacing: 0.1em;
  }
  .reset-input:focus { outline: none; border-color: var(--danger, #f26b6b); }
  .dialog-actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }

  .status {
    font-size: 12px;
    line-height: 1.45;
    border-radius: var(--radius-md, 8px);
    padding: 9px 12px;
    word-break: break-word;
  }
  .status.ok {
    color: var(--live, #4ade80);
    background: rgba(74, 222, 128, 0.07);
    border: 1px solid rgba(74, 222, 128, 0.25);
  }
  .status.err {
    color: var(--danger, #f26b6b);
    background: rgba(242, 107, 107, 0.07);
    border: 1px solid rgba(242, 107, 107, 0.25);
  }
</style>
