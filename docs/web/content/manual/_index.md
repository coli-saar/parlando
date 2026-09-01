---
title: Parlando manual
description: A guide to designing, conducting, and operating dialogue-game experiments with Parlando.
---

# Parlando manual

A dialogue experiment has two meanings that must remain aligned. Participants encounter a task:
they see information, communicate, act, and reach an outcome. Researchers analyze a condition:
they need to know which rules, settings, roles, and data policy produced each record. Parlando keeps
these meanings connected without collapsing them into one configuration file or one application.

{{< hero image="manual/images/great-tree-game.png" alt="The Crown participant view of The Great Tree, showing five limbs and the gates that control sunlight" primaryHref="manual/start/architecture/" primaryLabel="See how Parlando fits together" secondaryHref="manual/start/getting-started/" secondaryLabel="Run the example" >}}
Begin with the system boundary: what runs on the server, what runs in each participant's browser,
and where a game or agent fits. Then run the included game, enter once as each player, and inspect
the session that Parlando recorded.
{{< /hero >}}

## The idea in one page

A **game** defines the task: its state, legal actions, role-specific information, and completion. An
**experiment** defines a condition under which that game is run: recruitment, pairing, consent,
timing, communication, agents, and the resulting data flow. A **session** is one attempt under one immutable
revision of that condition.

Building a game means producing a Rust server crate and a compiled React participant application.
The Rust component owns authoritative mechanics; the React component renders role-specific
observations and proposes actions. Both use the same action and observation shapes, while Parlando
provides the study lifecycle around them.

This separation is the organizing principle of Parlando. Change a rule or the meaning of an action,
and you have changed the game. Change who is recruited, whether a role is automated, or how long a
participant may wait, and you have changed the experiment. Each session records both identities, so
an analysis can recover what happened and under which interpretation it happened.

Agents obey the same task boundary as humans. They receive one role's observation and propose the
same actions. A live human–agent condition belongs to the dashboard and the study record; a
headless agent–agent run belongs to a batch specification and a result directory. The distinction
is operational, not semantic: both use the same game.

## How the manual is organized

The opening chapters establish the system boundary and then lead through a complete local run. Read
[What Parlando is](start/architecture/), [install and run Great Tree](start/getting-started/),
[games, experiments, and sessions](start/concepts/), and
[run a local pilot](start/first-experiment/) in order. Together they supply the vocabulary and the
working installation used by the rest of the manual.

The second part explains how to implement a task. It begins with the Rust and React components of a
[dialogue game](build/create-game/), then adds [automated players](build/agents-and-voice/),
[headless agent studies](build/agent-agent/), and [speech](build/voice/). These chapters distinguish
the code a game author writes from the services supplied by Parlando.

The third part covers live data collection: [experiment configuration](run/configuration/),
[consent and privacy](run/consent-privacy/), [Prolific recruitment](run/prolific/), and
[data operations](run/data-operations/). Follow those chapters in that order when preparing a real
study.

The final part covers deployment and reference material. Start with the general
[deployment procedure](deploy/), then choose the Linux or Render realization appropriate to the
host. The reference chapters collect API signatures and migration details that are useful while
implementing a specific step.

## Current boundaries

Parlando 0.4.0 implements two active seats, `A` and `B`. Live studies may be human–human or
human–agent. Headless studies may place agents in both seats. One live server hosts one compiled game
and a catalogue of experiments for that game; different games use separate processes and databases.

Live studies require persistent storage and one running Parlando server per game. The participant
client supports typed communication, but each game must supply the React controls that expose it.
Human–human speech uses Speechmatics transcription. Human–agent speech transcribes the human and
synthesizes the agent's stored reply through ElevenLabs. Parlando does not store raw microphone
audio.

Every server start leaves participant intake closed until an administrator opens it. A healthy
server therefore means that the installation is available; it does not mean that a study
is recruiting.
