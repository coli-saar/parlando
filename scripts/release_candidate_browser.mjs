import { chromium, expect } from "@playwright/test";

/** Enters the hosted starter as a new participant with independent session storage. */
async function enter(browser, url) {
  const context = await browser.newContext();
  await context.addInitScript(() => {
    window.microphoneRequests = 0;
    const devices = navigator.mediaDevices;
    if (devices) {
      devices.getUserMedia = () => {
        window.microphoneRequests++;
        return Promise.reject(new Error("Typed play requested microphone access"));
      };
    }
  });
  const page = await context.newPage();
  page.on("pageerror", error => console.error(error));
  await page.goto(url, { waitUntil: "domcontentloaded" });
  const enterRoom = page.getByRole("button", { name: "Enter waiting room" });
  await expect(enterRoom).toBeEnabled();
  await expectTypedControls(page);
  await enterRoom.click();
  return page;
}

/** Confirms that the current participant screen offers no microphone setup or mute controls. */
async function expectTypedControls(page) {
  await expect(page.getByRole("button", { name: /microphone|prepare voice|(?:un)?mute mic/i })).toHaveCount(0);
  await expect(page.getByRole("combobox", { name: /microphone/i })).toHaveCount(0);
  await expect(page.locator(".mic-meter")).toHaveCount(0);
  await expect(page.locator('[data-mode="speech"]')).toHaveCount(0);
  expect(await page.evaluate(() => window.microphoneRequests)).toBe(0);
}

/** Sends one message from each seat and verifies one delivery per message in each browser. */
async function exchangeMessages(pages) {
  const messages = ["I will charge the beacon first.", "I will take the next turn."];
  for (const [index, page] of pages.entries()) {
    await page.getByRole("textbox", { name: "Message" }).fill(messages[index]);
    await page.getByRole("button", { name: "Send message" }).click();
    const partner = pages[1 - index];
    const conversation = partner.getByRole("list", { name: "Text conversation" });
    await expect(conversation.getByText(messages[index], { exact: true })).toHaveCount(1);
  }
  for (const page of pages) {
    const conversation = page.getByRole("list", { name: "Text conversation" });
    await expect(conversation.getByRole("listitem")).toHaveCount(2);
    for (const message of messages) {
      await expect(conversation.getByText(message, { exact: true })).toHaveCount(1);
    }
  }
}

/** Plays a hosted Typed experiment from intake through the shared terminal outcome. */
async function play(url) {
  const browser = await chromium.launch();
  try {
    const config = await fetch(new URL("api/config", url));
    expect(config.ok).toBe(true);
    expect((await config.json()).communication.mode).toBe("typed");
    const a = await enter(browser, url);
    await expect(a.getByRole("heading", { name: "Waiting for another participant" })).toBeVisible();
    await expectTypedControls(a);
    const b = await enter(browser, url);
    const pages = [a, b];
    for (const page of pages) {
      await expect(page.getByRole("heading", { name: "Beacon Relay" })).toBeVisible();
      await expect(page.getByRole("progressbar")).toHaveAttribute("value", "0");
      await expect(page.getByRole("region", { name: "Communication" })).toHaveAttribute("data-mode", "typed");
      await expectTypedControls(page);
    }
    await exchangeMessages(pages);
    const target = Number(await a.getByRole("progressbar").getAttribute("max"));
    expect([4, 5, 6]).toContain(target);
    for (let charge = 0; charge < target; charge++) {
      const current = charge % 2 === 0 ? a : b;
      const partner = current === a ? b : a;
      await expect(partner.getByRole("button", { name: "Add one tick" })).toBeDisabled();
      await current.getByRole("button", { name: "Add one tick" }).click();
      if (charge + 1 < target) {
        for (const page of pages) {
          await expect(page.getByRole("progressbar")).toHaveAttribute("value", String(charge + 1));
        }
      }
    }
    for (const page of pages) {
      await expect(page.getByText(`Beacon activated at ${target} ticks.`)).toBeVisible();
      await expectTypedControls(page);
    }
    await a.reload();
    await expect(a.getByText(`Beacon activated at ${target} ticks.`)).toBeVisible();
    await expectTypedControls(a);
  } finally {
    await browser.close();
  }
}

/** Plays one human seat and checks that the installed policy supplies the partner's pulses. */
async function playAgent(url) {
  const browser = await chromium.launch();
  try {
    const page = await enter(browser, url);
    await expect(page.getByRole("heading", { name: "Beacon Relay" })).toBeVisible();
    await expectTypedControls(page);
    const charge = page.getByRole("progressbar");
    const pulse = page.getByRole("button", { name: "Add one tick" });
    const target = Number(await charge.getAttribute("max"));
    expect([4, 5, 6]).toContain(target);
    await expect(charge).toHaveAttribute("value", "0");
    for (let before = 0; before < target; before += 2) {
      await expect(pulse).toBeEnabled();
      await pulse.click();
      if (before + 2 < target) {
        // Only one human action was sent. The second tick must come from seat B.
        await expect(charge).toHaveAttribute("value", String(before + 2));
      }
      if (before === 0) {
        const conversation = page.getByRole("list", { name: "Text conversation" });
        await expect(conversation.getByText("One tick from the Beacon partner.", { exact: true })).toHaveCount(1);
      }
    }
    await expect(page.getByText(`Beacon activated at ${target} ticks.`)).toBeVisible();
    await expectTypedControls(page);
  } finally {
    await browser.close();
  }
}

if (process.argv.includes("--agent")) {
  await playAgent(process.argv[2]);
} else {
  await play(process.argv[2]);
}
