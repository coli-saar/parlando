---
title: Install and run the example
---

# Install and run the example

The quickest way to understand Parlando is to run The Great Tree. It is a cooperative two-player
game with private role information, typed actions, optional voice, example agents, an administrator
dashboard, and structured export. Crown controls sunlight in the branches, while Root controls
water below ground. The players must discover how their controls interact and make three flowers
bloom.

Parlando is a library rather than a standalone server that can run without a game. A real deployment
always combines the Parlando packages with a particular game's Rust server and browser application.
The repository contains that complete combination for The Great Tree.

## Install the development tools

You need:

- a current stable Rust toolchain, including Cargo;
- Node.js 20.19 or newer, or Node.js 22.12 or newer;
- npm; and
- GNU Make.

On macOS or Linux, verify the tools with:

```sh
rustc --version
cargo --version
node --version
npm --version
make --version
```

Clone the repository and enter it:

```sh
git clone https://github.com/coli-saar/parlando.git
cd parlando
```

## Start The Great Tree

Run The Great Tree against the published Parlando 0.4.1 packages:

```sh
games/great-tree/run.sh
```

The first run installs JavaScript dependencies and compiles the browser application and Rust
server. Compilation may take several minutes. Leave the process running while you use the game.

If you are developing Parlando itself, use `games/great-tree/run.sh --local` instead. That command
temporarily uses the sibling `rust-server` and `js-client` checkouts without changing Great Tree's
published dependency declarations. Most game authors should omit `--local`.

## Create the administrator

Open [http://127.0.0.1:8080/admin](http://127.0.0.1:8080/admin). A new database has no
administrator, so the first visit presents an account-creation form. Choose the administrator name
and a password of at least 12 characters.

Complete this step before exposing any new installation to the public internet. First-visitor setup
remains open until the administrator account exists. The account survives ordinary restarts.

{{< figure src="manual/images/great-tree-dashboard.png" alt="The Great Tree administrator dashboard, showing an inactive pilot and its sessions, export, configuration, and privacy tabs" caption="The dashboard belongs to one compiled game. It lists the experiments and sessions stored for that game." >}}

## Stop and restart safely

Stop the local server with `Ctrl-C`. When you start it again, the administrator account,
experiments, configuration revisions, and completed sessions remain available. Participant intake
does not reopen automatically: every experiment returns to the inactive state after a server start.

This distinction is useful in production. A successful health check means that the installation is
available; it does not mean that recruitment is open.

The installation is now ready, but it has no experiment yet. Continue with
[games, experiments, and sessions](concepts/) to understand the objects the dashboard will create;
the following chapter then uses them to run the first two-browser pilot.
