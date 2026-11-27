//! Game-level tests: phases, actions, views, information boundaries, scoring,
//! analysis metadata and checkpoint recovery.

use parlando_game_sdk::{Game, Role};
use parlando_night_freight::disruption::Announcement;
use parlando_night_freight::plan::{Depart, LoadFilter, NoteEdit, Plan};
use parlando_night_freight::scenario::builtin;
use parlando_night_freight::sim::EventKind;
use parlando_night_freight::{Action, Analysis, Config, NightFreight, Phase, Seat, View};
use serde_json::Value;

/// Freight is seat A and Routing seat B in these tests.
const FREIGHT: Role = Role::A;
const ROUTING: Role = Role::B;

/// A session driver that can restore the game from a checkpoint after every
/// accepted action and checks the restored views are identical.
struct Driver {
    game: NightFreight,
    restore: bool,
}

impl Driver {
    fn new(scenarios: &[&str], seed: u64, max_runs: Option<u8>, restore: bool) -> Self {
        let config = Config {
            scenarios: Some(scenarios.iter().map(|s| s.to_string()).collect()),
            freight_seat: None,
            max_runs,
        };
        Self {
            game: NightFreight::init(config, seed).unwrap(),
            restore,
        }
    }

    /// Applies an action; returns the rejection code on failure.
    fn act(&mut self, who: Role, action: Action) -> Result<(), String> {
        let before = (
            view_json(&self.game, Role::A),
            view_json(&self.game, Role::B),
        );
        match self.game.step(who, action) {
            Err(r) => {
                let after = (
                    view_json(&self.game, Role::A),
                    view_json(&self.game, Role::B),
                );
                assert_eq!(before, after, "a rejection must not change the views");
                Err(r.code().to_string())
            }
            Ok(()) => {
                if self.restore {
                    let seen = (
                        view_json(&self.game, Role::A),
                        view_json(&self.game, Role::B),
                    );
                    let done = self
                        .game
                        .completion()
                        .map(|c| serde_json::to_value(c).unwrap());
                    let bytes = self.game.checkpoint().expect("checkpoint");
                    self.game = NightFreight::restore(&bytes).expect("restore");
                    let again = (
                        view_json(&self.game, Role::A),
                        view_json(&self.game, Role::B),
                    );
                    assert_eq!(seen, again, "restored views differ");
                    assert_eq!(
                        done,
                        self.game
                            .completion()
                            .map(|c| serde_json::to_value(c).unwrap())
                    );
                }
                Ok(())
            }
        }
    }

    fn edit(&mut self, who: Role, edit: NoteEdit) {
        self.act(who, Action::Edit { edit })
            .unwrap_or_else(|c| panic!("edit rejected: {c}"));
    }

    fn ready(&mut self, who: Role, ready: bool) -> Result<(), String> {
        self.act(who, Action::SetReady { ready })
    }

    fn view(&self, who: Role) -> View {
        if who == Role::A {
            self.game.view_a()
        } else {
            self.game.view_b()
        }
    }

    fn json(&self, who: Role) -> Value {
        view_json(&self.game, who)
    }

