//! Parlando SDK for Rust. Implement [`Game`] for a game session or
//! [`agent::Agent`] for a software player. Derive [`GameValue`] on the types
//! exchanged with Parlando.

extern crate self as parlando_game_sdk;
pub use parlando_game_derive::GameValue;

/// Serialization traits used by the generated compact writer.
#[doc(hidden)]
pub use serde as __serde;

use std::{borrow::Cow, fmt, sync::Arc};

use serde::{de, Deserialize, Deserializer, Serialize, Serializer};

pub mod agent;
mod encoder;
mod export;
pub mod game;

pub use game::{Game, Rejection, Role};

#[cfg(target_arch = "wasm32")]
#[doc(hidden)]
pub mod __bindings {
    wit_bindgen::generate!({
        path: "wit",
        world: "game",
        pub_export_macro: true,
    });
}

#[cfg(target_arch = "wasm32")]
#[doc(hidden)]
pub mod __session {
    pub use crate::game::{Frame, Session};
}
mod projection;
mod schema;
mod serde_bridge;
mod typed;
mod wire;

pub use projection::{ProjectedValue, Projection, Selection};
#[doc(hidden)]
pub use schema::CompactReader;

/// Result type used by generated compact decoders.
#[doc(hidden)]
pub type CompactDecodeResult<T> = anyhow::Result<T>;
pub use schema::{decode_declared, DecodeGameValue};

pub use schema::{
    decode_compact, decode_with_schema, encode_compact, Compact, EnumRepresentation, Field,
    GameSchema, GameValue, Schema, Variant, VariantPayload,
};

/// An immutable MessagePack value with a named or schema-declared representation.
///
/// Component values accept JSON-shaped data with distinct UTF-8 object keys,
/// finite floats, and integers in the signed or unsigned 64-bit range. A value
/// is limited to 8 MiB of encoded data, 64 nested containers, and 262,144 nodes
/// including object keys. Serde serialization and deserialization expose the
/// value itself, so callers can continue to use ordinary JSON for I/O.
#[derive(Clone)]
pub struct ComponentValue {
    bytes: Arc<Vec<u8>>,
    schema: Option<Arc<Schema>>,
}

impl ComponentValue {
    /// Validates exactly one value, then shares ownership of the supplied buffer.
    pub fn from_messagepack(bytes: Vec<u8>) -> anyhow::Result<Self> {
        wire::validate(&bytes)?;
        Ok(Self {
            bytes: Arc::new(bytes),
            schema: None,
        })
    }

    /// Encodes and validates a typed value in the component's MessagePack format.
    pub fn from_typed<T: Serialize + ?Sized>(value: &T) -> anyhow::Result<Self> {
        Self::from_codec_bytes(encode(value).map_err(anyhow::Error::msg)?)
    }

    /// Adopts a buffer emitted by an in-tree encoder that enforces the component wire format.
    ///
    /// The caller must establish that the buffer contains exactly one JSON-shaped
    /// value with distinct UTF-8 map keys and all component limits applied. This
    /// constructor checks the buffer size in constant time. Use
    /// [`Self::from_messagepack`] at an untrusted byte boundary.
    pub fn from_codec_bytes(bytes: Vec<u8>) -> anyhow::Result<Self> {
        Self::from_schema_bytes_inner(bytes, None)
    }

    /// Adopts a compact buffer and retains its declaration for later named access.
    ///
    /// The declaration must come from a validated `GameSchema`. This checks the
    /// size in constant time. Accessors interpret and check the portions they
    /// read against the declaration.
    pub fn from_schema_bytes(bytes: Vec<u8>, schema: Arc<Schema>) -> anyhow::Result<Self> {
        Self::from_schema_bytes_inner(bytes, Some(schema))
    }

    /// Retains the wire representation after applying constant-time size checks.
    fn from_schema_bytes_inner(
        bytes: Vec<u8>,
        schema: Option<Arc<Schema>>,
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(!bytes.is_empty(), "empty component payload");
        anyhow::ensure!(
            bytes.len() <= wire::BYTE_LIMIT,
            "component payload exceeds byte limit"
        );
        Ok(Self {
            bytes: Arc::new(bytes),
            schema,
        })
    }

