use rand::seq::SliceRandom;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

use super::ids::{LimbId, RootId};

/// Generates the one hidden fact of a session: which limb starts already flowered, and the
/// single bijection pairing every limb with the one root that both warms it (spec rule 3) and
/// is fed by it (spec rule 4). Deterministic in `seed` so replay reproduces the same session.
pub fn generate(seed: u64) -> (LimbId, [RootId; 5]) {
    let mut rng = StdRng::seed_from_u64(seed);

    let mut roots = RootId::ALL;
    roots.shuffle(&mut rng);
    // roots[i] is now the root paired with LimbId::ALL[i].

    let starting_index = rng.gen_range(0..LimbId::ALL.len());
    let starting_limb = LimbId::ALL[starting_index];

    (starting_limb, roots)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn same_seed_reproduces_the_same_bijection() {
        let (limb_a, bijection_a) = generate(42);
        let (limb_b, bijection_b) = generate(42);
        assert_eq!(limb_a, limb_b);
        assert_eq!(bijection_a, bijection_b);
    }

    #[test]
    fn bijection_pairs_every_limb_with_a_distinct_root() {
        for seed in 0..50u64 {
            let (_, bijection) = generate(seed);
            let distinct: HashSet<RootId> = bijection.iter().copied().collect();
            assert_eq!(
                distinct.len(),
                5,
                "seed {seed} produced a bijection that repeats a root: {bijection:?}"
            );
        }
    }

    #[test]
    fn starting_limb_is_always_one_of_the_five() {
        for seed in 0..50u64 {
            let (limb, _) = generate(seed);
            assert!(LimbId::ALL.contains(&limb));
        }
    }

    #[test]
    fn different_seeds_eventually_produce_different_bijections() {
        let outcomes: HashSet<(LimbId, [RootId; 5])> = (0..20u64).map(generate).collect();
        assert!(
            outcomes.len() > 1,
            "20 different seeds all produced the exact same session — RNG wiring is likely broken"
        );
    }
}
