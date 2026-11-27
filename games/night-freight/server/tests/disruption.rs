//! Disruption resolution, qualification, selection and the post-disruption world.

use parlando_night_freight::disruption::{
    plan_candidate, plan_disruption, Announcement, Candidate, DeadlineChange, DisruptionNotice,
    Effect, NewOrder, Selector,
};
use parlando_night_freight::scenario::builtin;
use parlando_night_freight::sim::{merge, simulate, Merged, Trace};
use parlando_night_freight::world::{Commodity, World};

/// The reference run of a built-in scenario.
fn reference(id: &str) -> (World, Merged, Trace) {
    let sc = builtin(id).unwrap();
    let world = sc.world();
    let merged = merge(&sc.reference_plan, &sc.reference_plan);
    let full = simulate(&world, &merged.plan, &merged.unreached);
    (world, merged, full)
}

fn candidate(id: &str, hour: u8, effect: Effect) -> Candidate {
    Candidate {
        id: id.into(),
        hour,
        effect,
        recovery: None,
    }
}

/// Selection is a deterministic function of the seed, and both candidates of
/// a scenario can be selected.
#[test]
fn selection_depends_on_the_seed() {
    let (world, merged, full) = reference("bath-bananas");
    let sc = builtin("bath-bananas").unwrap();
    let mut seen = std::collections::BTreeSet::new();
    for seed in 0..40 {
        let a = plan_disruption(&world, &merged, full.end_hour, &sc.disruptions, seed, 1).unwrap();
        let b = plan_disruption(&world, &merged, full.end_hour, &sc.disruptions, seed, 1).unwrap();
        assert_eq!(a.candidate, b.candidate);
        seen.insert(a.candidate);
    }
    assert_eq!(seen.len(), 2);
}

/// `MostLoadedEngine` resolves against the cars pulled at the disruption hour.
#[test]
fn most_loaded_engine_selector() {
    let (world, merged, full) = reference("bath-bananas");
    let c = candidate(
        "x",
        10,
        Effect::EngineFailure {
            engine: None,
            selector: Some(Selector::MostLoadedEngine),
        },
    );
    let (planned, breaks) = plan_candidate(&world, &merged, full.end_hour, &c).unwrap();
    assert!(breaks);
    assert_eq!(
        planned.notice,
        DisruptionNotice::EngineFailure {
            engine: "E1".into(),
            station: "corning".into()
        }
    );
    assert!(planned.world.engine("E1").unwrap().failed);
    assert_eq!(planned.world.start_hour(), 10);
    assert!(matches!(
        planned.routing,
        Some(Announcement::EngineFailure { .. })
    ));
    assert!(planned.freight.is_none());
    // At hour 0 nothing is coupled yet, so the selector finds no engine.
    let c0 = Candidate { hour: 0, ..c };
    assert!(plan_candidate(&world, &merged, full.end_hour, &c0).is_none());
}

/// `BusiestRemainingEdge` counts the legs still to start, ignores bridges,
/// and breaks ties lexicographically.
#[test]
fn busiest_remaining_edge_selector() {
    let (world, merged, full) = reference("bath-bananas");
    let c = candidate(
        "x",
        7,
        Effect::TrackClosure {
            a: None,
            b: None,
            selector: Some(Selector::BusiestRemainingEdge),
        },
    );
    let (planned, breaks) = plan_candidate(&world, &merged, full.end_hour, &c).unwrap();
    assert!(breaks);
    assert_eq!(
        planned.notice,
        DisruptionNotice::TrackClosure {
            a: "corning".into(),
            b: "dansville".into()
        }
    );
    let window = planned.world.restrictions.last().unwrap();
    assert_eq!((window.from_hour, window.until_hour), (7, 25));
    assert!(matches!(
        planned.routing,
        Some(Announcement::TrackClosure { .. })
    ));
}

/// A direct closure of a bridge does not apply.
#[test]
fn bridges_are_never_closed() {
    let (world, merged, full) = reference("corning-juice");
    let c = candidate(
        "x",
        3,
        Effect::TrackClosure {
            a: Some("corning".into()),
            b: Some("elmira".into()),
            selector: None,
        },
    );
    assert!(plan_candidate(&world, &merged, full.end_hour, &c).is_none());
}

/// Only candidates that break the merged plan qualify, and the hour must lie
/// inside the run.
#[test]
fn only_breaking_candidates_qualify() {
    let (world, merged, full) = reference("corning-juice");
    // The spare engine failing breaks nothing.
    let spare = candidate(
        "spare",
        7,
        Effect::EngineFailure {
            engine: Some("E3".into()),
            selector: None,
        },
    );
    let (_, breaks) = plan_candidate(&world, &merged, full.end_hour, &spare).unwrap();
    assert!(!breaks);
    assert!(plan_disruption(
        &world,
        &merged,
        full.end_hour,
        std::slice::from_ref(&spare),
        0,
        0
    )
    .is_none());
    // Too late in the day.
    let late = Candidate { hour: 23, ..spare };
    assert!(plan_candidate(&world, &merged, full.end_hour, &late).is_none());
}

/// A late order adds an order for Freight only, with deadline changes.
#[test]
fn late_order_with_deadline_changes() {
    let (world, merged, full) = reference("corning-juice");
    let c = candidate(
        "x",
        8,
        Effect::LateOrder {
            order: NewOrder {
                id: "O9".into(),
                commodity: Commodity::Bananas,
                count: 1,
                to: "corning".into(),
                deadline: 20,
            },
            deadline_changes: vec![DeadlineChange {
                order: "O2".into(),
                deadline: 12,
            }],
        },
    );
    let (planned, breaks) = plan_candidate(&world, &merged, full.end_hour, &c).unwrap();
    assert!(breaks);
    assert_eq!(planned.notice, DisruptionNotice::LateOrder {});
    assert!(planned.routing.is_none());
    let orders = &planned.world.state.orders;
    assert!(orders.iter().any(|o| o.id == "O9"));
    assert_eq!(orders.iter().find(|o| o.id == "O2").unwrap().deadline, 12);
    // Orders delivered before the disruption stay delivered.
    assert_eq!(orders.iter().find(|o| o.id == "O1").unwrap().delivered, 1);
    match planned.freight.unwrap() {
        Announcement::LateOrder {
            deadline_changes,
            order,
            ..
        } => {
            assert_eq!(order.id, "O9");
            assert_eq!(deadline_changes.len(), 1);
        }
        other => panic!("unexpected announcement {other:?}"),
    }
}