    /// Returns the stored wire representation without expanding its declared names.
    pub fn as_messagepack(&self) -> &[u8] {
        &self.bytes
    }

    /// Obtains compact bytes for a config or action described by the target schema.
    ///
    /// A value already using the target representation borrows its wire buffer.
    /// Other representations are transcoded through named MessagePack.
    pub fn to_schema_messagepack(&self, target: &Schema) -> anyhow::Result<Cow<'_, [u8]>> {
        match &self.schema {
            Some(schema) if std::ptr::eq(schema.as_ref(), target) || schema.as_ref() == target => {
                return Ok(Cow::Borrowed(self.as_messagepack()));
            }
            None if matches!(target, Schema::Dynamic) => {
                return Ok(Cow::Borrowed(self.as_messagepack()))
            }
            _ => {}
        }
        let named = match &self.schema {
            Some(_) => Cow::Owned(encode(self).map_err(anyhow::Error::msg)?),
            None => Cow::Borrowed(self.as_messagepack()),
        };
        if matches!(target, Schema::Dynamic) {
            Ok(named)
        } else {
            schema::encode_named(&named, target)
                .map(Cow::Owned)
                .map_err(anyhow::Error::msg)
        }
    }

    /// Deserializes the value into the requested type directly from MessagePack.
    ///
    /// The result can borrow strings from this value when its type supports
    /// borrowed Serde deserialization.
    pub fn decode<'a, T: Deserialize<'a>>(&'a self) -> anyhow::Result<T> {
        match &self.schema {
            Some(schema) => schema::decode_named(self.as_messagepack(), schema),
            None => decode(self.as_messagepack()).map_err(anyhow::Error::msg),
        }
    }
}

impl fmt::Debug for ComponentValue {
    /// Summarizes storage without printing potentially private game data.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ComponentValue")
            .field("bytes", &self.bytes.len())
            .finish()
    }
}

impl Serialize for ComponentValue {
    /// Streams the validated value into the target serializer.
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match &self.schema {
            Some(schema) => schema::serialize_named(self.as_messagepack(), schema, serializer),
            None => serde_bridge::serialize(self.as_messagepack(), serializer),
        }
    }
}

impl<'de> Deserialize<'de> for ComponentValue {
    /// Transcodes a JSON-shaped Serde value into validated MessagePack.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let bytes = serde_bridge::deserialize(deserializer)?;
        Self::from_codec_bytes(bytes).map_err(de::Error::custom)
    }
}

/// Decodes a host-supplied, validated MessagePack payload into a typed guest value.
///
/// Use [`ComponentValue::from_messagepack`] first for untrusted bytes. This
/// helper lets a guest read values its host has already validated, including
/// borrowing string fields from the input buffer.
pub fn decode<'a, T: Deserialize<'a>>(bytes: &'a [u8]) -> Result<T, String> {
    let mut decoder = rmp_serde::Deserializer::from_read_ref(bytes).with_human_readable();
    T::deserialize(&mut decoder).map_err(|err| err.to_string())
}

/// Encodes a typed game value for a MessagePack field in the component ABI.
///
/// Use a typed value with the same Serde attributes as its JSON
/// representation. Structs become maps with named fields and types that
/// distinguish human-readable formats use their human-readable representation.
/// The runtime accepts JSON-shaped values: string-keyed maps, arrays, strings,
/// booleans, null, finite floats and integers representable by `i64` or `u64`.
/// Use string map keys and a sequence of integers for byte arrays. Rust
/// `i128` and `u128` fields require a custom representation. For a
/// custom representation tied specifically to JSON (for example, a unit
/// struct serialized as null), pass an explicit JSON value. Serialization
/// failures and wire limit violations are returned as text for use with the
/// guest's WIT result.
pub fn encode<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>, String> {
    typed::encode(value)
}

