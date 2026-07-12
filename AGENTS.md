# Agent Instructions

## Document technical choices
- Document technical choices in `notes/technical-decisions.md`.
- When making implementation decisions, record the context, chosen approach, tradeoffs, and any follow-up questions or risks.
- Keep technical-choice notes focused and durable enough for future contributors to understand why the decision was made.

## Code quality
- Leave comments in your source code that document each function.
- For public traits/structs/functions, include detailed user-facing comments suitable for public rustdoc documentation.
- Avoid keeping legacy code around. Our aim is a clean codebase, not the preservation of old code parts. If it can be cut without compromising functionality, cut it.
- Existence of a test for a piece of code does not justify keeping that code around. Consider deleting both the test and the piece of code.
- Aim for clean generalizations over ad-hoc patches.
