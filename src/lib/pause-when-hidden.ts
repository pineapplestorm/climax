// Freeze every CSS animation while this window cannot be seen.
//
// Measured on the 1.1.0 desktop app: with a session running, each window kept
// drawing at full rate even when minimised or hidden in the tray, because the
// WebView is never told it went out of sight. The pulsing "live" dots alone cost
// about half a CPU core per window. This sets `html.cx-hidden` whenever the
// window is minimised, hidden, or the page reports itself hidden, and the rule
// in app.html pauses all animations under that class.
//
// The desktop app asks Tauri directly (minimised / visible), re-checking on the
// window's own resize and focus events plus a slow poll as a safety net, since
// hiding to the tray fires no event the page can hear. A browser tab only needs
// the Page Visibility API, which browsers already honour.

import { isTauri } from "$lib/transport";

const HIDDEN_CLASS = "cx-hidden";
const POLL_MS = 2000;

/** Start watching. Call once from a route's onMount; returns a cleanup. */
export function pauseAnimationsWhenHidden(): () => void {
  const root = document.documentElement;
  let stopped = false;
  const cleanups: Array<() => void> = [];

  const apply = (hidden: boolean) => {
    if (!stopped) root.classList.toggle(HIDDEN_CLASS, hidden);
  };

  let checking = false;
  async function check() {
    if (document.hidden) {
      apply(true);
      return;
    }
    if (!isTauri()) {
      apply(false);
      return;
    }
    if (checking) return;
    checking = true;
    try {
      const { getCurrentWindow } = await import("@tauri-apps/api/window");
      const win = getCurrentWindow();
      const [visible, minimised] = await Promise.all([win.isVisible(), win.isMinimized()]);
      apply(!visible || minimised);
    } catch (e) {
      // Leave animations running rather than freezing them on a failed check.
      console.warn("pause-when-hidden: window check failed", e);
      apply(false);
    } finally {
      checking = false;
    }
  }

  const onVisibility = () => void check();
  document.addEventListener("visibilitychange", onVisibility);
  cleanups.push(() => document.removeEventListener("visibilitychange", onVisibility));

  if (isTauri()) {
    const timer = setInterval(() => void check(), POLL_MS);
    cleanups.push(() => clearInterval(timer));
    // Minimise and restore arrive as resizes; showing from the tray usually
    // brings focus. Either way the poll catches anything these miss.
    void (async () => {
      try {
        const { getCurrentWindow } = await import("@tauri-apps/api/window");
        const win = getCurrentWindow();
        const offResize = await win.onResized(() => void check());
        const offFocus = await win.onFocusChanged(() => void check());
        if (stopped) {
          offResize();
          offFocus();
        } else {
          cleanups.push(offResize, offFocus);
        }
      } catch (e) {
        console.warn("pause-when-hidden: could not listen for window events", e);
      }
    })();
  }

  void check();

  return () => {
    stopped = true;
    for (const fn of cleanups) fn();
    root.classList.remove(HIDDEN_CLASS);
  };
}
