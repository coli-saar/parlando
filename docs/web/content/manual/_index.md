---
title: Parlando manual
description: Build, deploy, and run dialogue-game experiments with the Parlando CLI.
---

# Parlando manual

Parlando runs two-player dialogue-game experiments. A **game** defines the task.
Its WebAssembly component applies the rules and produces each player's view. Its
browser application displays that view and sends actions. A **package** contains
both builds. A **host** runs the packages and handles participant entry, pairing,
communication, monitoring, and data collection.

An **experiment** defines a study condition for one game.
A **session** is one play-through. Archive the package revision and settings with
your results so that the behavior participants experienced remains identifiable.

- [Install Parlando and play a local game](start/getting-started/) to try the complete workflow.
- [Build a game](build/create-game/) and [check the WASM package](build/packages/) before submitting it.
- [Deploy to a shared host](deploy/) to run an experiment with real participants.
- [Configure and run a study](run/configuration/) and [export its data](run/data-operations/).
- [Operate a host](deploy/host/) if you manage the service for your group.
