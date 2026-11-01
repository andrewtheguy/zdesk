// The paint worker's ordering contract, driven without a Worker or an
// OffscreenCanvas: the painter is injected, and the commands arrive exactly as
// the page posts them. What is being pinned is the part the restructure moved
// off the main thread — every command holds wire order through async decodes —
// and the one command that must not: `clear` ends the attachment, so it runs
// even when the chain is stuck on a draw that never finishes, and the commands
// queued behind that draw must not then land on the next attachment's canvas.
//
// Run with `bun test src/desktopPainterWorker.test.ts` from frontend/.
import assert from "node:assert/strict";
import { test } from "node:test";
import {
  createPainterWorker,
  type PainterEvent,
} from "./desktopPainterWorker.ts";
import type { createFramePainter, FramePainter } from "./framePainter.ts";

// A one-record-free batch frame: kind 0x02, flags 0, count 0, then its u32
// sequence. Valid enough for the kind check, which is all the worker itself reads.
const batchFrame = (sequence = 1, flags = 0) =>
  new Uint8Array([0x02, flags, 0, 0, sequence, 0, 0, 0]).buffer;
// A batch the gateway continued: its update goes on in the next.
const continuedFrame = (sequence: number) => batchFrame(sequence, 0x01);
// An audio frame's kind byte — the thing the session socket never carries, and
// the worker must drop rather than hand to the batch parser.
const audioFrame = () => new Uint8Array([0x03, 0, 0, 0]).buffer;

const settled = () => new Promise((resolve) => setTimeout(resolve, 0));

function harness() {
  const events: PainterEvent[] = [];
  const calls: string[] = [];
  const drawn: ArrayBuffer[] = [];
  let releaseDraw = () => {};
  let stallDraws = false;
  let clock = 0;
  let painterOptions: Parameters<typeof createFramePainter>[0] | null = null;

  const painter: FramePainter = {
    draw(frame) {
      calls.push("draw");
      drawn.push(frame);
      if (stallDraws) {
        return new Promise<void>((resolve) => {
          releaseDraw = resolve;
        });
      }
      return Promise.resolve();
    },
    clear() {
      calls.push("clear");
    },
    setVideoFormat(format) {
      calls.push(`format:${format.decode}`);
    },
    startGraphics() {
      calls.push("graphics");
    },
    startStandard() {
      calls.push("standard");
    },
    startTiles() {
      calls.push("tiles");
    },
    setRegionFormat(stream, format) {
      calls.push(`region:${stream} ${format.decode}`);
    },
    endRegion(stream) {
      calls.push(`end:${stream}`);
    },
    setGraphicsView(part) {
      calls.push(`view:${part.x},${part.y} ${part.w}x${part.h}`);
    },
    mirrorGraphics(display, part) {
      calls.push(`mirror:${display} ${part.x},${part.y} ${part.w}x${part.h}`);
    },
    blank(w, h) {
      calls.push(`blank:${w}x${h}`);
    },
  };

  const ctx = {
    fillStyle: "",
    rects: [] as number[][],
    fillRect(x: number, y: number, w: number, h: number) {
      calls.push("fill");
      this.rects.push([x, y, w, h]);
    },
    // What a mosaic view composes onto the canvas.
    drawImage() {
      calls.push("compose");
    },
  };
  const canvas = {
    width: 0,
    height: 0,
    getContext: () => ctx,
  };

  // The WebGL canvas, as far as the worker hears from it.
  const graphicsCanvas = new EventTarget();
  const host = createPainterWorker(
    (event) => events.push(event),
    (options) => {
      painterOptions = options;
      return painter;
    },
    () => clock,
  );
  host.handle({
    type: "init",
    canvas: canvas as unknown as OffscreenCanvas,
    graphics: graphicsCanvas as unknown as OffscreenCanvas,
    decoding: {
      runs: { hevc: false },
      nativeHevc: true,
    },
  });

  return {
    host,
    events,
    calls,
    drawn,
    ctx,
    canvas,
    graphicsCanvas,
    stall: () => {
      stallDraws = true;
    },
    release: () => {
      stallDraws = false;
      releaseDraw();
    },
    // Stop stalling the draws to come without letting go of the one already out —
    // which is the state a clear has to work in, rather than after.
    unstall: () => {
      stallDraws = false;
    },
    advance: (milliseconds: number) => {
      clock += milliseconds;
    },
    painterOptions: () => {
      assert.ok(painterOptions);
      return painterOptions;
    },
  };
}

