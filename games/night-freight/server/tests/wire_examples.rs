//! Generates the example JSON documents quoted in `WIRE-FORMAT.md`.
//!
//! The examples are produced by driving the real game, so they cannot drift
//! from the implementation: without `GENERATE_EXAMPLES=1` this test fails if
//! the files in `server/examples/` differ from freshly generated output.
//!
//! ```sh
//! GENERATE_EXAMPLES=1 cargo test --test wire_examples
//! ```

use parlando_game_sdk::{Game, Role};
use parlando_night_freight::plan::{Depart, LoadFilter, NoteEdit, Op};
use parlando_night_freight::scenario::builtin;
use parlando_night_freight::world::{CarKind, Commodity};
use parlando_night_freight::{Action, Analysis, Config, NightFreight};
use serde_json::{json, Value};

/// Applies an action that must be accepted.
fn act(game: &mut NightFreight, who: Role, action: Action) {
    game.step(who, action)
        .unwrap_or_else(|r| panic!("rejected: {}", r.code()));
}

fn edit(game: &mut NightFreight, who: Role, edit: NoteEdit) {
    act(game, who, Action::Edit { edit });
}

fn new_game(scenarios: &[&str], seed: u64) -> NightFreight {
    NightFreight::init(
        Config {
            scenarios: Some(scenarios.iter().map(|s| s.to_string()).collect()),
            freight_seat: None,
            max_runs: None,
        },
        seed,
    )
    .unwrap()
}

fn views(game: &NightFreight) -> [Value; 2] {
    [
        serde_json::to_value(game.view_a()).unwrap(),
        serde_json::to_value(game.view_b()).unwrap(),
    ]
}

