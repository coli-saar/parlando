//! Simulator and merge tests on the laboratory world.

mod common;

use common::*;
use parlando_night_freight::plan::{Depart, LoadFilter, Op, Stop};
use parlando_night_freight::scenario::builtin;
use parlando_night_freight::sim::{
    divergence, merge, simulate, simulate_cut, CarCount, EventKind, FailReason, HoldReason,
    MissReason,
};
use parlando_night_freight::world::{CarKind, Commodity, Restriction};

fn imm() -> Depart {
    Depart::Immediately
}

fn run(plan: &parlando_night_freight::plan::Plan) -> parlando_night_freight::sim::Trace {
    simulate(&lab_world(), plan, &[])
}

/// Couple takes the lowest car ids; precondition failures are retried and
/// never succeed when capacity or cars are missing.
#[test]
fn couple_uses_lowest_ids_and_respects_capacity() {
    let t = run(&plan(vec![(
        "E1",
        vec![stop("a", imm(), vec![couple_boxcars(2)])],
    )]));
    let done = find(&t, |k| matches!(k, EventKind::OpDone { .. }));
    assert_eq!(done.len(), 1);
    assert!(matches!(&done[0].kind, EventKind::OpDone { cars, .. } if cars == &vec![1, 2]));

    // Capacity 2: a third car is never coupled.
    let t = run(&plan(vec![(
        "E1",
        vec![stop("a", imm(), vec![couple_boxcars(2), couple_boxcars(1)])],
    )]));
    assert_eq!(find(&t, |k| matches!(k, EventKind::OpDone { .. })).len(), 1);
    assert_eq!(
        t.end_hour, 24,
        "an op that can never run keeps the engine active"
    );
    assert!(find(&t, |k| matches!(
        k,
        EventKind::OpAbandoned { reason: FailReason::OverCapacity { .. }, .. }
    ))
    .len() == 1);

    // No tanker at `a`.
    let t = run(&plan(vec![(
        "E1",
        vec![stop(
            "a",
            imm(),
            vec![Op::Couple {
                kind: CarKind::Tanker,
                load: LoadFilter::Any,
                count: 1,
            }],
        )],
    )]));
    assert!(find(&t, |k| matches!(k, EventKind::OpDone { .. })).is_empty());
}

/// Uncouple returns the lowest matching car ids and fails without cars.
#[test]
fn uncouple_returns_matching_cars() {
    let uncouple = |count| Op::Uncouple {
        kind: CarKind::Boxcar,
        load: LoadFilter::Any,
        count,
    };
    let t = run(&plan(vec![(
        "E1",
        vec![stop("a", imm(), vec![couple_boxcars(2), uncouple(1)])],
    )]));
    let done = find(&t, |k| matches!(k, EventKind::OpDone { .. }));
    assert!(matches!(&done[1].kind, EventKind::OpDone { cars, .. } if cars == &vec![1]));
    let t = run(&plan(vec![(
        "E1",
        vec![stop("a", imm(), vec![uncouple(1)])],
    )]));
    assert!(find(&t, |k| matches!(k, EventKind::OpDone { .. })).is_empty());
}

/// Load needs stock and empty compatible cars, and takes the configured hours.
#[test]
fn load_needs_stock_and_takes_time() {
    let t = run(&plan(vec![(
        "E1",
        vec![
            stop(
                "a",
                Depart::At { hour: 3 },
                vec![couple_boxcars(1), load_bananas(1)],
            ),
            stop("b", imm(), vec![load_bananas(1)]),
        ],
    )]));
    let started = find(&t, |k| matches!(k, EventKind::OpStarted { .. }));
    assert_eq!(started.len(), 1, "no bananas at b");
    assert_eq!(started[0].hour, 0);
    let done = find(&t, |k| {
        matches!(
            k,
            EventKind::OpDone {
                op: Op::Load { .. },
                ..
            }
        )
    });
    assert_eq!(done.len(), 1);
    assert_eq!(done[0].hour, 1);

    // A tanker cannot carry bananas.
    let t = run(&plan(vec![(
        "E2",
        vec![stop(
            "b",
            imm(),
            vec![
                Op::Couple {
                    kind: CarKind::Tanker,
                    load: LoadFilter::Any,
                    count: 1,
                },
                load_bananas(1),
            ],
        )],
    )]));
    assert!(find(&t, |k| matches!(k, EventKind::OpStarted { .. })).is_empty());
}

