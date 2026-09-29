import { constants } from "node:fs";
import { access } from "node:fs/promises";
import { spawn } from "node:child_process";
import { chromium } from "playwright";

/** Runs the Playwright installer and resolves only after it exits successfully. */
function installChromium() {
  return new Promise((resolve, reject) => {
    const installer = spawn(
      process.execPath,
      ["node_modules/playwright/cli.js", "install", "chromium"],
      { stdio: "inherit" },
    );
    installer.once("error", reject);
    installer.once("exit", (code, signal) => {
      if (code === 0) resolve();
      else reject(new Error(`Playwright browser installation failed (${signal ?? `exit ${code}`})`));
    });
  });
}

/** Reuses the exact Playwright Chromium when present and installs it only when absent. */
async function ensureChromium() {
  const executable = chromium.executablePath();
  try {
    await access(executable, constants.X_OK);
    console.log(`Using installed Playwright Chromium: ${executable}`);
  } catch {
    await installChromium();
    await access(executable, constants.X_OK);
  }
}

await ensureChromium();
