// The camera sender's pure halves: the H.264 configuration chosen for a
// geometry, and the part of a camera's frame fitted into the announced one. The
// capture and socket halves need a camera and a gateway, which is browser QA's
// business.
//
// Run with `bun test src/cameraSender.test.ts` from frontend/.
import assert from "node:assert/strict";
import { test } from "node:test";
import {
  CAMERA_FPS,
  CAMERA_HEIGHT,
  CAMERA_WIDTH,
  centreCrop,
  h264Config,
  normalizeConstrainedBaseline,
} from "./cameraSender";
import { encodeCameraFrame } from "./protocol";

test("a frame of the announced shape is shown whole", () => {
  assert.deepEqual(centreCrop(1280, 720, 640, 360), {
    x: 0,
    y: 0,
    width: 1280,
    height: 720,
  });
});

test("a 4:3 camera loses its top and bottom, centred", () => {
  // 640x480 into 16:9 keeps 640x360, 60 rows off each edge.
  assert.deepEqual(centreCrop(640, 480, 640, 360), {
    x: 0,
    y: 60,
    width: 640,
    height: 360,
  });
});

test("a camera wider than 16:9 loses its sides, centred", () => {
  // 21:9 at 1080 rows keeps 1920 of its 2560 columns.
  assert.deepEqual(centreCrop(2560, 1080, 640, 360), {
    x: 320,
    y: 0,
    width: 1920,
    height: 1080,
  });
});

test("the announced format fits level 3.1 at the bitrate floor", () => {
  // 40x23 macroblocks at 15 fps, and 640*360*15*0.1 = 345,600 bit/s.
  assert.deepEqual(h264Config(CAMERA_WIDTH, CAMERA_HEIGHT, CAMERA_FPS), {
    codec: "avc1.42e01f",
    bitrate: 345_600,
  });
});

test("a 720p30 camera fits level 3.1", () => {
  // 80x45 macroblocks at 30 fps = 108,000/s, exactly level 3.1's limit.
  assert.equal(h264Config(1280, 720, 30).codec, "avc1.42e01f");
});

test("1080p30 needs level 4.0 and 4K needs 5.1 at any rate", () => {
  assert.equal(h264Config(1920, 1080, 30).codec, "avc1.42e028");
  // 3840x2160 is 32,400 macroblocks a frame, past level 5.0's 22,080, even
  // though 15 fps is inside its macroblock rate.
  assert.equal(h264Config(3840, 2160, 15).codec, "avc1.42e033");
  assert.equal(h264Config(3840, 2160, 30).codec, "avc1.42e033");
  assert.equal(h264Config(3840, 2160, 60).codec, "avc1.42e034");
});

test("the bitrate is a tenth of a bit per pixel per frame, clamped", () => {
  // 1280*720*30*0.1 = 2.76 Mbit/s — inside the clamp, so exactly that.
  assert.equal(h264Config(1280, 720, 30).bitrate, 2_764_800);
  // A tiny capture stays at the floor, a 4K60 one at the ceiling.
  assert.equal(h264Config(160, 120, 15).bitrate, 300_000);
  assert.equal(h264Config(3840, 2160, 60).bitrate, 8_000_000);
});

test("baseline SPS units are normalized to the requested constrained profile", () => {
  const unit = new Uint8Array([
    0, 0, 0, 1, 0x27, 0x42, 0x00, 0x1f, 0xaa, 0, 0, 1, 0x28, 0xce, 0x3c,
  ]);
  normalizeConstrainedBaseline(unit);
  assert.deepEqual(
    Array.from(unit),
    [0, 0, 0, 1, 0x27, 0x42, 0xe0, 0x1f, 0xaa, 0, 0, 1, 0x28, 0xce, 0x3c],
  );
});

test("SPS normalization handles three-byte start codes and preserves other profiles", () => {
  const unit = new Uint8Array([
    0, 0, 1, 0x67, 0x42, 0x40, 0x1f, 0, 0, 1, 0x67, 0x64, 0x00, 0x28,
  ]);
  normalizeConstrainedBaseline(unit);
  assert.deepEqual(
    Array.from(unit),
    [0, 0, 1, 0x67, 0x42, 0xe0, 0x1f, 0, 0, 1, 0x67, 0x64, 0x00, 0x28],
  );
});

// The layout mirrors `camera` in src/protocol.rs; the Rust side's parser has
// its own tests over the same bytes, which is the two-ends check the audio
// frame gets between decodeAudioFrame and `audio::frame`.
test("a camera frame is kind then flags then the unit, keyframe in bit zero", () => {
  assert.deepEqual(
    Array.from(encodeCameraFrame(new Uint8Array([9, 8]), true) ?? []),
    [0x04, 0x01, 9, 8],
  );
  assert.deepEqual(
    Array.from(encodeCameraFrame(new Uint8Array([7]), false) ?? []),
    [0x04, 0x00, 7],
  );
});

test("an empty unit is refused, matching the gateway parser's rejection", () => {
  assert.equal(encodeCameraFrame(new Uint8Array(), true), null);
});