    /// Rewrites a role's notes to equal `target`, starting from their current notes.
    fn write_notes(&mut self, who: Role, target: &Plan) {
        let current = self.view(who).notes;
        for (engine, stops) in &target.engines {
            let have = current.engines.get(engine).cloned().unwrap_or_default();
            if have.len() > 1 {
                self.edit(
                    who,
                    NoteEdit::TruncateStops {
                        engine: engine.clone(),
                        from: 1,
                    },
                );
            }
            for _ in 0..have[0].ops.len() {
                self.edit(
                    who,
                    NoteEdit::RemoveOp {
                        engine: engine.clone(),
                        stop: 0,
                        index: 0,
                    },
                );
            }
            for (i, stop) in stops.iter().enumerate() {
                if i > 0 {
                    self.edit(
                        who,
                        NoteEdit::AppendStop {
                            engine: engine.clone(),
                            station: stop.station.clone(),
                        },
                    );
                }
                let existing = if i == 0 {
                    have[0].depart.clone()
                } else {
                    Depart::Immediately
                };
                if stop.depart != existing {
                    self.edit(
                        who,
                        NoteEdit::SetDepart {
                            engine: engine.clone(),
                            stop: i,
                            depart: stop.depart.clone(),
                        },
                    );
                }
                for (j, op) in stop.ops.iter().enumerate() {
                    self.edit(
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

    /// Both roles write the same plan and declare ready.
    fn run_plan(&mut self, plan: &Plan) {
        self.write_notes(FREIGHT, plan);
        self.write_notes(ROUTING, plan);
        self.ready(FREIGHT, true).unwrap();
        self.ready(ROUTING, true).unwrap();
    }

    fn finish_replay(&mut self) {
        self.act(FREIGHT, Action::ReplayFinished).unwrap();
        self.act(ROUTING, Action::ReplayFinished).unwrap();
    }

    fn proceed(&mut self) {
        self.act(FREIGHT, Action::Continue).unwrap();
        self.act(ROUTING, Action::Continue).unwrap();
    }
}

fn view_json(game: &NightFreight, who: Role) -> Value {
    let view = if who == Role::A {
        game.view_a()
    } else {
        game.view_b()
    };
    serde_json::to_value(view).unwrap()
}

/// The tutorial's reference plan with Routing holding the engine at Corning
/// until hour 10, which only Routing's notes say.
fn late_routing_plan(sc: &parlando_night_freight::scenario::Scenario) -> Plan {
    let mut late = sc.reference_plan.clone();
    late.engines.get_mut("E1").unwrap()[0].depart = Depart::At { hour: 10 };
    late
}

fn tutorial() -> Driver {
    Driver::new(&["tutorial"], 1, None, false)
}

/// Every action is rejected with `wrong_phase` outside its phase.
#[test]
fn actions_are_rejected_outside_their_phase() {
    let sc = builtin("tutorial").unwrap();
    let edit = Action::Edit {
        edit: NoteEdit::SetDepart {
            engine: "E1".into(),
            stop: 0,
            depart: Depart::At { hour: 1 },
        },
    };
    let ready = Action::SetReady { ready: true };
    let replay = Action::ReplayFinished;
    let cont = Action::Continue;
    let all = [edit.clone(), ready.clone(), replay.clone(), cont.clone()];
    let assert_only = |d: &mut Driver, allowed: &[usize]| {
        for (i, a) in all.iter().enumerate() {
            if allowed.contains(&i) {
                continue;
            }
            for who in [FREIGHT, ROUTING] {
                assert_eq!(
                    d.act(who, a.clone()),
                    Err("wrong_phase".into()),
                    "action {i}"
                );
            }
        }
    };
    let mut d = tutorial();
    assert_eq!(d.view(FREIGHT).phase, Phase::Planning);
    assert_only(&mut d, &[0, 1]);
    d.run_plan(&sc.naive_plan);
    assert_eq!(d.view(FREIGHT).phase, Phase::Replaying);
    assert_only(&mut d, &[2]);
    d.finish_replay();
    assert_eq!(d.view(FREIGHT).phase, Phase::Planning);
    d.run_plan(&sc.reference_plan);
    d.finish_replay();
    assert_eq!(d.view(FREIGHT).phase, Phase::ScenarioDone);
    assert_only(&mut d, &[3]);
    d.proceed();
    assert_eq!(d.view(FREIGHT).phase, Phase::Finished);
    assert!(d.game.completion().is_some());
    assert_only(&mut d, &[]);
}

/// Edits withdraw readiness; a run starts only when both are ready; repeating
/// an action is a `no_change`.
#[test]
fn readiness_rules() {
    let mut d = tutorial();
    d.ready(FREIGHT, true).unwrap();
    assert!(d.view(FREIGHT).ready.me && d.view(ROUTING).ready.partner);
    assert_eq!(d.ready(FREIGHT, true), Err("no_change".into()));
    assert_eq!(
        d.view(FREIGHT).phase,
        Phase::Planning,
        "one ready player does not start a run"
    );
    // The partner's edit does not withdraw my readiness; my own edit does.
    d.edit(
        ROUTING,
        NoteEdit::SetHeardArrival {
            engine: "E1".into(),
            stop: 0,
            hour: Some(3),
        },
    );
    assert!(d.view(FREIGHT).ready.me);
    d.edit(
        FREIGHT,
        NoteEdit::SetHeardArrival {
            engine: "E1".into(),
            stop: 0,
            hour: Some(3),
        },
    );
    assert!(!d.view(FREIGHT).ready.me);
    assert!(!d.view(ROUTING).ready.partner);
    d.ready(FREIGHT, true).unwrap();
    d.ready(FREIGHT, false).unwrap();
    d.ready(ROUTING, true).unwrap();
    assert_eq!(d.view(FREIGHT).phase, Phase::Planning);
    d.ready(FREIGHT, true).unwrap();
    assert_eq!(d.view(FREIGHT).phase, Phase::Replaying);
    assert_eq!(d.view(FREIGHT).runs_used, 1);
    // Replay and continue are once per role.
    d.act(FREIGHT, Action::ReplayFinished).unwrap();
    assert_eq!(
        d.act(FREIGHT, Action::ReplayFinished),
        Err("no_change".into())
    );
    assert!(d.view(FREIGHT).replay_finished.me && d.view(ROUTING).replay_finished.partner);
}

/// Edits are validated against shared facts only and reject with stable codes.
#[test]
fn edit_rejections_do_not_reveal_private_facts() {
    let mut d = tutorial();
    let engine = "E1".to_string();
    let reject = |d: &mut Driver, who, edit| d.act(who, Action::Edit { edit });
    // Routing's notes are a route: the next stop must be adjacent.
    assert_eq!(
        reject(
            &mut d,
            ROUTING,
            NoteEdit::AppendStop {
                engine: engine.clone(),
                station: "avon".into()
            }
        ),
        Err("not_adjacent".into())
    );
    // Freight's notes are an itinerary: any station is accepted.
    assert_eq!(
        reject(
            &mut d,
            FREIGHT,
            NoteEdit::AppendStop {
                engine: engine.clone(),
                station: "avon".into()
            }
        ),
        Ok(())
    );
    d.edit(
        FREIGHT,
        NoteEdit::TruncateStops {
            engine: engine.clone(),
            from: 1,
        },
    );
    assert_eq!(
        reject(
            &mut d,
            FREIGHT,
            NoteEdit::TruncateStops {
                engine: engine.clone(),
                from: 0
            }
        ),
        Err("fixed_stop".into())
    );
    assert_eq!(
        reject(
            &mut d,
            FREIGHT,
            NoteEdit::AppendStop {
                engine: "E9".into(),
                station: "avon".into()
            }
        ),
        Err("unknown_engine".into())
    );
    let deliver = |order: &str| NoteEdit::InsertOp {
        engine: "E1".into(),
        stop: 0,
        index: 0,
        op: parlando_night_freight::plan::Op::Deliver {
            order: order.into(),
        },
    };
    // Freight knows the orders; Routing cannot be told whether "O7" exists.
    assert_eq!(
        reject(&mut d, FREIGHT, deliver("O7")),
        Err("unknown_order".into())
    );
    assert_eq!(reject(&mut d, ROUTING, deliver("O7")), Ok(()));
    // Coupling a car that is not there is accepted: it fails only in the run.
    let couple = NoteEdit::InsertOp {
        engine: "E1".into(),
        stop: 0,
        index: 0,
        op: parlando_night_freight::plan::Op::Couple {
            kind: parlando_night_freight::world::CarKind::Tanker,
            load: LoadFilter::Any,
            count: 9,
        },
    };
    assert_eq!(reject(&mut d, FREIGHT, couple), Ok(()));
}

/// A failed run returns to planning with notes intact; the run budget ends the scenario.
#[test]
fn failed_runs_and_the_run_budget() {
    let sc = builtin("tutorial").unwrap();
    let mut d = Driver::new(&["tutorial"], 1, Some(2), false);
    d.run_plan(&sc.naive_plan);
    let run = d.view(FREIGHT).last_run.unwrap();
    assert!(!run.success && run.interrupted_at.is_none());
    assert_eq!(run.number, 1);
    assert!(run.orders.iter().all(|o| !o.met));
    d.finish_replay();
    let v = d.view(FREIGHT);
    assert_eq!(v.phase, Phase::Planning);
    assert!(!v.ready.me && !v.ready.partner);
    assert_eq!(v.notes, sc.naive_plan, "notes stay intact");
    assert!(v.last_run.is_some(), "the outcome is restorable");
    d.ready(FREIGHT, true).unwrap();
    d.ready(ROUTING, true).unwrap();
    d.finish_replay();
    // Budget used up: scenario over with zero orders met.
    let v = d.view(FREIGHT);
    assert_eq!(v.phase, Phase::ScenarioDone);
    let score = &v.scores[0];
    assert_eq!(
        (
            score.orders_met,
            score.orders_total,
            score.runs_used,
            score.replanned
        ),
        (0, 1, 2, false)
    );
}

/// Divergence shows where a player's expectation and reality part.
#[test]
fn divergence_per_role() {
    let sc = builtin("tutorial").unwrap();
    let mut d = tutorial();
    // Freight believes the train leaves at once; Routing's notes hold it until 10.
    d.write_notes(FREIGHT, &sc.reference_plan);
    d.write_notes(ROUTING, &late_routing_plan(&sc));
    d.ready(FREIGHT, true).unwrap();
    d.ready(ROUTING, true).unwrap();
    let f = d.view(FREIGHT).last_run.unwrap();
    let r = d.view(ROUTING).last_run.unwrap();
    assert!(!f.success);
    let fd = f.divergence.unwrap();
    // Both traces couple and load from hour 0; loading ends at hour 1, where
    // Freight expected the departure that Routing holds back until hour 10.
    assert_eq!((fd.hour, fd.engine.as_str()), (1, "E1"), "{fd:?}");
    assert!(matches!(fd.expected.unwrap().kind, EventKind::Depart { .. }));
    assert!(
        r.divergence.is_none(),
        "Routing's expectation matched reality"
    );
    assert_ne!(
        f.expected, r.expected,
        "each role gets their own expectation"
    );
    assert_eq!(f.actual, r.actual, "the real trace is shared");
}

/// The tutorial: a successful run ends the scenario without a disruption.
#[test]
fn tutorial_completes_without_disruption() {
    let sc = builtin("tutorial").unwrap();
    let mut d = tutorial();
    d.run_plan(&sc.reference_plan);
    let run = d.view(ROUTING).last_run.unwrap();
    assert!(run.success && run.interrupted_at.is_none());
    d.finish_replay();
    assert_eq!(d.view(FREIGHT).phase, Phase::ScenarioDone);
    d.proceed();
    let done = serde_json::to_value(d.game.completion().unwrap()).unwrap();
    assert_eq!(
        done["scores"][0],
        serde_json::json!({"scenario": "tutorial", "orders_met": 1, "orders_total": 1, "runs_used": 1, "replanned": false})
    );
}

/// The full corning-juice story: success, interruption, replan, success.
/// With `restore` the session is rebuilt from a checkpoint after every action.
fn play_corning_juice(seed: u64, restore: bool) -> (Value, String) {
    let sc = builtin("corning-juice").unwrap();
    let mut d = Driver::new(&["corning-juice"], seed, None, restore);
    d.run_plan(&sc.reference_plan);
    // Analysis metadata of the run names the chosen disruption.
    let analysis = d.game.metadata();
    let candidate = match analysis {
        Some(Analysis::Run {
            disruption: Some(id),
            interrupted_at: Some(_),
            number: 1,
            ..
        }) => id.clone(),
        other if restore => {
            // After a restore the metadata is empty by design.
            assert!(other.is_none());
            String::new()
        }
        other => panic!("unexpected analysis {other:?}"),
    };
    let f = d.view(FREIGHT);
    let run = f.last_run.clone().unwrap();
    let h = run
        .interrupted_at
        .expect("the successful reference run is interrupted");
    assert!(!run.success);
    // The film is cut: no event after the disruption hour, and the end of the
    // plan (the second delivery) is never shown.
    assert!(run.actual.events.iter().all(|e| e.hour <= h));
    assert!(matches!(
        run.actual.events.last().unwrap().kind,
        EventKind::Disrupted { .. }
    ));
    assert!(run.expected.events.iter().all(|e| e.hour <= h));
    assert!(!run
        .actual
        .events
        .iter()
        .any(|e| matches!(&e.kind, EventKind::OrderMet { order } if order == "O2")));
    d.finish_replay();

    // Act 2: world snapshot, replaced notes and one-sided announcements.
    let f = d.view(FREIGHT);
    let r = d.view(ROUTING);
    assert_eq!((f.act, f.phase, f.map.start_hour), (2, Phase::Planning, h));
    assert_eq!(
        f.notes, r.notes,
        "both notes become the remainder of the merged plan"
    );
    assert_eq!(f.runs_used, 1);
    let (fa, ra) = match (&f.briefing, &r.briefing) {
        (
            parlando_night_freight::Briefing::Freight {
                announcement: fa, ..
            },
            parlando_night_freight::Briefing::Routing {
                announcement: ra, ..
            },
        ) => (fa.clone(), ra.clone()),
        _ => panic!("wrong briefings"),
    };
    let id = if restore {
        match (&fa, &ra) {
            (Some(Announcement::LateOrder { .. }), None) => "late-bananas",
            (None, Some(Announcement::EngineFailure { .. })) => "e2-fails",
            other => panic!("one role must be told: {other:?}"),
        }
        .to_string()
    } else {
        candidate
    };
    match id.as_str() {
        "late-bananas" => assert!(matches!(
            (&fa, &ra),
            (Some(Announcement::LateOrder { .. }), None)
        )),
        "e2-fails" => assert!(matches!(
            (&fa, &ra),
            (None, Some(Announcement::EngineFailure { .. }))
        )),
        other => panic!("{other}"),
    }
    let recovery = sc
        .disruptions
        .iter()
        .find(|c| c.id == id)
        .unwrap()
        .recovery
        .clone()
        .unwrap();
    d.run_plan(&recovery);
    let run = d.view(FREIGHT).last_run.unwrap();
    assert!(run.success, "the recorded recovery works through the game");
    assert_eq!(run.act, 2);
    assert_eq!(run.number, 2);
    d.finish_replay();
    assert_eq!(d.view(FREIGHT).phase, Phase::ScenarioDone);
    d.proceed();
    (
        serde_json::to_value(d.game.completion().unwrap()).unwrap(),
        id,
    )
}

/// Both disruptions of corning-juice can occur, and checkpoint recovery after
/// every action leads to the same result as an uninterrupted session.
#[test]
fn corning_juice_replan_and_recovery() {
    let mut ids = std::collections::BTreeSet::new();
    for seed in 0..12 {
        let (control, id) = play_corning_juice(seed, false);
        let (restored, _) = play_corning_juice(seed, true);
        assert_eq!(control, restored, "seed {seed}");
        let score = &control["scores"][0];
        assert_eq!(score["replanned"], Value::Bool(true));
        assert_eq!(score["runs_used"], 2);
        let expected_total = if id == "late-bananas" { 3 } else { 2 };
        assert_eq!(score["orders_total"], expected_total);
        assert_eq!(score["orders_met"], expected_total);
        ids.insert(id);
    }
    assert_eq!(
        ids.len(),
        2,
        "both disruption candidates are selectable by seed"
    );
}

/// Using the last permitted run for a success leaves no budget for a replan,
/// so the day is shown to the end.
#[test]
fn no_disruption_when_the_budget_is_spent() {
    let sc = builtin("corning-juice").unwrap();
    let mut d = Driver::new(&["corning-juice"], 3, Some(1), false);
    d.run_plan(&sc.reference_plan);
    let run = d.view(FREIGHT).last_run.unwrap();
    assert!(run.success && run.interrupted_at.is_none());
    d.finish_replay();
    assert_eq!(d.view(FREIGHT).phase, Phase::ScenarioDone);
    assert!(!d.view(FREIGHT).scores[0].replanned);
}

/// Several scenarios in sequence; Continue advances and resets per-scenario state.
#[test]
fn scenario_sequence() {
    let mut d = Driver::new(&["tutorial", "corning-juice"], 5, None, false);
    d.run_plan(&builtin("tutorial").unwrap().reference_plan);
    d.finish_replay();
    d.proceed();
    let v = d.view(FREIGHT);
    assert_eq!(
        (v.scenario_index, v.scenario_count, v.scenario_id.as_str()),
        (1, 2, "corning-juice")
    );
    assert_eq!((v.phase, v.runs_used, v.act), (Phase::Planning, 0, 1));
    assert!(v.last_run.is_none());
    assert_eq!(v.scores.len(), 1);
    assert!(d.game.completion().is_none());
}

/// Analysis metadata: agreement on edits, full notes and outcomes on runs.
#[test]
fn analysis_metadata() {
    let sc = builtin("tutorial").unwrap();
    let mut d = tutorial();
    d.edit(
        FREIGHT,
        NoteEdit::AppendStop {
            engine: "E1".into(),
            station: "dansville".into(),
        },
    );
    match d.game.metadata() {
        Some(Analysis::Edit {
            actor: Seat::Freight,
            agreement,
        }) => assert!(*agreement < 1.0),
        other => panic!("{other:?}"),
    }
    d.edit(
        ROUTING,
        NoteEdit::AppendStop {
            engine: "E1".into(),
            station: "dansville".into(),
        },
    );
    match d.game.metadata() {
        Some(Analysis::Edit {
            actor: Seat::Routing,
            agreement,
        }) => assert_eq!(*agreement, 1.0),
        other => panic!("{other:?}"),
    }
    // A rejected edit does not replace the metadata.
    assert!(d
        .act(
            ROUTING,
            Action::Edit {
                edit: NoteEdit::TruncateStops {
                    engine: "E1".into(),
                    from: 0
                }
            }
        )
        .is_err());
    assert!(matches!(
        d.game.metadata(),
        Some(Analysis::Edit {
            actor: Seat::Routing,
            ..
        })
    ));
    d.ready(FREIGHT, true).unwrap();
    assert!(
        d.game.metadata().is_none(),
        "non-edit, non-run actions clear the metadata"
    );
    d.write_notes(FREIGHT, &sc.reference_plan);
    d.write_notes(ROUTING, &late_routing_plan(&sc));
    d.ready(FREIGHT, true).unwrap();
    d.ready(ROUTING, true).unwrap();
    match d.game.metadata() {
        Some(Analysis::Run {
            freight_notes,
            routing_notes,
            merged,
            actual,
            expected_freight,
            expected_routing,
            divergence_freight,
            divergence_routing,
            agreement,
            number,
            ..
        }) => {
            assert_eq!(*number, 1);
            assert_eq!(freight_notes, &sc.reference_plan);
            assert_eq!(routing_notes, &late_routing_plan(&sc));
            assert!(merged.engines["E1"].iter().any(|s| !s.ops.is_empty()));
            assert!(!actual.success && expected_freight.success && !expected_routing.success);
            assert!(divergence_freight.is_some() && divergence_routing.is_none());
            assert!(*agreement < 1.0);
        }
        other => panic!("{other:?}"),
    }
    d.act(FREIGHT, Action::ReplayFinished).unwrap();
    assert!(d.game.metadata().is_none());
}

/// Keys that Freight alone may see and keys that Routing alone may see.
const FREIGHT_ONLY: [&str; 7] = [
    "orders",
    "cars",
    "stock",
    "factories",
    "load_hours",
    "deliver_hours",
    "deadline",
];
const ROUTING_ONLY: [&str; 3] = ["travel_times", "restrictions", "capacities"];

/// Collects every object key outside the shared `last_run` replay.
fn private_keys(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::Object(map) => {
            for (k, v) in map {
                if k == "last_run" {
                    continue;
                }
                out.push(k.clone());
                private_keys(v, out);
            }
        }
        Value::Array(items) => items.iter().for_each(|i| private_keys(i, out)),
        _ => {}
    }
}

/// Checks one pair of views against the information boundary.
fn assert_boundary(d: &Driver, marker_f: &str, marker_r: &str, check_markers: bool) {
    let (f, r) = (d.json(FREIGHT), d.json(ROUTING));
    assert_eq!(f["briefing"]["type"], "freight");
    assert_eq!(r["briefing"]["type"], "routing");
    let (mut fk, mut rk) = (vec![], vec![]);
    private_keys(&f, &mut fk);
    private_keys(&r, &mut rk);
    for key in ROUTING_ONLY {
        assert!(!fk.iter().any(|k| k == key), "Freight view leaks {key}");
    }
    for key in FREIGHT_ONLY {
        assert!(!rk.iter().any(|k| k == key), "Routing view leaks {key}");
    }
    for e in f["map"]["engines"].as_array().unwrap() {
        assert!(
            e["available_from"].is_null(),
            "engine availability is Routing's fact"
        );
    }
    for e in r["map"]["engines"].as_array().unwrap() {
        assert!(e["available_from"].is_number());
    }
    if check_markers {
        let (ft, rt) = (f.to_string(), r.to_string());
        assert!(
            ft.contains(marker_f) && !ft.contains(marker_r),
            "Freight view: {ft}"
        );
        assert!(
            rt.contains(marker_r) && !rt.contains(marker_f),
            "Routing view: {rt}"
        );
    }
}

/// No view contains the partner's notes or briefing in any phase.
#[test]
fn information_boundary_in_every_phase() {
    let sc = builtin("corning-juice").unwrap();
    let mut d = Driver::new(&["corning-juice"], 4, None, false);
    // Distinct markers: heard_arrival annotations appear only in notes.
    let mark = |d: &mut Driver, who, hour| {
        d.edit(
            who,
            NoteEdit::SetHeardArrival {
                engine: "E1".into(),
                stop: 0,
                hour: Some(hour),
            },
        )
    };
    mark(&mut d, FREIGHT, 17);
    mark(&mut d, ROUTING, 19);
    let (mf, mr) = ("\"heard_arrival\":17", "\"heard_arrival\":19");
    // Planning, before any run.
    assert_boundary(&d, mf, mr, true);
    d.ready(FREIGHT, true).unwrap();
    assert_boundary(&d, mf, mr, true);
    // Replaying: a failed run (the naive plan), with the markers kept in the notes.
    let naive = sc.naive_plan.clone();
    d.write_notes(FREIGHT, &naive);
    d.write_notes(ROUTING, &naive);
    d.ready(FREIGHT, true).unwrap();
    d.ready(ROUTING, true).unwrap();
    assert_eq!(d.view(FREIGHT).phase, Phase::Replaying);
    assert_boundary(&d, mf, mr, true);
    // Planning after a failed run.
    d.finish_replay();
    assert_boundary(&d, mf, mr, true);
    // Replaying an interrupted success, then planning in act 2.
    let reference = sc.reference_plan.clone();
    d.write_notes(FREIGHT, &reference);
    d.write_notes(ROUTING, &reference);
    d.ready(FREIGHT, true).unwrap();
    d.ready(ROUTING, true).unwrap();
    assert!(d.view(FREIGHT).last_run.unwrap().interrupted_at.is_some());
    assert_boundary(&d, mf, mr, false);
    d.finish_replay();
    assert_eq!(d.view(FREIGHT).act, 2);
    assert_boundary(&d, mf, mr, false);
    // One-sided announcements.
    let (f, r) = (d.json(FREIGHT), d.json(ROUTING));
    let told = |v: &Value| !v["briefing"]["announcement"].is_null();
    assert_ne!(told(&f), told(&r), "exactly one role is told the details");
    // Scenario done and finished.
    let h = d.view(FREIGHT).map.start_hour;
    assert!(h > 0);
    let id = f["briefing"]["announcement"]["type"]
        .as_str()
        .or(r["briefing"]["announcement"]["type"].as_str())
        .unwrap()
        .to_string();
    let candidate = if id == "late_order" {
        "late-bananas"
    } else {
        "e2-fails"
    };
    let recovery = sc
        .disruptions
        .iter()
        .find(|c| c.id == candidate)
        .unwrap()
        .recovery
        .clone()
        .unwrap();
    d.write_notes(FREIGHT, &recovery);
    d.write_notes(ROUTING, &recovery);
    d.edit(
        ROUTING,
        NoteEdit::SetHeardArrival {
            engine: "E1".into(),
            stop: 0,
            hour: Some(19),
        },
    );
    d.ready(FREIGHT, true).unwrap();
    d.ready(ROUTING, true).unwrap();
    assert_boundary(&d, mf, mr, false);
    d.finish_replay();
    assert_eq!(d.view(FREIGHT).phase, Phase::ScenarioDone);
    assert_boundary(&d, mf, mr, false);
    d.proceed();
    assert_eq!(d.view(FREIGHT).phase, Phase::Finished);
    assert_boundary(&d, mf, mr, false);
}

/// The Freight role can be seated at B.
#[test]
fn freight_seat_b() {
    let config = Config {
        scenarios: Some(vec!["tutorial".into()]),
        freight_seat: Some("B".into()),
        max_runs: None,
    };
    let g = NightFreight::init(config, 0).unwrap();
    assert_eq!(g.view_a().role, Seat::Routing);
    assert_eq!(g.view_b().role, Seat::Freight);
    let bad = |c: Config| NightFreight::init(c, 0).is_err();
    assert!(bad(Config {
        scenarios: Some(vec!["nope".into()]),
        freight_seat: None,
        max_runs: None
    }));
    assert!(bad(Config {
        scenarios: Some(vec![]),
        freight_seat: None,
        max_runs: None
    }));
    assert!(bad(Config {
        scenarios: None,
        freight_seat: Some("c".into()),
        max_runs: None
    }));
    assert!(bad(Config {
        scenarios: None,
        freight_seat: None,
        max_runs: Some(0)
    }));
    // The default configuration plays the three built-in scenarios.
    let g = NightFreight::init(
        Config {
            scenarios: None,
            freight_seat: None,
            max_runs: None,
        },
        0,
    )
    .unwrap();
    assert_eq!(g.view_a().scenario_count, 3);
}

/// Freight's expected trace treats its stops as an itinerary: the ghost for
/// [Corning, Dansville] and for the non-adjacent [Corning, Elmira, Avon] both
/// run, and the merge is unaffected by the inserted stops.
#[test]
fn freight_itinerary_normalization() {
    use parlando_night_freight::plan::Stop;
    use parlando_night_freight::sim::{itinerary_to_route, merge, simulate};
    let sc = builtin("tutorial").unwrap();
    let world = sc.world();
    let reference = sc.reference_plan.clone();
    let ghost = simulate(&world, &itinerary_to_route(&world.network, &reference), &[]);
    assert!(ghost.success);

    let mut far = reference.clone();
    let mut stops = far.engines["E1"].clone();
    // Corning's ops stay; the delivery moves on to Avon via Dansville.
    stops.push(Stop::at("elmira"));
    stops.push(Stop::at("avon"));
    far.engines.insert("E1".into(), stops);
    let route = itinerary_to_route(&world.network, &far);
    let names: Vec<&str> = route.engines["E1"].iter().map(|s| s.station.as_str()).collect();
    // dansville -> elmira goes through corning; elmira -> avon through corning, dansville.
    assert_eq!(
        names,
        ["corning", "dansville", "corning", "elmira", "corning", "dansville", "avon"]
    );
    assert!(route.engines["E1"].iter().all(|s| s.depart == Depart::Immediately));
    assert!(simulate(&world, &route, &[]).success);

    // The merge keeps Freight's visit keys as written and ignores the route.
    let merged = merge(&reference, &far);
    assert_eq!(merged.plan.engines["E1"].len(), 2);
    assert!(merged.unreached.is_empty());
}
