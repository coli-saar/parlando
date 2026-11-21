//! Exercises wire invariants on derived types and custom Serde callbacks.

use serde::ser::{SerializeMap, SerializeSeq, SerializeStruct};

use super::*;
use crate::wire::validate;

/// Emits the input as a sequence with no advertised length.
struct UnknownLength<T>(T);

impl<T: Serialize> Serialize for UnknownLength<T> {
    /// Streams each input item through the unknown-length Serde API.
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(None)?;
        seq.serialize_element(&self.0)?;
        seq.end()
    }
}

/// Serializes an explicit field sequence, including repeated names when supplied.
struct NamedFields<'a>(&'a [&'static str]);

impl Serialize for NamedFields<'_> {
    /// Streams field names through the static Serde structure interface.
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut object = serializer.serialize_struct("NamedFields", self.0.len())?;
        for &key in self.0 {
            object.serialize_field(key, &())?;
        }
        object.end()
    }
}

/// Exposes the same field sequence through the dynamic map interface.
struct MapEntries<'a>(&'a [&'static str]);

impl Serialize for MapEntries<'_> {
    /// Writes each requested key through ordinary Serde key serialization.
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for key in self.0 {
            map.serialize_entry(key, &())?;
        }
        map.end()
    }
}

/// Checks exact equality for both bitmap collisions and separately stored strings.
#[test]
fn checks_static_key_contents_on_bitmap_collision() {
    static LEFT: [u8; 3] = *b"aba";
    static RIGHT: [u8; 3] = *b"aba";
    let first = std::str::from_utf8(&LEFT).unwrap();
    let duplicate = std::str::from_utf8(&RIGHT).unwrap();
    assert!(!std::ptr::eq(first, duplicate));

    // "aba" and "aca" share their length, first byte and last byte.
    assert!(validate(&encode(&NamedFields(&[first, "aca"])).unwrap()).is_ok());
    assert!(encode(&NamedFields(&[first, "aca", duplicate]))
        .unwrap_err()
        .contains("duplicate"));
}

/// Verifies the exact-key fallback for large static and dynamic objects.
#[test]
fn checks_duplicates_in_large_objects() {
    let mut keys = [
        "a0", "a1", "a2", "a3", "a4", "a5", "a6", "a7", "a8", "a9", "b0", "b1", "b2", "b3", "b4",
        "b5", "b6", "b7", "b8", "b9", "c0", "c1", "c2", "c3", "c4", "c5", "c6", "c7", "c8", "c9",
        "d0", "d1", "d2",
    ];
    assert!(validate(&encode(&NamedFields(&keys)).unwrap()).is_ok());
    assert!(validate(&encode(&MapEntries(&keys)).unwrap()).is_ok());

    keys[32] = keys[0];
    assert!(encode(&NamedFields(&keys))
        .unwrap_err()
        .contains("duplicate"));
    assert!(encode(&MapEntries(&keys))
        .unwrap_err()
        .contains("duplicate"));
}

/// Checks both valid declared lengths when Serde omits a conditional field.
#[test]
fn supports_conditional_static_fields() {
    #[derive(Serialize)]
    struct Conditional {
        first: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        second: Option<bool>,
        third: bool,
    }

    for second in [None, Some(true), None] {
        let object = Conditional {
            first: false,
            second,
            third: false,
        };
        let bytes = encode(&object).unwrap();
        assert_eq!(
            crate::decode::<serde_json::Value>(&bytes).unwrap(),
            serde_json::to_value(&object).unwrap()
        );
    }
}

/// Produces malformed map calls to exercise validation during serialization.
enum Invalid {
    Duplicate,
    NonStringKey,
    MissingValue,
    SwallowedError,
    IncorrectLength,
    OversizedLength,
    Binary,
}

impl Serialize for Invalid {
    /// Exercises cases that a derived Serialize implementation cannot produce.
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Duplicate => {
                let mut map = serializer.serialize_map(None)?;
                for key in ["a", "b", "c", "d", "e", "a"] {
                    map.serialize_entry(key, &())?;
                }
                map.end()
            }
            Self::NonStringKey => {
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry(&123, &())?;
                map.end()
            }
            Self::MissingValue => {
                let mut map = serializer.serialize_map(None)?;
                map.serialize_key("key")?;
                map.end()
            }
            Self::SwallowedError => {
                let mut seq = serializer.serialize_seq(None)?;
                let _ = seq.serialize_element(&f64::NAN);
                seq.end()
            }
            Self::IncorrectLength => serializer.serialize_seq(Some(1))?.end(),
            Self::OversizedLength => serializer.serialize_map(Some(usize::MAX))?.end(),
            Self::Binary => serializer.serialize_bytes(&[1, 2]),
        }
    }
}

/// Verifies that the encoder rejects invalid callback sequences as well as unsupported values.
#[test]
fn rejects_invalid_custom_serializers() {
    for value in [
        Invalid::Duplicate,
        Invalid::NonStringKey,
        Invalid::MissingValue,
        Invalid::SwallowedError,
        Invalid::IncorrectLength,
        Invalid::OversizedLength,
        Invalid::Binary,
    ] {
        assert!(encode(&value).is_err());
    }
    assert!(encode(&f32::INFINITY).is_err());
    assert!(encode(&i128::MAX).is_err());
}

/// Reproduces the three wire limits through typed serialization.
#[test]
fn enforces_wire_limits_during_writes() {
    let bytes = encode(&vec![(); NODE_LIMIT - 1]).unwrap();
    assert!(validate(&bytes).is_ok());
    assert!(encode(&vec![(); NODE_LIMIT]).unwrap_err().contains("node"));

    let bytes = encode(&"x".repeat(BYTE_LIMIT - 5)).unwrap();
    assert_eq!(bytes.len(), BYTE_LIMIT);
    assert!(validate(&bytes).is_ok());
    assert!(encode(&"x".repeat(BYTE_LIMIT - 4))
        .unwrap_err()
        .contains("byte"));

    let mut nested = serde_json::Value::Null;
    for _ in 0..DEPTH_LIMIT {
        nested = serde_json::Value::Array(vec![nested]);
    }
    assert!(validate(&encode(&nested).unwrap()).is_ok());
    assert!(encode(&[nested]).unwrap_err().contains("nesting"));
}

/// Checks that unknown-length output is complete without a validation scan.
#[test]
fn writes_unknown_length_containers() {
    let bytes = encode(&UnknownLength(UnknownLength(true))).unwrap();
    assert!(validate(&bytes).is_ok());
    assert_eq!(
        crate::decode::<Vec<Vec<bool>>>(&bytes).unwrap(),
        vec![vec![true]]
    );

    #[derive(Serialize)]
    struct Flattened {
        field: bool,
        #[serde(flatten)]
        extra: serde_json::Value,
    }

    let value = Flattened {
        field: true,
        extra: serde_json::json!({ "extra": "value" }),
    };
    let bytes = encode(&value).unwrap();
    assert!(validate(&bytes).is_ok());
    assert_eq!(
        crate::decode::<serde_json::Value>(&bytes).unwrap(),
        serde_json::to_value(&value).unwrap()
    );
}
