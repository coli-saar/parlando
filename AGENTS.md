# Agent Instructions

## Releases
- Follow [`docs/releasing-parlando.md`](docs/releasing-parlando.md) for every Parlando release.
- Treat the Rust `parlando` crate and `@coli-saar/parlando-client` as one lockstep release. Unless the user requests another version, increment the current version by `0.0.1`.
- Do not publish either package unless the user explicitly requests actual publication.

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

<!-- graft:start -->
## Graft — repo context graph

This repo is indexed in `graft/`: small linked markdown nodes that explain each
system and carry exact file:line spans, kept in sync with the code through git.

For ANY task here — understanding how something works, finding where code lives,
or scoping a change — get context from the graph before grepping or opening
source files. Re-ask freely (it's cheap) and reuse literal identifiers you
already have (symbol, error string, file name) as the query. New to this repo?
Run `graft map` first — a token-budgeted orientation (dir clusters, hubs,
hotspots), no LLM, no key.

- Run `graft ask "<your question>" --source` → ranked nodes with the relevant
  code spans inlined (each hit's ≤8-line crux by default; `--full` for whole
  definitions when the crux isn't enough). Match the tool to the task shape:
  for understanding or editing, the top node IS the answer — cite its
  `covers:` file:line spans and edit straight from `--source`. For
  exhaustive tasks ("every occurrence / every caller of this pattern"), ranked
  results are top-N, not complete — run `graft grep "<literal>"` instead
  (exhaustive over indexed files, grouped by enclosing symbol), falling back
  to raw `grep -rn` only for unindexed files.
- `graft skeleton <file>` → every definition's signature + span, ~10× cheaper
  than reading the file; use it to skim an API surface.
- `graft callers <symbol>` gives precomputed, exact edges — who calls this.
  Add `--direction out` for what it calls, or `--depth N` to walk
  transitively for the full blast radius. For structural questions, skip
  ranking and use this directly.
- Or browse: `graft/INDEX.md` lists every node; follow the links.
- Monorepos and folders of multiple repos rank fairly across sub-projects —
  hits carry `[scope/]` labels naming which one they're from. Narrow with
  `graft ask "<task>" --in <scope>/` once you know where you're working.

If a returned span is truncated ("+N more lines"), open the file at that exact
range before finalizing. Only open source files when a node genuinely lacks a
needed detail, and then at the exact file:line the node points to — never
re-read whole files.

After big code changes, refresh the graph with `graft build` (deterministic,
no API key, $0).
<!-- graft:end -->
