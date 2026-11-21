//! Exercises the Rust derive through the same schema interface used by independent games.

use parlando_component_sdk::{
    decode_compact, encode_compact, ComponentValue, ComponentWire, GameSchema, Schema,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::{collections::BTreeMap, fmt::Debug, sync::Arc};

#[derive(ComponentWire, Serialize, Deserialize, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
struct Turn {
    current_player: String,
    action: Action,
    extras: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    hint: Option<String>,
}

#[derive(ComponentWire, Serialize, Deserialize, PartialEq, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Action {
    Wait,
    MoveTo { destination: String },
}

#[derive(ComponentWire, Serialize, Deserialize, PartialEq, Debug)]
#[serde(tag = "kind", content = "data", rename_all = "kebab-case")]
enum Adjacent {
    NewRound,
    Value(i64),
    Pair(String, bool),
    Record { points: u64 },
}

#[derive(ComponentWire, Serialize, Deserialize, PartialEq, Debug)]
enum External {
    Unit,
    #[serde(rename = "measure")]
    Tuple(f64, u64),
    Wrapped(Adjacent),
    Score {
        value: i64,
    },
}

/// Verifies compact, named, typed, and external views of the same declared Rust value.
fn roundtrip<T: ComponentWire + Serialize + DeserializeOwned + PartialEq + Debug>(v: T) {
    let schema = Arc::new(T::schema());
    let bytes = encode_compact(&v).unwrap();
    let received = ComponentValue::from_schema_bytes(bytes.clone(), schema.clone()).unwrap();
    assert_eq!(received.decode::<T>().unwrap(), v);
    assert_eq!(decode_compact::<T>(&bytes).unwrap(), v);
    let public = serde_json::to_value(&v).unwrap();
    assert_eq!(serde_json::to_value(&received).unwrap(), public);
    let from_external: ComponentValue = serde_json::from_value(public).unwrap();
    assert_eq!(
        &*from_external.to_schema_messagepack(&schema).unwrap(),
        bytes
    );
}

/// Exercises payload tags, omitted fields, nested values and unrestricted map keys.
#[test]
fn generated_writers_agree_with_the_generic_host() {
    for action in [
        Action::Wait,
        Action::MoveTo {
            destination: "réserve".into(),
        },
    ] {
        roundtrip(Turn {
            current_player: "A".into(),
            action,
            extras: BTreeMap::from([("free text key".into(), "not interned".into())]),
            hint: None,
        });
    }
    roundtrip(Turn {
        current_player: "B".into(),
        action: Action::Wait,
        extras: BTreeMap::new(),
        hint: Some("a hint".into()),
    });
    for value in [
        Adjacent::NewRound,
        Adjacent::Value(-14),
        Adjacent::Pair("X".into(), true),
        Adjacent::Record { points: 7 },
    ] {
        roundtrip(External::Wrapped(value));
    }
    roundtrip(External::Tuple(1.5, 3));
    roundtrip(External::Unit);
    roundtrip(External::Score { value: -9 });
}

/// A policy receives both program-owned field labels and a borrow into the received payload.
#[test]
fn partial_named_reader_borrows_ordinary_text_from_compact_frame() {
    #[derive(Deserialize)]
    struct Name<'a> {
        #[serde(rename = "currentPlayer")]
        player: &'a str,
    }
    let value = Turn {
        current_player: "player A".into(),
        extras: BTreeMap::new(),
        hint: None,
        action: Action::Wait,
    };
    let bytes = encode_compact(&value).unwrap();
    let received = ComponentValue::from_schema_bytes(bytes, Turn::schema().into()).unwrap();
    let result: Name = received.decode().unwrap();
    assert_eq!(result.player, "player A");
    assert!(received
        .as_messagepack()
        .as_ptr_range()
        .contains(&result.player.as_ptr()));
}

/// The serialized descriptor is sufficient to interpret a payload without the guest type.
#[test]
fn parsed_description_interprets_external_values() {
    let schema = GameSchema::new(
        Schema::Dynamic,
        Action::schema(),
        Turn::schema(),
        Schema::String,
        Schema::U64,
        Schema::Dynamic,
    );
    let parsed = GameSchema::from_description(schema.to_description().unwrap().as_bytes()).unwrap();
    assert_eq!(parsed, schema);
    let action =
        ComponentValue::from_typed(&serde_json::json!({"destination":"up","type":"move_to"}))
            .unwrap();
    assert_eq!(
        &*action.to_schema_messagepack(&parsed.action).unwrap(),
        &[0x92, 1, 0xa2, b'u', b'p']
    );
}