/// Process turns loaded input cars into empty ones and fills output cars.
#[test]
fn process_uses_the_factory() {
    let process = Op::Process { count: 1 };
    let ops = vec![
        Op::Couple {
            kind: CarKind::Tanker,
            load: LoadFilter::Any,
            count: 1,
        },
        Op::Couple {
            kind: CarKind::Boxcar,
            load: LoadFilter::Loaded { commodity: Commodity::Oranges },
            count: 1,
        },
        process.clone(),
    ];
    let t = run(&plan(vec![("E2", vec![stop("b", imm(), ops)])]));
    let started = find(&t, |k| matches!(k, EventKind::OpStarted { .. }));
    assert_eq!(started.len(), 1);
    let done = find(&t, |k| {
        matches!(
            k,
            EventKind::OpDone {
                op: Op::Process { .. },
                ..
            }
        )
    });
    assert_eq!(done[0].hour, 2, "the factory takes two hours here");
    assert!(matches!(&done[0].kind, EventKind::OpDone { cars, .. } if cars == &vec![5, 3]));

    // Without the loaded boxcar the precondition fails.
    let t = run(&plan(vec![("E2", vec![stop("b", imm(), vec![process])])]));
    assert!(find(&t, |k| matches!(k, EventKind::OpStarted { .. })).is_empty());
}

/// Deliver marks an order met on time, supports partial delivery, and a
/// late delivery is not a success.
#[test]
fn deliver_and_orders() {
    let deliver = Op::Deliver { order: "O1".into() };
    let carry = vec![couple_boxcars(1), load_bananas(1)];
    let route = |ops_c: Vec<Op>| {
        plan(vec![(
            "E1",
            vec![
                stop("a", Depart::At { hour: 1 }, carry.clone()),
                stop("c", imm(), ops_c),
            ],
        )])
    };
    let t = run(&route(vec![deliver.clone()]));
    assert!(t.success);
    assert_eq!(
        find(&t, |k| matches!(k, EventKind::OrderMet { .. }))[0].hour,
        2
    );

    // Partial delivery of a two-car order.
    let mut world = lab_world();
    world.state.orders[0].count = 2;
    let t = simulate(&world, &route(vec![deliver.clone()]), &[]);
    assert!(!t.success);
    assert_eq!(t.orders[0].delivered, 1);
    assert!(find(&t, |k| matches!(k, EventKind::OrderMet { .. })).is_empty());

    // A deadline before the delivery hour: missed at the deadline, no OrderMet.
    let mut world = lab_world();
    world.state.orders[0].deadline = 1;
    let t = simulate(&world, &route(vec![deliver.clone()]), &[]);
    assert!(!t.success);
    assert_eq!(
        find(&t, |k| matches!(k, EventKind::OrderMissed { .. }))[0].hour,
        1
    );
    assert!(find(&t, |k| matches!(k, EventKind::OrderMet { .. })).is_empty());
    assert_eq!(t.orders[0].delivered_at, Some(2));

    // Wrong station: delivering at `a` fails.
    let t = run(&plan(vec![(
        "E1",
        vec![stop(
            "a",
            imm(),
            vec![couple_boxcars(1), load_bananas(1), deliver],
        )],
    )]));
    assert!(!t.success);
}

/// An order nobody serves is reported missed at its deadline hour.
#[test]
fn unserved_order_is_missed_at_the_deadline() {
    let t = run(&plan(vec![]));
    let missed = find(&t, |k| matches!(k, EventKind::OrderMissed { .. }));
    assert_eq!(missed.len(), 1);
    assert_eq!(missed[0].hour, 10);
    assert_eq!(t.end_hour, 10);
    assert!(!t.success);
}

