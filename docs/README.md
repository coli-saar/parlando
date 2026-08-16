# Parlando Documentation

Parlando lets researchers build a complete browser-based dialogue game and run
multiple experiments with it. You design both the participant experience and the
game mechanics. Parlando supplies reusable setup, communication, administration,
privacy, and data infrastructure around them.

One process runs one compiled game version—browser client and Rust mechanics—and
hosts a database-backed catalogue of experiments for that game. Each session
records the experiment revision and game version that produced it.

Choose a path according to the task you need to perform.

The [Design Principles](design-principles.md) are authoritative for public boundaries and terminology. Existing 0.2 games should follow [Migrating to the Clean API](migrating-to-clean-api.md).

## Create a dialogue game

1. Use [Building a Game](building-games.md) to design the participant experience,
   define the game mechanics, and connect the two.
2. Use [Browser Client Protocol](client-protocol.md) when implementing or
   debugging the participant client.
3. Read [Architecture](architecture.md) for the game–experiment–session model and
   the boundaries between the runtime, game crate, browser, and store.
4. Add an in-process or remote policy with [Agents](agents.md) when the study has
   a human–agent condition.

The `space-game/server` and `space-game/client` directories form one complete
worked example. The `generate-parlando-game` skill can generate the same project
shape from a game description.

## Configure and run experiments

1. Use [Running and Deployment](running-and-deployment.md) to start the compiled
   game process, choose its port and database, establish the first administrator,
   and deploy it with Docker or Render.
2. Open `/admin/experiments` to create, clone, edit, activate, and monitor
   experiments. Normal configuration is database-backed; an older YAML file is
   only a reference while its values are transferred through the dashboard.
3. Use [Data and Monitoring](data-and-monitoring.md) to interpret lifecycle state,
   configuration revisions, session provenance, exports, and participant-data
   deletion.

For voice studies, read [Audio Transport](audio-transport.md) before deployment.
It defines the authentication, PCM format, transcription boundary, TTS path,
buffering behavior, and process-local scaling constraint. Use
[Audio Testing](audio-testing.md) to test the transport without paid-provider
credentials and under sustained or impaired workloads.

## Security model

[Security Ground Rules and Threat Model](security-ground-rules.md) is the
authoritative source for Parlando's security objectives, accepted product choices,
realistic attackers, and finding-severity rules. In particular, it records
first-visitor administrator setup as an accepted bootstrap ceremony and treats
person-level repeat participation in anonymous recruitment as a study-design
matter rather than a software vulnerability.

## Privacy and institutional review

- [Unterlage zur datenschutzrechtlichen Plattformbewertung](datenschutz-pruefvorlage.md)
  is the German basis for a reusable DPO assessment of a self-hosted deployment.
- [Umgesetzte Datenschutz-Roadmap](datenschutz-code-roadmap.md) records the
  implemented privacy functions and their technical boundaries.
- [Participant Information and Privacy Notice v1.0](participant-information-v1.0.md)
  is the English participant-facing template.
- [Consent Items v1.0](consent-items-v1.0.yaml) contains the corresponding
  machine-readable consent template.

Together, these materials provide a concrete starting point for institutional
review: they document the implemented safeguards, expose the effective settings
of a running installation, and supply reusable participant-information text. The
deploying institution completes the study-specific decisions, including
controller, legal basis, retention, provider agreements, and release approval.

## Maintainer reference

[Publishing Packages](publishing-packages.md) covers local package smoke tests and
publishing for `parlando-server` and `@coli-saar/parlando-client`.
