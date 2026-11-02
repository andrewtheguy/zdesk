// The browser taking the picture's WebGL context away, and the session started
// over once it gives the context back: what the page says meanwhile, and that
// no reload is asked for.
//
// iOS Safari takes a page's WebGL context for a tab in the background or short
// of memory. Chromium takes every page's when its GPU process ends, which the
// DevTools protocol's `Browser.crashGpuProcess` makes happen, and gives them
// back with the process it starts again. The page treats the loss as a dropped
// socket (useRemoteDesktop.ts): it reconnects once the context is back, and the
// gateway starts a passed pipeline over for a page that holds nothing. What is
// asserted is the page's account of it, the DOM's: the status said while the
// context is gone, the picture shown again after, and no sentence asking for a
// reload. Nothing here looks at a pixel.
//
// It needs a gateway whose local config names a live RDP host, whose pipeline
// the page composes on the GPU (the passthrough spec's setup):
//
//     ZDESK_PLAYWRIGHT_BASE_URL=http://127.0.0.1:52888/ \
//     ZDESK_PLAYWRIGHT_USERNAME=admin \
//     ZDESK_PLAYWRIGHT_PASSWORD=… \
//     ZDESK_PLAYWRIGHT_EGFX_TARGET=win \
//     ZDESK_PLAYWRIGHT_EGFX_DISPLAYS_TARGET=win2 \
//     bunx playwright test '/webgl-context-restart\.spec\.ts$'
//
// The two-display case needs the second, a target asked for two virtual
// displays, and runs only when it is named.
import { expect, type Page, test } from "@playwright/test";

import { leaveSession, logInAndConnectTo } from "./support";

const EGFX_TARGET = process.env.ZDESK_PLAYWRIGHT_EGFX_TARGET;
/// The same for a target over two virtual displays, as the two-display spec has
/// it: its second display is a tab's, whose context goes with the same GPU.
const DISPLAYS_TARGET = process.env.ZDESK_PLAYWRIGHT_EGFX_DISPLAYS_TARGET;

/// What a session page says of itself while its sockets are down.
const RECONNECTING = "Reconnecting…";

/// Every alert and status text the page has shown, in order, each once: recorded
/// in the page, since one shown and taken back between two polls is still said.
async function recordSaid(page: Page) {
  await page.evaluate(() => {
    const said: string[] = [];
    (window as unknown as { said: string[] }).said = said;
    const note = () => {
      for (const node of Array.from(
        document.querySelectorAll('[role="alert"], .status'),
      )) {
        const text = node.textContent ?? "";
        if (text && said.at(-1) !== text) {
          said.push(text);
        }
      }
    };
    note();
    new MutationObserver(note).observe(document.body, {
      subtree: true,
      childList: true,
      characterData: true,
    });
  });
  return () =>
    page.evaluate(() => (window as unknown as { said: string[] }).said);
}

// Chromium itself, in its headless mode: the headless shell's renderer dies
// with its GPU process, and no page outlives that.
test.use({ channel: "chromium" });

test.describe("a picture drawn on the GPU", () => {
  test.skip(
    !EGFX_TARGET,
    "set ZDESK_PLAYWRIGHT_EGFX_TARGET to a target whose pipeline is passed",
  );

  test.afterEach(async ({ page }) => {
    await leaveSession(page);
  });

  test("starts the session over when the browser gives its context back", async ({
    page,
  }) => {
    test.setTimeout(90_000);
    await logInAndConnectTo(page, EGFX_TARGET ?? "", "", {});
    const graphics = page.locator("canvas.graphics");
    await expect(graphics).toBeVisible({ timeout: 20_000 });
    await expect(page.getByRole("alert")).toHaveCount(0);
    const said = await recordSaid(page);

    // The GPU process ended, and with it every page's contexts.
    const browser = page.context().browser();
    if (!browser) {
      throw new Error("no browser to end the GPU process of");
    }
    const cdp = await browser.newBrowserCDPSession();
    await cdp.send("Browser.crashGpuProcess");
    await cdp.detach();

    // The page drops its sockets, as for a network drop, and comes back.
    await expect
      .poll(async () => (await said()).includes(RECONNECTING), {
        message: "the page said it was reconnecting",
        timeout: 20_000,
      })
      .toBe(true);
    await expect(page.getByText(RECONNECTING)).toHaveCount(0, {
      timeout: 40_000,
    });
    await expect(graphics).toBeVisible({ timeout: 20_000 });
    await expect(page.getByRole("alert")).toHaveCount(0);
    expect(
      (await said()).filter((text) => /[Rr]eload the page/.test(text)),
      "no reload asked for",
    ).toEqual([]);
  });
});

test.describe("a picture drawn on the GPU, over two displays", () => {
  test.skip(
    !DISPLAYS_TARGET,
    "set ZDESK_PLAYWRIGHT_EGFX_DISPLAYS_TARGET to a target over two virtual displays",
  );

  test.afterEach(async ({ page }) => {
    await leaveSession(page);
  });

  // The session's page starts its session over, which lets the tab's display go
  // and announces it again only once the engine is back. The tab, whose context
  // went with the same GPU, rides that out: it is shown its display again, and
  // asks nobody to retry or reload.
  test("brings a second display's tab back with the session", async ({
    page,
    context,
  }) => {
    test.setTimeout(120_000);
    await logInAndConnectTo(page, DISPLAYS_TARGET ?? "", "", {});
    const graphics = page.locator("canvas.graphics");
    await expect(graphics).toBeVisible({ timeout: 20_000 });
    await page.getByRole("button", { name: "Open menu" }).click();
    const opening = context.waitForEvent("page");
    await page.getByRole("link", { name: "Open Display 2 ↗" }).click();
    const tab = await opening;
    await page.getByRole("button", { name: "Close menu" }).click();
    await tab.waitForURL(/\/display\/2$/);
    const tabGraphics = tab.locator("canvas.graphics");
    await expect(tabGraphics).toBeVisible({ timeout: 20_000 });
    await expect(tab.getByRole("alert")).toHaveCount(0);
    const said = await recordSaid(page);
    const tabSaid = await recordSaid(tab);

    const browser = page.context().browser();
    if (!browser) {
      throw new Error("no browser to end the GPU process of");
    }
    const cdp = await browser.newBrowserCDPSession();
    await cdp.send("Browser.crashGpuProcess");
    await cdp.detach();

    await expect
      .poll(async () => (await said()).includes(RECONNECTING), {
        message: "the session's page said it was reconnecting",
        timeout: 20_000,
      })
      .toBe(true);
    await expect(page.getByText(RECONNECTING)).toHaveCount(0, {
      timeout: 40_000,
    });
    await expect(graphics).toBeVisible({ timeout: 20_000 });
    // The tab's retries back off while the engine starts over: shown again
    // within a few of them, and never left on a refusal.
    await expect(tab.getByText(RECONNECTING)).toHaveCount(0, {
      timeout: 60_000,
    });
    await expect(tabGraphics).toBeVisible({ timeout: 20_000 });
    await expect(tab.getByRole("button", { name: "Retry" })).toHaveCount(0);
    await expect(tab.getByRole("alert")).toHaveCount(0);
    await expect(page.getByRole("alert")).toHaveCount(0);
    expect(
      [...(await said()), ...(await tabSaid())].filter((text) =>
        /[Rr]eload the page|Retry/.test(text),
      ),
      "no reload or retry asked for",
    ).toEqual([]);
    await tab.close();
  });
});
