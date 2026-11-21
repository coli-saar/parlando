---
title: Monitor sessions and export results
---

# Monitor sessions and export results

Open the experiment that admitted the participants. On a shared host, choose **Manage** beside its ID and load **Sessions**. The same dashboard allows you to inspect the session record, events, and replay data. Use the local dashboard's **Sessions** tab when running with `parlando serve`.

## Interpret a session

The experiment ID identifies the condition. The package revision identifies the exact hosted game and browser builds. Each session also records its configuration revision and game version. Keep those identifiers with analysis data. When a game or condition changes, use a fresh hosted experiment ID, which preserves the earlier condition and its results.

Review the session's lifecycle and participant outcomes before counting completions. A session may end because the task completed, a participant left, time ran out, or a technical problem occurred. The session view includes testing sessions and conditions that ended before gameplay. These can help diagnose the participant procedure even when they are outside the research corpus.

## Download the research corpus

Choose JSON, YAML, or CSV in **Download corpus** on a shared host, or open **Export** on the local dashboard. The export includes research sessions that entered gameplay. It carries participant and dialogue labels for pseudonymous analysis, actions, available conversation records, and outcome information. Testing sessions and operational details stay in the dashboard.

Record the export date and preserve the original file with its experiment information. Before distributing the data, review participant-authored messages and transcripts for identifying details. A pseudonymous label does not remove identifying information someone entered in a message.

## Back up before maintenance

On a shared host, back up the whole store, which includes the packages, pins, experiment databases, and dashboard accounts and audit data. For a local pilot, preserve the SQLite store and the exact package used for the study. Stop the service for a file-level copy, or use a backup method that produces a consistent database snapshot. An export alone cannot restore an installation or reproduce its full operational record.
