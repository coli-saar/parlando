# Beacon Relay

Two players alternate adding one or two ticks of charge. The beacon activates
when the team reaches the target exactly.

Run these commands from this directory with Python 3.10 or later and Node.js
with npm installed. The starter uses the SDK source at `python.sdk_path` in
`game.yaml`.

```sh
python3 -m venv .venv
. .venv/bin/activate
python -m pip install componentize-py==0.25.1
```

If `python.sdk_path` is empty, install the SDK in the virtual environment with
`python -m pip install /path/to/parlando-python-sdk`.

```sh
parlando check
parlando test
parlando build
parlando serve
```

Open the administration dashboard at http://127.0.0.1:8000/admin. Create an
experiment for this game, open it for intake, and copy its participant URL.
Open the URL in two separate browser profiles, enter the room on each, and take
turns adding charge.

Edit the game rules and role views in `game.py`, Python unit tests in
`tests/test_game.py`, the participant interface in `client/main.jsx`, and the
expected component behavior in `scenarios.yaml`. Build again before serving
changes.
