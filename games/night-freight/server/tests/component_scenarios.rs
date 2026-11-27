//! Generates `scenarios.yaml` for `parlando test` from the native game.
//!
//! The component scenarios record both role views and the completion after
//! every action of two scripted sessions, so that the compiled component can be
//! compared against the native implementation, including rejected actions and
//! checkpoint recovery (`recover: true`). Because the expected views are large,
//! they are generated rather than written by hand:
//!
//! ```sh
//! GENERATE_SCENARIOS=1 cargo test --test component_scenarios
//! ```
//!
//! Without the variable the test checks that the committed file is current.

use parlando_game_sdk::{Game, Role};
use parlando_night_freight::plan::{Depart, NoteEdit, Op, Plan, Stop};
use parlando_night_freight::scenario::builtin;
use parlando_night_freight::{Action, Analysis, Config, NightFreight, Seat};
use serde_json::{json, Value};

/// Records the frames of a scripted session.
struct Recorder {
    game: NightFreight,
    steps: Vec<Value>,
}

/// Both role views and the completion, in the form `parlando test` compares.
fn frame(game: &NightFreight) -> Value {
    let mut frame = json!({
        "a": serde_json::to_value(game.view_a()).unwrap(),
        "b": serde_json::to_value(game.view_b()).unwrap(),
    });
    if let Some(done) = game.completion() {
        frame["completion"] = serde_json::to_value(done).unwrap();
    }
    frame
}

fn role_name(role: Role) -> &'static str {
    if role == Role::A {
        "A"
    } else {
        "B"
    }
}

impl Recorder {
    fn new(config: Value, seed: u64) -> (Self, Value) {
        let scenarios = config["scenarios"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s.as_str().unwrap().to_string())
            .collect();
        let game = NightFreight::init(
            Config {
                scenarios: Some(scenarios),
                freight_seat: None,
                max_runs: config["max_runs"].as_u64().map(|n| n as u8),
            },
            seed,
        )
        .unwrap();
        let initial = frame(&game);
        (
            Self {
                game,
                steps: vec![],
            },
            initial,
        )
    }

    /// Applies an action, checking the expected rejection, and records the frame.
    fn step(&mut self, actor: Role, action: Action, reject: Option<&str>, recover: bool) {
        let json_action = serde_json::to_value(&action).unwrap();
        let outcome = self.game.step(actor, action);
        match (&outcome, reject) {
            (Ok(()), None) => {}
            (Err(r), Some(code)) => assert_eq!(r.code(), code),
            (got, want) => panic!(
                "step outcome {:?}, expected rejection {want:?}",
                got.as_ref().err().map(|r| r.code())
            ),
        }
        let mut step = json!({
            "actor": role_name(actor),
            "action": json_action,
            "expect": frame(&self.game),
        });
        if let Some(code) = reject {
            step["reject"] = json!(code);
        }
        if recover {
            step["recover"] = json!(true);
        }
        self.steps.push(step);
    }

    fn ok(&mut self, actor: Role, action: Action) {
        self.step(actor, action, None, false);
    }

    fn edit(&mut self, actor: Role, edit: NoteEdit) {
        self.ok(actor, Action::Edit { edit });
    }

    /// Rewrites the fields a seat owns (Freight: stops and ops; Routing:
    /// stops and departures) so its notes cover `target`.
    fn write(&mut self, actor: Role, seat: Seat, target: &Plan) {
        let current = if actor == Role::A {
            self.game.view_a().notes
        } else {
            self.game.view_b().notes
        };
        for (engine, stops) in &target.engines {
            let have: &Vec<Stop> = &current.engines[engine];
            let common = have
                .iter()
                .zip(stops)
                .take_while(|(h, t)| h.station == t.station)
                .count();
            if have.len() > common {
                self.edit(
                    actor,
                    NoteEdit::TruncateStops {
                        engine: engine.clone(),
                        from: common,
                    },
                );
            }
            for (i, stop) in stops.iter().enumerate() {
                let existing = (i < common).then(|| &have[i]);
                if i >= common {
                    self.edit(
                        actor,
                        NoteEdit::AppendStop {
                            engine: engine.clone(),
                            station: stop.station.clone(),
                        },
                    );
                }
                let (old_depart, old_ops): (Depart, Vec<Op>) = match existing {
                    Some(s) => (s.depart.clone(), s.ops.clone()),
                    None => (Depart::Immediately, vec![]),
                };
                if seat == Seat::Routing && stop.depart != old_depart {
                    self.edit(
                        actor,
                        NoteEdit::SetDepart {
                            engine: engine.clone(),
                            stop: i,
                            depart: stop.depart.clone(),
                        },
                    );
                }
                if seat == Seat::Freight && stop.ops != old_ops {
                    for _ in 0..old_ops.len() {
                        self.edit(
                            actor,
                            NoteEdit::RemoveOp {
                                engine: engine.clone(),
                                stop: i,
                                index: 0,
                            },
                        );
                    }
                    for (j, op) in stop.ops.iter().enumerate() {
                        self.edit(
                            actor,
                            NoteEdit::InsertOp {
                                engine: engine.clone(),
                                stop: i,
                                index: j,
                                op: op.clone(),
                            },
                        );
                    }
                }
            }
        }
    }

