// Whether this browser has a `VideoDecoder`, which takes the H.264 a target sends
// with nothing of this page's to stand in: a High Performance Mac's, and
// wlshare's, its video of the whole desktop and its tiles' moving regions'. Not
// asked of the decoder: a browser with a `VideoDecoder` and no H.264 is too
// unlikely to grey for, so having one is the answer, and a decoder that refuses
// the stream says so in the session. Read once, before the client mounts, beside
// the questions about the Mac's HEVC (appleMedia.ts) and an RDP host's H.264
// (rdpH264.ts).
//
// A browser without a `VideoDecoder` at all starts like any other: everything it
// cannot decode is greyed at the picker with the reason, and what it can — the
// gateway's tiles, a Standard Mac's Adaptive picture, wlshare's progressive tiles
// and an RDP host's pipeline without H.264, each decoded by the page — still
// starts. So the answer says no for it, and the picker greys by it
// (targetChoices.ts): the Mac's H.264, and wlshare's video and its tiles with
// motion. The gateway is not told: it is sent no choice the picker greys.

let answers: { appleH264: boolean } | null = null;

/** Read the browser once, and remember the answer. */
export async function askVideoCodecs(): Promise<void> {
  if (answers !== null) {
    return;
  }
  answers = { appleH264: typeof VideoDecoder !== "undefined" };
}

function answered(): { appleH264: boolean } {
  if (answers === null) {
    throw new Error("video codecs read before askVideoCodecs() resolved");
  }
  return answers;
}

/**
 * Whether it has a `VideoDecoder`, which is taken to decode H.264, the Mac's and
 * wlshare's. Only valid after `askVideoCodecs` has resolved, which `main.tsx`
 * awaits before mounting.
 */
export function decodesAppleH264(): boolean {
  return answered().appleH264;
}

/** Test seam: forget the answers so the question can be asked again. */
export function resetVideoCodecsForTests(): void {
  answers = null;
}
