import { chromium, expect } from "@playwright/test";

/** Signs in to the loopback host and starts one experiment in its game dashboard. */
async function startPreview(browser, adminOrigin, game) {
  const page = await browser.newPage();
  const username = "release-rehearsal";
  const password = process.env.PARLANDO_REHEARSAL_PASSWORD;
  if (!password) throw new Error("Missing release rehearsal account password");
  await page.goto(`${adminOrigin}/admin`, { waitUntil: "domcontentloaded" });
  const setup = page.getByRole("heading", { name: "Create your administrator account" });
  const login = page.getByRole("heading", { name: "Sign in", exact: true });
  await expect(setup.or(login)).toBeVisible();
  if (await setup.isVisible()) {
    const form = page.locator("#setup-form");
    await form.getByLabel("Username").fill(username);
    await form.getByLabel("Password", { exact: true }).fill(password);
    await form.getByLabel("Confirm password").fill(password);
    await form.getByRole("button", { name: "Create administrator" }).click();
  } else {
    const form = page.locator("#login");
    await form.getByLabel("Username").fill(username);
    await form.getByLabel("Password").fill(password);
    await form.getByRole("button", { name: "Sign in" }).click();
  }
  const row = page.getByRole("row").filter({ has: page.getByRole("link", { name: game, exact: true }) });
  await expect(row.getByRole("cell", { name: "Active", exact: true })).toBeVisible();
  await row.getByRole("link", { name: game, exact: true }).click();
  await expect(page).toHaveURL(`${adminOrigin}/${game}`);
  await page.getByRole("button", { name: "New experiment" }).click();
  const dialog = page.getByRole("dialog");
  await dialog.getByRole("textbox", { name: /^Experiment ID/ }).fill(game);
  await dialog.getByRole("button", { name: "Create experiment" }).click();
  await expect(dialog).toBeHidden();
  await page.getByText("Start experiment", { exact: true }).click();
  await page.locator(".launch-mode").filter({ hasText: "Local preview" })
    .getByRole("button", { name: "Start", exact: true }).click();
  await expect(page.getByRole("button", { name: "Stop", exact: true })).toBeVisible();
  return page;
}

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
async function play(adminOrigin, publicOrigin, game) {
  const browser = await chromium.launch();
  try {
    const admin = await startPreview(browser, adminOrigin, game);
    const url = `${publicOrigin}/e/${game}~${game}/`;
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

    await Promise.all(pages.map(page => page.context().close()));
    await admin.getByRole("button", { name: "Stop", exact: true }).click();
    await expect(admin.getByText("Start experiment", { exact: true })).toBeVisible();
  } finally {
    await browser.close();
  }
}

await play(...process.argv.slice(2));
