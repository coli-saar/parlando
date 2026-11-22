---
title: Monitor sessions and export results
---

# Monitor sessions and export results

Open the game ID from **Games**. Select the experiment from its catalogue and choose a session in **Sessions** to inspect its record, events, and replay.

## Interpret a session

The experiment ID identifies the condition. The package revision identifies the exact hosted game and browser builds. Each session also records its configuration revision and game version. Keep those identifiers with the analysis data and archive the original ZIP if you need to reproduce the results after a deployment update.

Review the session's lifecycle and participant outcomes before counting completions. A session may end because the task completed, a participant left, time ran out, or a technical problem occurred. The session view includes testing sessions and conditions that ended before gameplay. These can help diagnose the participant procedure even when they are outside the research corpus.

## Download the research corpus

Open **Export**, choose JSON, YAML, or CSV, and select **Download export**. The export includes research sessions that entered gameplay. It carries participant and dialogue labels for pseudonymous analysis, actions, available conversation records, and outcome information. Testing sessions and operational details stay in the dashboard.

Record the export date and preserve the original file with its experiment information. Before distributing the data, review participant-authored messages and transcripts for identifying details. A pseudonymous label does not remove identifying information someone entered in a message.

## Back up before maintenance

Back up the whole store, which includes current game packages, research databases, and dashboard accounts and audit data. Stop the service for a file-level copy, or use a backup method that produces a consistent database snapshot. An export alone cannot restore an installation or reproduce its full operational record.
