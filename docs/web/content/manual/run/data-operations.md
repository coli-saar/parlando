---
title: Monitor, export, and back up data
description: Inspect live sessions, download a corpus candidate, preserve provenance, and verify a restorable backup.
---

# Monitor, export, and back up data

Once intake opens, the installation produces operational signals and research records. Operational
signals answer whether the study is working now. Research records answer what happened under a saved
condition. The dashboard exposes both, but only the research record belongs in the corpus export.

## Monitor sessions while collecting data

Open the experiment's **Sessions** tab. Each session shows its lifecycle state and assigned roles.
Select a session to inspect accepted actions, messages or final transcripts, agent events, and
completion. Use this view to determine what happened in one participant pair.

Open **Operations** to inspect the installation rather than one session. It shows active and waiting
capacity, connection health, lifecycle deadlines, throughput, speech-service pressure, and storage
growth. Use host monitoring for durable alerts.

During collection:

1. Inspect the first several sessions individually rather than waiting for participant reports.
2. Compare connection health with task activity. A connected browser does not show that the
   participant is still acting.
3. Pause intake if failures cluster, provider latency becomes unacceptable, disk headroom shrinks,
   or capacity approaches the tested limit. Pausing prevents new arrivals without terminating
   established sessions.
4. Record the time and reason for each intervention with the study operations log.

Do not analyze screenshots of the dashboard. The session view is for diagnosis; the structured
export is the research interface.

## Understand what Parlando retains

Every live session records its experiment ID, immutable configuration revision, and semantic game
version. Keep these identifiers attached during analysis: the experiment revision explains the
administrative condition, while the game version explains the rules that interpreted each action.

Parlando retains participant creation and role assignment, accepted actions, resulting task states,
structured completion, and limited diagnostic information. Depending on the condition, it also
retains declaration evidence, typed messages, final transcripts, and agent activity. Invalid input
is represented by limited diagnostic facts rather than by retaining an arbitrary submitted payload.

Raw microphone audio, synthesized audio, and the transient Operations history are not durable
research records. Readable participant pseudonyms remain stable within one experiment and across
repeated exports, but free text may still identify a participant.

## Download the corpus candidate

After a pilot or collection block:

1. Open the experiment's **Export** tab.
2. Keep **Corpus candidate** selected.
3. Choose JSON when downstream work needs complete nested game and conversation data. Choose YAML
   for manual inspection. Use CSV only when the relevant game-specific fields can be represented in
   a flat table.
4. Select **Download export**.

{{< figure src="manual/images/experiment-export.png" alt="Export tab with Corpus candidate and JSON selected beside the Download export button" caption="The corpus candidate is available in JSON, YAML, and CSV. JSON preserves the complete nested structure." >}}

The export contains only the documented research fields. It includes non-testing sessions,
experiment-scoped participant and session labels, structured outcomes, and event times relative to
game start. It omits provider credentials, declaration evidence, administrator and recruitment
data, security information, and temporary operations metrics.

Local Preview sessions are testing data and do not enter the corpus candidate. If an expected
session is absent, first check its run mode and lifecycle state rather than assuming export failure.

Record the export time, experiment ID, revision, game version, agent identity where applicable, and
every transformation performed after download. Review messages, transcripts, state, transition
metadata, and completion for identifying or unnecessary content before widening access.

## Back up and restore SQLite

The persistent SQLite file is the live copy, not a backup. Use the host's volume snapshot facility
or SQLite's online backup mechanism while the service is running. For a directly accessible
database, a concrete backup and integrity check are:

```sh
sqlite3 /path/to/parlando.sqlite ".backup '/secure-backups/parlando-2026-09-01.sqlite'"
sqlite3 /secure-backups/parlando-2026-09-01.sqlite "PRAGMA integrity_check;"
```

Run these commands as an account allowed to read the database and write the protected backup
directory. `ok` from `PRAGMA integrity_check` establishes internal consistency, but not a usable
recovery procedure.

To verify recovery, copy the backup to a separate test installation, start the same game version
against it, sign in, open the experiment and sessions, and download an export. Record the successful
restore date. Remove the temporary restored copy under the retention policy.

Back up at an interval proportionate to recruitment and before deployment, schema conversion, or
participant deletion. Encrypt off-service copies, restrict them at least as tightly as the live
administrator area, record their locations, and expire them according to the study plan.

## Apply deletion to every controlled copy

Participant deletion removes the participant and every shared session in which that participant
appeared. It therefore also removes the counterpart's contribution to those sessions. The dashboard
previews the scope and blocks deletion while a session is unfinished.

Deleting the live record does not alter older exports, backups, analyst copies, or releases. A deletion
procedure must propagate the request to every controlled copy. If a released corpus has become
genuinely anonymous and the linkage has been destroyed, locating one former participant may no
longer be possible; the participant information should already have explained that boundary.

## Retain the evidence package

Store the final corpus candidate together with the experiment privacy report, final configuration
revision, semantic game version, game and agent build identities, deployment identifier, provider
settings, backup and restore record, and corpus transformation notes. Add the controller, legal
basis, approvals, contracts, retention decision, exclusions, and release assessment that Parlando
cannot infer.

This package connects the dataset to the participant procedure, software semantics, and operating
condition that produced it. The next part of the manual deploys the same game and database model on
a public host.
