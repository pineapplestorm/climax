<!--
  First-launch setup wizard. Shown once on a fresh install (real profile) over the
  dashboard; boot forces the dashboard window so it's seen. Flow:
    welcome -> connect (Stash URL + key, tested) -> bridge (install the Stash
    plugin) -> import (mirror sync) -> estimate (bulk reconstruction) -> done.
  Welcome also carries the host-vs-client fork: someone who already runs a Climax
  server has no library to set up here, and every step past this one would write
  to a local DB that goes unread the moment they switch. So they get out at the
  top rather than discovering the connect step's "skip for now" - which asks a
  different question (Stash isn't up yet, I'll do this later).

  Connect is required to advance (with a low-key "skip for now" that re-arms the
  wizard next launch); the bridge / import / estimate steps are skippable. The
  estimate step carries the honesty disclaimer that Stash logs WHEN you watched,
  not how long each sitting was, so the sessions are best-guesses (tagged
  "estimated", editable).

  Re-runnable from Settings -> General, which passes `canCancel` so an X and
  Escape close it outright. A first run gets neither - there is nothing to go
  back to. Note the re-run does NOT clear `onboarding_completed` first: it used
  to, which meant backing out left setup marked unfinished and the wizard fired
  on every launch until you walked it to the end. The flag now only ever moves
  to true, when Finish runs.