test("a batch frame reaches the painter; anything else is dropped", async () => {
  const h = harness();
  const frame = batchFrame();
  h.host.handle({ type: "frame", data: frame, sequence: 1, generation: 3 });
  h.host.handle({
    type: "frame",
    data: audioFrame(),
    sequence: 2,
    generation: 3,
  });
  await settled();
  assert.deepEqual(h.calls, ["draw"]);
  assert.equal(h.drawn[0], frame);
  assert.deepEqual(h.events, [
    {
      type: "painted",
      sequence: 1,
      generation: 3,
      queuedMs: 0,
      drawMs: 0,
    },
  ]);
});

test("an update's continued batches are held, acknowledged, and drawn with its last", async () => {
  const h = harness();
  const [first, second, last] = [
    continuedFrame(1),
    continuedFrame(2),
    batchFrame(3),
  ];
  h.host.handle({ type: "frame", data: first, sequence: 1, generation: 1 });
  h.host.handle({ type: "frame", data: second, sequence: 2, generation: 1 });
  await settled();
  // Nothing drawn, so nothing shown, but the gateway's window kept open.
  assert.deepEqual(h.drawn, []);
  assert.deepEqual(
    h.events.map((event) => event.type === "painted" && event.sequence),
    [1, 2],
  );
  h.host.handle({ type: "frame", data: last, sequence: 3, generation: 1 });
  await settled();
  assert.deepEqual(h.drawn, [first, second, last]);
});

test("held batches are drawn ahead of the next command, and dropped by a clear", async () => {
  const h = harness();
  const held = continuedFrame(1);
  h.host.handle({ type: "frame", data: held, sequence: 1, generation: 1 });
  h.host.handle({ type: "tilesStart" });
  await settled();
  assert.deepEqual(h.calls, ["draw", "tiles"]);
  h.host.handle({
    type: "frame",
    data: continuedFrame(2),
    sequence: 2,
    generation: 1,
  });
  await settled();
  h.host.handle({ type: "clear" });
  h.host.handle({
    type: "frame",
    data: batchFrame(3),
    sequence: 3,
    generation: 2,
  });
  await settled();
  assert.deepEqual(
    h.drawn.length,
    2,
    "the cleared attachment's held batch is not drawn",
  );
});

test("a clear while held batches draw stops them, and the command behind them", async () => {
  const h = harness();
  const [first, second] = [continuedFrame(1), continuedFrame(2)];
  h.host.handle({ type: "frame", data: first, sequence: 1, generation: 1 });
  h.host.handle({ type: "frame", data: second, sequence: 2, generation: 1 });
  h.stall();
  h.host.handle({ type: "mark", seq: 3 });
  await settled();
  assert.deepEqual(h.drawn, [first], "the first held batch drawing");
  h.host.handle({ type: "clear" });
  h.release();
  await settled();
  assert.deepEqual(
    h.drawn,
    [first],
    "nothing of the gone attachment after the clear",
  );
  assert.ok(
    !h.events.some((event) => event.type === "reached"),
    "the mark behind them was the gone attachment's",
  );
});

