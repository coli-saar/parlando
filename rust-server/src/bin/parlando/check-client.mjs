// Run from the browser project after npm resolves its lockfile. Exercise the public
// SDK decoder because published and candidate SDKs can have the same npm version.
import { readFileSync } from "node:fs";

const manifest = JSON.parse(readFileSync("package.json", "utf8"));
const sdk = "@coli-saar/parlando-client";
if (manifest.dependencies?.[sdk] || manifest.devDependencies?.[sdk]) {
  const { ParticipantClient } = await import(sdk);
  const client = new ParticipantClient({ baseUrl: "http://parlando.invalid" });
  for (const mode of ["speech", "typed"]) {
    globalThis.fetch = async () => Response.json({ communication: { mode }, consents: [] });
    if ((await client.getExperiment()).communication?.mode !== mode) {
      throw new Error("Installed Parlando browser SDK cannot read communication.mode. Install the browser package from the same release or candidate as the server, update the lockfile, then rebuild the game.");
    }
  }
}
