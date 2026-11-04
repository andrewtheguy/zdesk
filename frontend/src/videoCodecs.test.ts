// What the page reads of the browser's video decoding: any `VideoDecoder` is
// offered the Mac's H.264 unasked, wlshare's only where its decoder takes both
// profiles wlshare codes, an RDP host's as video only where its decoder takes it,
// and a browser with none is offered none; and the answers are read once, and not
// before they are asked.
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

const {
  askVideoCodecs,
  decodesAppleH264,
  decodesRdpVideo,
  decodesWlshareH264,
  resetVideoCodecsForTests,
} = await import("./videoCodecs.ts");

/** A `VideoDecoder` that takes the codecs `takes` says yes to. */
function decoder(takes: (codec: string) => boolean | "throws"): string[] {
  const asked: string[] = [];
  resetVideoCodecsForTests();
  globals.VideoDecoder = {
    isConfigSupported: async ({ codec }) => {
      asked.push(codec);
      const answer = takes(codec);
      if (answer === "throws") {
        throw new TypeError("not a codec it knows");
      }
      return { supported: answer };
    },
  };
  return asked;
}

test("wlshare's H.264 is asked in its profile, and an RDP host's", async () => {
  const asked = decoder(() => true);
  await askVideoCodecs();
  assert.deepEqual(asked.sort(), ["avc1.4d4020", "avc1.64002A"]);
  assert.equal(decodesWlshareH264(), true);
  assert.equal(decodesAppleH264(), true);
  assert.equal(decodesRdpVideo(), true);
});

test("a decoder that refuses an RDP host's H.264 is offered none as video", async () => {
  decoder((codec) => codec !== "avc1.4d4020");
  await askVideoCodecs();
  assert.equal(decodesRdpVideo(), false);
  assert.equal(decodesWlshareH264(), true);
});

test("a decoder that refuses wlshare's profile, or doubts it, is offered no wlshare H.264", async () => {
  decoder((codec) => codec !== "avc1.64002A");
  await askVideoCodecs();
  assert.equal(decodesWlshareH264(), false);
  assert.equal(decodesAppleH264(), true, "the Mac's is not asked");
  decoder(() => "throws");
  await askVideoCodecs();
  assert.equal(decodesWlshareH264(), false);
});

test("a browser with no VideoDecoder is offered no H.264", async () => {
  resetVideoCodecsForTests();
  globals.VideoDecoder = undefined;
  await askVideoCodecs();
  assert.equal(decodesAppleH264(), false);
  assert.equal(decodesWlshareH264(), false);
  assert.equal(decodesRdpVideo(), false);
});

test("the answers are read once, and not before they are asked", async () => {
  resetVideoCodecsForTests();
  globals.VideoDecoder = undefined;
  assert.throws(() => decodesAppleH264(), /before askVideoCodecs/);
  assert.throws(() => decodesWlshareH264(), /before askVideoCodecs/);
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
  assert.equal(decodesWlshareH264(), false);
});
