# Agent Instructions

## Git commands require explicit authorization
- Do not execute any `git` command unless the user specifically asks for that command or Git operation.
- General requests to inspect, edit, test, or clean up the repository do not authorize Git commands.
- Do not infer authorization from phrases such as "under version control," "tracked," "untracked," or "ready to commit." Explain any required Git operation and wait for an explicit request.

## Document technical choices
- Document technical choices in `notes/technical-decisions.md`.
- When making implementation decisions, record the context, chosen approach, tradeoffs, and any follow-up questions or risks.
- Keep technical-choice notes focused and durable enough for future contributors to understand why the decision was made.

## Separate public documentation from internal notes
- Keep `docs/` entirely user-facing and under version control. Write it as task-oriented technical documentation that follows the academic-exposition style: establish the reader's mental model, state current behavior and limitations, and avoid proposal or work-log language.
- Keep implementation plans, design explorations, audit working papers, private drafts, and technical-decision history in `notes/`. The directory is ignored and must not be committed or linked from versioned public documentation.
- A research paper or worked experiment belongs in `docs/` when it is intended to teach users or demonstrate a supported Parlando workflow. Private experiment logs and working drafts remain in `notes/`.
- When internal work becomes a supported user-facing contract, write a fresh document in `docs/` rather than publishing the internal note verbatim. Move stale design specifications out of `docs/` when they no longer describe a supported public task.

## Code quality
- Leave comments in your source code that document each function.
- For public traits/structs/functions, include detailed user-facing comments suitable for public rustdoc documentation.
- Avoid keeping legacy code around. Our aim is a clean codebase, not the preservation of old code parts. If it can be cut without compromising functionality, cut it.
- Existence of a test for a piece of code does not justify keeping that code around. Consider deleting both the test and the piece of code.
- Aim for clean generalizations over ad-hoc patches.

## Breaking database schema changes
- Do not add runtime migrations, compatibility readers, fallback paths, legacy schema branches, or export/import detours unless the user explicitly requests them.
- When a clean schema change makes a populated workspace database incompatible, treat updating that database as part of the implementation: identify the database actually used by the affected application, make a consistent backup beside it, and convert the live database in place with a one-off transactional operation.
- Preserve all information that has a clean correspondence in the new model. When old data is genuinely less expressive, derive the new value from durable records such as events where possible and document any unavoidable loss or interpretation.
- Remove superseded columns and representations after conversion. The finished application and database must expose only the new design; the backup is the recovery mechanism, not a second supported format.
- Before conversion, verify the source schema and that the database is not open by a running process. After conversion, verify the schema version, integrity, foreign keys, row counts, and validity and completeness of newly structured values.
- Record the backup path, conversion correspondence, validation results, and any historical-data limitations in `notes/technical-decisions.md`.
