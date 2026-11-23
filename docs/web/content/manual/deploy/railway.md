---
title: Deploy Parlando on Railway
---

# Deploy Parlando on Railway

One Railway service runs the shared Parlando host. A push to its connected GitHub branch replaces the server process. Game authors upload compiled game and agent ZIPs to the running host through the administrator site. A Railway volume holds the packages, accounts, and research databases across server deployments.

## Connect the server

1. Create an empty service in your Railway project, or select your existing Parlando service. Under **Settings → Source**, connect the Parlando GitHub repository, select your deployment branch (usually `main`), and enable autodeploy. Grant the Railway GitHub App access to that repository. Use the repository root as the build context. The root `railway.json` selects the shared-host Dockerfile, one replica, and the `/health` check. Leave the start command at its default. [Railway GitHub autodeploys](https://docs.railway.com/deployments/github-autodeploys)
2. Attach a volume to that service at **`/data`**. Parlando stores its files in `/data/parlando`. Expect a brief interruption while Railway moves the volume between deployments. Schedule server updates between active studies. [Railway health checks and volumes](https://docs.railway.com/deployments/healthchecks)
3. In **Settings → Networking**, generate a public domain for participants. Add a different hostname for the administrator dashboard to the **same service**, for example `admin.example.org`. Add the DNS records Railway displays and wait for HTTPS on both names. Route both domains to port `8000` if selecting a target port explicitly, and set the service variable `PORT=8000` in that case. [Railway public networking](https://docs.railway.com/networking/public-networking)
4. In the service's **Variables**, set `PARLANDO_ADMIN_URL` to the full administrator origin, such as `https://admin.example.org`, without a path. The host uses the generated Railway domain for participant links by default. Set `PARLANDO_PUBLIC_URL` explicitly when using a custom participant hostname.
5. Apply the staged changes and deploy. Open `/health` on the participant domain and expect `{"status":"ok"}`. Immediately open `/admin` on the administrator hostname and create the first admin account. Anyone who finds that hostname before you can claim the empty store. Share the participant URL after setting your password. Subsequent deployments use your existing account.

After your next push, check that the Railway deployment lists the pushed commit. Once healthy, sign in with the same account and confirm that your installed packages are still listed. Reopen the experiments you intend to run.

## Deploy a game and its agent

Use a local Parlando CLI matching the host and the game's [build tools](../build/packages/). For Counterweight Hotel, run the following from the repository root. Replace the example domain with your **administrator** origin. Authorize the CLI in the browser when prompted.

```sh
export PARLANDO_ADMIN_URL=https://admin.example.org
parlando login --server "$PARLANDO_ADMIN_URL"
cd games/counterweight-hotel
parlando build --manifest game.yaml
parlando agent set-game dist/counterweight-hotel-0.1.0.zip --manifest agent.yaml
parlando build --manifest agent.yaml
parlando deploy --host "$PARLANDO_ADMIN_URL" dist/counterweight-hotel-0.1.0.zip
parlando deploy --host "$PARLANDO_ADMIN_URL" dist/agent-counterweight_hotel.porter-11.0.0.zip
```

For other releases, use the ZIP paths printed by the builds. The agent is compiled against the pinned game ZIP. Upload the game first and its agent second. After replacing a game, deploy the matching agent again. Stop the game's experiments and let sessions finish before replacing either package.

On the administrator site, open **Games**, expand **Counterweight Hotel**, and verify that **Hotel porter** appears. Activate the game, open its dashboard, and create an experiment. In **Configuration**, choose **Human vs agent** and **Hotel porter**. Start a **Local preview** and open the participant link to check the pairing before opening official intake.
