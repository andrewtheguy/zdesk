// The browser taking the picture's WebGL context away: the session's page says
// so, offers Reconnect, and the picture comes back when it is pressed.
//
// iOS Safari takes a page's WebGL context for a tab in the background or short
// of memory. Chromium takes every page's when its GPU process ends, which the
// DevTools protocol's `Browser.crashGpuProcess` makes happen. The page leaves
// the session as for a takeover (useRemoteDesktop.ts), and reconnecting draws
// the picture on new canvases, which the gateway starts a passed pipeline over
// for. What is asserted is the page's account of it, the DOM's: the status and
// its button, and the picture shown again after. Nothing here looks at a pixel.
//
// It needs a gateway whose local config names a live RDP host, whose pipeline
// the page composes on the GPU (the passthrough spec's setup):
//
//     ZDESK_PLAYWRIGHT_BASE_URL=http://127.0.0.1:52888/ \
//     ZDESK_PLAYWRIGHT_USERNAME=admin \
//     ZDESK_PLAYWRIGHT_PASSWORD=… \
//     ZDESK_PLAYWRIGHT_EGFX_TARGET=win \
//     bunx playwright test '/webgl-context-lost\.spec\.ts$'
import { expect, test } from "@playwright/test";

import { leaveSession, logInAndConnectTo } from "./support";

const EGFX_TARGET = process.env.ZDESK_PLAYWRIGHT_EGFX_TARGET;

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

  test("says it was lost, and comes back when reconnected", async ({
    page,
  }) => {
    test.setTimeout(90_000);
    await logInAndConnectTo(page, EGFX_TARGET ?? "", "", {});
    const graphics = page.locator("canvas.graphics");
    await expect(graphics).toBeVisible({ timeout: 20_000 });
    await expect(page.getByRole("alert")).toHaveCount(0);

    // The GPU process ended, and with it every page's contexts.
    const browser = page.context().browser();
    if (!browser) {
      throw new Error("no browser to end the GPU process of");
    }
    const cdp = await browser.newBrowserCDPSession();
    await cdp.send("Browser.crashGpuProcess");
    await cdp.detach();

    // The page stops, and waits for the person rather than reconnecting.
    await expect(page.getByText("Picture lost")).toBeVisible({
      timeout: 20_000,
    });
    await expect(page.getByText("Reconnecting…")).toHaveCount(0);
    await expect(page.getByRole("alert")).toHaveCount(0);

    await page.getByRole("button", { name: "Reconnect" }).click();
    await expect(page.getByText("Picture lost")).toHaveCount(0, {
      timeout: 40_000,
    });
    await expect(graphics).toBeVisible({ timeout: 20_000 });
    await expect(page.getByRole("alert")).toHaveCount(0);
  });
});
