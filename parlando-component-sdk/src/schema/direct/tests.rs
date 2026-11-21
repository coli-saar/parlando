use serde::{Deserialize, Serialize};

use crate::{decode_declared, decode_with_schema, encode_compact, ComponentDecode, ComponentWire};

#[derive(Debug, PartialEq, Serialize, Deserialize, ComponentWire, ComponentDecode)]
struct Unit;

#[derive(Debug, PartialEq, Serialize, Deserialize, ComponentWire, ComponentDecode)]
struct Pair<T>(i8, T);

#[derive(Debug, PartialEq, Serialize, Deserialize, ComponentWire, ComponentDecode)]
struct Newtype<T>(T);

#[derive(Debug, PartialEq, Serialize, Deserialize, ComponentWire, ComponentDecode)]
#[serde(rename_all = "snake_case")]
enum Ordinal {
    StreetLevel,
    Roof,
}

#[derive(Debug, PartialEq, Serialize, Deserialize, ComponentWire, ComponentDecode)]
enum External {
    Unit,
    Tuple(u8, bool),
    Struct { value: [u16; 2] },
    Newtype(String),
}

#[derive(Debug, PartialEq, Serialize, Deserialize, ComponentWire, ComponentDecode)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
enum Adjacent {
    Unit,
    Tuple(char, u64),
    Struct { item: Option<Newtype<i64>> },
    Newtype(serde_json::Value),
}

#[derive(Debug, PartialEq, Serialize, Deserialize, ComponentWire, ComponentDecode)]
#[serde(tag = "type", rename_all = "camelCase")]
enum Internal<'a> {
    Wait,
    MoveTo {
        #[serde(rename = "floorLabel")]
        floor: &'a str,
        #[serde(borrow)]
        labels: Vec<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        next: Option<u8>,
    },
}

/// Compares generated decoding with both the original value and the schema adapter.
fn roundtrip<'de, T>(schema: &'de crate::Schema, bytes: &'de [u8], expected: &T)
where
    T: ComponentDecode<'de> + Deserialize<'de> + std::fmt::Debug + PartialEq,
{
    assert_eq!(&decode_declared::<T>(bytes).unwrap(), expected);
    assert_eq!(&decode_with_schema::<T>(schema, bytes).unwrap(), expected);
}

/// Covers borrowed and nested records, computed bounds, and every supported enum shape.
#[test]
fn matches_schema_decoding_for_declared_shapes() {
    macro_rules! parity {
        ($($value:expr),* $(,)?) => {$(
            let expected = $value;
            let bytes = encode_compact(&expected).unwrap();
            let schema = declaration(&expected);
            roundtrip(&schema, &bytes, &expected);
        )*};
    }

    /// Obtains the declaration with the type inferred from the expected value.
    fn declaration<T: ComponentWire>(_: &T) -> crate::Schema {
        T::schema()
    }

    parity!(
        Unit,
        Pair(-128, 171u8),
        Newtype(true),
        Ordinal::Roof,
        External::Unit,
        External::Tuple(4, true),
        External::Struct { value: [1, 1_000] },
        External::Newtype("text".to_owned()),
        Adjacent::Unit,
        Adjacent::Tuple('é', u64::MAX),
        Adjacent::Struct {
            item: Some(Newtype(i64::MIN))
        },
        Adjacent::Newtype(serde_json::json!({"a": [1, null]})),
        Internal::Wait,
        Internal::MoveTo {
            floor: "A",
            labels: vec!["B"],
            next: None
        },
        std::collections::BTreeMap::from([("key".to_owned(), vec![1u32, 2, 3])]),
        std::collections::HashMap::from([("key".to_owned(), 1.25f32)]),
    );
}

/// Verifies borrowed fields point into the compact input buffer.
#[test]
fn borrows_string_slots() {
    let expected = Internal::MoveTo {
        floor: "first",
        labels: vec!["second"],
        next: Some(3),
    };
    let bytes = encode_compact(&expected).unwrap();
    let Internal::MoveTo { floor, labels, .. } = decode_declared(&bytes).unwrap() else {
        panic!("expected MoveTo");
    };
    for text in [floor, labels[0]] {
        assert!(bytes.as_ptr_range().contains(&text.as_ptr()));
    }
}

/// Rejects malformed wire shapes, scalar ranges, and data beyond the root value.
#[test]
fn rejects_malformed_slots() {
    for bytes in [
        &[][..],
        &[0xc0],
        &[0x90],
        &[0x91, 0x04],
        &[0x91, 0xff],
        &[0x91, 0xcf, 1, 0, 0, 0, 0, 0, 0, 0],
        &[0x91, 0x01],
        &[0x92, 0x01, 0x01],
        &[0x93, 0x01, 0xcc, 0x01],
        &[0x93, 0x01, 0xcd, 0x01, 0x00, 0xc3],
        &[0x93, 0x01, 0xff, 0xc3],
        &[0x93, 0x01, 0x02, 0x01],
        &[0x92, 0x03, 0xa1, 0xff],
        &[0x92, 0x03, 0xa1, 0x41, 0xc0],
    ] {
        assert!(decode_declared::<External>(bytes).is_err(), "{bytes:x?}");
    }
    assert!(decode_declared::<Ordinal>(&[0x91, 0x00]).is_err());
    assert!(decode_declared::<[u8; 2]>(&[0x91, 0x02]).is_err());
    assert!(decode_declared::<f32>(&[0xca, 0x7f, 0x80, 0, 0]).is_err());
    assert!(decode_declared::<char>(&[0xa2, b'a', b'b']).is_err());
    let duplicates = &[0x82, 0xa1, b'a', 0x01, 0xa1, b'a', 0x02];
    assert!(decode_declared::<std::collections::HashMap<String, u8>>(duplicates).is_err());
    assert!(decode_declared::<serde_json::Value>(duplicates).is_err());
}

/// Checks byte and node ceilings and preserves physical nesting across dynamic slots.
#[test]
fn rejects_resource_limit_violations() {
    use crate::wire::{BYTE_LIMIT, DEPTH_LIMIT, NODE_LIMIT};

    assert!(decode_declared::<()>(&vec![0xc0; BYTE_LIMIT + 1]).is_err());
    let mut too_many_nodes = vec![0xdd];
    too_many_nodes.extend((NODE_LIMIT as u32).to_be_bytes());
    too_many_nodes.extend(vec![0xc0; NODE_LIMIT]);
    assert!(decode_declared::<Vec<()>>(&too_many_nodes).is_err());

    let mut nested = vec![0x91; DEPTH_LIMIT];
    nested.push(0xc0);
    assert!(decode_declared::<Vec<serde_json::Value>>(&nested).is_ok());
    nested.insert(0, 0x91);
    assert!(decode_declared::<Vec<serde_json::Value>>(&nested).is_err());
}
