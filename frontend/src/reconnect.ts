// The sentence a picture the page can no longer follow ends with (framePainter.ts):
// a pipeline whose compositor lost its WebGL context or refused a command, a
// Mac's picture that did not decode, a decoded picture or a display tab's that
// could not be shown.
// A reload reconnects the page to its session, which brings the picture back, so
// the banner offers it as a button too (RemoteDesktop.tsx).
export const RECONNECT = "Reload the page to reconnect.";

/** Whether `reason`, a failure the page shows, needs a reload to reconnect. */
export function asksToReconnect(reason: string): boolean {
  return reason.endsWith(RECONNECT);
}
