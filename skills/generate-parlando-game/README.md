# Generate Parlando Game Skill

This skill helps Codex or Claude Code generate complete Parlando dialogue games: Rust server adapter, browser client, configs, build files, tests, and run/deploy instructions.

## Install For Codex

Copy or symlink this directory into your Codex skills directory. If `CODEX_HOME` is set, use `$CODEX_HOME/skills`; otherwise `~/.codex/skills` is the usual location:

```sh
mkdir -p ~/.codex/skills
ln -s /path/to/parlando/skills/generate-parlando-game ~/.codex/skills/generate-parlando-game
```

Restart Codex or open a new thread. Then ask for a Parlando game, for example:

```text
Use the generate-parlando-game skill to create a two-player reference game where role A sees target cards and role B sees candidate cards.
```

## Install For Claude Code

Copy or symlink this directory into the local skills directory that your Claude Code install is configured to scan. A common setup is:

```sh
mkdir -p ~/.claude/skills
ln -s /path/to/parlando/skills/generate-parlando-game ~/.claude/skills/generate-parlando-game
```

Restart Claude Code so it reloads skills. Then invoke it by name:

```text
Use the generate-parlando-game skill to scaffold a Parlando negotiation game with human-vs-agent support.
```

## What It Assumes

- Parlando's Rust server support is installed with Cargo.
- The generated game server is installed locally with Cargo, for example into `.local/bin`.
- `js-client` has been published into the local yalc store as `@parlando/client`.
- Local Parlando docs may be present in `docs/`; otherwise the skill can reference the GitHub docs at https://github.com/coli-saar/parlando.

The generated game should include local run instructions and deployment notes for the target you request.
