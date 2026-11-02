// The session's claims, taken one at a time for the whole page
// (useRemoteDesktop.ts). Every claim that succeeds mints a token that replaces
// the one before, and the next claim reclaims the session with it: two sent under
// the same token are the owner and, to the gateway, another browser, whose claim
// ends the session. So each waits for the one before to be answered and its token
// kept, whichever run of the connection effect sent it, and whether or not the
// attempt that made it has been given up meanwhile: the token kept last is the
// one the gateway minted last.

/**
 * Steps run one at a time, each once the one before has settled, in the order
 * they were asked for. A step that fails holds up none after it.
 */
export function createTurns(): <T>(step: () => T | Promise<T>) => Promise<T> {
  let last: Promise<unknown> = Promise.resolve();
  return (step) => {
    const turn = last.then(step);
    last = turn.catch(() => {});
    return turn;
  };
}
