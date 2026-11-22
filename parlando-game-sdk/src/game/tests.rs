use super::*;
use crate::Schema;

/// Receives a display label and initializes a seeded count.
#[derive(GameValue)]
struct Config {
    label: String,
}

/// Adds a count on behalf of participant B.
#[derive(GameValue)]
struct Add(u64);

/// Lets A see the count and borrow the configured label.
#[derive(GameValue, Debug, PartialEq)]
struct AView<'a> {
    label: &'a str,
    count: u64,
}

/// Retains private session data without imposing Clone or Serde on the game.
struct Counter {
    config: Config,
    count: u64,
    last_amount: Option<u64>,
}

impl Game for Counter {
    type Config = Config;
    type Action = Add;
    type ViewA<'a> = AView<'a>;
    type ViewB<'a> = u64;
    type Completion<'a> = &'a str;
    type Metadata<'a> = u64;

    /// Starts with the recorded seed and no accepted move.
    fn init(config: Config, seed: u64) -> Result<Self, String> {
        Ok(Self {
            config,
            count: seed,
            last_amount: None,
        })
    }

    /// Rejects moves from A and records successful increments from B.
    fn step(&mut self, actor: Role, Add(amount): Add) -> Result<(), Rejection> {
        if actor == Role::A {
            return Err(Rejection::new("wrong_role"));
        }
        self.count += amount;
        self.last_amount = Some(amount);
        Ok(())
    }

    /// Exposes the label and count to A.
    fn view_a(&self) -> AView<'_> {
        AView {
            label: &self.config.label,
            count: self.count,
        }
    }

    /// Exposes the count to B.
    fn view_b(&self) -> u64 {
        self.count
    }

    /// Publishes the label when the count reaches its terminal value.
    fn completion(&self) -> Option<&str> {
        (self.count >= 10).then_some(self.config.label.as_str())
    }

    /// Publishes the increase made by the current action.
    fn metadata(&self) -> Option<u64> {
        self.last_amount
    }

    /// Writes an opaque snapshot of the count and label.
    fn checkpoint(&self) -> Option<Vec<u8>> {
        let mut bytes = self.count.to_le_bytes().to_vec();
        bytes.extend(self.config.label.as_bytes());
        Some(bytes)
    }

    /// Reconstructs the game using a compatible snapshot.
    fn restore(bytes: &[u8]) -> Result<Self, String> {
        let (count, label) = bytes.split_at_checked(8).ok_or("invalid checkpoint")?;
        let count = u64::from_le_bytes(count.try_into().unwrap());
        let label = String::from_utf8(label.to_vec()).map_err(|error| error.to_string())?;
        Self::init(Config { label }, count)
    }
}

/// Ensures describes and projections match each role's schema.
#[test]
fn emits_borrowed_and_distinct_views() {
    let schema = GameSchema::from_description(Session::<Counter>::describe().as_bytes()).unwrap();
    assert_eq!(*schema.config, Config::schema());
    assert_eq!(*schema.action, Add::schema());
    assert_eq!(*schema.view_a, AView::schema());
    assert_eq!(*schema.view_b, Schema::U64);
    let config = encode_compact(&Config {
        label: "seeded".into(),
    })
    .unwrap();
    let frame = Session::<Counter>::new().init(&config, 7).unwrap();
    assert_eq!(decode_declared::<u64>(&frame.view_b).unwrap(), 7);
    assert_eq!(
        decode_declared::<AView>(&frame.view_a).unwrap(),
        AView {
            label: "seeded",
            count: 7
        }
    );
    assert!(frame.completion.is_none());
    assert!(frame.metadata.is_none());
}

/// Checks that malformed input and rule rejections do not advance a restored game.
#[test]
fn restores_a_checkpoint_and_rejects_before_capturing_frame() {
    let config = encode_compact(&Config {
        label: "end".into(),
    })
    .unwrap();
    let checkpoint = Session::<Counter>::new()
        .init(&config, 7)
        .unwrap()
        .checkpoint
        .unwrap();
    let mut restored = Session::<Counter>::new();
    assert!(restored.restore(&[0]).is_err());
    restored.restore(&checkpoint).unwrap();

    let action = encode_compact(&Add(3)).unwrap();
    assert_eq!(restored.step(Role::A, &action).err().unwrap(), "wrong_role");
    assert_eq!(
        restored.step(Role::B, &[0xc0]).err().unwrap(),
        "invalid_component_action"
    );

    let frame = restored.step(Role::B, &action).unwrap();
    assert_eq!(decode_declared::<u64>(&frame.view_b).unwrap(), 10);
    assert_eq!(
        decode_declared::<&str>(&frame.completion.unwrap()).unwrap(),
        "end"
    );
    assert_eq!(decode_declared::<u64>(&frame.metadata.unwrap()).unwrap(), 3);
}

/// Detects ABI drift between the authoring SDK and the host in a checkout.
#[test]
fn packaged_wit_matches_the_host() {
    let host = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../rust-server");
    if host.join("Cargo.toml").exists() {
        for (path, sdk_bytes) in [
            ("game.wit", include_bytes!("../../wit/game.wit").as_slice()),
            (
                "agent/agent.wit",
                include_bytes!("../../wit/agent/agent.wit").as_slice(),
            ),
        ] {
            assert_eq!(
                std::fs::read(host.join("wit").join(path)).unwrap(),
                sdk_bytes,
                "{path}"
            );
        }
    }
}
