use super::ids::{LimbId, RootId};
use super::types::GreatTreeState;

pub fn limb_for_root(state: &GreatTreeState, root: RootId) -> LimbId {
    LimbId::ALL
        .iter()
        .copied()
        .find(|&limb| state.bijection[limb.index()] == root)
        .expect("bijection is a total function from LimbId, so every RootId has a preimage")
}

pub fn sun(state: &GreatTreeState, limb: LimbId) -> bool {
    state.sun_holds.contains(&limb)
}

pub fn water(state: &GreatTreeState, limb: LimbId) -> bool {
    state.limb_water[limb.index()]
}

pub fn flowered(state: &GreatTreeState, limb: LimbId) -> bool {
    sun(state, limb) && water(state, limb)
}

pub fn thawed(state: &GreatTreeState, root: RootId) -> bool {
    sun(state, limb_for_root(state, root))
}

/// Invariant maintained by `apply_set_sun`/`apply_set_flow`: `water_holds` only ever contains
/// currently-thawed roots, so membership alone is sufficient here.
pub fn running(state: &GreatTreeState, root: RootId) -> bool {
    state.water_holds.contains(&root)
}

/// Crown's `SetSun` action. Infallible: setting sun is never rejected by a domain rule (only
/// role/authorization can reject it, which is the adapter's job, not this pure function's).
///
/// - Setting `lit: true` on a limb already held is a no-op.
/// - Setting `lit: true` past the 3-limb cap evicts the oldest hold (spec rule 2). The evicted
///   limb's paired root re-freezes immediately and, if it was running, force-closes — spec
///   rule 3's "the water stops immediately" — and the evicted limb's own `limb_water` clears
///   too, so the limb fully dries out, not just its root. Sun is the limb's only water supply
///   (a root can only ever be opened while its warming limb currently has sun, per rule 3), so
///   once sun leaves there is no way for that water to still be "live"; treating it as gone is
///   the simpler, more intuitive behaviour and is what removes the flower/bark colour together.
/// - Setting `lit: false` removes an explicit hold early, with the same cascade.
///
/// Note: this makes the "watered, dark" appearance (water present, no sun) unreachable in
/// normal play for now — water can only ever be added while sun is already present, and now
/// sun leaving always takes the water with it. The appearance itself (`LimbAppearance::WetDark`
/// client-side) is intentionally left in place rather than removed, in case this gets
/// revisited.
pub fn apply_set_sun(state: &GreatTreeState, limb: LimbId, lit: bool) -> GreatTreeState {
    let mut next = state.clone();
    if lit {
        if next.sun_holds.contains(&limb) {
            return next;
        }
        if next.sun_holds.len() == 3 {
            let evicted = next.sun_holds.remove(0);
            force_close_paired_root(&mut next, evicted);
        }
        next.sun_holds.push(limb);
    } else {
        if !next.sun_holds.contains(&limb) {
            return next;
        }
        next.sun_holds.retain(|&held| held != limb);
        force_close_paired_root(&mut next, limb);
    }
    next
}