/// All example documents by file name.
fn documents() -> Vec<(&'static str, Value)> {
    let mut docs: Vec<(&'static str, Value)> = vec![];

    // Tutorial, planning, with a few notes written.
    let mut g = new_game(&["tutorial"], 1);
    let stop = |engine: &str| NoteEdit::AppendStop {
        engine: engine.into(),
        station: "dansville".into(),
    };
    edit(&mut g, Role::A, stop("E1"));
    edit(
        &mut g,
        Role::A,
        NoteEdit::InsertOp {
            engine: "E1".into(),
            stop: 0,
            index: 0,
            op: Op::Couple {
                kind: CarKind::Boxcar,
                load: LoadFilter::Any,
                count: 1,
            },
        },
    );
    edit(
        &mut g,
        Role::A,
        NoteEdit::InsertOp {
            engine: "E1".into(),
            stop: 0,
            index: 1,
            op: Op::Load {
                commodity: Commodity::Oranges,
                count: 1,
            },
        },
    );
    edit(
        &mut g,
        Role::A,
        NoteEdit::InsertOp {
            engine: "E1".into(),
            stop: 1,
            index: 0,
            op: Op::Deliver { order: "O1".into() },
        },
    );
    edit(&mut g, Role::B, stop("E1"));
    edit(
        &mut g,
        Role::B,
        NoteEdit::SetDepart {
            engine: "E1".into(),
            stop: 0,
            depart: Depart::At { hour: 1 },
        },
    );
    edit(
        &mut g,
        Role::B,
        NoteEdit::SetHeardArrival {
            engine: "E1".into(),
            stop: 1,
            hour: Some(2),
        },
    );
    act(&mut g, Role::A, Action::SetReady { ready: true });
    let [freight, routing] = views(&g);
    docs.push(("view-freight-planning.json", freight));
    docs.push(("view-routing-planning.json", routing));
    let analysis = |g: &NightFreight| serde_json::to_value(g.metadata().unwrap()).unwrap();

    // A failed run: Routing's departure was changed back to leave at once.
    edit(
        &mut g,
        Role::B,
        NoteEdit::SetDepart {
            engine: "E1".into(),
            stop: 0,
            depart: Depart::Immediately,
        },
    );
    docs.push(("analysis-edit.json", analysis(&g)));
    act(&mut g, Role::B, Action::SetReady { ready: true });
    docs.push(("analysis-run.json", analysis(&g)));
    let [freight, routing] = views(&g);
    docs.push(("view-freight-after-run.json", freight));
    docs.push(("view-routing-after-run.json", routing));

    // The completion of a finished session.
    let sc = builtin("tutorial").unwrap();
    let mut g = new_game(&["tutorial"], 1);
    for (who, seat_plan) in [(Role::A, &sc.reference_plan), (Role::B, &sc.reference_plan)] {
        for (engine, stops) in &seat_plan.engines {
            for (i, s) in stops.iter().enumerate() {
                if i > 0 {
                    edit(
                        &mut g,
                        who,
                        NoteEdit::AppendStop {
                            engine: engine.clone(),
                            station: s.station.clone(),
                        },
                    );
                }
                if who == Role::B && s.depart != Depart::Immediately {
                    edit(
                        &mut g,
                        who,
                        NoteEdit::SetDepart {
                            engine: engine.clone(),
                            stop: i,
                            depart: s.depart.clone(),
                        },
                    );
                }
                if who == Role::A {
                    for (j, op) in s.ops.iter().enumerate() {
                        edit(
                            &mut g,
                            who,
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
    for who in [Role::A, Role::B] {
        act(&mut g, who, Action::SetReady { ready: true });
    }
    for who in [Role::A, Role::B] {
        act(&mut g, who, Action::ReplayFinished);
    }
    let [freight, _] = views(&g);
    docs.push(("view-freight-scenario-done.json", freight));
    for who in [Role::A, Role::B] {
        act(&mut g, who, Action::Continue);
    }
    docs.push((
        "completion.json",
        serde_json::to_value(g.completion().unwrap()).unwrap(),
    ));

    // Act two after a disruption (engine failure, details to Routing only).
    let sc = builtin("corning-juice").unwrap();
    let seed = (0..64)
        .find(|&seed| {
            let mut g = new_game(&["corning-juice"], seed);
            drive(&mut g, &sc.reference_plan);
            matches!(g.metadata(), Some(Analysis::Run { disruption: Some(id), .. }) if id == "e2-fails")
        })
        .unwrap();
    let mut g = new_game(&["corning-juice"], seed);
    drive(&mut g, &sc.reference_plan);
    let [freight, routing] = views(&g);
    docs.push(("view-freight-interrupted-replay.json", freight));
    docs.push(("view-routing-interrupted-replay.json", routing));
    for who in [Role::A, Role::B] {
        act(&mut g, who, Action::ReplayFinished);
    }
    let [freight, routing] = views(&g);
    docs.push(("view-freight-act2.json", freight));
    docs.push(("view-routing-act2.json", routing));

    // Action payloads.
    docs.push((
        "actions.json",
        json!({
            "edit_append_stop": Action::Edit { edit: NoteEdit::AppendStop { engine: "E1".into(), station: "dansville".into() } },
            "edit_truncate_stops": Action::Edit { edit: NoteEdit::TruncateStops { engine: "E1".into(), from: 2 } },
            "edit_set_depart_at": Action::Edit { edit: NoteEdit::SetDepart { engine: "E1".into(), stop: 0, depart: Depart::At { hour: 5 } } },
            "edit_set_depart_after": Action::Edit { edit: NoteEdit::SetDepart { engine: "E2".into(), stop: 1, depart: Depart::After { engine: "E1".into(), plus_hours: 1 } } },
            "edit_set_depart_immediately": Action::Edit { edit: NoteEdit::SetDepart { engine: "E2".into(), stop: 1, depart: Depart::Immediately } },
            "edit_insert_couple": Action::Edit { edit: NoteEdit::InsertOp { engine: "E1".into(), stop: 1, index: 0, op: Op::Couple { kind: CarKind::Boxcar, load: LoadFilter::Any, count: 2 } } },
            "edit_insert_uncouple_loaded": Action::Edit { edit: NoteEdit::InsertOp { engine: "E1".into(), stop: 2, index: 1, op: Op::Uncouple { kind: CarKind::Boxcar, load: LoadFilter::Loaded { commodity: Commodity::Oranges }, count: 1 } } },
            "edit_insert_load": Action::Edit { edit: NoteEdit::InsertOp { engine: "E1".into(), stop: 2, index: 0, op: Op::Load { commodity: Commodity::Oranges, count: 2 } } },
            "edit_insert_process": Action::Edit { edit: NoteEdit::InsertOp { engine: "E2".into(), stop: 2, index: 0, op: Op::Process { count: 1 } } },
            "edit_insert_deliver": Action::Edit { edit: NoteEdit::InsertOp { engine: "E1".into(), stop: 3, index: 0, op: Op::Deliver { order: "O1".into() } } },
            "edit_remove_op": Action::Edit { edit: NoteEdit::RemoveOp { engine: "E1".into(), stop: 2, index: 0 } },
            "edit_set_heard_arrival": Action::Edit { edit: NoteEdit::SetHeardArrival { engine: "E1".into(), stop: 1, hour: Some(3) } },
            "edit_clear_heard_arrival": Action::Edit { edit: NoteEdit::SetHeardArrival { engine: "E1".into(), stop: 1, hour: None } },
            "set_ready": Action::SetReady { ready: true },
            "replay_finished": Action::ReplayFinished,
            "continue": Action::Continue,
        }),
    ));
    docs
}

/// Both roles write the plan's halves and declare ready (see the component
/// scenario generator for the owned-field split).
fn drive(g: &mut NightFreight, plan: &parlando_night_freight::plan::Plan) {
    for (who, freight) in [(Role::A, true), (Role::B, false)] {
        for (engine, stops) in &plan.engines {
            for (i, s) in stops.iter().enumerate() {
                if i > 0 {
                    edit(
                        g,
                        who,
                        NoteEdit::AppendStop {
                            engine: engine.clone(),
                            station: s.station.clone(),
                        },
                    );
                }
                if !freight && s.depart != Depart::Immediately {
                    edit(
                        g,
                        who,
                        NoteEdit::SetDepart {
                            engine: engine.clone(),
                            stop: i,
                            depart: s.depart.clone(),
                        },
                    );
                }
                if freight {
                    for (j, op) in s.ops.iter().enumerate() {
                        edit(
                            g,
                            who,
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
    for who in [Role::A, Role::B] {
        act(g, who, Action::SetReady { ready: true });
    }
}

/// Regenerates the examples with `GENERATE_EXAMPLES=1`, otherwise checks them.
#[test]
fn wire_examples_are_current() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples");
    let generate = std::env::var_os("GENERATE_EXAMPLES").is_some();
    if generate {
        std::fs::create_dir_all(&dir).unwrap();
    }
    for (name, value) in documents() {
        let text = serde_json::to_string_pretty(&value).unwrap() + "\n";
        let path = dir.join(name);
        if generate {
            std::fs::write(&path, &text).unwrap();
        } else {
            let have = std::fs::read_to_string(&path).unwrap_or_default();
            assert!(
                have == text,
                "{name} is stale: run GENERATE_EXAMPLES=1 cargo test --test wire_examples"
            );
        }
    }
}
