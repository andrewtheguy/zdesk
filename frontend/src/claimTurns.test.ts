import assert from "node:assert/strict";
import { test } from "node:test";
import { createTurns } from "./claimTurns.ts";

// A gateway's claim as `SessionManager::claim` decides it: the token it is sent
// with is the owner's when it is the one minted last, and any other is another
// browser's, whose claim ends the session. Answered by hand.
function gateway() {
  let current = "t0";
  let minted = 0;
  const ended: string[] = [];
  const pending: (() => void)[] = [];
  const claim = (token: string | null) =>
    new Promise<string>((answer) => {
      pending.push(() => {
        if (token !== current) {
          ended.push(`claim under ${token}`);
        }
        minted += 1;
        current = `t${minted}`;
        answer(current);
      });
    });
  return {
    claim,
    ended,
    current: () => current,
    answerNext: () => pending.shift()?.(),
    pending: () => pending.length,
  };
}

const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

test("an ended run's claim and its replacement's go in turn, under the token kept last", async () => {
  const gw = gateway();
  const storage: { token: string | null } = { token: "t0" };
  const turns = createTurns();
  // Two runs of the connection effect, the first given up while its claim is out.
  const claimAndKeep = () =>
    turns(async () => {
      const token = await gw.claim(storage.token);
      storage.token = token;
      return token;
    });
  const ended = claimAndKeep();
  const replacement = claimAndKeep();
  await settle();
  assert.equal(gw.pending(), 1, "the replacement's claim waits");
  gw.answerNext();
  await ended;
  await settle();
  gw.answerNext();
  await replacement;
  assert.deepEqual(gw.ended, []);
  assert.equal(storage.token, gw.current());
});

test("a token dropped in turn stays dropped after a claim that was out", async () => {
  const gw = gateway();
  const storage: { token: string | null } = { token: "t0" };
  const turns = createTurns();
  const out = turns(async () => {
    storage.token = await gw.claim(storage.token);
  });
  const dropped = turns(() => {
    storage.token = null;
  });
  await settle();
  gw.answerNext();
  await out;
  await dropped;
  assert.equal(storage.token, null);
});

test("a step that failed does not hold up the next", async () => {
  const turns = createTurns();
  await assert.rejects(turns(() => Promise.reject(new Error("refused"))));
  assert.equal(await turns(() => "t1"), "t1");
});
