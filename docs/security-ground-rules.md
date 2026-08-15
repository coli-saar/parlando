# Security Ground Rules and Threat Model

Status: **Authoritative**  
Applies to: Parlando security reviews, release decisions, and deployment guidance

This document defines what Parlando is trying to protect, which risks the project
accepts, and how future security findings must be classified. When another audit,
plan, note, or checklist conflicts with this document, this document controls.
Changing these ground rules requires an intentional product decision recorded in
`notes/technical-decisions.md`, not merely a new scanner result or generic security
recommendation.

Security reviews should describe concrete limitations, likelihood, and deployment
conditions. A finding is evidence for a release decision, not an automatic
categorical launch prohibition. Blanket launch language is reserved for a
demonstrated path to consequences that are both plausible in the intended
deployment and unacceptable to the responsible researcher after available
mitigations.

## Deployment context

Parlando is self-hosted academic research software for browser-based dialogue
games. A typical installation runs one researcher's compiled game, collects
low-risk game actions and dialogue from recruited adults for a limited intake
window, and is operated by a trusted university team. It is internet-facing and
must withstand ordinary participant misuse and opportunistic probing, but it is
not a medical-record system, financial service, identity provider, hostile
multi-tenant platform, or long-lived store for high-impact personal data.

The proportional security goals are:

1. A participant cannot read or control another participant's private session.
2. Unauthenticated visitors cannot administer experiments or export research
   data after installation setup is complete.
3. Browser input, game actions, and authored study content cannot execute code on
   the server or in another user's browser.
4. Public intake cannot cheaply exhaust the host, fill durable storage, or create
   uncontrolled paid-provider spend.
5. Provider credentials, administrator credentials, and participant bearer
   credentials do not enter research exports or normal logs.
6. Ordinary operator mistakes have bounded consequences through documented TLS,
   secret, backup, update, and recovery practices.

The deployment operator, administrator, host administrator, and installed game or
agent code are trusted. In particular, agent actions, messages, and requests for
speech synthesis are authored study behavior rather than hostile public input.
Parlando therefore does not impose an application-level TTS quota on agents;
ordinary provider timeouts and bounded audio buffers still prevent failed network
operations from occupying resources indefinitely. Participants and arbitrary
internet clients are not.
Configured speech, model, hosting, and recruitment providers are processors or
infrastructure selected by the operator and must be assessed for the particular
study. Parlando does not try to defend its database from a malicious host root or
its research data from a malicious authenticated administrator.

## Accepted product choices that are not security findings

### The first visitor may create the administrator

A fresh database deliberately lets the first visitor to `/admin` create the one
administrator credential. This is the supported bootstrap ceremony. The operator
is expected to visit the installation before distributing its URL, and the
singleton insert must be atomic.

**Security reviews must not report the first-visitor claim window as a
vulnerability, weakness, or release blocker.** It is an accepted product choice,
including when the server is reachable beyond loopback. Findings remain valid if
the implementation permits a second administrator setup, overwrites an existing
credential, bypasses authentication after setup, or fails to enforce the
singleton atomically.

### Anonymous people may participate more than once

For open or anonymous recruitment, Parlando does not promise that one natural
person can contribute only once. Reliably recognizing the same person across
visits requires an identity or linkability signal. Parlando must not manufacture
that signal through browser fingerprinting, durable cross-study identifiers,
IP-address uniqueness, or similar covert tracking.

**Repeat participation by one person is therefore a recruitment-design and data
quality consideration, not a Parlando security vulnerability. Security reviews
must not report the absence of person-level deduplication as a finding.** A study
may disclose the rule to participants, inspect results statistically, and use its
recruitment platform's eligibility controls without adding identity tracking to
Parlando.