/// `Immediately`, `At` and `After` depart conditions.
#[test]
fn depart_conditions() {
    let t = run(&plan(vec![(
        "E1",
        vec![stop("a", imm(), vec![]), stop("b", imm(), vec![])],
    )]));
    assert_eq!(depart_hour(&t, "E1"), Some(0));

    let t = run(&plan(vec![(
        "E1",
        vec![
            stop("a", Depart::At { hour: 3 }, vec![]),
            stop("b", imm(), vec![]),
        ],
    )]));
    assert_eq!(depart_hour(&t, "E1"), Some(3));

    // E2 arrives at `a` at hour 1; E1 waits for it plus one hour.
    let after = Depart::After {
        engine: "E2".into(),
        plus_hours: 1,
    };
    let t = run(&plan(vec![
        (
            "E1",
            vec![stop("a", after.clone(), vec![]), stop("b", imm(), vec![])],
        ),
        (
            "E2",
            vec![stop("b", imm(), vec![]), stop("a", imm(), vec![])],
        ),
    ]));
    assert_eq!(depart_hour(&t, "E1"), Some(2));

    // The named engine is already present when the waiting engine arrives:
    // E1 reaches `b` at hour 1 and E2, standing there since hour 0, leaves
    // one hour later.
    let t = run(&plan(vec![
        (
            "E1",
            vec![stop("a", imm(), vec![]), stop("b", imm(), vec![])],
        ),
        (
            "E2",
            vec![
                stop(
                    "b",
                    Depart::After {
                        engine: "E1".into(),
                        plus_hours: 1,
                    },
                    vec![],
                ),
                stop("c", imm(), vec![]),
            ],
        ),
    ]));
    assert_eq!(depart_hour(&t, "E2"), Some(2));

    // The named engine never comes: the engine never leaves.
    let t = run(&plan(vec![
        (
            "E1",
            vec![stop("a", after, vec![]), stop("b", imm(), vec![])],
        ),
        ("E2", vec![stop("b", imm(), vec![])]),
    ]));
    assert_eq!(depart_hour(&t, "E1"), None);
    assert_eq!(t.end_hour, 24);
}

/// Single track: the lower engine id enters first; the other holds until the
/// edge is free, and may enter in the arrival hour.
#[test]
fn signals_hold_the_second_train() {
    let t = run(&plan(vec![
        (
            "E1",
            vec![stop("a", imm(), vec![]), stop("b", imm(), vec![])],
        ),
        (
            "E2",
            vec![stop("b", imm(), vec![]), stop("a", imm(), vec![])],
        ),
    ]));
    assert_eq!(depart_hour(&t, "E1"), Some(0));
    assert_eq!(depart_hour(&t, "E2"), Some(1));
    let holds = find(&t, |k| matches!(k, EventKind::Hold { .. }));
    assert_eq!(holds.len(), 1, "one Hold event per blockage");
    assert!(
        matches!(&holds[0].kind, EventKind::Hold { engine, reason: HoldReason::Signal, .. } if engine == "E2")
    );
}

/// Closed edges hold the engine, which keeps working ops while held and
/// leaves when the window ends.
#[test]
fn closed_edges_hold_and_do_not_abandon_ops() {
    let mut world = lab_world();
    world.restrictions.push(Restriction {
        a: "b".into(),
        b: "a".into(),
        from_hour: 0,
        until_hour: 3,
    });
    let p = plan(vec![(
        "E1",
        vec![
            stop("a", imm(), vec![couple_boxcars(1), load_bananas(1)]),
            stop("b", imm(), vec![]),
        ],
    )]);
    let t = simulate(&world, &p, &[]);
    assert_eq!(depart_hour(&t, "E1"), Some(3));
    let holds = find(&t, |k| matches!(k, EventKind::Hold { .. }));
    assert_eq!(holds.len(), 1);
    assert!(matches!(
        &holds[0].kind,
        EventKind::Hold {
            reason: HoldReason::Closed,
            ..
        }
    ));
    assert!(find(&t, |k| matches!(k, EventKind::OpAbandoned { .. })).is_empty());
    assert_eq!(
        find(&t, |k| matches!(
            k,
            EventKind::OpDone {
                op: Op::Load { .. },
                ..
            }
        ))
        .len(),
        1
    );
}

