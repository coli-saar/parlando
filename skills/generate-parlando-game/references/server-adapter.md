# Component mechanics

In Rust, implement `parlando_game_sdk::Game` on the type that owns one
session. Derive `GameValue` for types exchanged with the host and export
the game with `parlando_game_sdk::export!(YourGame)`. Use the scaffold to
check the exact types and signatures for the project's SDK release.

Implement `init` from the supplied configuration and seed, and validate the
acting `Role` and action in `step`. Return a stable `Rejection` code for an
expected rule violation. The host can reuse the instance immediately after
rejection, so validate before changing the game or advancing its RNG.

Compute complete role-specific observations in `view_a` and `view_b`.
Return the shared terminal value from `completion`. Put role-private facts
in the corresponding final view. If the specification includes analysis
metadata, update or clear it on each accepted turn.

Keep accepted transitions reproducible from the configuration, seed,
and observed action sequence. Use optional paired checkpoint and restore
methods when appropriate. A checkpoint must capture all state required to
resume, including RNG state. Use the starter's `scenarios.yaml` syntax
and `parlando test` to check recovery in the compiled component.

For Python projects, use the generated typed `Game` class,
`Mechanics = mechanics(YourGame)`, and the matching Python SDK from the
starter's instructions. The CLI builds it with `componentize-py` and
validates the same component contract. Both language paths produce the
same package layout.