/// Shared by both eviction and explicit `lit: false`: the limb's paired root can no longer be
/// thawed, so it must stop running right away. Root's own held-set loses that slot too — an
/// iced root cannot count against Root's 3-root cap, matching the vocab's "Iced: clicking the
/// gate does nothing" (there is no fourth "open but frozen" visual state). The limb's own
/// `limb_water` clears too: losing sun dries the limb out completely, not just its root.
fn force_close_paired_root(state: &mut GreatTreeState, limb: LimbId) {
    let root = state.bijection[limb.index()];
    state.water_holds.retain(|&held| held != root);
    state.limb_water[limb.index()] = false;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlowRejection {
    RootFrozen,
}

/// Root's `SetFlow` action.
///
/// - Opening a root that is not currently thawed is rejected (spec rule 3) — this is the one
///   domain-rule rejection in the whole game.
/// - Opening a root already held is a no-op.
/// - Opening past the 3-root cap evicts the oldest hold (spec rule 2). Unlike the `SetSun`
///   eviction cascade, THIS eviction is Root's own resource decision (automatic, but still
///   Root choosing to move on) and DOES clear the evicted root's fed limb's `limb_water` latch
///   — exactly as if Root had explicitly closed that gate.
/// - Closing an open root (`open: false`) is the same "Root's own choice" case: clears the fed
///   limb's `limb_water` latch immediately.
pub fn apply_set_flow(
    state: &GreatTreeState,
    root: RootId,
    open: bool,
) -> Result<GreatTreeState, FlowRejection> {
    let mut next = state.clone();
    if open {
        if next.water_holds.contains(&root) {
            return Ok(next);
        }
        if !thawed(&next, root) {
            return Err(FlowRejection::RootFrozen);
        }
        if next.water_holds.len() == 3 {
            let evicted_root = next.water_holds.remove(0);
            let evicted_limb = limb_for_root(&next, evicted_root);
            next.limb_water[evicted_limb.index()] = false;
        }
        next.water_holds.push(root);
        let limb = limb_for_root(&next, root);
        next.limb_water[limb.index()] = true;
    } else {
        if !next.water_holds.contains(&root) {
            return Ok(next);
        }
        next.water_holds.retain(|&held| held != root);
        let limb = limb_for_root(&next, root);
        next.limb_water[limb.index()] = false;
    }
    Ok(next)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_state() -> GreatTreeState {
        GreatTreeState {
            crown_seat: super::super::types::CrownSeat::A,
            sun_holds: vec![],
            water_holds: vec![],
            limb_water: [false; 5],
            bijection: [
                RootId::Hand,
                RootId::Knot,
                RootId::Tip,
                RootId::Swollen,
                RootId::Deep,
            ],
        }
    }

    #[test]
    fn setting_sun_true_adds_a_hold_below_the_cap() {
        let state = empty_state();
        let next = apply_set_sun(&state, LimbId::Spire, true);
        assert!(sun(&next, LimbId::Spire));
        assert_eq!(next.sun_holds.len(), 1);
    }

    #[test]
    fn setting_sun_true_on_an_already_held_limb_is_a_no_op() {
        let state = apply_set_sun(&empty_state(), LimbId::Spire, true);
        let next = apply_set_sun(&state, LimbId::Spire, true);
        assert_eq!(next.sun_holds, vec![LimbId::Spire]);
    }

    #[test]
    fn a_fourth_sun_hold_evicts_the_oldest() {
        let mut state = empty_state();
        state = apply_set_sun(&state, LimbId::Spire, true);
        state = apply_set_sun(&state, LimbId::Hook, true);
        state = apply_set_sun(&state, LimbId::Fork, true);
        let next = apply_set_sun(&state, LimbId::Cradle, true);
        assert!(!sun(&next, LimbId::Spire), "oldest hold must be evicted");
        assert!(sun(&next, LimbId::Hook));
        assert!(sun(&next, LimbId::Fork));
        assert!(sun(&next, LimbId::Cradle));
        assert_eq!(next.sun_holds.len(), 3);
    }

    #[test]
    fn explicit_sun_false_removes_an_early_hold() {
        let state = apply_set_sun(&empty_state(), LimbId::Spire, true);
        let next = apply_set_sun(&state, LimbId::Spire, false);
        assert!(!sun(&next, LimbId::Spire));
        assert!(next.sun_holds.is_empty());
    }

    #[test]
    fn sun_false_on_a_limb_without_a_hold_is_a_no_op() {
        let state = empty_state();
        let next = apply_set_sun(&state, LimbId::Spire, false);
        assert_eq!(next.sun_holds, state.sun_holds);
    }

    #[test]
    fn losing_sun_force_closes_the_paired_root_and_dries_the_limb() {
        let mut state = empty_state();
        state = apply_set_sun(&state, LimbId::Spire, true);
        // Spire warms and is fed by RootId::Hand under this fixture's bijection.
        state.water_holds.push(RootId::Hand);
        state.limb_water[LimbId::Spire.index()] = true;

        let next = apply_set_sun(&state, LimbId::Spire, false);

        assert!(
            !next.water_holds.contains(&RootId::Hand),
            "root must re-freeze and close"
        );
        assert!(
            !water(&next, LimbId::Spire),
            "losing sun must dry the limb out completely"
        );
        assert!(
            !flowered(&next, LimbId::Spire),
            "sun&&water is false, so it must have shriveled"
        );
    }

    #[test]
    fn eviction_triggers_the_same_root_closing_cascade_as_explicit_false() {
        let mut state = empty_state();
        state = apply_set_sun(&state, LimbId::Spire, true);
        state.water_holds.push(RootId::Hand);
        state.limb_water[LimbId::Spire.index()] = true;
        state = apply_set_sun(&state, LimbId::Hook, true);
        state = apply_set_sun(&state, LimbId::Fork, true);

        let next = apply_set_sun(&state, LimbId::Cradle, true);

        assert!(!next.sun_holds.contains(&LimbId::Spire));
        assert!(!next.water_holds.contains(&RootId::Hand));
        assert!(
            !water(&next, LimbId::Spire),
            "eviction must dry the limb out, same as explicit false"
        );
    }

    #[test]
    fn thawed_reflects_whether_the_warming_limb_currently_holds_sun() {
        let state = apply_set_sun(&empty_state(), LimbId::Hook, true);
        // Hook warms RootId::Knot under this fixture's bijection.
        assert!(thawed(&state, RootId::Knot));
        assert!(!thawed(&state, RootId::Hand));
    }

    #[test]
    fn opening_a_frozen_root_is_rejected() {
        let state = empty_state(); // nothing thawed
        let result = apply_set_flow(&state, RootId::Hand, true);
        assert_eq!(result, Err(FlowRejection::RootFrozen));
    }

    #[test]
    fn opening_a_thawed_root_sets_water_holds_and_the_fed_limbs_water_latch() {
        let state = apply_set_sun(&empty_state(), LimbId::Spire, true);
        let next = apply_set_flow(&state, RootId::Hand, true).unwrap();
        assert!(running(&next, RootId::Hand));
        assert!(water(&next, LimbId::Spire));
        assert!(flowered(&next, LimbId::Spire));
    }

    #[test]
    fn opening_an_already_open_root_is_a_no_op() {
        let state = apply_set_sun(&empty_state(), LimbId::Spire, true);
        let state = apply_set_flow(&state, RootId::Hand, true).unwrap();
        let next = apply_set_flow(&state, RootId::Hand, true).unwrap();
        assert_eq!(next.water_holds, vec![RootId::Hand]);
    }

    #[test]
    fn a_fourth_water_hold_evicts_the_oldest_and_dries_out_its_fed_limb() {
        let mut state = empty_state();
        state = apply_set_sun(&state, LimbId::Fork, true);
        state = apply_set_sun(&state, LimbId::Cradle, true);
        state = apply_set_sun(&state, LimbId::Spire, true); // now [Fork, Cradle, Spire] hold sun
        state = apply_set_flow(&state, RootId::Tip, true).unwrap(); // fed by Fork
        state = apply_set_flow(&state, RootId::Swollen, true).unwrap(); // fed by Cradle
        state = apply_set_flow(&state, RootId::Hand, true).unwrap(); // fed by Spire

        let next = apply_set_flow(&state, RootId::Knot, true); // Hook is not currently sunlit
        assert_eq!(
            next,
            Err(FlowRejection::RootFrozen),
            "Knot's warming limb (Hook) has no sun"
        );
    }

    #[test]
    fn closing_an_open_root_explicitly_dries_out_its_fed_limb() {
        let state = apply_set_sun(&empty_state(), LimbId::Spire, true);
        let state = apply_set_flow(&state, RootId::Hand, true).unwrap();
        let next = apply_set_flow(&state, RootId::Hand, false).unwrap();
        assert!(!running(&next, RootId::Hand));
        assert!(
            !water(&next, LimbId::Spire),
            "explicit close must clear the limb water latch"
        );
    }

    #[test]
    fn closing_a_root_that_is_not_open_is_a_no_op() {
        let state = empty_state();
        let next = apply_set_flow(&state, RootId::Hand, false).unwrap();
        assert_eq!(next.water_holds, state.water_holds);
    }
}
