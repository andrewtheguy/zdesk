// The session claims one run of the connection effect makes, taken one at a time
// (useRemoteDesktop.ts). Every claim that succeeds mints a token that replaces
// the one before, and the next claim reclaims the session with it: two sent under
// the same token are the owner and, to the gateway, another browser, whose claim
// ends the session. So each claim waits for the one before, whose token is kept
// first, even when the attempt that made it has been given up meanwhile.
//
// A run that has ended keeps nothing: the run that replaced it claims under the
// token it found, and a late answer to the ended run must not overwrite the one
// its replacement kept.

/**
 * Claims through `claim`, one at a time. `keep` is given each answer before the
 * next claim is sent, while `live` says the run is still the page's.
 */
export function claimsInTurn<T>(
  claim: (force: boolean) => Promise<T>,
  keep: (claimed: T) => void,
  live: () => boolean,
): (force: boolean) => Promise<T> {
  let claiming: Promise<unknown> = Promise.resolve();
  return (force) => {
    const turn = claiming
      .then(() => claim(force))
      .then((claimed) => {
        if (live()) {
          keep(claimed);
        }
        return claimed;
      });
    claiming = turn.catch(() => {});
    return turn;
  };
}
