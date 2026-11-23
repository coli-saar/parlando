---
title: Deploy Parlando on Railway
---

# Deploy Parlando on Railway

One Railway service runs the shared Parlando host. A push to its connected GitHub branch replaces the server process. Game authors upload compiled game and agent ZIPs to the running host through the administrator site. A Railway volume holds the packages, accounts, and research databases across server deployments.

## Connect the server

1. Create an empty service in your Railway project, or select your existing Parlando service. Under **Settings → Source**, connect the Parlando GitHub repository, select your deployment branch (usually `main`), and enable autodeploy. Grant the Railway GitHub App access to that repository. Use the repository root as the build context. The root `railway.json` selects the shared-host Dockerfile, one replica, and the `/health` check. Leave the start command at its default. [Railway GitHub autodeploys](https://docs.railway.com/deployments/github-autodeploys)
2. Attach a volume to that service at **`/data`**. Parlando stores its files in `/data/parlando`. Expect a brief interruption while Railway moves the volume between deployments. Schedule server updates between active studies. [Railway health checks and volumes](https://docs.railway.com/deployments/healthchecks)
3. In **Settings → Networking**, choose **Generate Domain**. Railway supplies the hostname to Parlando automatically. The same HTTPS address serves games and the dashboard. [Railway public networking](https://docs.railway.com/networking/public-networking)
4. Apply the staged changes and deploy. Open `/admin` on the generated domain immediately and set your admin password before sharing the URL. On a fresh volume, the first visitor to complete setup claims the host. Check `/health` on the same domain for `{"status":"ok"}`.

Game client JavaScript on this domain runs with the same browser permissions as the dashboard. Upload games from publishers you trust with your signed-in admin session.

After your next push, check that the Railway deployment lists the pushed commit. Once healthy, sign in with the same account and confirm that your installed packages are still listed. Reopen the experiments you intend to run.

## Deploy a game and its agent

Use a local Parlando CLI matching the host and the game's [build tools](../build/packages/). For Counterweight Hotel, run the following from the repository root. Copy the generated service URL into the first line. Authorize the CLI in the browser when prompted.

```sh
server=https://your-service.up.railway.app
parlando login --server "$server"
cd games/counterweight-hotel
parlando build --manifest game.yaml
parlando agent set-game dist/counterweight-hotel-0.1.0.zip --manifest agent.yaml
parlando build --manifest agent.yaml
parlando deploy --host "$server" dist/counterweight-hotel-0.1.0.zip
parlando deploy --host "$server" dist/agent-counterweight_hotel.porter-11.0.0.zip
```

For other releases, use the ZIP paths printed by the builds. The agent is compiled against the pinned game ZIP. Upload the game first and its agent second. After replacing a game, deploy the matching agent again. Stop the game's experiments and let sessions finish before replacing either package.

On the administrator site, open **Games**, expand **Counterweight Hotel**, and verify that **Hotel porter** appears. Activate the game, open its dashboard, and create an experiment. In **Configuration**, choose **Human vs agent** and **Hotel porter**. Start a **Local preview** and open the participant link to check the pairing before opening official intake.
