use parlando_game_sdk::{Game, Role};
use parlando_great_tree::GreatTree;
use serde_json::{json, Value};

/// Round-trips an action through JSON, exactly as the wire protocol would, so this test only
/// depends on the public serde shape (spec §13's `Action` contract), not on crate-internal types.
fn set_sun_json(limb: &str, lit: bool) -> Value {
    json!({ "type": "setSun", "limb": limb, "lit": lit })
}

fn set_flow_json(root: &str, open: bool) -> Value {
    json!({ "type": "setFlow", "root": root, "open": open })
}

#[test]
fn a_full_play_through_reaches_completion() {
    let config = serde_json::from_value(json!({})).unwrap();
    let mut game = GreatTree::init(config, 123).unwrap();

    let crown_view = serde_json::to_value(game.view_a()).unwrap();
    let root_view = serde_json::to_value(game.view_b()).unwrap();
    assert_eq!(crown_view["role"], "crown");
    assert_eq!(root_view["role"], "root");
    assert!(crown_view.get("roots").is_none());
    assert!(root_view.get("limbs").is_none());

    assert!(
        game.completion().is_none(),
        "one starting bloom is not a win yet"
    );

    let limb_ids = ["spire", "hook", "fork", "cradle", "nub"];
    let root_ids = ["hand", "knot", "tip", "swollen", "deep"];

    // Brute-force every (limb, root) sun+water combination in turn. This does not need to know
    // the hidden bijection — it just tries pairs until roots stop rejecting as frozen, exactly
    // as two players fumbling toward each other's reports would, and stops the instant three
    // limbs are flowering.
    'outer: for &limb in &limb_ids {
        let action: parlando_great_tree::Action =
            serde_json::from_value(set_sun_json(limb, true)).unwrap();
        game.step(Role::A, action).unwrap();

        for &root in &root_ids {
            let flow_action: parlando_great_tree::Action =
                serde_json::from_value(set_flow_json(root, true)).unwrap();
            let _ = game.step(Role::B, flow_action);
            if let Some(completion) = game.completion() {
                assert!(completion.flowered_limbs.len() >= 3);
                break 'outer;
            }
        }
    }

    assert!(
        game.completion().is_some(),
        "brute-forcing every pair must eventually win"
    );
}
