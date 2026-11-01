// A picture's account of its context: once the browser takes it away the
// picture is lost for good, the context given back included, so it neither
// draws nor deletes what it made in the old one. Against a WebGL context that
// records what it is asked, since a test has no GPU.
import assert from "node:assert/strict";
import { test } from "node:test";
import { openPictureCanvas, watchPictureContext } from "./glPicture.ts";

/** A canvas whose WebGL 2 context records its calls; `lost` is what it reports. */
function fakeCanvas() {
  const calls: string[] = [];
  const state = { lost: false };
  const gl = new Proxy(
    {
      isContextLost: () => state.lost,
      getProgramParameter: () => true,
      getParameter: () => 4096,
      drawingBufferWidth: 0,
      drawingBufferHeight: 0,
    } as Record<string, unknown>,
    {
      get(target, name: string) {
        if (name in target) {
          return target[name];
        }
        return (...args: unknown[]) => {
          calls.push(name);
          return args.length === 0 || name.startsWith("create") ? {} : null;
        };
      },
    },
  );
  const canvas = Object.assign(new EventTarget(), {
    width: 1,
    height: 1,
    getContext: () => gl,
  }) as unknown as OffscreenCanvas;
  return { canvas, calls, state };
}

const FRAGMENT = "#version 300 es\nvoid main() {}";

test("a picture whose context was taken stays lost when it is given back", () => {
  const { canvas, calls, state } = fakeCanvas();
  const picture = openPictureCanvas(canvas, FRAGMENT);
  assert.equal(picture.lost(), false);
  state.lost = true;
  canvas.dispatchEvent(new Event("webglcontextlost"));
  // Given back: the context says it is not lost, the picture still is.
  state.lost = false;
  assert.equal(picture.lost(), true);
  assert.throws(() => picture.usable(), /took the picture's WebGL context/);
  calls.length = 0;
  picture.close();
  assert.deepEqual(calls, [], "nothing of the old context deleted in the new");
});

test("a picture is lost by the browser's word, not before", () => {
  const { canvas, state } = fakeCanvas();
  const picture = openPictureCanvas(canvas, FRAGMENT);
  // The context reports itself lost ahead of the event: its calls are no-ops,
  // and the picture says nothing yet.
  state.lost = true;
  assert.equal(picture.lost(), false);
  picture.usable();
  canvas.dispatchEvent(new Event("webglcontextlost"));
  assert.equal(picture.lost(), true);
});

test("no picture is made in a context that is lost", () => {
  const { canvas, state } = fakeCanvas();
  state.lost = true;
  assert.throws(() => openPictureCanvas(canvas, FRAGMENT));
});

test("a picture made in the context given back is closed as any other", () => {
  const { canvas, calls, state } = fakeCanvas();
  openPictureCanvas(canvas, FRAGMENT).close();
  state.lost = true;
  canvas.dispatchEvent(new Event("webglcontextlost"));
  state.lost = false;
  const picture = openPictureCanvas(canvas, FRAGMENT);
  calls.length = 0;
  picture.close();
  assert.ok(calls.includes("deleteProgram"));
});

test("the canvas's watcher asks the context back and reports both ends", () => {
  const { canvas } = fakeCanvas();
  const heard: string[] = [];
  watchPictureContext(canvas, {
    lost: () => heard.push("lost"),
    restored: () => heard.push("restored"),
  });
  const taken = new Event("webglcontextlost", { cancelable: true });
  canvas.dispatchEvent(taken);
  assert.ok(taken.defaultPrevented);
  canvas.dispatchEvent(new Event("webglcontextrestored"));
  assert.deepEqual(heard, ["lost", "restored"]);
});