    /// Both roles write their half of the plan and declare ready.
    fn run(&mut self, plan: &Plan) {
        self.write(Role::A, Seat::Freight, plan);
        self.write(Role::B, Seat::Routing, plan);
        self.ok(Role::A, Action::SetReady { ready: true });
        self.ok(Role::B, Action::SetReady { ready: true });
    }

    fn both(&mut self, action: Action, recover_second: bool) {
        self.ok(Role::A, action.clone());
        self.step(Role::B, action, None, recover_second);
    }
}

/// A session that exercises rejections, a failed run, a replan of the notes,
/// success, and completion, with recovery in four different phases.
fn tutorial_session() -> Value {
    let sc = builtin("tutorial").unwrap();
    let (mut r, initial) = Recorder::new(json!({"scenarios": ["tutorial"]}), 1);
    r.step(Role::B, Action::ReplayFinished, Some("wrong_phase"), false);
    r.step(Role::A, Action::Continue, Some("wrong_phase"), false);
    let append = |station: &str| NoteEdit::AppendStop {
        engine: "E1".into(),
        station: station.into(),
    };
    r.step(
        Role::B,
        Action::Edit {
            edit: append("avon"),
        },
        Some("not_adjacent"),
        false,
    );
    r.step(
        Role::A,
        Action::Edit {
            edit: NoteEdit::TruncateStops {
                engine: "E1".into(),
                from: 0,
            },
        },
        Some("fixed_stop"),
        false,
    );
    r.step(
        Role::A,
        Action::Edit {
            edit: append("nowhere"),
        },
        Some("unknown_station"),
        true,
    );
    // First attempt: couple and load at Corning, where no boxcar stands, so
    // the run fails.
    let naive = sc.naive_plan.clone();
    r.write(Role::A, Seat::Freight, &naive);
    r.write(Role::B, Seat::Routing, &naive);
    r.step(Role::A, Action::SetReady { ready: true }, None, true);
    r.step(
        Role::A,
        Action::SetReady { ready: true },
        Some("no_change"),
        false,
    );
    r.ok(Role::B, Action::SetReady { ready: true });
    r.step(
        Role::A,
        Action::SetReady { ready: false },
        Some("wrong_phase"),
        true,
    );
    r.both(Action::ReplayFinished, false);
    // Second attempt: Freight saw that the boxcar stands in Dansville's yard,
    // so both roles rewrite the trip to fetch it first.
    let reference = sc.reference_plan.clone();
    r.write(Role::A, Seat::Freight, &reference);
    r.write(Role::B, Seat::Routing, &reference);
    r.ok(Role::B, Action::SetReady { ready: true });
    r.ok(Role::A, Action::SetReady { ready: true });
    r.both(Action::ReplayFinished, true);
    r.step(Role::A, Action::Continue, None, false);
    r.step(Role::A, Action::Continue, Some("no_change"), false);
    r.step(Role::B, Action::Continue, None, true);
    json!({
        "name": "Tutorial: rejections, a failed run, a successful run and completion",
        "seed": 1,
        "config": {"scenarios": ["tutorial"]},
        "initial": initial,
        "steps": r.steps,
    })
}

/// The first seed whose disruption is the engine failure.
fn engine_failure_seed() -> u64 {
    let sc = builtin("corning-juice").unwrap();
    (0..64)
        .find(|&seed| {
            let (mut r, _) = Recorder::new(json!({"scenarios": ["corning-juice"]}), seed);
            r.run(&sc.reference_plan);
            matches!(r.game.metadata(), Some(Analysis::Run { disruption: Some(id), .. }) if id == "e2-fails")
        })
        .expect("a seed selects the engine failure")
}

/// A session with a disruption: the reference plan succeeds, is cut short by
/// the failure of E2, and the recorded recovery completes the day.
fn corning_juice_session() -> Value {
    let sc = builtin("corning-juice").unwrap();
    let seed = engine_failure_seed();
    let (mut r, initial) = Recorder::new(json!({"scenarios": ["corning-juice"]}), seed);
    r.run(&sc.reference_plan);
    r.both(Action::ReplayFinished, true);
    let recovery = sc
        .disruptions
        .iter()
        .find(|c| c.id == "e2-fails")
        .unwrap()
        .recovery
        .clone()
        .unwrap();
    r.run(&recovery);
    r.both(Action::ReplayFinished, true);
    r.both(Action::Continue, false);
    json!({
        "name": "Corning juice: interrupted by an engine failure, replanned and completed",
        "seed": seed,
        "config": {"scenarios": ["corning-juice"]},
        "initial": initial,
        "steps": r.steps,
    })
}

/// The complete YAML text of `scenarios.yaml`.
fn build() -> String {
    serde_yaml::to_string(&json!([tutorial_session(), corning_juice_session()])).unwrap()
}

/// Regenerates the file with `GENERATE_SCENARIOS=1`, otherwise checks it.
#[test]
fn component_scenarios_file_is_current() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../scenarios.yaml");
    let text = build();
    if std::env::var_os("GENERATE_SCENARIOS").is_some() {
        std::fs::write(path, &text).unwrap();
    } else {
        let have = std::fs::read_to_string(path).expect("scenarios.yaml exists");
        assert!(have == text, "scenarios.yaml is stale: run GENERATE_SCENARIOS=1 cargo test --test component_scenarios");
    }
}
