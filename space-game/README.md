# Parlando Space Game Client

Demo browser client for the Parlando Space Game. This project is a consumer of the reusable `@parlando/client` SDK; it should not import SDK source by relative path or rely on a local checkout of the Rust server.

## Project role

The Parlando JavaScript split has two parts:

- `../js-client` is the reusable SDK package published as `@parlando/client`.
- `../space-game` is this demo game client, with Space Game-specific UI, assets, state interpretation, controls, and tests.

The dependency direction is one-way: this app depends on the SDK. The SDK must not depend on this app.

## Normal install

Once `@parlando/client` is published to GitHub Packages, install this app like a normal client project:

```bash
npm install
npm run build
```

Developers need npm/GitHub Packages auth configured for the package scope. A typical `.npmrc` entry is:

```ini
@parlando:registry=https://npm.pkg.github.com
//npm.pkg.github.com/:_authToken=${GITHUB_TOKEN}
```

Adjust the scope if the package is published under a different GitHub organization.

## Testing unpublished SDK changes

Use `yalc` for local SDK debugging instead of changing this app to `file:../js-client`.

Build and publish the SDK locally:

```bash
cd ../js-client
npm run yalc
```

Attach it to this app:

```bash
cd ../space-game
yalc add @parlando/client
npm install
npm run build
```

After SDK edits, push the new local package from `../js-client`:

```bash
npm run build
yalc push
```

To return to the published dependency:

```bash
yalc remove @parlando/client
npm install
```

## Using SDK widgets

This app can use SDK widgets for Parlando platform state, such as microphone readiness and transcription progress:

```tsx
import { VoiceStatusChip, TranscriptionStatusChip } from "@parlando/client/react";
```

Game-specific UI should stay here. For example, the station map, inventory/action controls, game status panels, and Space Game copy belong in this project, not in the SDK.

## Runtime expectation

The client expects a Parlando-compatible server that exposes the documented `/api/*` and `/ws/game/*` routes. In production-like deployments, the Rust server can serve this app's built `dist` directory through `server.client_dist_path`.
