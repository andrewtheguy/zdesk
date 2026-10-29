// Whether this browser's own `VideoDecoder` takes the H.264 a target sends with
// nothing of this page's to stand in: a High Performance Mac's, and wlshare's,
// its video of the whole desktop and its tiles' moving regions'. Read once,
// before the client mounts, beside the questions about the Mac's HEVC
// (appleMedia.ts) and an RDP host's H.264 (rdpH264.ts).
//
// The Mac's is not asked of the decoder: a browser with a `VideoDecoder` and no
// H.264 is too unlikely to grey for, so having one is the answer, and a decoder
// that refuses the stream says so in the session. wlshare's is asked, since its
// video is the target's default picture and a browser whose decoder refuses it
// would show nothing where the progressive tiles would draw: both profiles
// wlshare codes, as a doubt reads as no, a missing `VideoDecoder` among them.
//
// A browser without a `VideoDecoder` at all starts like any other: everything it
// cannot decode is greyed at the picker with the reason, and what it can — the
// gateway's tiles, a Standard Mac's Adaptive picture, wlshare's progressive tiles
// and an RDP host's pipeline without H.264, each decoded by the page — still
// starts. So the answers say no for it, and the picker greys by them
// (targetChoices.ts): the Mac's H.264, and wlshare's video and its tiles with
// motion. The gateway is not told: it is sent no choice the picker greys.

/**
 * wlshare's H.264 of a 1080p desktop at the 60 fps its level is figured at
 * (level 4.2), in the shape the gateway announces it: Constrained Baseline,
 * libx264's, and High, a VA-API encoder's. Which one a session gets is the
 * host's, so both are asked.
 */
const WLSHARE_PROBES = ["avc1.42C02A", "avc1.64002A"];

let answers: { appleH264: boolean; wlshareH264: boolean } | null = null;

async function decodes(codec: string): Promise<boolean> {
  try {
    const reply = await VideoDecoder.isConfigSupported({ codec });
    return reply.supported === true;
  } catch {
    return false;
  }
}

/** Read the browser once, and remember the answers. */
export async function askVideoCodecs(): Promise<void> {
  if (answers !== null) {
    return;
  }
  const wlshare = await Promise.all(WLSHARE_PROBES.map(decodes));
  answers = {
    appleH264: typeof VideoDecoder !== "undefined",
    wlshareH264: wlshare.every(Boolean),
  };
}

function answered(): { appleH264: boolean; wlshareH264: boolean } {
  if (answers === null) {
    throw new Error("video codecs read before askVideoCodecs() resolved");
  }
  return answers;
}

/**
 * Whether it has a `VideoDecoder`, which is taken to decode the Mac's H.264. Only
 * valid after `askVideoCodecs` has resolved, which `main.tsx` awaits before
 * mounting.
 */
export function decodesAppleH264(): boolean {
  return answered().appleH264;
}

/** Whether its decoder said it takes both of wlshare's H.264 profiles. */
export function decodesWlshareH264(): boolean {
  return answered().wlshareH264;
}

/** Test seam: forget the answers so the question can be asked again. */
export function resetVideoCodecsForTests(): void {
  answers = null;
}