/// An op that never becomes possible is abandoned when the engine leaves,
/// with the failed precondition as the reason; the op in progress finishes
/// first.
#[test]
fn departure_abandons_impossible_ops_with_reasons() {
    let ops = vec![
        couple_boxcars(1),
        load_bananas(1),
        Op::Uncouple {
            kind: CarKind::Boxcar,
            load: LoadFilter::Empty,
            count: 1,
        },
    ];
    let t = run(&plan(vec![(
        "E1",
        vec![stop("a", imm(), ops), stop("b", imm(), vec![])],
    )]));
    let abandoned = find(&t, |k| matches!(k, EventKind::OpAbandoned { .. }));
    assert_eq!(abandoned.len(), 1);
    assert_eq!(abandoned[0].hour, 1, "the engine waited for the load");
    assert!(matches!(
        &abandoned[0].kind,
        EventKind::OpAbandoned { index: 2, reason: FailReason::NoMatchingCarsOnEngine {
            wanted: 1, found: 0, coupled, ..
        }, .. } if coupled == &vec![CarCount { load: Some(Commodity::Bananas), count: 1 }]
    ));
    assert_eq!(depart_hour(&t, "E1"), Some(1));
    assert_eq!(
        find(&t, |k| matches!(k, EventKind::OpDone { op: Op::Load { .. }, .. })).len(),
        1
    );
}

/// The abandoned reason of the only op of a one-stop plan at `station`.
fn reason_of(station: &str, engine: &str, ops: Vec<Op>) -> FailReason {
    let t = run(&plan(vec![(
        engine,
        vec![stop(station, imm(), ops), stop("c", imm(), vec![])],
    )]));
    let abandoned = find(&t, |k| matches!(k, EventKind::OpAbandoned { .. }));
    match &abandoned.last().expect("an abandoned op").kind {
        EventKind::OpAbandoned { reason, .. } => reason.clone(),
        _ => unreachable!(),
    }
}

/// Every precondition has its own machine-readable reason.
#[test]
fn failure_reasons() {
    let loaded_oranges = LoadFilter::Loaded { commodity: Commodity::Oranges };
    // The yard at `a` has three empty boxcars, none of oranges.
    assert_eq!(
        reason_of("a", "E1", vec![Op::Couple { kind: CarKind::Boxcar, load: loaded_oranges, count: 1 }]),
        FailReason::NoMatchingCarsInYard {
            kind: CarKind::Boxcar,
            load: loaded_oranges,
            wanted: 1,
            found: 0,
            yard: vec![CarCount { load: None, count: 3 }],
        }
    );
    assert_eq!(
        reason_of("a", "E1", vec![couple_boxcars(3)]),
        FailReason::OverCapacity { wanted: 3, carried: 0, capacity: 2 }
    );
    assert!(matches!(
        reason_of("a", "E1", vec![Op::Uncouple { kind: CarKind::Boxcar, load: LoadFilter::Any, count: 1 }]),
        FailReason::NoMatchingCarsOnEngine { found: 0, wanted: 1, .. }
    ));
    assert_eq!(
        reason_of("a", "E1", vec![couple_boxcars(1), Op::Load { commodity: Commodity::Oranges, count: 1 }]),
        FailReason::NoStockHere { commodity: Commodity::Oranges }
    );
    assert_eq!(
        reason_of("a", "E1", vec![load_bananas(1)]),
        FailReason::NoEmptyCarsOnEngine { kind: CarKind::Boxcar, wanted: 1, found: 0 }
    );
    assert_eq!(reason_of("a", "E1", vec![Op::Process { count: 1 }]), FailReason::NoFactoryHere);
    assert_eq!(
        reason_of("b", "E2", vec![Op::Process { count: 1 }]),
        FailReason::NoInputCars { load: Commodity::Oranges, wanted: 1, found: 0 }
    );
    let deliver = Op::Deliver { order: "O1".into() };
    assert_eq!(
        reason_of("a", "E1", vec![deliver]),
        FailReason::WrongDestination { order: "O1".into(), to: "c".into() }
    );
}

/// `Deliver` at the destination without the right load names what the engine
/// carries.
#[test]
fn deliver_without_loads_names_the_cargo() {
    let t = run(&plan(vec![(
        "E1",
        vec![
            stop("a", imm(), vec![couple_boxcars(1)]),
            stop("c", imm(), vec![Op::Deliver { order: "O1".into() }]),
        ],
    )]));
    let abandoned = find(&t, |k| matches!(k, EventKind::OpAbandoned { .. }));
    assert_eq!(abandoned.len(), 1, "reported at the end of the run");
    assert!(matches!(
        &abandoned[0].kind,
        EventKind::OpAbandoned { reason: FailReason::NoOrderLoadsOnEngine { commodity: Commodity::Bananas, coupled, .. }, .. }
            if coupled == &vec![CarCount { load: None, count: 1 }]
    ));
    assert_eq!(abandoned[0].hour, t.end_hour);
}

