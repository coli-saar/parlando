# Host and deploy component games

One `parlando host` process can run experiments with different games. A **package revision**
identifies the exact manifest, Wasm component and participant assets by a SHA-256 digest.
Uploading a revision makes it available for selection. An operator creates an experiment
by pinning its ID and configuration to that revision. Each experiment has its own SQLite
database and participant URL.

## Deploy a tested package

Start in a game project with the [Parlando CLI](component-games.md). The operator supplies
an HTTPS server origin and a credential scoped to the game ID. You can use the URL directly
or define a short name in `~/.config/parlando/servers.yaml` (under `$XDG_CONFIG_HOME`
when set).

```yaml
lab: https://experiments.example.org
```

On macOS and Linux, pipe the upload token from your secret manager to
`parlando login --server lab --game beacon`. The CLI stores it in macOS Keychain
or the Linux Secret Service. Linux desktop installs require `secret-tool`.
In CI, inject `PARLANDO_DEPLOY_TOKEN` as a masked secret and set
`PARLANDO_DEPLOY_ORIGIN` to the corresponding URL. An LLM running these commands
needs the same scoped credential as any other client. Keep credentials in the
execution environment or keyring and keep them out of prompts and project files.
Build only trusted game projects. Cargo build scripts, native tests, and npm
scripts run on the developer machine. In CI, inject the upload token into the
deployment step after building and testing the artifact.

```sh
parlando test
parlando build
parlando check --manifest dist/parlando/game.yaml
parlando deploy --server lab --package dist/parlando --json
```

The result includes the server, game ID, version, package revision, and component digest.
Retrying the same bytes returns the same revision. A change to a participant asset
produces a different package revision even if the component digest stays the same.
The game-scoped upload token authorizes adding packages for that game. Selecting
a package for participants requires the operator credential.

## Select a revision and open intake

Inject `PARLANDO_HOST_ADMIN_TOKEN` and set `PARLANDO_HOST_ADMIN_ORIGIN` to the host URL
in the operator's shell or CI job. Use the package revision from the deploy receipt.

```sh
parlando select --server lab --experiment pilot --game beacon --revision "$revision"
parlando intake --server lab --experiment pilot testing
```

`select` accepts an optional `--config experiment.yaml` for study settings.
The host owns the database location, participant origin and bundled client path.
Open the participant URL returned by `select` in two independent browser profiles.
Use `active` for participant collection and `inactive` to close intake. The
operator API requires the admin bearer token from the first request. Local
`parlando serve` provides the browser dashboard. Hosted experiments expose
participant pages and authenticated management routes.

To use changed rules or configuration, select the new revision or configuration
under a new experiment ID. Existing IDs keep the same pin across process restarts.
Restarting the host closes intake. Reopen the experiment explicitly when ready.
Back up the complete store together. It holds the package files, pins and databases
needed to serve and replay those experiments.

## Provision the shared host

Build from the repository root or load the versioned host image archive generated
by the **Build Parlando CLI and host image** workflow. The image listens on the
provider's `PORT` and stores durable files under `/data/parlando`. Use a single
replica and a persistent volume mounted at `/data`.

```sh
docker build -f deploy/component-host/Dockerfile -t parlando-host:local .
docker run --rm -p 127.0.0.1:8000:8000 -v parlando-data:/data \
  -e PARLANDO_PUBLIC_URL=http://127.0.0.1:8000 \
  -e PARLANDO_HOST_ADMIN_TOKEN -e PARLANDO_DEPLOY_GAME -e PARLANDO_DEPLOY_TOKEN \
  parlando-host:local
```

Set the three forwarded variables before running the example. Generate distinct random
tokens, for example with a password manager generating 32 random bytes per token.
`PARLANDO_DEPLOY_GAME` is the ID of the game the upload token can publish. For
multiple games mount a secret JSON file mapping each game ID to a distinct token and
set `PARLANDO_GRANTS_FILE` to its container path. The service account (UID 10001)
must be able to read it. Restart the host after rotating tokens. The game Wasm
component receives neither these variables nor filesystem access. Participant
JavaScript executes on the host's web origin, so select browser assets only from
publishers trusted for that origin.

On **Render**, create a Blueprint from [the host configuration](../render.yaml).
It declares a persistent disk, the health check, two generated secrets and the game ID
input. Set the game ID when creating the Blueprint. The image uses Render's public
URL automatically. [Render Blueprint reference](https://render.com/docs/blueprint-spec)

On **Railway**, deploy the repository root. Railway reads [the service configuration](../railway.json)
and builds the shared host image from the configured Dockerfile. Configure the same three variables as service variables.
From a checkout linked to that service, add its volume once, assign its public
domain, and deploy.

```sh
printf '%s' "$PARLANDO_HOST_ADMIN_TOKEN" | railway variable set PARLANDO_HOST_ADMIN_TOKEN --stdin --skip-deploys
printf '%s' "$PARLANDO_DEPLOY_TOKEN" | railway variable set PARLANDO_DEPLOY_TOKEN --stdin --skip-deploys
railway variable set "PARLANDO_DEPLOY_GAME=$PARLANDO_DEPLOY_GAME" --skip-deploys
railway volume add --mount-path /data
railway domain
railway up --ci
```

The image uses the Railway public domain. Set `PARLANDO_PUBLIC_URL` on either
provider when using a custom public HTTPS origin. Once `/health` reports
success, subsequent game uploads use only the Parlando service URL.
[Railway configuration reference](https://docs.railway.com/config-as-code/reference), [Railway variable commands](https://docs.railway.com/cli/variable) and [Railway volume commands](https://docs.railway.com/cli/volume).

## Build CLI distributions

The manual **Build Parlando CLI and host image** GitHub Actions workflow produces
versioned archives for Linux x86-64 and macOS arm64, along with SHA-256 checksums.
Each CLI archive contains the binary and its matching Rust and Python SDK sources. Extract them
together and put the directory containing `parlando` on your PATH. The host image
archive can be imported with `docker load`. Workflow artifacts let maintainers test
the distribution before an authorized release.
