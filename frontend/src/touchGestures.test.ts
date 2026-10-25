// What fingers mean. The gesture layer decides once per two-finger drag
// whether the fingers are pinching or scrolling, and the decision holds for the
// rest of the gesture; a two-finger tap still has to survive both. One finger
// resting long enough holds the left button.
//
// Driven through a stand-in element and document, since the decision is
// arithmetic on touch coordinates and needs no layout, and on a stand-in clock,
// since a hold is decided by time.
//
// Run with `bun test src/touchGestures.test.ts` from frontend/.
import assert from "node:assert/strict";
import { afterEach, beforeEach, test } from "node:test";
import type { ClientMsg } from "./protocol.ts";
import {
  attachTouchGestures,
  type GestureView,
  type TouchGestures,
} from "./touchGestures.ts";

type Handler = (e: TouchEvent) => void;

const REMOTE = { w: 2000, h: 1000 };
// Pixels the remote draws per point; a test changes it for a dense desktop.
let remoteScale = 1;

let sent: ClientMsg[] = [];
let view: GestureView;
let gestures: TouchGestures;
const handlers = new Map<string, Handler>();

const element = {
  addEventListener(type: string, handler: Handler) {
    handlers.set(type, handler);
  },
  removeEventListener(type: string) {
    handlers.delete(type);
  },
  getBoundingClientRect: () => ({ left: 0, top: 0 }),
} as unknown as HTMLElement;

// The overlay reads the viewport through the document; nothing else does.
Object.assign(globalThis, {
  document: { documentElement: { clientWidth: 400, clientHeight: 800 } },
});

const touch = (identifier: number, clientX: number, clientY: number) =>
  ({ identifier, clientX, clientY, force: 0 }) as unknown as Touch;

function dispatch(
  type: "touchstart" | "touchmove" | "touchend",
  touches: Touch[],
  changed: Touch[] = touches,
): void {
  handlers.get(type)?.({
    touches: touches as unknown as TouchList,
    changedTouches: changed as unknown as TouchList,
    preventDefault() {},
    stopImmediatePropagation() {},
  } as unknown as TouchEvent);
}

const wheels = () => sent.filter((msg) => msg.type === "wheel");
const buttons = () => sent.filter((msg) => msg.type === "mouseButton");

const left = (pressed: boolean) => ({
  type: "mouseButton",
  button: "left",
  pressed,
  clicks: 1,
});

// The clock the gestures read, and the timers they set, moved on by advance().
let now = 0;
let timers: { at: number; id: number; run: () => void }[] = [];
let nextTimerId = 1;
const realSetTimeout = globalThis.setTimeout;
const realClearTimeout = globalThis.clearTimeout;
const realDateNow = Date.now;

function advance(ms: number): void {
  const until = now + ms;
  for (;;) {
    const due = timers
      .filter((timer) => timer.at <= until)
      .sort((a, b) => a.at - b.at)[0];
    if (!due) {
      break;
    }
    timers = timers.filter((timer) => timer !== due);
    now = due.at;
    due.run();
  }
  now = until;
}

// That many scroll ticks, each 32 remote px along one axis.
const ticks = (count: number, dx: number, dy: number) =>
  Array.from({ length: count }, () => ({
    type: "wheel",
    dx,
    dy,
    unit: "pixel",
  }));

// Two fingers, 50px apart, moved together by the given offsets in turn.
function twoFingerDrag(steps: readonly { dx: number; dy: number }[]): void {
  dispatch("touchstart", [touch(1, 100, 400), touch(2, 150, 400)]);
  for (const step of steps) {
    dispatch("touchmove", [
      touch(1, 100 + step.dx, 400 + step.dy),
      touch(2, 150 + step.dx, 400 + step.dy),
    ]);
  }
}

beforeEach(() => {
  // Far from 0, which the gestures read as "no tap yet".
  now = 1_000_000;
  timers = [];
  Object.assign(globalThis, {
    setTimeout: (run: () => void, ms = 0) => {
      const id = nextTimerId++;
      timers.push({ at: now + ms, id, run });
      return id;
    },
    clearTimeout: (id: number) => {
      timers = timers.filter((timer) => timer.id !== id);
    },
  });
  Date.now = () => now;
  sent = [];
  remoteScale = 1;
  view = { fit: 0.2, zoom: 1, pan: { x: 0, y: 0 } };
  handlers.clear();
  gestures = attachTouchGestures(element, {
    send: (msg) => sent.push(msg),
    remoteSize: () => ({ ...REMOTE, scale: remoteScale }),
    view: () => view,
    applyView: (zoom, pan) => {
      view = { fit: view.fit, zoom, pan };
    },
  });
  gestures.notePointer(1000, 500);
});

afterEach(() => {
  gestures.detach();
  Object.assign(globalThis, {
    setTimeout: realSetTimeout,
    clearTimeout: realClearTimeout,
  });
  Date.now = realDateNow;
});

test("fingers moving in parallel scroll, in the natural direction", () => {
  // 12px classifies the gesture as a scroll and names its axis. The desktop is
  // shown at a fifth of its size, so the 44px the fingers covered is 220px of
  // it: six ticks, and 28px carried.
  twoFingerDrag([
    { dx: 0, dy: 12 },
    { dx: 0, dy: 44 },
  ]);
  assert.deepEqual(wheels(), ticks(6, 0, -32));
  assert.equal(view.zoom, 1, "a scroll never zooms");
});