test("held batches drawn ahead of a command are composed on a mosaic", async () => {
  const globals = globalThis as { OffscreenCanvas?: unknown };
  const before = globals.OffscreenCanvas;
  globals.OffscreenCanvas = class {
    width: number;
    height: number;
    constructor(width: number, height: number) {
      this.width = width;
      this.height = height;
    }
    getContext() {
      return { fillStyle: "", fillRect() {}, drawImage() {} };
    }
  };
  try {
    const h = harness();
    const draw = { sx: 0, sy: 0, sw: 8, sh: 8, dx: 0, dy: 0, dw: 8, dh: 8 };
    h.host.handle({
      type: "view",
      view: { w: 8, h: 8, scale: 1, draws: [draw] },
      seq: 1,
    });
    await settled();
    h.calls.length = 0;
    h.host.handle({
      type: "frame",
      data: continuedFrame(2),
      sequence: 2,
      generation: 1,
    });
    h.host.handle({ type: "mark", seq: 3 });
    await settled();
    assert.deepEqual(h.calls, ["draw", "compose"]);
    assert.ok(h.events.some((event) => event.type === "reached"));
  } finally {
    globals.OffscreenCanvas = before;
  }
});

test("a mark is echoed only once the draw ahead of it has finished", async () => {
  const h = harness();
  h.stall();
  h.host.handle({
    type: "frame",
    data: batchFrame(4),
    sequence: 4,
    generation: 2,
  });
  h.host.handle({ type: "mark", seq: 3 });
  await settled();
  // The picture is still being drawn: a notice over it stays up.
  assert.deepEqual(h.events, []);
  h.release();
  await settled();
  assert.deepEqual(h.events, [
    {
      type: "painted",
      sequence: 4,
      generation: 2,
      queuedMs: 0,
      drawMs: 0,
    },
    { type: "reached", seq: 3 },
  ]);
});

test("resize and videoFormat hold their place behind a stalled draw", async () => {
  const h = harness();
  h.stall();
  h.host.handle({
    type: "frame",
    data: batchFrame(4),
    sequence: 4,
    generation: 2,
  });
  h.host.handle({ type: "resize", w: 640, h: 480, seq: 7, view: null });
  h.host.handle({
    type: "videoFormat",
    format: { decode: "avc1.640033" },
  });
  await settled();
  // The draw is still out; nothing behind it has run, and the echo the page's
  // layout state waits on has not been sent.
  assert.deepEqual(h.calls, ["draw"]);
  assert.equal(h.canvas.width, 0);
  assert.deepEqual(h.events, []);
  h.advance(12);
  h.release();
  await settled();
  assert.deepEqual(h.calls, [
    "draw",
    "fill",
    "blank:640x480",
    "format:avc1.640033",
  ]);
  assert.equal(h.canvas.width, 640);
  assert.equal(h.canvas.height, 480);
  assert.equal(h.ctx.fillStyle, "#000");
  assert.deepEqual(h.ctx.rects, [[0, 0, 640, 480]]);
  assert.deepEqual(h.events, [
    {
      type: "painted",
      sequence: 4,
      generation: 2,
      queuedMs: 0,
      drawMs: 12,
    },
    { type: "resized", seq: 7 },
  ]);
});

test("a graphics pipeline starts in its place, behind the frames before it", async () => {
  // The run after the start must find a compositor with nothing in it, and the
  // frames before it must not: both are what the order is for.
  const h = harness();
  h.stall();
  h.host.handle({
    type: "frame",
    data: batchFrame(1),
    sequence: 1,
    generation: 1,
  });
  await settled();
  h.host.handle({ type: "graphicsStart" });
  h.unstall();
  h.host.handle({
    type: "frame",
    data: batchFrame(2),
    sequence: 2,
    generation: 1,
  });
  await settled();
  assert.deepEqual(h.calls, ["draw"]);
  h.release();
  await settled();
  assert.deepEqual(h.calls, ["draw", "graphics", "draw"]);
});

test("a Mac's own picture starts in its place, behind the frames before it", async () => {
  // The first rectangle after the start must find a decoder with nothing in it.
  const h = harness();
  h.stall();
  h.host.handle({
    type: "frame",
    data: batchFrame(1),
    sequence: 1,
    generation: 1,
  });
  await settled();
  h.host.handle({ type: "standardStart" });
  h.unstall();
  h.host.handle({
    type: "frame",
    data: batchFrame(2),
    sequence: 2,
    generation: 1,
  });
  await settled();
  assert.deepEqual(h.calls, ["draw"]);
  h.release();
  await settled();
  assert.deepEqual(h.calls, ["draw", "standard", "draw"]);
});

