//! Policy tests: the information boundary, the automatic phase steps, the
//! chat cues and a whole session between two partners.

use super::*;
use crate::plan::{NoteEdit, Op};
use crate::world::Commodity;
use crate::{Config, NightFreight};
use parlando_game_sdk::agent::Transition;
use parlando_game_sdk::{Game, Role};

/// A two-seat tutorial session; Freight is seat A.
fn game(max_runs: u8) -> NightFreight {
    NightFreight::init(
        Config {
            scenarios: Some(vec!["tutorial".into()]),
            freight_seat: None,
            max_runs: Some(max_runs),
        },
        1,
    )
    .unwrap()
}

/// The view of a seat.
fn view_of(g: &NightFreight, role: Role) -> View {
    if role == Role::A {
        g.view_a()
    } else {
        g.view_b()
    }
}

/// Seat index in two-element arrays.
fn ix(role: Role) -> usize {
    usize::from(role == Role::B)
}

/// Applies a decision of `role` to the game and delivers the result to both
/// partners the way Parlando does: every accepted action reaches both seats.
fn commit(g: &mut NightFreight, ps: &mut [Partner; 2], role: Role, action: Action) {
    g.step(role, action.clone())
        .unwrap_or_else(|e| panic!("{role:?} {action:?} rejected: {}", e.code()));
    for r in [Role::A, Role::B] {
        ps[ix(r)].update(view_of(g, r));
    }
}

#[test]
fn cues_follow_the_words() {
    assert!(read_cues("OK, I'm ready").ready);
    assert!(read_cues("let's run it").ready);
    let hold = read_cues("not ready, wait");
    assert!(hold.hold && !hold.ready);
    assert!(read_cues("what do you know about the orders?").facts);
    assert_eq!(read_cues("the oranges are at Corning"), Cues::default());
}

#[test]
fn briefing_states_only_the_own_roles_facts() {
    let g = game(2);
    let freight = describe_briefing(&view_of(&g, Role::A));
    let routing = describe_briefing(&view_of(&g, Role::B));
    assert!(freight.contains("order O1") && !freight.contains("takes 3h"));
    assert!(routing.contains("avon to dansville takes 3h") && !routing.contains("order O1"));
}

/// The partner's `Edit` payload must never influence the policy.
#[test]
fn partner_edit_payloads_are_ignored() {
    let g = game(2);
    let mut p = Partner::new();
    p.update(view_of(&g, Role::B));
    let before = p.decide();
    // The human (Freight, seat A) edits; Parlando also delivers it to us.
    let secret = Action::Edit {
        edit: NoteEdit::InsertOp {
            engine: "E77".into(),
            stop: 0,
            index: 0,
            op: Op::Deliver { order: "O77".into() },
        },
    };
    let event = Observation {
        transition: Some(Transition {
            actor: Role::A,
            action: secret,
            view: view_of(&g, Role::B),
        }),
        message: None,
    };
    futures_lite_block_on(p.observe(event)).unwrap();
    assert_eq!(p.cues, Cues::default(), "an edit is not a chat cue");
    let after = p.decide();
    let render = |r: &Response<Action>| format!("{:?} {:?}", r.action, r.message);
    assert!(!render(&after).contains("77"));
    // The first decision introduced the partner; the second adds nothing.
    assert!(before.message.is_some());
    assert!(after.action.is_none() && after.message.is_none());
}

/// Drives a ready future that never suspends (the policy does no I/O).
fn futures_lite_block_on<F: std::future::Future>(f: F) -> F::Output {
    use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
    fn noop(_: *const ()) {}
    fn clone(_: *const ()) -> RawWaker {
        RawWaker::new(std::ptr::null(), &VTABLE)
    }
    static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, noop, noop, noop);
    let waker = unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), &VTABLE)) };
    let mut cx = Context::from_waker(&waker);
    let mut f = std::pin::pin!(f);
    match f.as_mut().poll(&mut cx) {
        Poll::Ready(v) => v,
        Poll::Pending => panic!("the partner policy must finish in one poll"),
    }
}