The same limitation means one person can create two anonymous participant
sessions and occupy both seats of a human–human room. This self-pairing behavior
is required for straightforward local testing and is not currently prevented.
It is primarily a study-validity limitation, not a confidentiality boundary:
Parlando does not claim that two anonymous capabilities prove two different
people. In a voice study, operating both live roles is also a relatively
conspicuous, low-utility form of manipulation rather than a realistic route to
other participants' data. A study for which distinct-human pairing is essential
must address that through recruitment or optional admission policy; no core
tracking-based fix is planned.

This does not excuse resource abuse. Automated creation of enough participants,
rooms, messages, audio, or provider calls to harm availability, corrupt the
usable dataset at scale, or incur material cost is an in-scope abuse scenario and
should be controlled with bounded inputs, quotas, rate limits, lifecycle cleanup,
and provider budgets.

Rejected game actions remain research and operational evidence. Rate limiting
must retain bounded, analyzable rejection events with stable reason codes and
size/fingerprint metadata, while avoiding storage of arbitrarily large rejected
payloads. Repeated rate-limit violations may be coalesced so the audit mechanism
cannot itself become a disk-exhaustion primitive.

### Optional privacy-preserving admission control

When a study requires one use per issued invitation, the clean mechanism is a
random, high-entropy, single-use admission capability:

1. The recruitment workflow issues one random token for each invitation.
2. Parlando imports or receives only a cryptographic hash of each valid token.
3. Participant admission atomically marks that hash as redeemed.
4. Admission records are excluded from research and corpus exports.
5. Parlando stores no worker ID, real-world identity, device fingerprint,
   IP-based identity, or cross-study identifier for this purpose.

This prevents reuse of the same invitation without teaching Parlando who the
person is. It cannot prevent one person from obtaining several legitimate
invitations or recruitment accounts; that remains the recruitment provider's
policy boundary. If even the party issuing tokens must be unable to link issuance
to redemption, an external blind-signed token issuer is the appropriate advanced
design. Neither form is required for ordinary anonymous intake, and the current
absence of this optional feature is not a security defect.

### Other explicit non-goals

- Protecting data from the trusted deployment operator, database administrator,
  host root, or deliberately installed malicious game/agent code.
- Providing strong availability against a sustained distributed denial-of-service
  attack. Infrastructure-level filtering may be added by the operator.
- Claiming that pseudonymous dialogue is automatically anonymous or safe to
  publish without content and linkage review.
- Supporting high-assurance multi-tenant isolation between mutually distrustful
  research teams in one process.
- Using invasive participant tracking as an anti-abuse mechanism.
- Treating first-visitor administrator claiming as an in-scope threat; only
  deviations from its atomic singleton and post-setup authentication boundaries
  are reviewable findings.

## Configuration authority

Study behavior and operational security policy are configured in the
database-backed administrator dashboard. This includes administrator IP-range
policy; it is not configured through environment variables. Environment values
are limited to secrets that must not enter durable configuration and bootstrap
coordinates genuinely needed before the database and dashboard can open, such as
the listener port, database location, and client artifact path. Administrator
username, password hash, role, and IP policy are database-backed rather than
environment-configured.

Administrator IP ranges are an optional, proportionate defense around the small
academic operator surface. Parlando applies dashboard-configured CIDR ranges to
the direct network peer and permits only two concurrent password verifications.
When a trusted reverse proxy hides the original client address, the equivalent
IP/VPN rule belongs at that proxy rather than in untrusted forwarding headers.

## Realistic attack scenarios

Reviews should concentrate on attacks plausible for a small, public academic
deployment:

- An opportunistic internet client probes unauthenticated endpoints, default
  credentials, path handling, exposed diagnostics, and outdated dependencies.
- A participant changes identifiers or replays credentials to enter another room,
  observe private state, impersonate a role, skip consent gates, or submit actions
  the game did not offer.
- A participant submits crafted text or protocol data that later appears in the
  dashboard, another participant's browser, logs, or exports, attempting stored
  cross-site scripting, injection, parser failure, or spreadsheet-formula abuse.