test("a swipe delivered as one move scrolls by all of it", () => {
  // The browser is free to coalesce a fast swipe into a single touchmove: the
  // travel that classifies the gesture has to count, or a flick scrolls nothing.
  twoFingerDrag([{ dx: 0, dy: 100 }]);
  assert.deepEqual(wheels(), ticks(15, 0, -32));
});

test("a zoomed-in view scrolls the less for the same travel", () => {
  // Content follows the fingers: at 1:1 the 44px they covered is 44px of the
  // desktop, one tick.
  view = { fit: 0.2, zoom: 5, pan: { x: 0, y: 0 } };
  twoFingerDrag([
    { dx: 0, dy: 12 },
    { dx: 0, dy: 44 },
  ]);
  assert.deepEqual(wheels(), ticks(1, 0, -32));
});

test("a dense desktop scrolls by its points, not its pixels", () => {
  // Drawn at two pixels a point, the 220 pixels the fingers covered are 110
  // points: three ticks.
  remoteScale = 2;
  twoFingerDrag([
    { dx: 0, dy: 12 },
    { dx: 0, dy: 44 },
  ]);
  assert.deepEqual(wheels(), ticks(3, 0, -32));
});

test("a sideways drag scrolls sideways", () => {
  twoFingerDrag([
    { dx: 12, dy: 0 },
    { dx: 44, dy: 0 },
  ]);
  assert.deepEqual(wheels(), ticks(6, -32, 0));
});

test("a diagonal drag locks onto the axis it was classified on", () => {
  // Mostly downwards, then hard to the right: the sideways travel arrives after
  // the lock and never reaches the wire as a horizontal tick.
  twoFingerDrag([
    { dx: 4, dy: 12 },
    { dx: 60, dy: 44 },
  ]);
  assert.deepEqual(wheels(), ticks(6, 0, -32));
});

test("fingers changing their distance pinch, and never scroll", () => {
  dispatch("touchstart", [touch(1, 100, 400), touch(2, 150, 400)]);
  dispatch("touchmove", [touch(1, 80, 400), touch(2, 170, 400)]);
  dispatch("touchmove", [touch(1, 50, 400), touch(2, 200, 400)]);
  assert.ok(view.zoom > 1, `expected a zoom, got ${view.zoom}`);
  assert.deepEqual(wheels(), []);
});

test("a scroll that drifts apart stays a scroll", () => {
  dispatch("touchstart", [touch(1, 100, 400), touch(2, 150, 400)]);
  dispatch("touchmove", [touch(1, 100, 412), touch(2, 150, 412)]);
  // The same downward travel, with the fingers spreading as they go.
  dispatch("touchmove", [touch(1, 60, 444), touch(2, 190, 444)]);
  assert.deepEqual(wheels(), ticks(6, 0, -32));
  assert.equal(view.zoom, 1);
});

test("two fingers that stay put are still a right-click", () => {
  dispatch("touchstart", [touch(1, 100, 400), touch(2, 150, 400)]);
  dispatch("touchmove", [touch(1, 102, 401), touch(2, 151, 400)]);
  dispatch("touchend", [touch(2, 151, 400)], [touch(1, 102, 401)]);
  dispatch("touchend", [], [touch(2, 151, 400)]);
  assert.deepEqual(
    sent.filter((msg) => msg.type === "mouseButton"),
    [
      { type: "mouseButton", button: "right", pressed: true, clicks: 1 },
      { type: "mouseButton", button: "right", pressed: false, clicks: 1 },
    ],
  );
});

test("a scroll releases without clicking anything", () => {
  twoFingerDrag([
    { dx: 0, dy: 12 },
    { dx: 0, dy: 44 },
  ]);
  dispatch("touchend", [touch(2, 150, 444)], [touch(1, 100, 444)]);
  dispatch("touchend", [], [touch(2, 150, 444)]);
  assert.deepEqual(
    sent.filter((msg) => msg.type === "mouseButton"),
    [],
  );
});

test("a finger resting half a second holds the left button until it lifts", () => {
  dispatch("touchstart", [touch(1, 200, 400)]);
  // A little drift is still a finger at rest.
  dispatch("touchmove", [touch(1, 205, 403)]);
  advance(499);
  assert.deepEqual(buttons(), []);
  advance(1);
  assert.deepEqual(buttons(), [left(true)]);
  assert.deepEqual(sent.at(-2), { type: "mouseMove", x: 1000, y: 500 });

  // Held, the finger drags at the scale the desktop is shown at.
  dispatch("touchmove", [touch(1, 225, 403)]);
  assert.deepEqual(sent.at(-1), { type: "mouseMove", x: 1100, y: 500 });
  advance(1000);
  dispatch("touchend", [], [touch(1, 225, 403)]);
  advance(1000);
  assert.deepEqual(buttons(), [left(true), left(false)]);
});

test("a finger that moves the cursor first never becomes a hold", () => {
  dispatch("touchstart", [touch(1, 200, 400)]);
  dispatch("touchmove", [touch(1, 220, 400)]);
  advance(2000);
  dispatch("touchend", [], [touch(1, 220, 400)]);
  advance(1000);
  assert.deepEqual(buttons(), []);
});

test("a tap is still one click, not a hold", () => {
  dispatch("touchstart", [touch(1, 200, 400)]);
  advance(100);
  dispatch("touchend", [], [touch(1, 200, 400)]);
  advance(1000);
  assert.deepEqual(buttons(), [left(true), left(false)]);
});

test("a second finger landing before the hold is no hold", () => {
  dispatch("touchstart", [touch(1, 200, 400)]);
  advance(200);
  dispatch("touchstart", [touch(1, 200, 400), touch(2, 250, 400)]);
  advance(2000);
  assert.deepEqual(buttons(), []);
});
