// Whether this browser's own `VideoDecoder` takes the H.264 a target sends with
// nothing of this page's to stand in: a High Performance Mac's, wlshare's, its
// video of the whole desktop and its tiles' moving regions', and an RDP host's
// whole desktop, passed as video under H.264 full screen. Read once,
// before the client mounts, beside the questions about the Mac's HEVC
// (appleMedia.ts) and an RDP host's H.264 (rdpH264.ts).
//
// The Mac's is not asked of the decoder: a browser with a `VideoDecoder` and no
// H.264 is too unlikely to grey for, so having one is the answer, and a decoder
// that refuses the stream says so in the session. wlshare's is asked, since its
// video is the target's default picture and a browser whose decoder refuses it
// would show nothing where the progressive tiles would draw: the profile
// wlshare codes, as a doubt reads as no, a missing `VideoDecoder` among them.
//
// An RDP host's is asked too, and the gateway is told (`rdp_video`): it is what
// H.264 full screen needs, and all it needs, since nothing composes it, so a
// browser that cannot compose the host's pipeline (rdpGraphics.ts) can still
// start a target that passes one, drawn that way.
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
 * (level 4.2), in the shape the gateway announces it: High, which libx264 and
 * a VA-API encoder both code.
 */
const WLSHARE_PROBE = "avc1.64002A";

/**
 * An RDP host's H.264, as a Windows 11 host's sequence parameter set names it:
 * Main profile, level 3.2 (rdpH264.ts). A larger desktop's names a higher level,
 * which a decoder that takes this one and refuses that says of in the session.
 */
const RDP_PROBE = "avc1.4d4020";

interface Answers {
  appleH264: boolean;
  wlshareH264: boolean;
  rdpVideo: boolean;
}

let answers: Answers | null = null;

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
  const [wlshare, rdp] = await Promise.all([
    decodes(WLSHARE_PROBE),
    decodes(RDP_PROBE),
  ]);
  answers = {
    appleH264: typeof VideoDecoder !== "undefined",
    wlshareH264: wlshare,
    rdpVideo: rdp,
  };
}

function answered(): Answers {
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

/** Whether its decoder said it takes wlshare's H.264. */
export function decodesWlshareH264(): boolean {
  return answered().wlshareH264;
}

/** Whether its decoder said it takes an RDP host's H.264, as video. */
export function decodesRdpVideo(): boolean {
  return answered().rdpVideo;
}

/** Test seam: forget the answers so the question can be asked again. */
export function resetVideoCodecsForTests(): void {
  answers = null;
}