/// An engine does not leave while an op is in progress, even if its depart
/// condition holds, and leaves right after the op completes.
#[test]
fn departure_waits_for_the_op_in_progress() {
    let t = run(&plan(vec![(
        "E1",
        vec![
            stop("a", imm(), vec![couple_boxcars(1), load_bananas(1)]),
            stop("b", imm(), vec![]),
        ],
    )]));
    assert_eq!(depart_hour(&t, "E1"), Some(1));
    assert!(find(&t, |k| matches!(k, EventKind::OpAbandoned { .. })).is_empty());
    let started = find(&t, |k| matches!(k, EventKind::OpStarted { .. }));
    assert_eq!(started.len(), 1);
    assert!(matches!(&started[0].kind, EventKind::OpStarted { finish_hour: 1, index: 1, .. }));
    assert_eq!(started[0].hour, 0);
}

/// A chain of ops finishes before departure even when every op waits for the
/// previous one: couple, load (1 hour), then uncouple the loaded car.
#[test]
fn departure_waits_for_a_chain_of_ops() {
    let uncouple_loaded = Op::Uncouple {
        kind: CarKind::Boxcar,
        load: LoadFilter::Loaded { commodity: Commodity::Bananas },
        count: 1,
    };
    let t = run(&plan(vec![(
        "E1",
        vec![
            stop("a", imm(), vec![couple_boxcars(1), load_bananas(1), uncouple_loaded]),
            stop("b", imm(), vec![]),
        ],
    )]));
    assert_eq!(depart_hour(&t, "E1"), Some(1));
    assert!(find(&t, |k| matches!(k, EventKind::OpAbandoned { .. })).is_empty());
    assert_eq!(find(&t, |k| matches!(k, EventKind::OpDone { .. })).len(), 3);
}

/// Before the depart condition holds, an op that waits for another engine is
/// retried every hour, and the engine leaves once it ran.
#[test]
fn waiting_ops_are_retried_before_departure() {
    // E2 reaches `a` at hour 1 and uncouples a loaded boxcar there; E1 waits
    // at `a` (its depart condition is `At 3`) and couples that car in time.
    // With `Immediately` E1 would leave at hour 0, as nothing is possible.
    let t = run(&plan(vec![
        (
            "E1",
            vec![
                stop("a", Depart::At { hour: 3 }, vec![Op::Couple {
                    kind: CarKind::Boxcar,
                    load: LoadFilter::Loaded { commodity: Commodity::Oranges },
                    count: 1,
                }]),
                stop("b", imm(), vec![]),
            ],
        ),
        (
            "E2",
            vec![
                stop("b", imm(), vec![Op::Couple {
                    kind: CarKind::Boxcar,
                    load: LoadFilter::Loaded { commodity: Commodity::Oranges },
                    count: 1,
                }]),
                stop("a", imm(), vec![Op::Uncouple {
                    kind: CarKind::Boxcar,
                    load: LoadFilter::Loaded { commodity: Commodity::Oranges },
                    count: 1,
                }]),
            ],
        ),
    ]));
    let done: Vec<(u8, String)> = find(&t, |k| matches!(k, EventKind::OpDone { .. }))
        .iter()
        .map(|e| match &e.kind {
            EventKind::OpDone { engine, .. } => (e.hour, engine.clone()),
            _ => unreachable!(),
        })
        .collect();
    assert_eq!(done, vec![(0, "E2".into()), (1, "E2".into()), (1, "E1".into())]);
    assert_eq!(depart_hour(&t, "E1"), Some(3));
    assert!(find(&t, |k| matches!(k, EventKind::OpAbandoned { .. })).is_empty());
}

/// Ops run in the first-possible order, not in list order.
#[test]
fn ops_run_out_of_list_order() {
    let t = run(&plan(vec![(
        "E1",
        vec![
            stop("a", imm(), vec![load_bananas(1), couple_boxcars(1)]),
            stop("b", imm(), vec![]),
        ],
    )]));
    let kinds: Vec<(u8, usize)> = t
        .events
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::OpDone { index, .. } | EventKind::OpStarted { index, .. } => {
                Some((e.hour, *index))
            }
            _ => None,
        })
        .collect();
    // Couple (index 1) at hour 0, Load (index 0) starts at 0 and is done at 1.
    assert_eq!(kinds, vec![(0, 1), (0, 0), (1, 0)]);
    assert!(find(&t, |k| matches!(k, EventKind::OpAbandoned { .. })).is_empty());
}

