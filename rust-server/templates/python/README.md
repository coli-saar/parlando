# Beacon Relay

Two players take turns adding one or two ticks of charge. Reach the target exactly
to activate the beacon.

Install Python 3.10 or later and Node.js with npm. Create a local Python
environment and install the component build tool.

```sh
python3 -m venv .venv
.venv/bin/python -m pip install componentize-py==0.25.1
```

If `python.sdk_path` is empty in `game.yaml`, install the matching SDK.

```sh
.venv/bin/python -m pip install parlando-python-sdk==__SDK_VERSION__
```

Build the package and try it locally.

```sh
parlando test
parlando build
parlando check --package dist/__GAME_ID__-0.1.0.zip
```

Follow the [local pilot walkthrough](https://coli-saar.github.io/parlando/manual/start/getting-started/#start-a-local-server)
to start a server, deploy `dist/__GAME_ID__-0.1.0.zip`, select its revision for an
experiment, and open testing intake. Use the game ID from your `game.yaml` and enter from
two separate browser profiles to play both seats.

Edit the rules in `game.py`, the participant application in `client/main.jsx`,
and the scenario in `scenarios.yaml`. Build and upload each changed package before selecting its revision.

See the [experimenter manual](https://coli-saar.github.io/parlando/manual/) for
package contents, shared-host deployment, and experiment operation.
