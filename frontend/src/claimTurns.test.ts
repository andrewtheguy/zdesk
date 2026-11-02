import assert from "node:assert/strict";
import { test } from "node:test";
import { claimsInTurn } from "./claimTurns.ts";

// A gateway's claims, answered by hand: each records the token it was sent with,
// as the page's storage held it then.
function gateway(storage: { token: string }) {
  const sent: { token: string; answer: (token: string) => void }[] = [];
  const claim = () =>
    new Promise<string>((answer) => {
      sent.push({ token: storage.token, answer });
    });
  return { sent, claim };
}

const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

test("a claim is sent only once the one before was answered, under its token", async () => {
  const storage = { token: "t0" };
  const { sent, claim } = gateway(storage);
  const inTurn = claimsInTurn(
    claim,
    (token) => {
      storage.token = token;
    },
    () => true,
  );
  const first = inTurn(false);
  const second = inTurn(false);
  await settle();
  assert.equal(sent.length, 1);
  sent[0].answer("t1");
  assert.equal(await first, "t1");
  await settle();
  assert.equal(sent.length, 2);
  assert.equal(sent[1].token, "t1");
  sent[1].answer("t2");
  assert.equal(await second, "t2");
  assert.equal(storage.token, "t2");
});

test("an ended run's late answer leaves its replacement's token", async () => {
  const storage = { token: "t0" };
  const { sent, claim } = gateway(storage);
  const keep = (token: string) => {
    storage.token = token;
  };
  let endedLive = true;
  const ended = claimsInTurn(claim, keep, () => endedLive);
  const replacement = claimsInTurn(claim, keep, () => true);
  const late = ended(false);
  endedLive = false;
  const current = replacement(false);
  await settle();
  assert.equal(sent.length, 2);
  sent[1].answer("t-replacement");
  await current;
  sent[0].answer("t-ended");
  await late;
  assert.equal(storage.token, "t-replacement");
});

test("a claim that failed does not hold up the next", async () => {
  let calls = 0;
  const inTurn = claimsInTurn(
    () => {
      calls += 1;
      return calls === 1
        ? Promise.reject(new Error("refused"))
        : Promise.resolve("t1");
    },
    () => {},
    () => true,
  );
  await assert.rejects(inTurn(false));
  assert.equal(await inTurn(false), "t1");
});
