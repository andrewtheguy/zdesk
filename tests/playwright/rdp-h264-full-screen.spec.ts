// What a session started with H.264 full screen puts on its sockets, and what the
// page does with it: an RDP host's whole desktop in H.264, passed as each
// display's video stream rather than as the graphics pipeline.
//
// Everything asserted is decided by the system and not by a machine's timing: the
// render the gateway resolved, the control messages and the order of the records
// behind them, the records' own framing and keyframe flags, the acknowledgments
// the page sends once its paint worker has painted a batch, which WebAssembly the
// page fetched, and the DOM's account of an error. Nothing here looks at a pixel.
// A VIDEO record is parsed here for itself, from src/protocol.rs's layout.
//
// It needs a gateway whose local config names a live RDP host with the pipeline
// on, as `egfx-passthrough.spec.ts` does, and the same opt-in as that spec's
// H.264 tests. Nothing has to play on the host: it draws its whole desktop in
// H.264.
//
//     ZDESK_PLAYWRIGHT_EGFX_H264_TARGET=win \
//     bunx playwright test '/rdp-h264-full-screen\.spec\.ts$'
import { expect, type Page, test, type Worker } from "@playwright/test";

import { leaveSession, logInAndConnectTo, returnToPicker } from "./support";

/// The opt-in, and the target's name in one.
const TARGET = process.env.ZDESK_PLAYWRIGHT_EGFX_H264_TARGET;

/// What every session here is started with.
const FULL_SCREEN = { pipeline: "adaptive-desktop" } as const;

/// The paint worker's script, as the build names it (desktopPainter.ts).
const PAINT_WORKER = "**/desktopPainter.worker-*.js";

/// The wire, copied from src/protocol.rs rather than imported from the SPA.
const BATCH_FRAME_KIND = 0x02;
const BATCH_HEADER_LEN = 8;
const OP_VIDEO = 0x03;
const VIDEO_HEADER_LEN = 10;
const VIDEO_KEYFRAME = 0x01;

interface Unit {
  w: number;
  h: number;
  keyframe: boolean;
  /** The NAL unit types it carries, by its own start codes. */
  nals: number[];
}

interface Batch {
  sequence: number;
  units: Unit[];
  /** Whether the records exactly filled the frame and were all VIDEO. */
  exact: boolean;
}

/// The NAL unit types of an Annex B access unit.
function nalTypes(unit: Buffer): number[] {
  const types: number[] = [];
  for (let at = 0; at + 3 < unit.length; at += 1) {
    if (unit[at] === 0 && unit[at + 1] === 0 && unit[at + 2] === 1) {
      types.push(unit[at + 3] & 0x1f);
      at += 3;
    }
  }
  return types;
}

function parseBatch(payload: Buffer): Batch {
  const units: Unit[] = [];
  let at = BATCH_HEADER_LEN;
  let exact = true;
  while (at < payload.length) {
    if (
      payload.readUInt8(at) !== OP_VIDEO ||
      at + VIDEO_HEADER_LEN > payload.length
    ) {
      exact = false;
      break;
    }
    const flags = payload.readUInt8(at + 1);
    const length = payload.readUInt32LE(at + 6);
    const start = at + VIDEO_HEADER_LEN;
    if (start + length > payload.length) {
      exact = false;
      break;
    }
    units.push({
      w: payload.readUInt16LE(at + 2),
      h: payload.readUInt16LE(at + 4),
      keyframe: (flags & VIDEO_KEYFRAME) !== 0,
      nals: nalTypes(payload.subarray(start, start + length)),
    });
    at = start + length;
  }
  return {
    sequence: payload.readUInt32LE(4),
    units,
    exact: exact && at === payload.length,
  };
}

interface Seen {
  /** Every control message's `type`, in arrival order, from both sockets. */
  controlTypes: string[];
  render?: string;
  /** Every `videoFormat`'s `decode`. */
  formats: string[];
  /** What the page said of the host's H.264 as video (`rdp_video`). */
  decodesVideo?: string | null;
  /** The display socket's batches, since it last attached. */
  batches: Batch[];
  acknowledged: number[];
  /** Messages the page sent on its session socket, by `type`. */
  sent: string[];
  /** Every WebAssembly module the page fetched. */
  wasm: string[];
}