/// Encodes a typed role observation for a component frame.
pub use encode as encode_observation;

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Serialize, Serializer};

    /// Uses different wire representations depending on the serializer mode.
    struct HumanReadable;

    impl Serialize for HumanReadable {
        /// Selects the representation used by JSON when the encoder requests it.
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            if serializer.is_human_readable() {
                serializer.serialize_str("readable")
            } else {
                serializer.serialize_u8(1)
            }
        }
    }

    #[derive(Serialize)]
    enum Variant {
        Unit,
        Newtype(u8),
        Tuple(u8, bool),
        Struct { field: u8 },
    }

    /// Verifies typed fields, Serde field attributes, and externally tagged variants.
    #[test]
    fn encodes_named_human_readable_views() {
        #[derive(Serialize)]
        struct Observation {
            #[serde(rename = "gameRole")]
            role: HumanReadable,
            #[serde(skip_serializing_if = "Option::is_none")]
            optional: Option<bool>,
            variants: [Variant; 4],
        }

        let observation = Observation {
            role: HumanReadable,
            optional: None,
            variants: [
                Variant::Unit,
                Variant::Newtype(1),
                Variant::Tuple(2, false),
                Variant::Struct { field: 3 },
            ],
        };
        let bytes = encode_observation(&observation).unwrap();
        let mut prior = Vec::new();
        observation
            .serialize(
                &mut rmp_serde::Serializer::new(&mut prior)
                    .with_struct_map()
                    .with_human_readable(),
            )
            .unwrap();
        assert_eq!(bytes, prior);
        let decoded: serde_json::Value = rmp_serde::from_slice(&bytes).unwrap();
        assert_eq!(decoded, serde_json::to_value(&observation).unwrap());
    }

    /// Confirms borrowed typed reads and shared immutable bytes after cloning.
    #[test]
    fn decodes_borrowed_types_from_shared_bytes() {
        #[derive(Debug, Deserialize, PartialEq, Serialize)]
        struct View<'a> {
            role: &'a str,
            counts: [u64; 2],
        }

        let expected = View {
            role: "A",
            counts: [1, 0],
        };
        let payload = ComponentValue::from_typed(&expected).unwrap();
        let shared = payload.clone();
        assert!(std::ptr::eq(
            payload.as_messagepack(),
            shared.as_messagepack()
        ));
        assert_eq!(shared.decode::<View<'_>>().unwrap(), expected);
        assert_eq!(
            decode::<View<'_>>(shared.as_messagepack()).unwrap(),
            expected
        );
    }

    /// Checks raw ingress validation and bounds the reader if a codec violates its contract.
    #[test]
    fn bounds_codec_receipt_and_its_readers() {
        assert!(ComponentValue::from_codec_bytes(vec![]).is_err());
        assert!(ComponentValue::from_codec_bytes(vec![0; wire::BYTE_LIMIT + 1]).is_err());
        for bytes in [
            vec![0xdd, 0xff, 0xff, 0xff, 0xff],
            [vec![0x91; wire::DEPTH_LIMIT + 1], vec![0xc0]].concat(),
        ] {
            assert!(ComponentValue::from_messagepack(bytes.clone()).is_err());
            let received = ComponentValue::from_codec_bytes(bytes).unwrap();
            assert!(serde_json::to_value(received).is_err());
        }
    }

    /// Confirms that JSON I/O exposes regular values and never embeds wire bytes.
    #[test]
    fn transcodes_json_without_changing_its_shape() {
        let json = serde_json::json!({
            "array": [null, true, 0.25, -7, i64::MIN, u64::MAX, "é"],
            "object": {"nested": {}},
        });
        let encoded = ComponentValue::from_typed(&json).unwrap();
        assert_eq!(serde_json::to_value(&encoded).unwrap(), json);

        let restored: ComponentValue = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(restored.decode::<serde_json::Value>().unwrap(), json);
        assert_eq!(serde_json::to_value(restored).unwrap(), json);
        assert!(serde_json::from_str::<ComponentValue>(r#"{"x":0,"x":1}"#).is_err());
    }
}
