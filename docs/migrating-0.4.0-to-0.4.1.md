# Migrating Parlando 0.4.0 to 0.4.1

Parlando 0.4.1 is a compatible patch release. It corrects Prolific external-study URL validation;
it does not change the public Rust API, TypeScript API, participant protocol, experiment
configuration, or SQLite schema.

## Update both package dependencies

Treat the runtime and browser SDK as one coordinated upgrade. Update the Rust game server to:

```toml
parlando = "0.4.1"
```

Update the participant client to:

```json
"@coli-saar/parlando-client": "^0.4.1"
```

Refresh both lockfiles and verify that Cargo and npm resolve 0.4.1.

## Keep existing code and data

No database conversion is required. Retain the normal database backup and deployment rollback
procedure, but do not run an SQLite migration for this release.

No Rust or JavaScript source migration is required. Existing `GameFactory`, `Game`, and TypeScript
participant-client integrations remain valid. Experiment configuration and participant protocol
values are unchanged.

Prolific studies should continue to use the URL displayed by the dashboard:

```text
https://games.example.edu/e/my-experiment/?PROLIFIC_PID={{%PROLIFIC_PID%}}&STUDY_ID={{%STUDY_ID%}}&SESSION_ID={{%SESSION_ID%}}
```

The 0.4.1 server now validates this real experiment entry route. A URL for another experiment or
the nonexistent `/participant` path remains invalid.

## Validate the upgrade

After updating both dependencies:

1. build and test the game server and participant client;
2. open the dashboard through the deployment's public origin;
3. confirm that the displayed Prolific setup URL names the intended experiment;
4. save the linked Prolific study configuration and confirm that provider readiness succeeds; and
5. run a two-participant Prolific test through admission, pairing, completion, and provider return.
