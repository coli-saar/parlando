---
title: The Parlando model
---

# The Parlando model

Parlando separates a stable task implementation from the study conditions under which the task is
run. Three objects express this separation: a **game**, an **experiment**, and a **session**. Most
configuration questions become straightforward once these objects are distinct.

{{< figure src="manual/images/parlando-concepts.svg" alt="One compiled game contains several configured experiments, and each experiment contains many recorded sessions" caption="One game implementation can support several study conditions. Each play-through remains attached to the exact condition that produced it." >}}

## A game defines the task

A **game** is the participant-facing browser application together with the rules that govern the
task. The browser application controls what participants see and how they act. The Rust game code
decides which actions are legal, how the task changes, what each role is allowed to observe, and
when the task is complete.

The distinction matters when a game contains private information. Parlando keeps one authoritative
task state, but each role receives its own observation. A participant should never receive the
other role's private facts and merely hide them in the interface.

A game has an identity and a semantic version. Changing rules, observations, or the meaning of
recorded actions normally requires a new game version. Changing recruitment, session limits, or
whether an agent occupies one role normally does not.

## An experiment defines a study condition

An **experiment** is a saved live data-collection configuration for one game version. It specifies the
condition under which sessions will run, including:

- participant information and consent declarations;
- direct or Prolific recruitment;
- human–human or human–agent pairing;
- waiting, reconnect, idle, and maximum-lifetime limits;
- voice, transcription, and speech synthesis;
- capacity limits; and
- the privacy contract that governs the resulting research record.

You create and edit experiments in the administrator dashboard. Every successful save creates a
numbered configuration revision. A session keeps the revision that was current when that session
was created, so later edits do not rewrite the meaning of earlier data.

Use separate experiments for conditions you want to compare. For example, one Great Tree process
might host a human–human condition and a human–agent condition. Both use the same task
rules, but they have different experiment configurations and participant links.

## A session is one play-through

A **session** is one attempt by the two assigned roles to complete the game under one experiment.
It begins with participant entry and pairing, continues through readiness and interaction, and ends
with completion, departure, timeout, or technical failure.

The session record contains role assignments, accepted actions, resulting task states, and the
structured outcome. Depending on the condition, it also contains consent evidence, typed messages,
final transcripts, and agent activity. Every session records its game version and experiment
revision, so the data remain tied to the rules and condition that produced them.

Headless agent–agent runs reuse the game and agent interfaces but do not use the live experiment
catalogue. A YAML run specification defines scenarios, seeds, and seat assignments and writes result
files rather than dashboard sessions. This keeps policy evaluation separate from participant data
collection.

## Participants occupy roles

Every active game session has roles `A` and `B`. A human participant or an agent can occupy either
role. Roles belong to the task; they do not imply that the players have the same information or the
same available actions.

Parlando assigns human participants study-specific pseudonymous labels. It does not ask them for a
name. When Prolific is used, the recruitment identity is retained separately from ordinary research
and corpus exports.

## What you configure where

Parlando presents three levels of control:

| Level | Typical contents | Change method |
| --- | --- | --- |
| Game | Interface, typed-message controls, rules, observations, actions, completion | Edit and rebuild the game |
| Installation | Public address, persistent data, participant application, administrator access, provider secrets | Start command and Game settings |
| Experiment | Consent, pairing, limits, agents, voice and speech, recruitment | Administrator dashboard |

Put a choice in game code when it changes the meaning of the task. Put it in experiment
configuration when it changes how the same task is administered. Keep credentials in the
installation's protected settings, not in game code, browser assets, or experiment text.

## The normal lifecycle

For a direct human–human study, the lifecycle is:

1. An administrator creates an inactive experiment and saves its configuration.
2. The administrator starts the experiment, which opens participant intake.
3. Two participants open the experiment-specific link, review information, and provide any required
   declarations.
4. Parlando pairs them and assigns roles `A` and `B`.
5. Both participants become ready and play from their role-specific observations.
6. The game reports a structured completion, or Parlando ends the session for another recorded
   reason.
7. The administrator monitors the session and exports the durable research record.

Stopping intake prevents new participants from entering. It does not abruptly terminate sessions
already in progress. A process restart, by contrast, cannot reconstruct live interaction and leaves
all experiments inactive until an administrator reviews and reopens them.

The next chapter applies this lifecycle to the running Great Tree installation. It creates one
experiment revision, records one session, inspects the result, and demonstrates the difference
between testing and official intake in [a local pilot](first-experiment/).
