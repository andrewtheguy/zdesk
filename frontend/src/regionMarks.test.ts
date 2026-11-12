import assert from "node:assert/strict";
import { test } from "node:test";
import {
  createRegionMarks,
  type MarkBox,
  type MarkLayer,
  markBox,
  type RegionWhy,
} from "./regionMarks.ts";

interface Outline {
  box?: MarkBox;
  why?: RegionWhy;
}

function fakeLayer(): { layer: MarkLayer<Outline>; shown: Set<Outline> } {
  const shown = new Set<Outline>();
  return {
    shown,
    layer: {
      add() {
        const el: Outline = {};
        shown.add(el);
        return el;
      },
      place(el, box, why) {
        el.box = box;
        el.why = why;
      },
      remove(el) {
        shown.delete(el);
      },
    },
  };
}

const size = { w: 1280, h: 800 };

test("a mark is a share of the framebuffer, whatever the canvas is scaled to", () => {
  assert.deepEqual(
    markBox({ x: 320, y: 200, w: 640, h: 400, why: "moving" }, size),
    { left: 25, top: 25, width: 50, height: 50 },
  );
});

test("a mark outside the framebuffer, or of no size, is no outline", () => {
  assert.equal(
    markBox({ x: 1000, y: 0, w: 300, h: 10, why: "moving" }, size),
    null,
  );
  assert.equal(markBox({ x: 0, y: 0, w: 0, h: 10, why: "burst" }, size), null);
  assert.equal(
    markBox({ x: 0, y: 0, w: 10, h: 10, why: "burst" }, { w: 0, h: 0 }),
    null,
  );
});

test("a stream's next mark takes the place of its last, in the same outline", () => {
  const { layer, shown } = fakeLayer();
  const marks = createRegionMarks(layer, () => size);
  marks.set(2, { x: 0, y: 0, w: 640, h: 400, why: "burst" });
  const [outline] = shown;
  assert.equal(outline.why, "burst");
  marks.set(2, { x: 320, y: 200, w: 640, h: 400, why: "moving" });
  assert.equal(shown.size, 1);
  assert.equal([...shown][0], outline);
  assert.equal(outline.why, "moving");
  assert.deepEqual(outline.box, { left: 25, top: 25, width: 50, height: 50 });
});

test("an outline goes with its stream, and no other's", () => {
  const { layer, shown } = fakeLayer();
  const marks = createRegionMarks(layer, () => size);
  marks.set(1, { x: 0, y: 0, w: 64, h: 64, why: "moving" });
  marks.set(2, { x: 64, y: 64, w: 64, h: 64, why: "burst" });
  marks.end(1);
  marks.end(1);
  assert.deepEqual(
    [...shown].map((outline) => outline.why),
    ["burst"],
  );
});

test("a picture that starts over takes every outline", () => {
  const { layer, shown } = fakeLayer();
  const marks = createRegionMarks(layer, () => size);
  marks.set(1, { x: 0, y: 0, w: 64, h: 64, why: "moving" });
  marks.set(2, { x: 64, y: 64, w: 64, h: 64, why: "burst" });
  marks.clear();
  assert.equal(shown.size, 0);
  marks.resized();
  assert.equal(shown.size, 0, "and a new size brings none back");
  marks.set(1, { x: 0, y: 0, w: 64, h: 64, why: "moving" });
  assert.equal(shown.size, 1, "and the stream's next mark is a new outline");
});

test("a mark ahead of its framebuffer is outlined once the size is known", () => {
  const { layer, shown } = fakeLayer();
  let framebuffer: { w: number; h: number } | null = null;
  const marks = createRegionMarks(layer, () => framebuffer);
  marks.set(1, { x: 320, y: 200, w: 640, h: 400, why: "moving" });
  assert.equal(shown.size, 0);
  framebuffer = size;
  marks.resized();
  assert.deepEqual(
    [...shown].map((outline) => outline.box),
    [{ left: 25, top: 25, width: 50, height: 50 }],
  );
  // A framebuffer the mark is not inside takes its outline, and one it is
  // inside again lays it by the new size.
  framebuffer = { w: 640, h: 400 };
  marks.resized();
  assert.equal(shown.size, 0);
  framebuffer = { w: 2560, h: 1600 };
  marks.resized();
  assert.deepEqual(
    [...shown].map((outline) => outline.box),
    [{ left: 12.5, top: 12.5, width: 25, height: 25 }],
  );
});
