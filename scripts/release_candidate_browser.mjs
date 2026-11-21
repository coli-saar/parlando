import { chromium, expect } from "@playwright/test";

/** Enters the hosted starter as a new participant with independent session storage. */
async function enter(browser, url) {
  const context = await browser.newContext();
  const page = await context.newPage();
  page.on("pageerror", error => console.error(error));
  await page.goto(url, { waitUntil: "domcontentloaded" });
  await page.getByRole("button", { name: "Enter waiting room" }).click();
  return page;
}

/** Plays Beacon Relay from intake through the shared terminal outcome. */
async function play(url) {
  const browser = await chromium.launch();
  try {
    const a = await enter(browser, url);
    await expect(a.getByRole("heading", { name: "Waiting for another participant" })).toBeVisible();
    const b = await enter(browser, url);
    for (const page of [a, b]) {
      await expect(page.getByRole("heading", { name: "Beacon Relay" })).toBeVisible();
      await expect(page.getByRole("progressbar")).toHaveAttribute("value", "0");
    }
    const target = Number(await a.getByRole("progressbar").getAttribute("max"));
    expect([4, 5, 6]).toContain(target);
    for (let charge = 0; charge < target; charge++) {
      const current = charge % 2 === 0 ? a : b;
      const partner = current === a ? b : a;
      await expect(partner.getByRole("button", { name: "Add one tick" })).toBeDisabled();
      await current.getByRole("button", { name: "Add one tick" }).click();
      if (charge + 1 < target) {
        for (const page of [a, b]) {
          await expect(page.getByRole("progressbar")).toHaveAttribute("value", String(charge + 1));
        }
      }
    }
    for (const page of [a, b]) {
      await expect(page.getByText(`Beacon activated at ${target} ticks.`)).toBeVisible();
    }
    await a.reload();
    await expect(a.getByText(`Beacon activated at ${target} ticks.`)).toBeVisible();
  } finally {
    await browser.close();
  }
}

await play(process.argv[2]);