/// Watch both sockets and the page's fetches, registered before navigation.
function watch(page: Page): Seen {
  const seen: Seen = {
    controlTypes: [],
    formats: [],
    batches: [],
    acknowledged: [],
    sent: [],
    wasm: [],
  };
  page.on("request", (request) => {
    if (request.url().endsWith(".wasm")) {
      seen.wasm.push(new URL(request.url()).pathname);
    }
  });
  const control = (text: string) => {
    const message = JSON.parse(text);
    if (typeof message.type !== "string") {
      return;
    }
    seen.controlTypes.push(message.type);
    if (message.type === "connected") {
      seen.render = message.render;
    }
    if (message.type === "videoFormat") {
      seen.formats.push(message.decode);
    }
  };
  page.on("websocket", (ws) => {
    const url = new URL(ws.url());
    if (url.pathname === "/ws") {
      seen.decodesVideo = url.searchParams.get("rdp_video");
      ws.on("framereceived", ({ payload }) => {
        if (typeof payload === "string") {
          control(payload);
        }
      });
      ws.on("framesent", ({ payload }) => {
        if (typeof payload === "string") {
          seen.sent.push(JSON.parse(payload).type);
        }
      });
      return;
    }
    if (url.pathname !== "/ws/display") {
      return;
    }
    // A display socket is an attachment of its own, numbered from one.
    seen.batches = [];
    seen.acknowledged = [];
    ws.on("framesent", ({ payload }) => {
      if (typeof payload !== "string") {
        return;
      }
      const message = JSON.parse(payload);
      if (message.type === "paintAck") {
        seen.acknowledged.push(message.sequence);
      }
    });
    ws.on("framereceived", ({ payload }) => {
      if (typeof payload === "string") {
        control(payload);
        return;
      }
      if (payload.readUInt8(0) === BATCH_FRAME_KIND) {
        seen.batches.push(parseBatch(payload));
      }
    });
  });
  return seen;
}

const units = (seen: Seen): Unit[] => seen.batches.flatMap((b) => b.units);
const keyframes = (seen: Seen): number =>
  units(seen).filter((unit) => unit.keyframe).length;

/// Whether every unit is what the page was promised: a keyframe flag on exactly
/// the units with an IDR picture, each with its parameter sets ahead of it.
function checkUnits(seen: Seen) {
  for (const batch of seen.batches) {
    expect(
      batch.exact,
      "records must be VIDEO and exactly fill the frame",
    ).toBe(true);
  }
  const all = units(seen);
  expect(all.length).toBeGreaterThan(0);
  expect(all[0].keyframe, "a display's stream starts at a keyframe").toBe(true);
  for (const unit of all) {
    expect(unit.keyframe).toBe(unit.nals.includes(5));
    if (unit.keyframe) {
      const idr = unit.nals.indexOf(5);
      expect(unit.nals.slice(0, idr)).toEqual(expect.arrayContaining([7, 8]));
    }
  }
}