#[test]
fn ready_cue_presses_ready_and_hold_withdraws_it() {
    let mut g = game(2);
    let mut ps = [Partner::new(), Partner::new()];
    for r in [Role::A, Role::B] {
        ps[ix(r)].update(view_of(&g, r));
    }
    let intro = ps[0].decide();
    assert!(intro.message.unwrap().contains("order O1"));
    ps[0].hear("ready when you are");
    let r = ps[0].decide();
    assert_eq!(r.action, Some(Action::SetReady { ready: true }));
    commit(&mut g, &mut ps, Role::A, r.action.unwrap());
    ps[0].hear("wait, not ready yet");
    let r = ps[0].decide();
    assert_eq!(r.action, Some(Action::SetReady { ready: false }));
}

/// Two partners and a scripted "human" who says "ready" in every planning
/// phase play the scenario to its end: replay and continue are automatic and
/// the game accepts every action the policies send.
#[test]
fn two_partners_complete_the_phase_loop() {
    let mut g = game(2);
    let mut ps = [Partner::new(), Partner::new()];
    for r in [Role::A, Role::B] {
        ps[ix(r)].update(view_of(&g, r));
    }
    let mut chatter = 0;
    for _ in 0..200 {
        let phase = view_of(&g, Role::A).phase;
        if phase == Phase::Finished {
            break;
        }
        if phase == Phase::Planning {
            chatter += 1;
            for p in ps.iter_mut() {
                p.hear("ok, ready");
            }
        }
        for role in [Role::A, Role::B] {
            let r = ps[ix(role)].decide();
            if let Some(a) = r.action {
                commit(&mut g, &mut ps, role, a);
            }
            if let Some(text) = r.message {
                let other = 1 - ix(role);
                ps[other].hear(&text);
            }
        }
    }
    assert_eq!(view_of(&g, Role::A).phase, Phase::Finished);
    assert!(chatter >= 2, "both runs went through a planning phase");
}

/// Lets partner `role` act until it has nothing more to propose, the way the
/// host does: every accepted action reaches both partners and prompts the next
/// decision, and a rejection is reported to the proposer. Returns the messages
/// it sent, in order. A message travels with its action, so one that goes with
/// a rejected action is not sent.
fn drive(g: &mut NightFreight, ps: &mut [Partner; 2], role: Role) -> Vec<String> {
    let mut sent = Vec::new();
    for _ in 0..50 {
        let r = ps[ix(role)].decide();
        match r.action {
            Some(action) => match g.step(role, action) {
                Ok(()) => {
                    for seat in [Role::A, Role::B] {
                        ps[ix(seat)].update(view_of(g, seat));
                    }
                    sent.extend(r.message);
                }
                Err(e) => ps[ix(role)].rejected(e.code()),
            },
            None => {
                sent.extend(r.message);
                return sent;
            }
        }
    }
    panic!("the partner never settled");
}

/// A tutorial session with both partners holding their opening views.
fn routing_session() -> (NightFreight, [Partner; 2]) {
    let g = game(2);
    let mut ps = [Partner::new(), Partner::new()];
    for r in [Role::A, Role::B] {
        ps[ix(r)].update(view_of(&g, r));
    }
    (g, ps)
}

/// Freight's reference notes of the tutorial, entered as the human would.
fn freight_reference(g: &mut NightFreight, ps: &mut [Partner; 2]) {
    let edit = |edit| Action::Edit { edit };
    for station in ["dansville"] {
        commit(
            g,
            ps,
            Role::A,
            edit(NoteEdit::AppendStop { engine: "E1".into(), station: station.into() }),
        );
    }
    let ops = [
        (0, Op::Couple { kind: crate::world::CarKind::Boxcar, load: crate::plan::LoadFilter::Any, count: 1 }),
        (0, Op::Load { commodity: Commodity::Oranges, count: 1 }),
        (1, Op::Deliver { order: "O1".into() }),
    ];
    for (stop, op) in ops {
        let index = view_of(g, Role::A).notes.engines["E1"][stop].ops.len();
        commit(g, ps, Role::A, edit(NoteEdit::InsertOp { engine: "E1".into(), stop, index, op }));
    }
}

