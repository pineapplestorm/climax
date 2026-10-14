// The word the interface uses for an O: "cumshot" (the default) or "orgasm".
//
// DISPLAY ONLY. Every type, prop, command, setting key and CSS class keeps
// saying cumshot whichever is chosen - only what a person reads changes.
//
// How to use it: write the copy in the DEFAULT wording and pass it through
// `w()` on its way to the screen:
//
//   <span>{w("Cumshots today")}</span>
//   title={w("Log a cumshot on this scene")}
//   {w(`${n} cumshot${n === 1 ? "" : "s"}`)}
//
// `w()` rewrites the word (and "a cumshot" -> "an orgasm") when the setting
// asks for it, and returns the string untouched otherwise. Source stays readable
// and greppable in one wording, and a string that misses its `w()` simply shows
// the default instead of breaking. Never pass it an identifier, a metric key or
// anything that is compared or sent to the backend.
//
// The choice lives on the backend (settings key `wording`), so it follows the
// user to every client. This module holds the reactive copy: `w()` reads it, so
// any template calling `w()` re-renders the moment it changes.

import { emit, listen } from "@tauri-apps/api/event";
import { api } from "$lib/api";
import { isTauri } from "$lib/transport";

export type Wording = "cumshot" | "orgasm";

/** Last value seen, so a window paints the right word from its first frame
 *  instead of flashing the default while the backend read is in flight. */
const CACHE_KEY = "climax_wording";
/** Sent between this app's windows (tracker and dashboard are separate
 *  webviews) so a change made in one shows in the other at once. */
const CHANGED_EVENT = "wording_changed";

function isWording(v: unknown): v is Wording {
  return v === "cumshot" || v === "orgasm";
}

function cached(): Wording {
  try {
    const v = localStorage.getItem(CACHE_KEY);
    if (isWording(v)) return v;
  } catch {
    // No storage (private mode, SSR prerender): the default is fine.
  }
  return "cumshot";
}

class WordingStore {
  value = $state<Wording>(cached());

  private apply(v: Wording) {
    this.value = v;
    try {
      localStorage.setItem(CACHE_KEY, v);
    } catch {
      // Storage unavailable: the in-memory value still drives this window.
    }
  }

  /** Read the saved choice from the backend. Safe to call repeatedly. */
  async load(): Promise<void> {
    try {
      const raw = await api.wordingGet();
      if (isWording(raw)) this.apply(raw);
    } catch (e) {
      // An older server that predates the setting, or one that is unreachable.
      // Keep whatever is showing.
      console.warn("wording: load failed", e);
    }
  }

  /** Change the word everywhere: this window now, the backend, and this app's
   *  other window. Rolls back if the backend refuses. */
  async set(v: Wording): Promise<void> {
    const before = this.value;
    this.apply(v);
    try {
      const saved = await api.wordingSet(v);
      if (isWording(saved)) this.apply(saved);
      if (isTauri()) void emit(CHANGED_EVENT, this.value);
    } catch (e) {
      this.apply(before);
      throw e;
    }
  }

  /** Call once from a route's onMount. Loads the choice and keeps it current:
   *  the other window's changes arrive as an event, and a change made from
   *  another device is picked up when this window is next looked at. Returns
   *  a cleanup function. */
  start(): () => void {
    void this.load();
    const onFocus = () => void this.load();
    window.addEventListener("focus", onFocus);

    let unlisten: (() => void) | null = null;
    let stopped = false;
    if (isTauri()) {
      listen<string>(CHANGED_EVENT, (e) => {
        if (isWording(e.payload)) this.apply(e.payload);
      })
        .then((fn) => {
          if (stopped) fn();
          else unlisten = fn;
        })
        .catch((e) => console.warn("wording: listen failed", e));
    }
    return () => {
      stopped = true;
      window.removeEventListener("focus", onFocus);
      unlisten?.();
    };
  }
}

export const wording = new WordingStore();

/** Rewrite default-wording copy into the "orgasm" wording. Pure, so it can be
 *  tested without the store. Case follows the source ("Cumshots" -> "Orgasms"),
 *  and the article follows the new word ("a cumshot" -> "an orgasm"). */
export function toOrgasmWording(text: string): string {
  return text
    .replace(/\b([Aa])(\s+)(?=cumshots?\b)/g, "$1n$2")
    .replace(/\bcumshot(s?)\b/gi, (m, s: string) => {
      const upper = m === m.toUpperCase();
      const capital = m[0] === "C";
      const out = "orgasm" + s.toLowerCase();
      if (upper) return out.toUpperCase();
      return capital ? "O" + out.slice(1) : out;
    });
}

/** Put a piece of default-wording copy into the user's chosen wording. */
export function w(text: string): string {
  return wording.value === "orgasm" ? toOrgasmWording(text) : text;
}