test("wlshare's tiles and their regions hold their place among the frames", async () => {
  // A region's decoder must be built before its first unit and retired after its
  // last, and a picture started over must not land on frames before it.
  const h = harness();
  h.stall();
  h.host.handle({
    type: "frame",
    data: batchFrame(1),
    sequence: 1,
    generation: 1,
  });
  await settled();
  h.host.handle({ type: "tilesStart" });
  h.host.handle({
    type: "regionFormat",
    stream: 2,
    format: { decode: "avc1.64001F" },
  });
  h.unstall();
  h.host.handle({
    type: "frame",
    data: batchFrame(2),
    sequence: 2,
    generation: 1,
  });
  h.host.handle({ type: "regionEnd", stream: 2 });
  await settled();
  assert.deepEqual(h.calls, ["draw"]);
  h.release();
  await settled();
  assert.deepEqual(h.calls, [
    "draw",
    "tiles",
    "region:2 avc1.64001F",
    "draw",
    "end:2",
  ]);
});

test("the part a display shows, and a tab's mirror, hold their place behind a stalled draw", async () => {
  // Both come with a resize, behind the frames before it, and must apply to the
  // picture the frames before them left: the view to the pipeline's, the mirror
  // to the tab's.
  const h = harness();
  h.stall();
  h.host.handle({
    type: "frame",
    data: batchFrame(1),
    sequence: 1,
    generation: 1,
  });
  h.host.handle({
    type: "graphicsView",
    part: { x: 1440, y: 0, w: 1440, h: 900 },
  });
  h.host.handle({
    type: "graphicsMirror",
    display: 2,
    part: { x: 1440, y: 0, w: 1440, h: 900 },
  });
  await settled();
  assert.deepEqual(h.calls, ["draw"]);
  h.release();
  await settled();
  assert.deepEqual(h.calls, [
    "draw",
    "view:1440,0 1440x900",
    "mirror:2 1440,0 1440x900",
  ]);
});

test("a later batch reports the time it waited behind earlier paint", async () => {
  const h = harness();
  h.stall();
  h.host.handle({
    type: "frame",
    data: batchFrame(1),
    sequence: 1,
    generation: 5,
  });
  await settled();
  h.advance(9);
  h.host.handle({
    type: "frame",
    data: batchFrame(2),
    sequence: 2,
    generation: 5,
  });
  h.advance(3);
  h.release();
  await settled();
  assert.deepEqual(h.events, [
    {
      type: "painted",
      sequence: 1,
      generation: 5,
      queuedMs: 0,
      drawMs: 12,
    },
    {
      type: "painted",
      sequence: 2,
      generation: 5,
      queuedMs: 3,
      drawMs: 0,
    },
  ]);
});

test("clear ends the attachment even with a draw that never finishes", async () => {
  const h = harness();
  h.host.handle({ type: "resize", w: 640, h: 480, seq: 1, view: null });
  await settled();
  h.stall();
  h.host.handle({
    type: "frame",
    data: batchFrame(),
    sequence: 1,
    generation: 1,
  });
  await settled();
  h.host.handle({ type: "clear" });
  await settled();
  // A clear that waited its turn here would wait forever: this is the freeze it
  // exists to end, not an ordinary boundary.
  assert.deepEqual(h.calls, ["fill", "blank:640x480", "draw", "clear"]);
  assert.equal(h.canvas.width, 0);
  assert.equal(h.canvas.height, 0);
});