-->
<script lang="ts">
  import { w } from "$lib/wording.svelte";
  import { onMount } from "svelte";
  import Icon from "$lib/Icon.svelte";
  import { api, type MirrorStatus, type TestConnectionResult, type BridgeStatus } from "$lib/api";
  import { serverConfig } from "$lib/server-config.svelte";
  import { isTauri } from "$lib/transport";

  type Props = {
    onClose: () => void;
    /** True when the user opened this themselves from Settings, which makes it
     *  cancellable: an X and Escape both just close it, leaving everything as it
     *  was. False on a genuine first run, where there is nothing to go back to
     *  and the only way past is the per-step skip (which correctly leaves setup
     *  unfinished, so the wizard returns next launch). */
    canCancel?: boolean;
    /** Take the client branch: close the wizard and land on Settings -> Server /
     *  connection. Only reachable from the welcome step, and only in the one
     *  situation where it makes sense (see `serverBranch`). */
    onConnectToServer?: () => void;
  };
  let { onClose, canCancel = false, onConnectToServer }: Props = $props();

  /** Whether to offer the "I already run a server" fork on the welcome step.
   *  Two exclusions, both cases where the fork would be nonsense: a browser tab
   *  IS a client of a server already (Settings hides the Server page there for
   *  the same reason), and a desktop app in client mode is connected to one. The
   *  fork deliberately does NOT mark onboarding complete - if the user backs out
   *  of the server page the wizard should still be waiting, and once they do
   *  switch, the flag stops being read at all (the native `onboarding_needed`
   *  returns false on `!stash_enabled`, and the frontend call routes to the
   *  server's own flag). */
  const serverBranch = $derived(
    onConnectToServer !== undefined && isTauri() && serverConfig.url === null,
  );

  type Step =
    | "welcome"
    | "connect"
    | "bridge"
    | "import"
    | "importing"
    | "estimate"
    | "estimating"
    | "done";
  let step = $state<Step>("welcome");

  // ---- Connect step ----
  let stashUrl = $state("http://localhost:9999");
  let stashApiKey = $state("");
  let showApiKey = $state(false);
  let testing = $state(false);
  let testResult = $state<TestConnectionResult | null>(null);
  const connectionOk = $derived(testResult?.ok === true);

  // ---- Bridge step ----
  let bridgeStatus = $state<BridgeStatus | null>(null);
  let bridgeBusy = $state(false); // installing
  let bridgeError = $state<string | null>(null);
  // Set when we couldn't even READ the bridge status (Stash unreachable) and
  // have no prior reading to fall back on, so the step shows an offline message
  // + Retry instead of an endless "Checking..." spinner. Cleared on any success.
  let bridgeProbeError = $state<string | null>(null);

  // ---- Import / estimate steps ----
  let imported = $state(false); // did the user run the import this run?
  let estimatedCount = $state<number | null>(null);
  let importStatus = $state<MirrorStatus | null>(null);
  let errorMsg = $state<string | null>(null);
  let busy = $state(false);

  const importPct = $derived(
    importStatus && importStatus.total > 0
      ? Math.min(100, Math.round((importStatus.done / importStatus.total) * 100))
      : null,
  );

  const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

  // Prefill the connection inputs with whatever's already saved (defaults to
  // localhost:9999 on a fresh install).
  onMount(async () => {
    try {
      const conn = await api.stashConnectionGet();
      stashUrl = conn.url || "http://localhost:9999";
      stashApiKey = conn.api_key ?? "";
    } catch (e) {
      console.error("stash_connection_get failed", e);
    }
  });

  // Poll the bridge status while the bridge step is showing (so opening /
  // refreshing Stash, or an in-flight install completing, reflects live). Paused
  // during an install (bridgeBusy) so a stale read can't clobber the result.
  $effect(() => {
    if (step !== "bridge" || bridgeBusy) return;
    let cancelled = false;
    const tick = async () => {
      try {
        const s = await api.bridgeStatus();
        if (cancelled) return;
        bridgeStatus = s;
        bridgeProbeError = null;
      } catch (e) {
        // Once we have a reading, a transient blip just keeps the last status.
        // Before the first successful read there's nothing to keep, so surface
        // the outage rather than spin forever.
        if (!cancelled && bridgeStatus === null) bridgeProbeError = String(e);
      }
    };
    tick();
    const iv = setInterval(tick, 2000);
    return () => {
      cancelled = true;
      clearInterval(iv);
    };
  });

  // ---- Connect actions ----

  // The backend classifies Stash failures into plain sentences ("Couldn't
  // reach Stash at ...", "Stash rejected the request (HTTP 401). ..."). Show
  // those as-is; swap anything raw (an unexpected driver/transport chain) for
  // the fallback. The raw string goes to the console either way.
  function humanError(e: unknown, fallback: string): string {
    const s = String(e).trim();
    // Designed, but carries the GraphQL message after a colon - still show it.
    if (s.startsWith("Stash returned an error")) return s;
    const looksDesigned =
      /^[A-Z]/.test(s) && s.endsWith(".") && !s.includes(": ") && s.length <= 200;
    return looksDesigned ? s : fallback;
  }

  async function testConnection() {
    if (testing) return;
    testing = true;
    testResult = null;
    try {
      const key = stashApiKey.trim();
      await api.stashConnectionSet(stashUrl.trim(), key.length > 0 ? key : null);
      testResult = await api.stashConnectionTest();
      if (!testResult.ok && testResult.error) {
        console.error("stash connection test failed", testResult.error);
        testResult = {
          ...testResult,
          error: humanError(testResult.error, "Connection failed. Check the address and try again."),
        };
      }
    } catch (e) {
      // The test COMMAND itself failed (couldn't save the settings or reach
      // Climax's own backend) - not a verdict on Stash. Designed sentences
      // (client mode's "Couldn't reach the Climax server.") show as-is.
      console.error("stash connection set/test failed", e);
      testResult = {
        ok: false,
        version: null,
        scene_count: null,
        error: humanError(e, "Couldn't run the test. Try again."),
      };
    } finally {
      testing = false;
    }
  }

  // Close the wizard WITHOUT marking onboarding complete, so it re-arms next
  // launch (the user can finish setup once Stash is reachable).
  function skipSetup() {
    onClose();
  }

  // ---- Bridge actions ----
  async function installBridge() {
    if (bridgeBusy) return;
    bridgeError = null;
    bridgeBusy = true;
    try {
      bridgeStatus = await api.bridgeInstall();
      bridgeProbeError = null;
    } catch (e) {
      console.error("bridge_install failed", e);
      // Designed backend copy (the install-timeout message, "Couldn't reach
      // Stash. ...", HTTP/GraphQL verdicts) renders as-is; raw chains don't.
      bridgeError = humanError(e, "Couldn't install the bridge. Check that Stash is running, then try again.");
    } finally {
      bridgeBusy = false;
    }
  }

  async function recheckBridge() {
    bridgeError = null;
    try {
      bridgeStatus = await api.bridgeStatus();
      bridgeProbeError = null;
    } catch (e) {
      console.error("bridge_status failed", e);
      // No prior reading -> it's an outage (Retry primary); otherwise a blip.
      // bridgeProbeError is only truthiness-checked (a fixed offline message
      // renders), so it can hold the raw error.
      if (bridgeStatus === null) bridgeProbeError = String(e);
      else bridgeError = humanError(e, "Couldn't check the bridge. Try again.");
    }
  }

  // ---- Import / estimate actions ----
  async function runImport() {
    errorMsg = null;
    busy = true;
    step = "importing";
    try {
      await api.mirrorSyncNow();
      // Poll until the sync returns to idle (after we've seen it start). The
      // launch auto-sync may also be running; either way it converges to idle.
      let sawActive = false;
      let polls = 0;
      let completed = false;
      const deadline = Date.now() + 10 * 60 * 1000; // 10-min safety cap
      while (Date.now() < deadline) {
        await sleep(500);
        const s = await api.mirrorStatusGet();
        importStatus = s;
        polls++;
        if (s.phase !== "idle") sawActive = true;
        if (sawActive && s.phase === "idle") { completed = true; break; } // started, then finished
        if (!sawActive && s.phase === "idle" && polls >= 6) { completed = true; break; } // tiny / empty library
      }
      if (!completed) {
        // Hit the cap while a sync was still running - don't claim success.
        // Set it here rather than throwing. The catch below would otherwise have
        // to recognise this string to tell a timeout apart from a real failure,
        // and a matcher on copy breaks silently the moment the copy is edited.
        errorMsg =
          "The import is taking a while. It may still finish on its own - check Settings in a few minutes.";
        step = "import";
        return;
      }
      // Idle isn't success: a failed run also settles back to idle, leaving
      // its (designed) error on the status - surface it instead of advancing.
      const syncError = importStatus?.last_error;
      if (syncError) {
        console.error("history import failed", syncError);
        errorMsg = humanError(syncError, "The import couldn't finish. Try again, or run it later from Settings.");
        step = "import";
        return;
      }
      imported = true;
      step = "estimate";
    } catch (e) {
      console.error("history import failed", e);
      // Only real failures reach here now - the still-running-at-the-cap case
      // sets its own message above and returns, so there is nothing to tell
      // apart. The console has the detail.
      errorMsg = "The import couldn't finish. Try again, or run it later from Settings.";
      step = "import";
    } finally {
      busy = false;
    }
  }

  function skipImport() {
    imported = false;
    step = "done";
  }

  async function runEstimate() {
    errorMsg = null;
    busy = true;
    step = "estimating";
    try {
      estimatedCount = await api.onboardingEstimateSessions();
      step = "done";
    } catch (e) {
      console.error("onboarding_estimate_sessions failed", e);
      errorMsg = "Couldn't estimate your sessions. Try again, or do it later from Settings.";
      step = "estimate";
    } finally {
      busy = false;
    }
  }

  function skipEstimate() {
    step = "done";
  }

  async function finish() {
    busy = true;
    try {
      await api.onboardingComplete();
    } catch (e) {
      console.error("onboarding_complete failed", e);
    }
    onClose();
  }
