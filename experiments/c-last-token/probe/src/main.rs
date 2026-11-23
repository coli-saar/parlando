//! Contract checks and in-process timings through the Parlando component host.
use parlando::component::{ComponentValue, GameFrame, GameSession, LoadedGame};
use parlando::PlayerRole;
use serde_json::{json, Value};
use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use PlayerRole::{A, B};

#[cfg(target_os = "macos")]
mod stress;

/// Encode an ordinary named value as an input to Parlando's schema encoder.
fn payload(value: Value) -> ComponentValue {
    ComponentValue::from_typed(&value).unwrap()
}

/// Apply a move that the caller expects to be accepted.
fn take(
    game: &GameSession,
    state: &GameFrame,
    actor: PlayerRole,
    action: &ComponentValue,
) -> GameFrame {
    game.step(state, action, actor).unwrap().unwrap()
}

/// Assert that both views are correct and that only the active seat can move.
fn views(state: &GameFrame, remaining: u8, turn: Option<PlayerRole>) {
    for role in [A, B] {
        assert_eq!(
            serde_json::to_value(state.observation(role)).unwrap(),
            json!({"remaining": remaining, "your_turn": turn == Some(role)}),
        );
    }
}

/// Assert the stable rule code and require the guest call itself to succeed.
fn rejected(
    game: &GameSession,
    state: &GameFrame,
    actor: PlayerRole,
    action: &ComponentValue,
    code: &str,
) {
    match game.step(state, action, actor).unwrap() {
        Err(rejection) => assert_eq!(rejection.code, code),
        Ok(_) => panic!("expected rejection {code}"),
    }
}

/// Verify seed, isolation, input handling, completion, and replay via Parlando.
fn check(factory: &LoadedGame, config: &ComponentValue, actions: &[ComponentValue; 3]) {
    let [one, two, three] = actions;
    for seed in [0, 1, 2, 3, u64::MAX] {
        let game = factory.session();
        let state = game.init(config, seed).unwrap();
        views(&state, 12 + (seed % 4) as u8, Some(A));
    }

    let game = factory.session();
    let other = factory.session();
    let initial = game.init(config, 0).unwrap();
    let independent = other.init(config, 3).unwrap();

    rejected(&game, &initial, B, one, "wrong_turn");
    rejected(
        &game,
        &initial,
        A,
        &payload(json!({"type": "take_four"})),
        "invalid_component_action",
    );
    let after_a = take(&game, &initial, A, three);
    views(&after_a, 9, Some(B));
    assert!(after_a.completion().is_none());
    assert!(after_a.turn_checkpoint().unwrap().is_none());

    let after_b = take(&game, &after_a, B, three);
    let after_a_again = take(&game, &after_b, A, three);
    let one_left = take(&game, &after_a_again, B, two);
    rejected(&game, &one_left, A, two, "too_many_tokens");
    rejected(&game, &one_left, A, three, "too_many_tokens");
    let terminal = take(&game, &one_left, A, one);
    views(&terminal, 0, None);
    assert_eq!(
        serde_json::to_value(terminal.completion().unwrap()).unwrap(),
        json!({"winner": "A"})
    );
    rejected(&game, &terminal, B, one, "game_over");

    // after_a is historical now. Its new branch must start with nine tokens and B.
    let fork = take(&game, &after_a, B, one);
    views(&fork, 8, Some(A));
    let fork = take(&game, &fork, A, three);
    let fork = take(&game, &fork, B, three);
    let fork = take(&game, &fork, A, one);
    let fork = take(&game, &fork, B, one);
    views(&fork, 0, None);
    assert_eq!(
        serde_json::to_value(fork.completion().unwrap()).unwrap(),
        json!({"winner": "B"})
    );
    views(&initial, 12, Some(A));
    views(&independent, 15, Some(A));
    let independent_next = take(&other, &independent, A, two);
    views(&independent_next, 13, Some(B));
    println!(
        "Passed seed, views, moves, rejections, both winners, isolation, and historical replay"
    );
}

/// Report the median and full range of batch means in the requested unit.
fn summarize(label: &str, unit: &str, mut means: Vec<f64>) {
    means.sort_by(f64::total_cmp);
    println!(
        "{label}: median {:.3} {unit}, range {:.3}–{:.3} {unit}",
        means[means.len() / 2],
        means[0],
        means[means.len() - 1]
    );
}

/// Measure in-process factory load, fresh initialization and cached accepted moves.
fn benchmark(path: &Path, factory: &LoadedGame, config: &ComponentValue, one: &ComponentValue) {
    const SAMPLES: usize = 21;
    const GAMES: u32 = 1000;
    const MOVES: usize = 12;

    let loads = (0..SAMPLES)
        .map(|_| {
            let before = Instant::now();
            let loaded = LoadedGame::load(path).unwrap();
            let ms = before.elapsed().as_secs_f64() * 1e3;
            black_box(loaded);
            ms
        })
        .collect();

    let mut init_means = Vec::new();
    let mut move_means = Vec::new();
    for batch in 0..=SAMPLES {
        let mut init = Duration::ZERO;
        let mut moves = Duration::ZERO;
        for _ in 0..GAMES {
            let before = Instant::now();
            let game = factory.session();
            let mut state = game.init(config, 0).unwrap();
            init += before.elapsed();
            let before = Instant::now();
            for actor in [A, B].into_iter().cycle().take(MOVES) {
                state = take(&game, &state, actor, black_box(one));
            }
            moves += before.elapsed();
            assert!(black_box(&state).completion().is_some());
        }
        if batch != 0 {
            init_means.push(init.as_secs_f64() * 1e6 / f64::from(GAMES));
            move_means.push(moves.as_secs_f64() * 1e6 / (f64::from(GAMES) * MOVES as f64));
        }
    }
    summarize("Factory load", "ms/load", loads);
    summarize("New session + initialization", "µs/session", init_means);
    summarize("Cached accepted turn", "µs/turn", move_means);
}

/// Load the component, check behavior, and optionally collect timings.
fn main() {
    let mut args = std::env::args_os().skip(1);
    let mode = args.next().unwrap_or_else(|| "check".into());
    assert!(
        matches!(mode.to_str(), Some("check" | "bench" | "stress")),
        "usage: last-token-probe [check|bench|stress] [component.wasm]"
    );
    let path = args.next().map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../build/last-token.wasm")
    });
    assert!(args.next().is_none(), "unexpected argument");
    let factory = LoadedGame::load(&path).unwrap();
    let config = payload(json!({}));
    let actions = ["take_one", "take_two", "take_three"].map(|kind| payload(json!({"type": kind})));
    check(&factory, &config, &actions);
    match mode.to_str() {
        Some("bench") => benchmark(&path, &factory, &config, &actions[0]),
        #[cfg(target_os = "macos")]
        Some("stress") => stress::run(&factory, &config, &actions[0]).unwrap(),
        #[cfg(not(target_os = "macos"))]
        Some("stress") => panic!("stress mode requires macOS memory accounting"),
        _ => {}
    }
}