test("the next attachment paints without waiting for the stuck one", async () => {
  const h = harness();
  h.stall();
  h.host.handle({
    type: "frame",
    data: batchFrame(1),
    sequence: 1,
    generation: 1,
  });
  await settled();
  h.host.handle({ type: "clear" });
  // The stuck draw is *still* stuck — that is the whole point, and releasing it
  // here would test the recovery rather than the escape. The new target: the echo
  // the page's "Connecting to the remote desktop…" overlay is held up by, and then
  // its own first batch.
  h.unstall();
  h.host.handle({ type: "resize", w: 800, h: 600, seq: 2, view: null });
  h.host.handle({
    type: "frame",
    data: batchFrame(1),
    sequence: 1,
    generation: 2,
  });
  await settled();
  assert.equal(h.canvas.width, 800);
  assert.deepEqual(
    h.events.filter((event) => event.type === "resized"),
    [{ type: "resized", seq: 2 }],
  );
  assert.deepEqual(
    h.events.filter((event) => event.type === "painted"),
    [{ type: "painted", sequence: 1, generation: 2, queuedMs: 0, drawMs: 0 }],
  );
});

test("a frame queued before the clear never reaches the next attachment", async () => {
  const h = harness();
  h.stall();
  // Two behind the wedge: the one being drawn, and one that never got its turn.
  h.host.handle({
    type: "frame",
    data: batchFrame(1),
    sequence: 1,
    generation: 1,
  });
  await settled();
  h.host.handle({
    type: "frame",
    data: batchFrame(2),
    sequence: 2,
    generation: 1,
  });
  h.host.handle({ type: "clear" });
  h.release();
  await settled();
  // The second one is for a desktop that is gone, and the clear did not wait for
  // it — releasing it must not paint it onto whatever comes next.
  assert.deepEqual(h.calls, ["draw", "clear"]);
});

test("the painter's callbacks travel back as events", () => {
  const h = harness();
  const options = h.painterOptions();
  options.onVideoError("no decoder");
  options.onVideoError(null);
  options.onVideoNeedsKeyframe("went quiet");
  assert.deepEqual(h.events, [
    { type: "videoError", reason: "no decoder" },
    { type: "videoError", reason: null },
    { type: "videoNeedsKeyframe", reason: "went quiet" },
  ]);
});

test("a picture is shown for the attachment the worker is on, by its count of clears", () => {
  const h = harness();
  const options = h.painterOptions();
  options.onGraphicsShown?.(true);
  h.host.handle({ type: "clear" });
  options.onGraphicsShown?.(true);
  assert.deepEqual(
    h.events.filter((event) => event.type === "graphicsShown"),
    [
      { type: "graphicsShown", shown: true, epoch: 0 },
      { type: "graphicsShown", shown: true, epoch: 1 },
    ],
  );
});

test("the WebGL context taken away and given back is told to the page, and asked back", () => {
  const h = harness();
  const taken = new Event("webglcontextlost", { cancelable: true });
  h.graphicsCanvas.dispatchEvent(taken);
  assert.ok(taken.defaultPrevented, "asked back, or the browser keeps it");
  h.graphicsCanvas.dispatchEvent(new Event("webglcontextrestored"));
  assert.deepEqual(h.events, [
    { type: "contextLost" },
    { type: "contextRestored" },
  ]);
  assert.deepEqual(
    h.calls,
    [],
    "the painter hears nothing: the page starts over",
  );
});

test("what a picture says while the context is gone is kept from the page, until the clear", () => {
  const h = harness();
  const say = (reason: string | null) =>
    h.painterOptions().onVideoError(reason);
  h.graphicsCanvas.dispatchEvent(new Event("webglcontextlost"));
  say(
    "This browser could not compose the host's graphics (the browser took the picture's WebGL context away). Reload the page to start the session over.",
  );
  h.graphicsCanvas.dispatchEvent(new Event("webglcontextrestored"));
  say("still not the page's");
  h.host.handle({ type: "clear" });
  say(null);
  say("the next attachment's, said");
  assert.deepEqual(
    h.events.filter((event) => event.type === "videoError"),
    [
      { type: "videoError", reason: null },
      { type: "videoError", reason: "the next attachment's, said" },
    ],
  );
});