test.describe("a target started with H.264 full screen", () => {
  // A session's connect and logon, and a keyframe asked of the host, are each
  // seconds.
  test.describe.configure({ timeout: 90_000 });

  test.skip(
    !TARGET,
    "set ZDESK_PLAYWRIGHT_EGFX_H264_TARGET=<target> against a gateway with a live RDP host",
  );

  test.afterEach(async ({ page }) => {
    await leaveSession(page);
  });

  test("is sent its display's H.264 as video, and paints it with no compositor", async ({
    page,
  }) => {
    const seen = watch(page);
    await logInAndConnectTo(page, TARGET ?? "", "", FULL_SCREEN);

    await expect
      .poll(() => seen.render, { timeout: 20_000 })
      .toBe("the host's H.264 of the whole desktop, passed through");
    expect(seen.decodesVideo).toBe("true");
    await expect
      .poll(() => seen.acknowledged.length, { timeout: 20_000 })
      .toBeGreaterThan(2);

    // A stream announced as the host's own profile, never a pipeline.
    expect(seen.formats[0]).toMatch(/^avc1\.[0-9A-F]{6}$/);
    expect(seen.controlTypes).not.toContain("graphicsStart");
    expect(seen.controlTypes).not.toContain("graphicsView");
    checkUnits(seen);
    // Every unit is the whole of the display, which is the session's desktop.
    const [{ w, h }] = units(seen);
    for (const unit of units(seen)) {
      expect([unit.w, unit.h]).toEqual([w, h]);
    }
    // Nothing of the compositor is loaded for it.
    expect(seen.wasm.filter((path) => /egfx/.test(path))).toEqual([]);
    await expect(page.getByRole("alert")).toHaveCount(0);

    await returnToPicker(page);
  });

  test("resumes a page that comes back, from a keyframe", async ({ page }) => {
    const seen = watch(page);
    await logInAndConnectTo(page, TARGET ?? "", "", FULL_SCREEN);
    await expect
      .poll(() => seen.acknowledged.length, { timeout: 20_000 })
      .toBeGreaterThan(2);

    // The same session, reattached: the stream announced again and started
    // again at a keyframe, which the host was asked for.
    const before = seen.controlTypes.length;
    await page.reload();
    await expect(page.getByRole("button", { name: "Open menu" })).toBeVisible({
      timeout: 20_000,
    });
    await expect
      .poll(() => seen.acknowledged.length, { timeout: 20_000 })
      .toBeGreaterThan(0);
    const after = seen.controlTypes.slice(before);
    expect(after).toContain("connected");
    expect(after).toContain("videoFormat");
    expect(after).not.toContain("picker");
    checkUnits(seen);
    await expect(page.getByRole("alert")).toHaveCount(0);

    await returnToPicker(page);
  });

  test("starts again at a keyframe after its decoder fails, as iOS's does", async ({
    page,
  }) => {
    // The paint worker's first decoder fails when the test says, as a hardware
    // decoder on an iPhone does now and then mid-session: its error callback
    // says "Decoder failure", and it decodes nothing more. Said once a frame
    // has painted, so a still desktop fails it as surely as a moving one.
    await page.context().route(PAINT_WORKER, async (route) => {
      const response = await route.fetch();
      const failing = `{
        const Real = globalThis.VideoDecoder;
        let made = 0;
        let fail = null;
        globalThis.VideoDecoder = class extends Real {
          constructor(init) {
            made += 1;
            const first = made === 1;
            let failed = false;
            super({ ...init, output: (frame) => (failed ? frame.close() : init.output(frame)) });
            if (first) {
              fail = () => {
                failed = true;
                super.close();
                init.error(new DOMException("Decoder failure", "EncodingError"));
              };
            }
          }
        };
        globalThis.decodersMade = () => made;
        globalThis.failDecoder = () => {
          const failing = fail;
          fail = null;
          failing?.();
          return failing !== null;
        };
      }\n`;
      await route.fulfill({
        response,
        body: failing + (await response.text()),
      });
    });
    let worker: Worker | undefined;
    page.on("worker", (made) => {
      if (/desktopPainter\.worker/.test(made.url())) {
        worker = made;
      }
    });
    const seen = watch(page);
    await logInAndConnectTo(page, TARGET ?? "", "", FULL_SCREEN);
    await expect
      .poll(() => seen.acknowledged.length, { timeout: 20_000 })
      .toBeGreaterThan(0);
    const failed = await worker?.evaluate(() =>
      (globalThis as { failDecoder?: () => boolean }).failDecoder?.(),
    );
    expect(failed, "the paint worker's first decoder was failed").toBe(true);

    // The page asks for a keyframe, the gateway asks the host, and the picture
    // comes back on a second decoder, with the session still the same one.
    await expect
      .poll(() => seen.sent.includes("refresh"), { timeout: 30_000 })
      .toBe(true);
    await expect
      .poll(() => keyframes(seen), { timeout: 20_000 })
      .toBeGreaterThan(1);
    const resumedAt = seen.batches.length;
    await expect
      .poll(
        () =>
          seen.batches
            .slice(resumedAt)
            .some((batch) => seen.acknowledged.includes(batch.sequence)),
        { timeout: 20_000 },
      )
      .toBe(true);
    expect(
      await worker?.evaluate(() =>
        (globalThis as { decodersMade?: () => number }).decodersMade?.(),
      ),
    ).toBeGreaterThan(1);
    checkUnits(seen);
    // The same session all along: nothing since it connected went to the picker.
    expect(
      seen.controlTypes.slice(seen.controlTypes.indexOf("connected")),
    ).not.toContain("picker");
    // What the failure said is taken down once the picture paints again.
    await expect(page.getByRole("alert")).toHaveCount(0, { timeout: 10_000 });

    await page.context().unrouteAll();
    await returnToPicker(page);
  });
});
