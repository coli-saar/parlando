# The Last Screening

Two operators are locked inside the Orpheum. From the booth, the projectionist loads glass slides and aims two lanterns. From the wings, the stagehand positions an aperture and an opaque tower. Each operator has part of the cue sheet. Completing the evening’s two reels lifts the safety curtains over both exits.

![The projection booth during Midnight Crossing](booth.jpg)

## Play together

Install Rust’s `wasm32-unknown-unknown` target, `wasm-tools`, and Node.js. Then run this command from the repository root.

```sh
games/last-screening/run.sh
```

Open [the local dashboard](http://127.0.0.1:8080/admin/experiments), create an experiment with two human players, and activate it. The game settings are `{}`. Give the participant URL to both players. On one computer, use two separate browser profiles or a regular window and a private window. Text conversation works with the default setup. Configure voice in the dashboard if desired.

The launcher builds the portable game package under `dist/last-screening-0.1.0.zip` and binds
the host to loopback. Set `HOST` to bind to a reachable address, or `PORT`
to change the port. The default database is `.local/component.sqlite` inside
the game directory.

## Play solo with the screening partner

Start the same server and create a separate experiment in the dashboard. Set **Participant pairing** to **Human vs agent**, choose **Screening partner**, save the configuration, and activate the experiment. Open its participant URL in one browser window. The launcher registers this partner with the shared host. The policy needs no model, API key, or separate process.

With the default game settings you operate the projection booth. To play backstage, set the game YAML to `projectionist_seat: b` before activation. At the start of each reel, read the partner’s mark in the intercom and reply with the number on your own cue card. For example, if your card says *Tower at 4*, send `4`. The partner then works its equipment while you operate yours. Exchange marks again when the cue sheet turns.

## Operate the theater

Drag inside the scene to look around. Click a lantern or fly rail to inspect it, or use the labeled equipment buttons. With focus in the game, **1** and **2** select the two stations and **0** returns to the room view. The booth can change slides and aim at one of seven marks. The wings can move the two pieces of scenery along those marks. The cue drawing shows the mark assigned to your room. Tell your partner what it says.

*Moonrise at Bell Rock* calls for the moon framed by the window. *Midnight Crossing* keeps the same window lit while the ship moves behind the tower and emerges on the other side. Both players must pass through their own exit to finish.

## Choose a Parlando source

| Command | Rust runtime | Browser SDK |
| --- | --- | --- |
| `./run.sh` or `./run.sh --local` | Sibling `rust-server/` | Sibling `js-client/` |
| `./run.sh --published` | crates.io `parlando 0.6.0` | npm `@coli-saar/parlando-client 0.6.0` |

Run those commands from this directory. Add `--build-only` to compile without starting the server. `PORT` changes the listener port. `PARLANDO_DATABASE_URL` changes the database location. The compiled browser assets are in `client/web/dist/`. Switching source updates Cargo’s resolved lockfile. The local build installs the sibling SDK without changing the registry manifest or lockfile.

For a single-seat visual preview, build once, then run `npm run dev` from `client/web/` and open `/?preview` on the printed Vite URL. The preview offers a choice of room and reel and lets you operate that seat’s equipment. Live two-player sessions run through the participant URL.

## Author another reel

Edit the production score in [server/src/production.rs](server/src/production.rs). Each reel has an ID, a list of conditions that must remain true throughout the reel, and a nonempty sequence of frames. Every frame names a phase and a condition on the current picture. The stage must also match the two cue marks, which the server deals separately for each reel from the session seed.

For example, the crossing holds `InWindow(Light)` throughout. Its three frames compare the ship’s position with the tower using `AtTower(Ship, Less)`, `AtTower(Ship, Equal)`, and `AtTower(Ship, Greater)`. Losing the light or moving a flat off its mark resets the sequence. The score’s unit test gives a second composition using these same pieces: hold the ship in the window and move the moon across the tower in the opposite direction.

Add the reel to `PRODUCTION` in performance order. Give its ID and phases cue copy in [client/web/src/game.ts](client/web/src/game.ts) and an illustrated cue in [client/web/src/GameView.tsx](client/web/src/GameView.tsx). Reels using the existing slides and flats share the same controls and scene geometry. Run `cargo test --manifest-path server/Cargo.toml` from this directory to check the mechanics.
