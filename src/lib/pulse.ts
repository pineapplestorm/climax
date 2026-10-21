// The "live" pulse on the green session dots, drawn at a LOW frame rate.
//
// Why not a CSS animation: any running CSS animation makes the window produce a
// new frame 60 times a second, and on Windows the fixed cost of presenting each
// frame was about a third of a CPU core no matter how small the thing moving was
// (measured on 1.1.1's first build: a visible tracker with only the dot animating
// cost ~44% of a core, ~31% of it in the WebView's graphics process). A slow
// glow does not need 60 frames a second, so this drives it from one shared timer
// at TICKS_PER_SECOND instead, which is all the window then has to present.
//
// How it works: an element opts in with `use:pulse={isLive}`. Each tick sets one
// number, `--cx-pulse`, on every live element: 0 at rest, 1 at the peak of the
// pulse. The element's CSS turns that into the ring's size and fade (usually on
// a `::after`, so only transform and opacity change and nothing is repainted).
// One timer per window drives every dot in it, and it stops entirely when no
// element is live.
//
// Nothing updates while the window is minimised or hidden (`html.cx-hidden`, set
// by pause-when-hidden.ts) or the page is hidden, so an unseen window costs
// nothing.

const TICKS_PER_SECOND = 10;
const PERIOD_MS = 1600;

const live = new Set<HTMLElement>();
let timer: ReturnType<typeof setInterval> | null = null;

/** easeInOutQuad: the same shape as the CSS `ease-in-out` the old pulse used. */
function ease(x: number): number {
  return x < 0.5 ? 2 * x * x : 1 - (-2 * x + 2) ** 2 / 2;
}

/** Where in the pulse we are: rest (0) -> peak (1) -> rest, once per period. */
function level(now: number): number {
  const phase = (now % PERIOD_MS) / PERIOD_MS;
  const triangle = phase < 0.5 ? phase * 2 : (1 - phase) * 2;
  return ease(triangle);
}

function tick() {
  if (document.hidden || document.documentElement.classList.contains("cx-hidden")) return;
  const v = level(performance.now()).toFixed(3);
  for (const el of live) el.style.setProperty("--cx-pulse", v);
}

function sync() {
  if (live.size > 0 && timer === null) {
    tick();
    timer = setInterval(tick, 1000 / TICKS_PER_SECOND);
  } else if (live.size === 0 && timer !== null) {
    clearInterval(timer);
    timer = null;
  }
}

function set(node: HTMLElement, on: boolean) {
  if (on) {
    live.add(node);
  } else {
    live.delete(node);
    node.style.removeProperty("--cx-pulse");
  }
  sync();
}

/** Svelte action: `use:pulse={isLive}`. Pulses while the argument is true. */
export function pulse(node: HTMLElement, on: boolean = true) {
  set(node, on);
  return {
    update(next: boolean) {
      set(node, next);
    },
    destroy() {
      set(node, false);
    },
  };
}
