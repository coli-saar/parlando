# Test the Prolific integration

Parlando includes a process-boundary test suite for its Prolific integration. The suite starts a
real Parlando server with a deterministic two-player game, starts an independent Prolific emulator,
and drives both programs through their public HTTP and WebSocket interfaces. It uses a fresh
file-backed SQLite database for every run.

The suite checks Parlando's provider contract and lifecycle behavior without contacting Prolific or
creating paid submissions. It complements, but does not replace, an end-to-end Prolific test or a
small pilot. It never opens or modifies a game's workspace database: the runner gives the test
server a newly created temporary `prolific-test.sqlite` file and deletes it after a successful run
unless `--keep` is set.

## Run the scenario matrix

From the repository:

```sh
cd rust-server-tests
cargo run --bin prolific-test-runner
```

The first run builds two companion programs when they are not already present:

- `prolific-mock`, an independent implementation of the Prolific API and Secure external URL
  contract used by Parlando;
- `prolific-test-server`, a real Parlando server containing a small deterministic game.

The runner prints the complete matrix before starting. During execution, every scenario receives a
stable identifier and a `PASS`, `FAIL`, or `SKIP` result. A failure includes the operation and
response that caused it. Dependent scenarios are reported as skipped rather than disappearing from
the output.

The default matrix covers:

- game-level API origin, token, and workspace verification;
- rejection of an incorrect completion-path action;
- complete study, project, URL, timing, and completion-path activation preflight;
- valid, tampered, and identity-mismatched Secure external URL tokens;
- two verified Prolific participants forming one dyadic game through real game WebSockets;
- missing, explicitly declined, and accepted required consent as separate workflows;
- rejection of direct intake in a Prolific experiment and Prolific identity on both assigned roles;
- both an active waiting-room departure and a waiting deadline producing the same Unmatched
  handoff;
- brief and expired waiting-room disconnects;
- assigned-but-not-started dyads which either lose one participant or reach their shared deadline;
- different outcomes for an active leaver and the partner who remained;
- brief reconnect and single- or two-participant reconnect expiry;
- session inactivity with no input, heartbeats, accepted activity, and rejected activity;
- absolute session lifetime;
- an ordinary two-participant game completing with the configured completion code for both
  participants;
- the same completed submission re-entering Parlando and recovering its existing research identity
  and durable terminal result;
- an unsigned launch using the documented submission-API fallback and then continuing through the
  ordinary waiting-room workflow;
- dashboard timing, shared session causes, participant-specific outcomes, Prolific identities, and
  read-only submission reconciliation; and
- simultaneous leaves, completion racing with leave, and repeated terminal operations; and
- the absence of provider mutation, payment, or bonus calls.

The matrix is an executable specification rather than only a smoke test. Required consent is
checked at the server boundary for Prolific participants as well as in the browser. Reconnect
expiry derives all recipient outcomes from one room-wide connection snapshot, so two disconnected
participants both receive `connection_lost`; neither is mislabeled as the good-faith partner.

## Read the result

The final table reports each scenario and its elapsed time. A successful run exits with status zero
only when every scenario passed. Any failure or skip produces a nonzero exit status, which makes the
runner suitable for continuous integration.

Each run writes a timestamped artifact directory under `target/prolific-tests/` containing:

- `report.json`, the complete machine-readable scenario matrix;
- `prolific-mock.stdout.log` and `prolific-mock.stderr.log`;
- `prolific-test-server.stdout.log` and `prolific-test-server.stderr.log`.

Failed runs preserve their temporary database automatically and print its location. Use `--keep` to
preserve the database after a successful run. Use `--output PATH` to select another artifact root,
`--fail-fast` to stop after the first failed dependency boundary, or `--list` to inspect the matrix
without starting processes.

## Run the mock independently

The provider emulator is an ordinary standalone program:

```sh
cd rust-server-tests
cargo run --bin prolific-mock -- --bind 127.0.0.1:4101
```

It listens only on a loopback address. It owns independent JSON records, RSA signing material,
fault controls, and a redacted request journal. It does not import Parlando's Prolific wire types,
so an incorrect field definition is not automatically duplicated on both sides of the test.

For a manually controlled test installation, set the game-level **Prolific API base URL** to the
mock origin. Keep the default `https://api.prolific.com` for real studies. Plain HTTP is appropriate
only for a loopback mock; remote provider endpoints should use HTTPS.

## What the suite cannot establish

The emulator tests Parlando against the documented contract represented by its independent
fixtures. It cannot prove that Prolific has not changed undocumented production behavior, that a
particular workspace has access to Secure external URLs or test participants, or that the
participant-facing Prolific application processes each completion action as expected.

Before opening a paid study, use Prolific's end-to-end test-participant facility when it is available
to the workspace. A dyadic test requires two test participants. Otherwise run a small pilot and
verify the same normal-completion, unmatched, and partner-left paths in both the Parlando dashboard
and the Prolific submission table.
