---
title: What Parlando is
description: Understand how Parlando divides a two-player dialogue game between a Rust server, browser clients, game-specific code, and optional external agents.
---

# What Parlando is

Parlando is the reusable infrastructure around a two-player dialogue game. It admits participants,
records consent, assigns the two player seats, carries actions and observations, enforces session
limits, and preserves the resulting study record. It does not define the task itself. Each game adds
the rules and interface that give those general operations a particular meaning.

To build a game, an experimenter supplies two components: a Rust server crate that defines the task
and a browser application that presents it. The two components agree on the shapes of actions,
observations, and completion. Parlando supplies the live-study shell around them; game authors do
not reimplement participant entry, pairing, consent, storage, or communication infrastructure.

The system consequently crosses a deployment boundary. One Rust process runs on the server, while
one JavaScript application runs in each human participant's browser. Game-specific code exists on
both sides of that boundary: Rust code governs the task, and a React interface presents one role's
view of it.

{{< figure class="architecture-figure" src="manual/images/parlando-system-architecture.svg" alt="Parlando architecture with a Rust server, two participant browsers, game-specific server and client components, and an optional external agent" caption="Teal components belong to Parlando; gold components belong to the game. All interaction passes through the authoritative Rust server. An external agent can occupy a player seat without receiving hidden server state." >}}

## What an experimenter actually builds

A small game project has the following conceptual shape. File names may differ, but the
responsibilities do not.

```text
my-game/
├── server/                    Rust crate and executable
│   ├── Cargo.toml             depends on parlando
│   ├── src/
│   │   ├── game/              types, pure mechanics, Game and GameFactory
│   │   ├── agents.rs          optional compiled agents
│   │   └── main.rs            metadata, database, browser assets, server start
│   └── tests/                 mechanics and server-boundary tests
├── client/                    browser application
│   ├── package.json           depends on @coli-saar/parlando-client
│   └── web/src/
│       ├── game/              matching TypeScript types and role-specific views
│       ├── App.tsx            ParticipantApp and the active game screen
│       └── *.test.tsx         interface and interaction tests
└── deployment files          optional container or hosting configuration
```

The client build produces static files. The Rust executable serves those files and one compiled
game implementation. An administrator then creates one or more experiments for that game through
the dashboard; experiments are configuration and data-collection conditions, not additional code.
External Python agents are separate processes and need not live in this project.

The practical development loop is consequently short: specify the two roles and task contract,
implement and test the Rust mechanics, render each role's observation in React, compile the browser
application, start the Rust server with that build, and configure a pilot experiment. The
[game-building chapter](../build/create-game/) develops each step and explains the game-creation
skill that can generate the initial project.

## One game has a server part and a browser part

A complete Parlando game is not only a web page and not only a server program. Its **server part**
implements the task state, legal actions, role-specific observations, and completion. Its
**browser part** renders an observation and turns a participant's controls into actions. The two
parts define corresponding Rust and TypeScript types, but they have different authority.

The distinction protects private task information. The server may know facts needed to run the
whole game, but it sends each seat only that seat's observation. The browser should never receive
the other role's private information and merely hide it visually.

## The Rust server is authoritative

The server combines Parlando with the selected game's Rust module. Parlando handles participant
entry, declarations, pairing, readiness, timeouts, communication, monitoring, export, and storage.
When a player proposes an action, Parlando passes it to the game module.

The game module decides whether the action is legal and, if so, computes the next authoritative
state. It then derives a separate observation for seats `A` and `B`. Parlando records the accepted
transition and delivers each observation to the appropriate player. A browser or external agent
cannot declare that an action succeeded by changing its own local state.

## The JavaScript client runs in each human browser

The browser likewise combines two layers. The Parlando JavaScript client manages participant
startup, waiting, reconnection, messages, and optional audio. The game's React interface decides
how an observation appears and which controls produce actions.

Live speech uses a second authenticated connection. In a human–human speech session, the server
relays each microphone stream to the other browser and sends both streams to transcription. In a
human–agent speech session, it transcribes the human's stream for the agent and relays synthesized
agent replies to the browser. These supported configurations are explained in
[Enable typed or spoken dialogue](../build/voice/).

This split lets game authors design a task-specific interface without reimplementing admission,
pairing, or communication. It also means that changing presentation need not change the server rules,
provided that the interface continues to send the same actions and interpret the same observations.

## Every live session has two active seats

Parlando names the two seats `A` and `B`. A human–human session normally has two browser clients,
one for each seat. The participants do not exchange authoritative state directly: both connect to
the server, which mediates the shared task and sends each role its permitted view.

A game may give the seats different domain names, information, or actions. The generic seat names
describe Parlando's seat assignment; names such as Crown and Root describe roles within one particular
game. One compiled game can run many sessions, but each live session still has exactly two active
seats.

## An agent can occupy a player seat

An agent implements the same player boundary as a human interface. It receives one seat's initial
observation, later observations, partner messages, and shared completion; it returns typed messages
or actions. It does not receive the game's authoritative state or the other seat's private
observation.

A Rust agent may run inside the server process. An external agent—commonly a Python service—attaches
through Parlando's agent service. In the current live human–agent mode, the human occupies seat `A`
and the agent replaces the browser at seat `B`. In a headless agent–agent study, agents occupy both
seats and no participant browser is needed.

## What you supply and what Parlando supplies

| Execution boundary | Parlando supplies | The game or study supplies |
| --- | --- | --- |
| Rust server | Admission, experiments, pairing, session lifecycle, communication, persistence, monitoring, export | Task state, action validation, role observations, completion, optional compiled agents |
| Participant browser | Startup flow, connection and reconnection, synchronized observations, messages, optional audio | React presentation and task controls |
| External agent process | A supported connection to the game | Policy code, model access, policy-specific configuration and credentials |

This architecture is the basis for the rest of the manual. Next,
[install and start Great Tree](getting-started/), then learn how
[games, experiments, and sessions](concepts/) organize the study before running the first pilot.
