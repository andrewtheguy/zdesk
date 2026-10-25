// A failed connect's message is shown on the picker against the attempt it
// belongs to, and taken down when the picker moves to another screen: a target
// closed or another opened, or the throughput view. Left up, it read as the
// failure of whichever target was on screen next (#668).
//
// What is asserted is the page's decision to show or drop the message, by its
// alert role; its wording is the gateway's and is not checked.
//
// It needs a gateway with two targets whose connects fail, such as two `vnc`
// targets on closed ports of 127.0.0.1, and `[meter].enabled` for the throughput
// view's case, then:
//
//     ZDESK_PLAYWRIGHT_BASE_URL=http://127.0.0.1:PORT/ \
//     ZDESK_PLAYWRIGHT_USERNAME=admin \
//     ZDESK_PLAYWRIGHT_PASSWORD=... \
//     ZDESK_PLAYWRIGHT_REFUSED_TARGETS=closed-one,closed-two \
//     bunx playwright test '/picker-error\.spec\.ts$'
import { expect, type Page, test } from "@playwright/test";

import { leaveSession, logIn, startTarget, targetNamePattern } from "./support";

const REFUSED_TARGETS = (process.env.ZDESK_PLAYWRIGHT_REFUSED_TARGETS ?? "")
  .split(",")
  .filter((name) => name !== "");

/// Start `target` and wait for the picker to say it failed.
async function failToStart(page: Page, target: string): Promise<void> {
  await startTarget(page, target, {});
  await expect(page.getByRole("alert")).toBeVisible({ timeout: 20_000 });
  await expect(
    page.getByRole("heading", { name: "Pick a target" }),
  ).toBeVisible();
}

function targetButton(page: Page, target: string) {
  return page.getByRole("button", { name: targetNamePattern(target) });
}

test.beforeEach(() => {
  test.skip(
    REFUSED_TARGETS.length < 2,
    "set ZDESK_PLAYWRIGHT_REFUSED_TARGETS=<a>,<b>: two targets whose connects fail",
  );
});

test.afterEach(async ({ page }) => {
  await leaveSession(page);
});

// A failed start puts the picker back with every target closed and the message
// above them, so each move from there is a click that opens something.

test("opening the failed target again drops its message", async ({ page }) => {
  const [first] = REFUSED_TARGETS;
  await logIn(page);
  await failToStart(page, first);
  await targetButton(page, first).click();
  await expect(targetButton(page, first)).toHaveAttribute(
    "aria-expanded",
    "true",
  );
  await expect(page.getByRole("alert")).toBeHidden();
});

test("opening another target drops the message", async ({ page }) => {
  const [first, second] = REFUSED_TARGETS;
  await logIn(page);
  await failToStart(page, first);
  await targetButton(page, second).click();
  await expect(targetButton(page, second)).toHaveAttribute(
    "aria-expanded",
    "true",
  );
  await expect(page.getByRole("alert")).toBeHidden();
});

// Offered only on a gateway with `[meter].enabled`.
test("the throughput view drops the message", async ({ page }) => {
  const [first] = REFUSED_TARGETS;
  await logIn(page);
  await failToStart(page, first);
  const throughput = page.getByRole("button", { name: "Throughput" });
  test.skip(
    !(await throughput.isVisible()),
    "the gateway offers no throughput view: enable [meter]",
  );
  await throughput.click();
  await page.getByRole("button", { name: "Back to targets" }).click();
  await expect(
    page.getByRole("heading", { name: "Pick a target" }),
  ).toBeVisible();
  await expect(page.getByRole("alert")).toBeHidden();
});
