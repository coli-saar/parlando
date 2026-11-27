//! Scenario authoring rules, checked for every built-in scenario: the naive
//! plan fails, the reference plan succeeds, at least one disruption candidate
//! breaks the reference plan, and a recorded recovery succeeds after it.

use parlando_night_freight::disruption::plan_candidate;
use parlando_night_freight::scenario::{builtin, Scenario, BUILTIN_IDS};
use parlando_night_freight::sim::{merge, simulate, EventKind, Trace};

/// Runs a plan the way the joint run does when both players wrote the same notes.
fn run(scenario: &Scenario, plan: &parlando_night_freight::plan::Plan) -> Trace {
    let merged = merge(plan, plan);
    assert!(merged.unreached.is_empty(), "plan has unreached ops");
    simulate(&scenario.world(), &merged.plan, &merged.unreached)
}

/// Summarizes a trace for assertion messages.
fn describe(trace: &Trace) -> String {
    let mut out = String::new();
    for e in &trace.events {
        out.push_str(&format!("{:>2}h {:?}\n", e.hour, e.kind));
    }
    out.push_str(&format!("orders: {:?}\n", trace.orders));
    out
}

/// The plan a pair would try first fails in every scenario.
#[test]
fn naive_plans_fail() {
    for id in BUILTIN_IDS {
        let scenario = builtin(id).unwrap();
        let trace = run(&scenario, &scenario.naive_plan);
        assert!(
            !trace.success,
            "{id}: naive plan succeeded\n{}",
            describe(&trace)
        );
    }
}

/// A plan that meets every order exists in every scenario.
#[test]
fn reference_plans_succeed() {
    for id in BUILTIN_IDS {
        let scenario = builtin(id).unwrap();
        let trace = run(&scenario, &scenario.reference_plan);
        assert!(
            trace.success,
            "{id}: reference plan failed\n{}",
            describe(&trace)
        );
    }
}

/// Scored scenarios have a candidate that breaks the reference plan, and every
/// candidate that does so has a recorded recovery that succeeds.
#[test]
fn disruptions_break_the_reference_and_recoveries_succeed() {
    for id in BUILTIN_IDS {
        let scenario = builtin(id).unwrap();
        let world = scenario.world();
        let merged = merge(&scenario.reference_plan, &scenario.reference_plan);
        let full = simulate(&world, &merged.plan, &merged.unreached);
        let mut breaking = 0;
        for candidate in &scenario.disruptions {
            let (planned, breaks) = plan_candidate(&world, &merged, full.end_hour, candidate)
                .unwrap_or_else(|| panic!("{id}/{}: candidate does not apply", candidate.id));
            if !breaks {
                continue;
            }
            breaking += 1;
            let recovery = candidate.recovery.as_ref().unwrap_or_else(|| {
                panic!("{id}/{}: breaking candidate has no recovery", candidate.id)
            });
            let after = simulate(&planned.world, recovery, &[]);
            assert!(
                after.success,
                "{id}/{}: recovery failed\n{}",
                candidate.id,
                describe(&after)
            );
        }
        if id == "tutorial" {
            assert!(scenario.disruptions.is_empty());
        } else {
            assert!(
                breaking >= 1,
                "{id}: no candidate breaks the reference plan"
            );
            assert_eq!(
                breaking,
                scenario.disruptions.len(),
                "{id}: every candidate should break the reference plan"
            );
        }
    }
}

/// The corning-juice arithmetic of the design holds under the simulator.
#[test]
fn corning_juice_reference_timeline() {
    let scenario = builtin("corning-juice").unwrap();
    let trace = run(&scenario, &scenario.reference_plan);
    let at = |hour: u8, pred: &dyn Fn(&EventKind) -> bool| {
        trace.events.iter().any(|e| e.hour == hour && pred(&e.kind))
    };
    assert!(at(
        3,
        &|k| matches!(k, EventKind::Arrive { engine, station, .. } if engine == "E1" && station == "dansville")
    ));
    assert!(at(
        4,
        &|k| matches!(k, EventKind::Arrive { engine, station, .. } if engine == "E1" && station == "corning")
    ));
    assert!(at(
        7,
        &|k| matches!(k, EventKind::OrderMet { order } if order == "O1")
    ));
    assert!(at(
        7,
        &|k| matches!(k, EventKind::Arrive { engine, station, .. } if engine == "E2" && station == "elmira")
    ));
    assert!(at(
        14,
        &|k| matches!(k, EventKind::OrderMet { order } if order == "O2")
    ));
    assert_eq!(trace.end_hour, 14);
}
