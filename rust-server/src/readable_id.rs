use rand::seq::SliceRandom;

const ADVERBS: &str = "
    calmly clearly closely evenly gently gladly lightly neatly openly plainly
    quietly safely softly steadily sweetly truly warmly wisely brightly briskly
    cleanly deeply eagerly fairly freely kindly loosely merrily mildly nearly
    nicely proudly purely quickly rarely smoothly surely tenderly vividly
";

const ADJECTIVES: &str = "
    amber aqua autumn blue bold breezy bright bronze calm cedar cherry clear
    coral cozy crisp dawn dusky fair fern fresh gentle golden green hazel icy
    ivory jade lilac lively lunar mellow mint misty navy nimble olive peach
    pearl pine plum quiet rosy ruby sandy silver sky snowy solar spring sunny
    swift teal tidy velvet violet warm willow winter
";

const PARTICIPANT_NOUNS: &str = "
    alpaca badger beaver bison bobcat butterfly camel caribou cat chamois
    cheetah chickadee chipmunk crane deer dolphin donkey dormouse dove duck
    eagle egret falcon ferret finch fox gazelle gecko giraffe goat goose grouse
    hamster hare hedgehog heron horse hummingbird ibex kingfisher koala lamb
    lark lemur leopard llama lynx magpie marmot meerkat moose mouse otter owl
    panda parrot penguin pika pony puffin rabbit raccoon robin seal sparrow
    squirrel starling stoat stork swallow swan swift tapir thrush turtle vicuna
    wallaby weasel whale wren yak zebra
";

const DIALOGUE_NOUNS: &str = "
    alcove arch bay beacon bridge brook canal canyon cascade cavern clearing
    cloud cove creek dawn delta dune echo field fjord forest garden grove harbor
    haven hill horizon island lagoon lake lantern meadow mesa moon orbit orchard
    path peak pond prairie quay reef ridge river shore sky spring star summit
    terrace trail valley vista waterfall wave woodland zenith atlas aurora
    compass comet constellation cosmos current gateway glade inlet lighthouse
    maple meteor nebula oasis ocean passage pine planet rainbow stream sunset
";

/// Generates a succinct three-word random identifier ending in a participant-only animal noun.
pub fn participant_id() -> String {
    generate(PARTICIPANT_NOUNS)
}

/// Generates a succinct three-word random identifier ending in a dialogue-only place or object noun.
pub fn dialogue_id() -> String {
    generate(DIALOGUE_NOUNS)
}

/// Generates one readable identifier from the shared modifiers and supplied noun namespace.
fn generate(nouns: &str) -> String {
    let mut rng = rand::thread_rng();
    let adverbs = ADVERBS.split_whitespace().collect::<Vec<_>>();
    let adjectives = ADJECTIVES.split_whitespace().collect::<Vec<_>>();
    let nouns = nouns.split_whitespace().collect::<Vec<_>>();
    format!(
        "{}-{}-{}",
        adverbs
            .choose(&mut rng)
            .expect("adverb list must not be empty"),
        adjectives
            .choose(&mut rng)
            .expect("adjective list must not be empty"),
        nouns.choose(&mut rng).expect("noun list must not be empty")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// Confirms both namespaces are compact, three-part, and distinguished by their nouns.
    #[test]
    fn participant_and_dialogue_names_use_disjoint_noun_sets() {
        let participant_nouns = PARTICIPANT_NOUNS.split_whitespace().collect::<HashSet<_>>();
        let dialogue_nouns = DIALOGUE_NOUNS.split_whitespace().collect::<HashSet<_>>();
        assert!(participant_nouns.is_disjoint(&dialogue_nouns));
        for _ in 0..100 {
            let participant = participant_id();
            let dialogue = dialogue_id();
            assert_eq!(participant.split('-').count(), 3);
            assert_eq!(dialogue.split('-').count(), 3);
            assert!(PARTICIPANT_NOUNS
                .split_whitespace()
                .any(|noun| participant.ends_with(noun)));
            assert!(DIALOGUE_NOUNS
                .split_whitespace()
                .any(|noun| dialogue.ends_with(noun)));
        }
    }
}