/// All permutations of a list.
fn permutations(ops: &[Op]) -> Vec<Vec<Op>> {
    if ops.len() <= 1 {
        return vec![ops.to_vec()];
    }
    let mut out = Vec::new();
    for i in 0..ops.len() {
        let mut rest = ops.to_vec();
        let head = rest.remove(i);
        for mut tail in permutations(&rest) {
            tail.insert(0, head.clone());
            out.push(tail);
        }
    }
    out
}

/// The order of the ops at a stop never changes the outcome.
#[test]
fn op_order_never_changes_the_outcome() {
    let uncouple_loaded = Op::Uncouple {
        kind: CarKind::Boxcar,
        load: LoadFilter::Loaded { commodity: Commodity::Bananas },
        count: 1,
    };
    let ops = vec![uncouple_loaded, load_bananas(1), couple_boxcars(1)];
    let outcome = |ops: Vec<Op>| {
        let t = run(&plan(vec![(
            "E1",
            vec![stop("a", imm(), ops), stop("b", imm(), vec![])],
        )]));
        let done: Vec<(u8, Op)> = t
            .events
            .iter()
            .filter_map(|e| match &e.kind {
                EventKind::OpDone { op, .. } => Some((e.hour, op.clone())),
                _ => None,
            })
            .collect();
        (done, depart_hour(&t, "E1"), t.success, t.end_hour,
         find(&t, |k| matches!(k, EventKind::OpAbandoned { .. })).len())
    };
    let first = outcome(ops.clone());
    assert_eq!(first.0.len(), 3);
    assert_eq!(first.4, 0);
    for perm in permutations(&ops) {
        assert_eq!(outcome(perm.clone()), first, "permutation {perm:?}");
    }
}

/// Every permutation of the ops at a stop of the scenario's reference plan
/// still meets all orders with the same result.
fn assert_permutation_invariant(id: &str, engine: &str, station: &str) {
    use parlando_night_freight::world::World;
    let sc = builtin(id).unwrap();
    let world: World = sc.world();
    let base = merge(&sc.reference_plan, &sc.reference_plan);
    let expected = simulate(&world, &base.plan, &[]);
    assert!(expected.success);
    let n = base.plan.engines[engine]
        .iter()
        .position(|s| s.station == station && !s.ops.is_empty())
        .unwrap();
    let ops = base.plan.engines[engine][n].ops.clone();
    for perm in permutations(&ops) {
        let mut plan = base.plan.clone();
        plan.engines.get_mut(engine).unwrap()[n].ops = perm.clone();
        let t = simulate(&world, &plan, &[]);
        assert!(t.success, "{id}: permutation {perm:?} fails");
        assert_eq!(t.orders, expected.orders, "{id}: {perm:?}");
        assert_eq!(t.end_hour, expected.end_hour, "{id}: {perm:?}");
    }
}

/// The reference plans of the tutorial and corning-juice survive any op order.
#[test]
fn reference_plans_survive_any_op_order() {
    assert_permutation_invariant("tutorial", "E1", "dansville");
    assert_permutation_invariant("corning-juice", "E1", "corning");
    assert_permutation_invariant("corning-juice", "E2", "corning");
}

/// A missed order says what came closest.
#[test]
fn order_misses_are_explained() {
    let why = |t: &parlando_night_freight::sim::Trace| {
        match &find(t, |k| matches!(k, EventKind::OrderMissed { .. }))[0].kind {
            EventKind::OrderMissed { why, .. } => why.clone(),
            _ => unreachable!(),
        }
    };
    let deliver = Op::Deliver { order: "O1".into() };
    let carry = vec![couple_boxcars(1), load_bananas(1)];
    let route = |ops_c: Vec<Op>| {
        plan(vec![(
            "E1",
            vec![
                stop("a", imm(), carry.clone()),
                stop("c", imm(), ops_c),
            ],
        )])
    };
    // Nobody moves.
    let t = run(&plan(vec![]));
    assert_eq!(
        why(&t),
        MissReason::NoCarReached { commodity: Commodity::Bananas, to: "c".into() }
    );
    // A loaded car reaches c at hour 2 but nobody delivers it.
    let t = run(&route(vec![]));
    assert_eq!(
        why(&t),
        MissReason::ReachedNotDelivered {
            commodity: Commodity::Bananas, to: "c".into(), reached_at: 2, delivered: 0, count: 1
        }
    );
    // The same with a deadline before the arrival.
    let mut world = lab_world();
    world.state.orders[0].deadline = 1;
    let t = simulate(&world, &route(vec![]), &[]);
    assert_eq!(
        why(&t),
        MissReason::ReachedAfterDeadline {
            commodity: Commodity::Bananas, to: "c".into(), reached_at: 2, deadline: 1
        }
    );
    // Delivered, but late.
    let t = simulate(&world, &route(vec![deliver]), &[]);
    assert_eq!(
        why(&t),
        MissReason::DeliveredAfterDeadline {
            commodity: Commodity::Bananas, to: "c".into(), delivered_at: 2, deadline: 1
        }
    );
}

