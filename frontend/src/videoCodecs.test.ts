// What the page reads of the browser's video decoding: any `VideoDecoder` is
// offered H.264, the Mac's and wlshare's, unasked, and a browser with none is
// not; and the answer is read once, and not before it is asked.
import assert from "node:assert/strict";
import { test } from "node:test";

const globals = globalThis as unknown as {
  VideoDecoder:
    | {
        isConfigSupported: (config: {
          codec: string;
        }) => Promise<{ supported?: boolean }>;
      }
    | undefined;
};

const { askVideoCodecs, decodesAppleH264, resetVideoCodecsForTests } =
  await import("./videoCodecs.ts");

test("a VideoDecoder is the answer, and its decoder is not asked", async () => {
  const asked: string[] = [];
  resetVideoCodecsForTests();
  globals.VideoDecoder = {
    isConfigSupported: async ({ codec }) => {
      asked.push(codec);
      return { supported: false };
    },
  };
  await askVideoCodecs();
  assert.deepEqual(asked, []);
  assert.equal(decodesAppleH264(), true);
});

test("a browser with no VideoDecoder is offered no H.264", async () => {
  resetVideoCodecsForTests();
  globals.VideoDecoder = undefined;
  await askVideoCodecs();
  assert.equal(decodesAppleH264(), false);
});

test("the answer is read once, and not before it is asked", async () => {
  resetVideoCodecsForTests();
  globals.VideoDecoder = undefined;
  assert.throws(() => decodesAppleH264(), /before askVideoCodecs/);
  await askVideoCodecs();
  globals.VideoDecoder = {
    isConfigSupported: async () => ({ supported: true }),
  };
  await askVideoCodecs();
  assert.equal(
    decodesAppleH264(),
    false,
    "asked again, it kept its first answer",
  );
});
