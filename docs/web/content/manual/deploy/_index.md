---
title: Design a deployment
---

# Design a deployment

A Parlando deployment is a stateful research instrument, not a stateless copy of a website. The
participant application can be rebuilt, but the database defines the installation's administrator,
experiments, revisions, participants, sessions, and evidence. The deployment design should therefore
begin with state, trust boundaries, and failure behavior rather than with a hosting product.

## Deploy one game

The Linux and container chapters implement the same sequence:

1. Build the React participant application and Rust game executable from pinned dependencies.
2. Choose a durable data location outside the replaceable application directory and restrict it to
   the service account.
3. Start one Parlando server with the production network and data settings.
4. Put one HTTPS endpoint in front of the server and verify that it supports Parlando's live
   connections.
5. Open `/admin`, create the administrator, restrict administrative access, and enter shared
   provider credentials under **Game settings**.
6. Create or restore experiments, but leave participant intake inactive.
7. From an external browser, run a two-participant Local Preview through entry, pairing, actions,
   disconnection, reconnection, and completion. Exercise each configured agent and speech provider.
8. Download an export, create an off-service backup, restore it into another installation,
   and verify its sessions and export.
9. Record the game version, source revision or image digest, public origin, database location,
   provider settings, and experiment revision. Start official intake only after this record and the
   pilot agree.

Use [the static Linux package](cross-compiling-for-linux/) when an institution manages the host.
Use [Docker on Render](render/) for the included container path. The remaining sections explain why
the steps above use one process, one database, and one secure origin.

## Choose the unit of deployment

One server serves one compiled game and the experiment catalogue for that game. A second game uses
a separate server and data store. This boundary keeps task version, experiment
configuration, and collected records within one identifiable installation.

Use one live server instance per installation. The included deployment patterns do not support
horizontal scaling, and multiple replicas can break live sessions.

Every process start returns experiments to inactive. This is a checkpoint after replacement or
failure: an administrator must verify the installation before recruitment resumes. Process health
and participant intake are intentionally different states.

## Separate bootstrap from study policy

The host supplies only values needed before the dashboard can open:

| Setting | Purpose |
| --- | --- |
| `--host` | Listener interface; production normally uses `0.0.0.0` behind an ingress |
| `--port` or `PORT` | Listener port |
| `--database-url` or `PARLANDO_DATABASE_URL` | Durable SQLite location |
| `--client-dist` or `PARLANDO_CLIENT_DIST` | Compiled participant application |

The dashboard owns experiments, consent, pairing, limits, communication, agents, and retention.
Authenticated **Game settings** own shared institutional values and provider credentials. Keeping
study policy out of container arguments makes every saved condition versioned and inspectable.

## Make persistence explicit

Place the database on a persistent writable volume outside any replaceable package directory.
Ephemeral storage may appear to work until a deployment, restart, or platform spin-down removes the
administrator, conditions, and collected data.

Persistent storage is still only the live copy. Arrange encrypted off-service backups before
recruitment, restore one into a separate installation, and verify the dashboard and an export.
Monitor free space and include backups in access, retention, and deletion procedures.

## Establish the administrative boundary

The first visit to a new database creates the administrator. Complete this ceremony before sharing
or advertising the service address. Use an institutional password manager and a password of at
least 12 characters.

Restrict administrator network access through dashboard CIDR settings or a trusted ingress. This is
defense in depth; the dashboard still requires its own session authentication and request
protections. Store provider secrets only through authenticated settings or the protected remote
process that uses them.

## Put one secure origin in front

Serve the participant application and Parlando from one HTTPS origin. If you use a reverse proxy,
configure it to support Parlando's long-lived live-session connections. Test entry, play,
reconnection, and speech through the public address; a page that merely loads is not sufficient.

## Select a packaging route

The packaging choice changes how the same deployment unit reaches the host:

| Route | Choose it when | It does not provide |
| --- | --- | --- |
| Local run | Developing and using Local Preview | Public ingress, service management, durable hosting |
| Static Linux package | An institution manages the Linux host | Reverse proxy, service manager, database, credentials |
| Docker service | A container platform manages builds and processes | Automatic persistent storage, backup, or multi-replica state |

The [Linux package](cross-compiling-for-linux/) combines a static server with compiled participant
assets. The [Render guide](render/) shows a Docker realization. They share the operating model
above.

## Verify the deployed instrument

Before intake, test the public route rather than only the local binary:

1. `/health` reports success with the intended persistent data location;
2. administrator setup is complete and access-controlled;
3. two isolated participants can enter, pair, act, reconnect, and complete;
4. each remote agent, speech provider, and recruitment path works from its real network boundary;
5. the corpus candidate downloads and contains the intended fields; and
6. an off-service backup restores into another installation.

Record the semantic game version, deployed source revision or image digest, database location,
provider configuration, and final experiment revision with the study. Monitor the first production
sessions before increasing recruitment.

Continue with [Cross-compile for Linux](cross-compiling-for-linux/) when deploying to an
institution-managed host, or [Deploy on Render](render/) for the included Docker realization.
