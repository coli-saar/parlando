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
.venv/bin/python -m pip install parlando-python-sdk==__CLIENT_VERSION__
```

Build the package and try it locally.

```sh
parlando test
parlando build
parlando serve --manifest dist/parlando/game.yaml
```

Open [the local dashboard](http://127.0.0.1:8000/admin). Create an administrator
and an experiment, then start **Local preview**. Enter from two separate browser
profiles and play both seats.

Edit the rules in `game.py`, the participant application in `client/main.jsx`,
and the scenario in `scenarios.yaml`. Build again before serving or uploading
a changed package.

See the [experimenter manual](https://coli-saar.github.io/parlando/manual/) for
package contents, shared-host deployment, and experiment operation.