- A script creates participants, opens sockets, sends oversized or frequent
  messages/audio, or triggers speech/model providers to exhaust memory, disk,
  connections, researcher budget, or the useful intake capacity.
- An operator accidentally exposes a provider key in stored configuration, an
  export, logs, a container layer, source control, or a public backup.
- A failed write, partial migration, full disk, restart, or missing backup causes
  accepted research events to disappear or become inconsistent with live state.
- A deployment serves bearer credentials without TLS, trusts arbitrary forwarded
  headers, permits an unintended browser origin, or exposes an old unpatched
  binary.

These scenarios are materially more relevant than controls designed for national
security systems, regulated clinical records, financial custody, malicious
administrators, or determined global adversaries.

## Finding and severity rules

Every reported finding must identify a concrete attacker, reachable code or
deployment path, prerequisites, existing controls, likely impact in the deployment
context above, and a proportionate remediation. Generic checklist gaps and
accepted choices must not be promoted into vulnerabilities without that chain.

- **Critical:** an unauthenticated remote attacker can obtain administrator
  control, execute server-side code, extract the research database or runtime
  secrets, or comparably compromise the whole installation.
- **High:** a participant can cross session boundaries or expose private research
  data; stored content executes in an administrator's browser; or a cheap public
  action can plausibly cause major data loss, sustained outage, or material paid
  provider spend during intake.
- **Medium:** exploitation has meaningful but bounded confidentiality, integrity,
  availability, or cost impact under realistic conditions.
- **Low:** limited defense-in-depth weakness with narrow impact or substantial
  prerequisites.
- **Informational:** useful operational hardening or maintainability advice, not a
  release-blocking vulnerability.

A behavior explicitly accepted in this document is omitted from findings unless
the implementation violates the stated boundary. If an optional control is later
implemented or the product begins to promise stronger properties, reviews should
assess that actual promise and implementation.

Room limits are ordinary lifecycle controls, not claims of adversarial
containment. Waiting, disconnected, idle, and absolute session bounds should end
abandoned work, release voice/agent capacity, and record a durable terminal
reason. They do not need to prevent a determined participant from starting a new
anonymous session.

Capacity is reserved once, before a room becomes a research session, rather than
through unrelated WebSocket, audio, and provider semaphores. The dashboard-backed
policy bounds active sessions, waiting rooms, unattached participant credentials,
and reserved transcription streams. One live game connection and one live audio
connection per room role replace their predecessor on reconnect, so reconnecting
does not consume another reservation. TTS is intentionally outside this capacity
budget because agents are trusted.

The browser sends a game-channel heartbeat once per second. It detects a quietly
closed browser and supports prompt transport recovery, but it is in-memory
transport liveness only: heartbeats are not research events, do not write SQLite,
and do not extend the meaningful inactivity deadline. A missing heartbeat closes
the transport after 90 seconds; the room remains recoverable for five minutes.
Waiting rooms expire after ten minutes, playing rooms after thirty minutes without
meaningful participant activity, and all rooms after four hours.

The game channel has only a generous overload shaper outside game semantics: a
burst of 200 messages and 100 messages per second thereafter. Excess messages are
dropped without disconnecting the participant and are recorded through the same
bounded rejection aggregates. Valid actions have no separate quota. This ceiling
is far above plausible human play and exists so a credentialed script cannot turn
the one-second heartbeat parser into an unbounded CPU loop.

## Minimum public-intake posture

Before recruiting participants, the operator should use HTTPS, create the
administrator, use strong unique operator and provider credentials, keep secrets
out of images and source control, configure administrator CIDR ranges in the
dashboard or enforce them at a trusted proxy, configure resource and
provider-spend bounds, test export and restore procedures, retain only the data
the study needs, and run
the reviewed release artifact with current security patches. These are deployment
responsibilities proportionate to short-lived academic crowdsourcing; they do not
change the accepted product choices above.