/// An engine cannot operate or depart before `available_from`.
#[test]
fn available_from_delays_everything() {
    let mut world = lab_world();
    world.state.engines[0].available_from = 3;
    let p = plan(vec![(
        "E1",
        vec![
            stop("a", imm(), vec![couple_boxcars(1)]),
            stop("b", imm(), vec![]),
        ],
    )]);
    let t = simulate(&world, &p, &[]);
    assert_eq!(
        find(&t, |k| matches!(k, EventKind::OpDone { .. }))[0].hour,
        3
    );
    assert_eq!(depart_hour(&t, "E1"), Some(3));
}

/// A failed engine can uncouple cars but never departs.
#[test]
fn failed_engines_do_not_depart() {
    let mut world = lab_world();
    world.state.engines[0].failed = true;
    let p = plan(vec![(
        "E1",
        vec![
            stop("a", imm(), vec![couple_boxcars(1)]),
            stop("b", imm(), vec![]),
        ],
    )]);
    let t = simulate(&world, &p, &[]);
    assert_eq!(find(&t, |k| matches!(k, EventKind::OpDone { .. })).len(), 1);
    assert_eq!(depart_hour(&t, "E1"), None);
}

/// Simulation is deterministic.
#[test]
fn simulation_is_deterministic() {
    let sc = builtin("corning-juice").unwrap();
    let m = merge(&sc.reference_plan, &sc.reference_plan);
    let a = simulate(&sc.world(), &m.plan, &m.unreached);
    let b = simulate(&sc.world(), &m.plan, &m.unreached);
    assert_eq!(a, b);
}

/// Merge: Routing's route, Freight's ops attached by visit key.
#[test]
fn merge_attaches_ops_by_visit_key() {
    let load = load_bananas(1);
    let uncouple = Op::Uncouple {
        kind: CarKind::Boxcar,
        load: LoadFilter::Any,
        count: 1,
    };
    // A station visited twice.
    let routing = plan(vec![(
        "E1",
        vec![
            stop("a", Depart::At { hour: 2 }, vec![Op::Process { count: 9 }]),
            stop("b", imm(), vec![]),
            stop("a", imm(), vec![]),
        ],
    )]);
    let freight = plan(vec![(
        "E1",
        vec![
            stop("a", imm(), vec![load.clone()]),
            stop("b", imm(), vec![]),
            stop("a", imm(), vec![uncouple.clone()]),
        ],
    )]);
    let merged = merge(&routing, &freight);
    assert!(merged.unreached.is_empty());
    let stops: &Vec<Stop> = &merged.plan.engines["E1"];
    assert_eq!(stops[0].ops, vec![load]);
    assert_eq!(
        stops[0].depart,
        Depart::At { hour: 2 },
        "depart is route-owned"
    );
    assert_eq!(stops[2].ops, vec![uncouple.clone()]);

    // Routing omits the second visit: Freight's op there is unreached.
    let short = plan(vec![(
        "E1",
        vec![stop("a", imm(), vec![]), stop("b", imm(), vec![])],
    )]);
    let merged = merge(&short, &freight);
    assert_eq!(merged.unreached.len(), 1);
    assert_eq!(merged.unreached[0].station, "a");
    assert_eq!(merged.unreached[0].visit, 2);
    let t = simulate(&lab_world(), &merged.plan, &merged.unreached);
    let events = find(&t, |k| matches!(k, EventKind::OpUnreached { .. }));
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].hour, 0);

    // A station Freight planned for that Routing never visits.
    let freight_c = plan(vec![(
        "E1",
        vec![stop("a", imm(), vec![]), stop("c", imm(), vec![uncouple])],
    )]);
    let merged = merge(&short, &freight_c);
    assert_eq!(merged.unreached.len(), 1);
    assert_eq!(merged.unreached[0].station, "c");

    // Routing's ops and annotations are dropped.
    let mut annotated = routing.clone();
    annotated.engines.get_mut("E1").unwrap()[0].heard_arrival = Some(5);
    let merged = merge(&annotated, &plan(vec![]));
    assert!(merged.plan.engines["E1"]
        .iter()
        .all(|s| s.ops.is_empty() && s.heard_arrival.is_none()));
}

