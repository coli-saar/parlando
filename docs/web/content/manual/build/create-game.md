---
title: Build a game in Rust or Python
---

# Build a game in Rust or Python

Start from the playable project in the [installation walkthrough](../start/getting-started/). Change the rules in one place, the participant view in another, and the expected behavior in a scenario. Run the same tests before each deployment.

| What you want to change | Starter file |
| --- | --- |
| Rules, legal actions, private state and completion | `src/lib.rs` for Rust or `game.py` for Python |
| What each participant sees and how they act | `client/main.jsx` |
| An expected sequence of moves and results | `scenarios.yaml` |
| The game's public name, version, or build inputs | `game.yaml` |

## Start with Python

Install `parlando` and Node.js with npm as in the walkthrough. Install Python 3.10 or later. On macOS or Linux, use these commands.

```sh
parlando new beacon --lang python
cd beacon
python3 -m venv .venv
.venv/bin/python -m pip install componentize-py==0.25.1 parlando-python-sdk==0.7.0
parlando test
parlando build
parlando serve --manifest dist/parlando/game.yaml
```

Use a different parent directory if you already created the Rust starter there. The Python build packages the interpreter with the game component. The shared host runs that component through the same interface as the Rust build.

## Define the task before changing the browser

Decide what each role may observe and what its actions can do. For example, if player A has a private clue, put it in A's view and keep it out of B's view and shared completion data. The browser receives its own role's observation. The server checks every proposed action in the component, including actions a participant submits outside the visible controls.

Use the session seed for any randomized task setup. An invalid action should leave the state and its random sequence unchanged. This lets Parlando reconstruct a recorded play-through from its seed and accepted actions. Use the SDK's checkpoint support if you need to save additional state for recovery.

## Change and test the game

1. Edit the starter rule, such as the target charge. Keep the browser view driven by the target reported by the game.
2. Update the deterministic scenario for that rule. Include an accepted turn, a rejected turn, both role views, and the terminal outcome.
3. Run `parlando test`. It runs native tests and checks scenario expectations through the actual WASM interface.
4. Run `parlando build` and repeat the [two-browser pilot](../start/getting-started/#play-both-seats) with the built package.

The starter connects the browser to Parlando's participant application. Keep its session and communication controls when changing the task display. Send actions through the SDK and render the observation the component returns.

See [package a game](packages/) for what the host accepts. Use the [Rust Game SDK](https://docs.rs/parlando-game-sdk/) or the [Python Game SDK](https://pypi.org/project/parlando-python-sdk/) for the language API.