</script>

<!-- Escape on the window rather than the overlay: the overlay only receives key
     events while something inside it holds focus, and on the first screen that
     is not guaranteed. Guarded in the handler rather than wrapped in an {#if} -
     svelte:window cannot sit inside a block. -->
<svelte:window onkeydown={(e) => { if (canCancel && e.key === "Escape") onClose(); }} />

<div class="overlay" role="dialog" aria-modal="true" tabindex="-1">
  <div class="card">
    {#if canCancel}
      <button class="wiz-close" onclick={onClose} title="Close setup" aria-label="Close setup">
        <Icon name="x" size={15} />
      </button>
    {/if}
    {#if step === "welcome"}
      <img class="logo" src="/climax-icon.png" alt="Climax" width="44" height="44" />
      <h2>Welcome to Climax</h2>
      <p>
        Climax is a full-featured expansion of Stash's limited built-in stats tracking.
        It builds on the history Stash already keeps, adding sessions, watch time,
        trends and records, and syncing back as you go. Let's get you set up. It only
        takes a few moments.
      </p>
      <div class="actions end">
        <button class="primary" onclick={() => (step = "connect")}>Get started</button>
      </div>
      {#if serverBranch}
        <div class="branch">
          <p class="muted">
            Already running Climax on a server? You don't need to set any of this up
            here.
          </p>
          <button class="ghost" onclick={onConnectToServer}>Connect to a server</button>
        </div>
      {/if}

    {:else if step === "connect"}
      <h2>Connect to Stash</h2>
      <p>
        Climax reads your scene metadata and play history straight from Stash. Enter
        your Stash address, then test the connection.
      </p>
      <label class="field">
        <!-- The example lives in the LABEL, not the placeholder: this field is
             prefilled on mount (saved value, or the localhost default), so the
             placeholder below only ever shows if you clear the box. Both examples
             carry http:// on purpose - a bare host:port is rejected before it
             reaches Stash. -->
        <span class="field-label">
          Stash address
          <span class="muted">e.g. http://localhost:9999 or http://192.168.x.x:9999</span>
        </span>
        <input
          class="input"
          type="text"
          bind:value={stashUrl}
          placeholder="http://localhost:9999"
          spellcheck="false"
          autocomplete="off"
          oninput={() => (testResult = null)}
        />
      </label>
      <label class="field">
        <span class="field-label">API key <span class="muted">(optional)</span></span>
        <span class="key-row">
          <input
            class="input"
            type={showApiKey ? "text" : "password"}
            bind:value={stashApiKey}
            placeholder="Leave empty if Stash has no authentication"
            spellcheck="false"
            autocomplete="off"
            oninput={() => (testResult = null)}
          />
          <button type="button" class="key-toggle" onclick={() => (showApiKey = !showApiKey)}>
            {showApiKey ? "Hide" : "Show"}
          </button>
        </span>
      </label>
      <div class="test-row">
        <button class="ghost" disabled={testing} onclick={testConnection}>
          {testing ? "Testing..." : "Test connection"}
        </button>
        {#if testResult}
          {#if testResult.ok}
            <span class="status ok"
              >✓ Connected{testResult.version ? ` to Stash ${testResult.version}` : ""}{testResult.scene_count != null
                ? ` · ${testResult.scene_count.toLocaleString()} scenes`
                : ""}</span
            >
          {:else}
            <span class="status err">✗ {testResult.error ?? "Connection failed"}</span>
          {/if}
        {/if}
      </div>
      <div class="actions">
        <button class="link" onclick={skipSetup}>Skip for now</button>
        <button class="primary" disabled={!connectionOk} onclick={() => (step = "bridge")}>
          Continue
        </button>
      </div>

    {:else if step === "bridge"}
      <h2>Install the Stash bridge</h2>
      <p>
        The bridge is a small Stash plugin that tells Climax what you're watching in
        real time. Without it, Climax can still import your history but can't track
        new sessions live.
      </p>

      {#if bridgeStatus === null && bridgeProbeError}
        <div class="bridge-state">
          <span class="dot off"></span>
          <span class="muted">Couldn't reach Stash to check for the bridge.</span>
        </div>
      {:else if bridgeStatus === null}
        <div class="bridge-state">
          <div class="spinner small"></div>
          <span class="muted">Checking Stash...</span>
        </div>
      {:else if bridgeStatus.installed}
        <div class="bridge-state">
          <span class="dot {bridgeStatus.connected ? 'live' : 'idle'}"></span>
          <span>
            Bridge installed{bridgeStatus.version ? ` (v${bridgeStatus.version})` : ""}.
            {#if bridgeStatus.connected}
              <strong>Live now.</strong>
            {:else if bridgeStatus.enabled}
              <!-- Only worth suggesting when the plugin is actually switched on.
                   While it is disabled a refresh changes nothing, and the amber
                   line below gives the real fix - telling the user to refresh
                   first sends them to do something that cannot work. -->
              Refresh your Stash tab to activate it.
            {/if}
          </span>
        </div>
        {#if !bridgeStatus.enabled}
          <!-- The line above already says it is installed; this only has to say
               it is off and where to switch it on. "disabled / Enable" matches
               the wording Stash itself uses for the toggle. -->
          <p class="warn">
            It's disabled in Stash. Enable it under Settings → Plugins.
          </p>
        {/if}
      {:else}
        <div class="bridge-state">
          <span class="dot off"></span>
          <span class="muted">Not installed yet.</span>
        </div>
        <p class="muted small-note">
          Climax can install it for you from the public plugin index, or you can add
          "Climax Bridge" yourself in Stash under Settings → Plugins.
        </p>
      {/if}

      {#if bridgeError}<p class="error">{bridgeError}</p>{/if}

      <div class="actions">
        <div class="left">
          <!-- Always offer an escape while the bridge isn't confirmed installed
               (incl. the checking / offline states) so a Stash blip here can't
               trap the user. Skip advances the wizard to Import. -->
          {#if !bridgeStatus?.installed}
            <button class="link" onclick={() => (step = "import")}>Skip</button>
            {#if bridgeStatus && !bridgeStatus.installed}
              <button class="link" onclick={recheckBridge} disabled={bridgeBusy}>
                I already have it
              </button>
            {/if}
          {/if}
        </div>
        {#if bridgeStatus?.installed}
          <button class="primary" onclick={() => (step = "import")}>Continue</button>
        {:else if bridgeStatus === null && bridgeProbeError}
          <button class="primary" disabled={bridgeBusy} onclick={recheckBridge}>Try again</button>
        {:else if bridgeStatus === null}
          <button class="primary" disabled>Checking...</button>
        {:else}
          <button class="primary" disabled={bridgeBusy} onclick={installBridge}>
            {bridgeBusy ? "Installing..." : "Install bridge"}
          </button>
        {/if}
      </div>

    {:else if step === "import"}
      <h2>Import your Stash history?</h2>
      <p>
        Climax can pull in everything Stash already knows you've watched - play history
        and {w("cumshots")} - as far back as your records go. Your dashboard starts with real
        numbers instead of a blank slate.
      </p>
      <p class="muted">You can do this later from Settings.</p>
      {#if errorMsg}<p class="error">{errorMsg}</p>{/if}
      <div class="actions">
        <button class="ghost" onclick={skipImport}>Not now</button>
        <button class="primary" onclick={runImport}>Import history</button>
      </div>

    {:else if step === "importing"}
      <h2>Importing your history</h2>
      {#if importPct !== null}
        <div class="bar"><div class="fill" style="width:{importPct}%"></div></div>
        <p class="muted center">
          {importStatus?.done.toLocaleString()} of {importStatus?.total.toLocaleString()} scenes
        </p>
      {:else}
        <div class="spinner"></div>
        <p class="muted center">Reading your library...</p>
      {/if}

    {:else if step === "estimate"}
      <h2>Estimate your past sessions?</h2>
      <p>
        Stash records <em>when</em> you watched each scene, but not how long any one
        sitting lasted. Climax can group that history into best-guess sessions so your
        timeline and trends aren't empty.
      </p>
      <div class="callout">
        These are estimates, not records. You can edit or delete any of them later,
        and they'll stay tagged <strong>estimated</strong> in your Sessions list.
      </div>
      {#if errorMsg}<p class="error">{errorMsg}</p>{/if}
      <div class="actions">
        <button class="ghost" onclick={skipEstimate}>Not now</button>
        <button class="primary" onclick={runEstimate}>Estimate sessions</button>
      </div>

    {:else if step === "estimating"}
      <h2>Estimating sessions</h2>
      <div class="spinner"></div>
      <p class="muted center">This can take a moment for a large history.</p>

    {:else if step === "done"}
      <img class="logo" src="/climax-icon.png" alt="Climax" width="44" height="44" />
      <h2>You're all set</h2>
      {#if imported && estimatedCount !== null}
        <p>
          Imported your Stash history and created
          <strong>{estimatedCount.toLocaleString()}</strong>
          estimated {estimatedCount === 1 ? "session" : "sessions"}. Start a session
          any time from the tracker window.
        </p>
      {:else if imported}
        <p>Imported your Stash history. Start a session any time from the tracker window.</p>
      {:else}
        <p>
          You're starting fresh - Climax will track new sessions as you go. You can
          import your Stash history later from Settings.
        </p>
      {/if}
      <div class="actions end">
        <button class="primary" onclick={finish} disabled={busy}>Finish</button>
      </div>
    {/if}
  </div>
</div>

<style>
  .overlay {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.62);
    backdrop-filter: blur(5px);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 1600;
    animation: fade-in 160ms ease-out;
  }
  @keyframes fade-in { from { opacity: 0 } to { opacity: 1 } }

  .card {
    /* Anchors the close button below. */
    position: relative;
    background: #16181f;
    border: 1px solid #2a2d36;
    border-radius: 14px;
    padding: 30px 32px 22px;
    max-width: 480px;
    width: 92vw;
    box-shadow: 0 28px 70px rgba(0, 0, 0, 0.6);
    animation: pop-in 220ms cubic-bezier(0.2, 0.8, 0.2, 1);
  }
  /* Only rendered on a manual re-run. Quiet by default so it never competes
     with the step's own primary action. */
  .wiz-close {
    position: absolute;
    top: 12px;
    right: 12px;
    display: flex;
    align-items: center;
    padding: 5px;
    border: none;
    background: none;
    color: var(--fg-subtle, #5c6273);
    border-radius: var(--radius-sm, 6px);
    cursor: pointer;
    transition: color 120ms, background 120ms;
  }
  .wiz-close:hover {
    color: var(--fg, #eceef3);
    background: rgba(255, 255, 255, 0.06);
  }
  @keyframes pop-in {
    from { transform: translateY(8px) scale(0.98); opacity: 0 }
    to { transform: none; opacity: 1 }
  }

  .logo {
    width: 44px;
    height: 44px;
    border-radius: 10px;
    display: block;
    margin-bottom: 14px;
  }
  h2 {
    margin: 0 0 12px;
    font-size: 20px;
    font-weight: 600;
    color: #f4f6f8;
    letter-spacing: -0.01em;
  }
  p {
    margin: 0 0 12px;
    font-size: 13.5px;
    color: #c2c6cf;
    line-height: 1.6;
  }
  p em { color: #f4f6f8; font-style: italic; }
  p strong { color: #f4f6f8; font-weight: 600; }
  .muted { color: #8a909e; font-size: 12.5px; }
  .small-note { margin-top: 8px; }
  .center { text-align: center; }
  .warn {
    color: #f2c14e;
    font-size: 12.5px;
    margin: 8px 0 0;
  }
  .error {
    color: #f87171;
    font-size: 12.5px;
    margin: 4px 0 8px;
  }

  /* ---- Connect step ---- */
  .field {
    display: block;
    margin: 0 0 12px;
  }
  .field-label {
    display: block;
    font-size: 12px;
    color: #c2c6cf;
    margin-bottom: 5px;
  }
  .field-label .muted { font-size: 11px; }
  .input {
    width: 100%;
    box-sizing: border-box;
    padding: 8px 11px;
    border-radius: 8px;
    border: 1px solid #2a2d36;
    background: #0f1116;
    color: #e7e9ed;
    font-size: 13px;
    font-family: inherit;
    outline: none;
    transition: border-color 120ms;
  }
  .input:focus { border-color: #ef6b7a; }
  .input::placeholder { color: #5b616e; }
  .key-row {
    display: flex;
    gap: 8px;
    align-items: stretch;
  }
  .key-row .input { flex: 1; }
  .key-toggle {
    padding: 0 12px;
    border-radius: 8px;
    border: 1px solid #2a2d36;
    background: #1b1d25;
    color: #c2c6cf;
    font-size: 12px;
    cursor: pointer;
    font-family: inherit;
  }
  .key-toggle:hover { background: #23262f; }
  .test-row {
    display: flex;
    align-items: center;
    gap: 12px;
    flex-wrap: wrap;
    margin: 4px 0 4px;
  }
  .status {
    font-size: 12.5px;
    line-height: 1.4;
  }
  .status.ok { color: #4ade80; }
  .status.err { color: #f87171; }

  /* ---- Bridge step ---- */
  .bridge-state {
    display: flex;
    align-items: center;
    gap: 10px;
    background: #1b1d25;
    border: 1px solid #2a2d36;
    border-radius: 8px;
    padding: 11px 14px;
    font-size: 13px;
    color: #c2c6cf;
    margin: 2px 0 4px;
  }
  .bridge-state strong { color: #4ade80; font-weight: 600; }
  .dot {
    width: 9px;
    height: 9px;
    border-radius: 50%;
    flex: 0 0 auto;
    background: #5b616e;
  }
  .dot.off { background: #5b616e; }
  .dot.idle { background: #f2c14e; }
  .dot.live {
    background: #4ade80;
    box-shadow: 0 0 0 0 rgba(74, 222, 128, 0.6);
    animation: pulse 1.6s ease-out infinite;
  }
  @keyframes pulse {
    to { box-shadow: 0 0 0 7px rgba(74, 222, 128, 0); }
  }

  .callout {
    background: #1b1d25;
    border: 1px solid #2a2d36;
    border-left: 3px solid #ef6b7a;
    border-radius: 8px;
    padding: 12px 14px;
    font-size: 12.5px;
    color: #c2c6cf;
    line-height: 1.55;
    margin: 0 0 16px;
  }
  .callout strong { color: #f2e8d4; font-weight: 600; }

  .actions {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 10px;
    margin-top: 20px;
  }
  .actions.end { justify-content: flex-end; }

  /* Welcome only: the host-vs-client fork. Sits BELOW the primary action behind
     a rule so it reads as the other road, not a second call to action. */
  .branch {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 14px;
    margin-top: 18px;
    padding-top: 16px;
    border-top: 1px solid #2a2d36;
  }
  .branch p {
    margin: 0;
    flex: 1;
  }
  .branch button { flex: 0 0 auto; }
  .actions .left {
    display: flex;
    align-items: center;
    gap: 4px;
  }

  button {
    padding: 9px 18px;
    border-radius: 9px;
    font-size: 13px;
    font-weight: 500;
    border: 1px solid #2a2d36;
    background: transparent;
    color: #e7e9ed;
    cursor: pointer;
    transition: background 120ms, border-color 120ms, opacity 120ms;
    font-family: inherit;
  }
  button:disabled { opacity: 0.6; cursor: default; }
  .ghost:hover { background: #1d1f28; border-color: #3a3e4a; }
  .primary {
    background: #ef6b7a;
    border-color: #ef6b7a;
    color: #fff;
    font-weight: 600;
  }
  .primary:hover:not(:disabled) { background: #f17e8b; border-color: #f17e8b; }
  .primary:disabled { background: #ef6b7a; border-color: #ef6b7a; }
  .link {
    background: transparent;
    border: none;
    color: #8a909e;
    padding: 9px 8px;
    font-size: 12.5px;
    font-weight: 500;
  }
  .link:hover:not(:disabled) { color: #c2c6cf; text-decoration: underline; }

  .bar {
    height: 8px;
    border-radius: 999px;
    background: #23262f;
    overflow: hidden;
    margin: 6px 0 10px;
  }
  .fill {
    height: 100%;
    background: #ef6b7a;
    border-radius: 999px;
    transition: width 300ms ease-out;
  }

  .spinner {
    width: 30px;
    height: 30px;
    border-radius: 50%;
    border: 3px solid #2a2d36;
    border-top-color: #ef6b7a;
    margin: 14px auto 12px;
    animation: spin 800ms linear infinite;
  }
  .spinner.small {
    width: 16px;
    height: 16px;
    border-width: 2px;
    margin: 0;
  }
  @keyframes spin { to { transform: rotate(360deg) } }
</style>