/// Divergence names the earliest differing event per engine.
#[test]
fn divergence_finds_the_earliest_difference() {
    let route = vec![stop("a", imm(), vec![]), stop("b", imm(), vec![])];
    let late = vec![
        stop("a", Depart::At { hour: 2 }, vec![]),
        stop("b", imm(), vec![]),
    ];
    let a = run(&plan(vec![("E1", route.clone())]));
    let same = run(&plan(vec![("E1", route)]));
    assert_eq!(divergence(&a, &same), None);
    let b = run(&plan(vec![("E1", late)]));
    let d = divergence(&a, &b).unwrap();
    assert_eq!(d.engine, "E1");
    assert_eq!(d.hour, 0);
    assert!(matches!(d.actual.unwrap().kind, EventKind::Depart { .. }));
    assert!(matches!(d.expected.unwrap().kind, EventKind::Depart { .. }));
}

/// A cut places in-transit engines at their destination, completes running
/// ops, and keeps the unexecuted part of the plan.
#[test]
fn cut_settles_in_flight_work() {
    let sc = builtin("bath-bananas").unwrap();
    let m = merge(&sc.reference_plan, &sc.reference_plan);
    // Hour 2: both engines are on their way to Avon.
    let (trace, cut) = simulate_cut(&sc.world(), &m.plan, &m.unreached, 2);
    let cut = cut.unwrap();
    assert!(trace.events.iter().all(|e| e.hour <= 2));
    let e1 = cut.world.engine("E1").unwrap();
    assert_eq!(
        (e1.station.as_str(), e1.available_from, e1.arrived_at),
        ("avon", 4, 4)
    );
    assert_eq!(cut.remainder.engines["E1"][0].station, "avon");
    assert_eq!(
        cut.remainder.engines["E1"][0].ops.len(),
        1,
        "the load has not run yet"
    );
    // Hour 4: E2's load (started at 3) is in progress and completes now.
    let (_, cut) = simulate_cut(&sc.world(), &m.plan, &m.unreached, 4);
    let cut = cut.unwrap();
    let e2 = cut.world.engine("E2").unwrap();
    assert_eq!(e2.available_from, 5);
    let car3 = cut.world.state.cars.iter().find(|c| c.id == 3).unwrap();
    assert_eq!(car3.load, Some(Commodity::Bananas));
    assert!(cut.remainder.engines["E2"][0].ops.is_empty());
    assert_eq!(cut.world.state.hour, 4);
    // The simulation ended before the cut hour.
    let tut = builtin("tutorial").unwrap();
    let m = merge(&tut.reference_plan, &tut.reference_plan);
    assert!(simulate_cut(&tut.world(), &m.plan, &m.unreached, 20)
        .1
        .is_none());
}

/// With `Immediately` and nothing possible, the engine leaves at once and the
/// op is reported with its reason.
#[test]
fn impossible_ops_do_not_hold_an_engine() {
    let t = run(&plan(vec![(
        "E1",
        vec![
            stop("a", imm(), vec![Op::Couple {
                kind: CarKind::Boxcar,
                load: LoadFilter::Loaded { commodity: Commodity::Oranges },
                count: 1,
            }]),
            stop("b", imm(), vec![]),
        ],
    )]));
    assert_eq!(depart_hour(&t, "E1"), Some(0));
    assert!(matches!(
        &find(&t, |k| matches!(k, EventKind::OpAbandoned { .. }))[0].kind,
        EventKind::OpAbandoned { reason: FailReason::NoMatchingCarsInYard { found: 0, .. }, .. }
    ));
}