/// The scripted Routing partner, steered by typed commands, writes the
/// reference route and departure. With the reference Freight notes the joint
/// run meets the order; the partner then watches the replay on its own.
#[test]
fn commands_let_the_routing_partner_win_the_tutorial() {
    let (mut g, mut ps) = routing_session();
    let intro = drive(&mut g, &mut ps, Role::B);
    assert!(intro[0].contains("Type help"), "{intro:?}");

    ps[1].hear("E1 to Dansville; E1 stop 0 leave now");
    let sent = drive(&mut g, &mut ps, Role::B);
    assert_eq!(sent.len(), 2, "one confirmation per command: {sent:?}");
    assert!(sent[0].contains("E1 now goes on to dansville"), "{sent:?}");
    assert!(sent[1].contains("leave now"), "{sent:?}");
    let notes = view_of(&g, Role::B).notes;
    assert_eq!(notes.route("E1"), vec!["corning", "dansville"]);

    freight_reference(&mut g, &mut ps);
    ps[1].hear("show notes");
    let sent = drive(&mut g, &mut ps, Role::B);
    assert!(sent[0].contains("[0 corning"), "{sent:?}");

    // Edits withdrew readiness; "ready" now presses it again.
    ps[1].hear("ok, ready");
    commit(&mut g, &mut ps, Role::A, Action::SetReady { ready: true });
    drive(&mut g, &mut ps, Role::B);
    assert_eq!(view_of(&g, Role::B).phase, Phase::Replaying);
    let run = view_of(&g, Role::B).last_run.unwrap();
    assert!(run.orders.iter().all(|o| o.met), "{run:?}");

    // Replay and the scenario's end are automatic for the partner.
    commit(&mut g, &mut ps, Role::A, Action::ReplayFinished);
    drive(&mut g, &mut ps, Role::B);
    assert_eq!(view_of(&g, Role::B).phase, Phase::ScenarioDone);
}

/// A rejected edit is explained, the unfinished command is dropped with the
/// commands behind it, and the notes keep the edits that were accepted.
#[test]
fn a_rejection_is_reported_and_drops_the_rest() {
    let (mut g, mut ps) = routing_session();
    drive(&mut g, &mut ps, Role::B);
    ps[1].hear("E1 go bath dansville; E1 stop 0 leave at 3");
    let sent = drive(&mut g, &mut ps, Role::B);
    let reply = sent.last().unwrap();
    assert!(reply.contains("not directly connected"), "{reply}");
    assert!(reply.contains("E1 to dansville"), "{reply}");
    assert!(reply.contains("dropped the rest"), "{reply}");
    let notes = view_of(&g, Role::B).notes;
    assert_eq!(notes.route("E1"), vec!["corning", "bath"]);
    assert_eq!(notes.engines["E1"][0].depart, crate::plan::Depart::Immediately);
    // Nothing is left to do afterwards.
    assert!(ps[1].decide().message.is_none());
}

/// Commands and cues in one message, a field the role does not use, help,
/// and commands outside planning.
#[test]
fn commands_follow_the_role_and_the_phase() {
    let (mut g, mut ps) = routing_session();
    drive(&mut g, &mut ps, Role::B);
    ps[1].hear("help");
    assert!(drive(&mut g, &mut ps, Role::B)[0].contains("My note commands"));
    // Routing does not use ops, and the reply says so.
    ps[1].hear("E1 stop 0 couple 1 boxcar;ready");
    let sent = drive(&mut g, &mut ps, Role::B);
    assert!(sent[0].contains("my ops are not used"), "{sent:?}");
    // The ready cue survived the edit and was applied after it.
    assert!(view_of(&g, Role::B).ready.me);
    // Both ready: the run starts, the phase leaves planning, and commands
    // heard now are dropped with a notice.
    commit(&mut g, &mut ps, Role::A, Action::SetReady { ready: true });
    assert_eq!(view_of(&g, Role::B).phase, Phase::Replaying);
    ps[1].hear("E1 to dansville");
    let r = ps[1].decide();
    assert_eq!(r.action, Some(Action::ReplayFinished));
    assert!(r.message.unwrap().contains("only edit notes while planning"));
}

/// The partner's own replies are never read as commands by a partner that
/// hears them, so two scripted partners cannot start an edit loop.
#[test]
fn replies_are_not_commands() {
    let (mut g, mut ps) = routing_session();
    drive(&mut g, &mut ps, Role::B);
    ps[1].hear("E1 to dansville; E1 stop 0 leave at 1; help; show notes");
    let sent = drive(&mut g, &mut ps, Role::B);
    assert_eq!(sent.len(), 3, "two confirmations, then help and notes together");
    for text in sent {
        let vocab = commands::Vocabulary::new([("dansville", "Dansville"), ("corning", "Corning")]);
        let parsed = commands::parse_message(&text, &vocab);
        assert!(parsed.items.is_empty(), "{text:?} reads as {:?}", parsed.items);
    }
}
