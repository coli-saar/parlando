//! Declares the JSON meaning of compact MessagePack payloads.

use std::{
    borrow::Cow,
    collections::{BTreeMap, HashMap, HashSet},
    sync::Arc,
};

use anyhow::{ensure, Result};
use serde::{de::DeserializeOwned, ser::SerializeSeq, Deserialize, Serialize, Serializer};

use crate::wire::{BYTE_LIMIT, DEPTH_LIMIT, NODE_LIMIT};

mod direct;
mod named;
mod read;
mod transcode;

pub use direct::{decode_declared, CompactReader, DecodeGameValue};

pub(crate) use named::encode_named;
pub(crate) use read::{decode_named, serialize_named};

/// Describes the public JSON shape of one v4 component payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum Schema {
    /// Uses the ordinary JSON-shaped MessagePack representation.
    Dynamic,
    /// Represents JSON null.
    Null,
    /// Represents a Boolean.
    Bool,
    /// Represents a signed integer.
    I64,
    /// Represents an unsigned integer.
    U64,
    /// Represents a finite floating-point number.
    F64,
    /// Represents a UTF-8 string.
    String,
    /// Represents either null or the supplied shape.
    Option(Box<Schema>),
    /// Represents an array of homogeneously typed items.
    List(Box<Schema>),
    /// Represents an array with an ordered, fixed element schema.
    Tuple(Vec<Schema>),
    /// Represents a map with arbitrary distinct string keys and uniform values.
    Map(Box<Schema>),
    /// Represents a JSON object whose declared keys occupy fixed array positions on the wire.
    Struct(Vec<Field>),
    /// Represents a tagged enum whose discriminator is an ordinal in compact MessagePack.
    Enum {
        /// Determines how the tag and payload are represented in public JSON.
        representation: EnumRepresentation,
        /// Variants in compact wire order, starting at ordinal zero.
        variants: Vec<Variant>,
    },
}

/// One positional field in a compact struct or enum variant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Field {
    /// Public JSON property name.
    pub name: String,
    /// Schema used for the positional field value.
    pub schema: Schema,
    /// Omits a null option from the public object while preserving its compact slot.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub omit_none: bool,
}

/// One named variant with its compact payload layout.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Variant {
    /// Tag name that appears in public JSON.
    pub name: String,
    /// Fields placed after the ordinal for a payload-bearing enum.
    pub payload: VariantPayload,
}

/// Declares the public JSON tagging convention for an enum.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum EnumRepresentation {
    /// Emits a unit variant as a string or a payload variant as a one-property object.
    External,
    /// Places the tag beside the variant's named fields.
    Internal {
        /// Public name of the tag property.
        tag: String,
    },
    /// Places the tag and the variant payload into distinct properties.
    Adjacent {
        /// Public name of the tag property.
        tag: String,
        /// Public name of the payload property.
        content: String,
    },
    /// Describes the unsupported Serde representation whose variant is ambiguous at ingress.
    Untagged,
}

/// Describes the public and compact shape of one enum variant's payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum VariantPayload {
    /// The variant carries no fields.
    Unit,
    /// One unnamed value occupies a single compact slot.
    Newtype(Box<Schema>),
    /// Positional values occupy adjacent compact slots.
    Tuple(Vec<Schema>),
    /// Named values occupy adjacent compact slots.
    Struct(Vec<Field>),
}

impl VariantPayload {
    /// Returns how many slots follow the compact discriminant.
    pub(crate) fn len(&self) -> usize {
        match self {
            Self::Unit => 0,
            Self::Newtype(_) => 1,
            Self::Tuple(fields) => fields.len(),
            Self::Struct(fields) => fields.len(),
        }
    }
}

impl Schema {
    /// Checks the schema tree before accepting an independently authored declaration.
    pub fn validate(&self) -> Result<()> {
        let mut remaining = NODE_LIMIT;
        self.check(0, &mut remaining)
    }

    /// Checks recursive shapes and charges them against the declaration budget.
    fn check(&self, depth: usize, remaining: &mut usize) -> Result<()> {
        ensure!(
            depth <= DEPTH_LIMIT && *remaining > 0,
            "component schema exceeds complexity limit"
        );
        *remaining -= 1;
        match self {
            Self::Option(inner) | Self::List(inner) | Self::Map(inner) => {
                inner.check(depth + 1, remaining)?
            }
            Self::Tuple(items) => {
                for item in items {
                    item.check(depth + 1, remaining)?;
                }
            }
            Self::Struct(fields) => check_fields(fields, None, depth, remaining)?,
            Self::Enum {
                representation,
                variants,
            } => {
                ensure!(
                    !variants.is_empty(),
                    "component schema enum has no variants"
                );
                let reserved = match representation {
                    EnumRepresentation::Internal { tag } => Some(tag.as_str()),
                    EnumRepresentation::Adjacent { tag, content } => {
                        ensure!(
                            tag != content,
                            "component schema enum tag and content names coincide"
                        );
                        None
                    }
                    EnumRepresentation::External => None,
                    EnumRepresentation::Untagged => {
                        anyhow::bail!("untagged component schema enum is not supported")
                    }
                };
                let mut names = HashSet::new();
                for variant in variants {
                    ensure!(*remaining > 0, "component schema exceeds complexity limit");
                    *remaining -= 1;
                    ensure!(
                        names.insert(variant.name.as_str()),
                        "duplicate component schema variant {}",
                        variant.name
                    );
                    match &variant.payload {
                        VariantPayload::Unit => {}
                        VariantPayload::Struct(fields) => {
                            check_fields(fields, reserved, depth, remaining)?
                        }
                        VariantPayload::Newtype(item) => {
                            ensure!(
                                reserved.is_none(),
                                "internally tagged component enum requires a named payload"
                            );
                            item.check(depth + 1, remaining)?;
                        }
                        VariantPayload::Tuple(items) => {
                            ensure!(
                                reserved.is_none(),
                                "internally tagged component enum requires a named payload"
                            );
                            for item in items {
                                item.check(depth + 1, remaining)?;
                            }
                        }
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }
}

/// Checks named fields and prevents collisions with an internal enum tag.
fn check_fields(
    fields: &[Field],
    reserved: Option<&str>,
    depth: usize,
    remaining: &mut usize,
) -> Result<()> {
    let mut names = HashSet::new();
    for field in fields {
        ensure!(
            reserved != Some(field.name.as_str()) && names.insert(field.name.as_str()),
            "duplicate component schema field {}",
            field.name
        );
        ensure!(
            !field.omit_none || matches!(field.schema, Schema::Option(_)),
            "omit_none requires an optional field"
        );
        field.schema.check(depth + 1, remaining)?;
    }
    Ok(())
}

/// The complete v4 schema returned once by a guest's `describe` export.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GameSchema {
    /// Runtime configuration sent to initialization.
    pub config: Arc<Schema>,
    /// Actions sent to the component.
    pub action: Arc<Schema>,
    /// Observations returned for participant A.
    pub view_a: Arc<Schema>,
    /// Observations returned for participant B.
    pub view_b: Arc<Schema>,
    /// Optional completion returned with a terminal frame.
    pub completion: Arc<Schema>,
    /// Optional transition metadata returned with a frame.
    pub metadata: Arc<Schema>,
}

impl GameSchema {
    /// Constructs the six payload declarations returned by a v4 guest.
    pub fn new(
        config: Schema,
        action: Schema,
        view_a: Schema,
        view_b: Schema,
        completion: Schema,
        metadata: Schema,
    ) -> Self {
        Self {
            config: Arc::new(config),
            action: Arc::new(action),
            view_a: Arc::new(view_a),
            view_b: Arc::new(view_b),
            completion: Arc::new(completion),
            metadata: Arc::new(metadata),
        }
    }

    /// Parses the bounded UTF-8 JSON declaration and validates all payload schemas.
    pub fn from_description(bytes: &[u8]) -> Result<Self> {
        ensure!(
            bytes.len() <= BYTE_LIMIT,
            "component schema exceeds byte limit"
        );
        let schema: Self = serde_json::from_slice(bytes)?;
        schema.check()?;
        Ok(schema)
    }

    /// Serializes and validates the JSON string returned by the guest export.
    pub fn to_description(&self) -> Result<String, String> {
        self.check().map_err(|error| error.to_string())?;
        let text = serde_json::to_string(self).map_err(|error| error.to_string())?;
        if text.len() > BYTE_LIMIT {
            return Err("component schema exceeds byte limit".to_owned());
        }
        Ok(text)
    }

    /// Checks all trees against one declaration-wide complexity budget.
    fn check(&self) -> Result<()> {
        let mut remaining = NODE_LIMIT;
        for schema in [
            &self.config,
            &self.action,
            &self.view_a,
            &self.view_b,
            &self.completion,
            &self.metadata,
        ] {
            schema.check(0, &mut remaining)?;
        }
        Ok(())
    }
}

/// Connects a Rust type's declaration to the compact positional representation it emits.
///
/// Implementations used by a guest must ensure their declaration describes every
/// value they encode. Derive the trait for a game-facing struct or enum so its
/// layout and public field names come from the same Rust declaration.
pub trait GameValue {
    /// Returns the public declaration corresponding to the compact representation.
    fn schema() -> Schema;

    /// Serializes the value with positional structs and ordinal enum discriminants.
    fn encode_compact<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error>;
}

/// Adapts a borrowed component wire type for Serde's compound serializers.
pub struct Compact<'a, T: ?Sized>(pub &'a T);

impl<T: GameValue + ?Sized> Serialize for Compact<'_, T> {
    /// Emits the compact representation rather than the type's ordinary Serde shape.
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.encode_compact(serializer)
    }
}

/// Encodes a guest-produced value in the compact representation declared by the type.
pub fn encode_compact<T: GameValue + ?Sized>(value: &T) -> Result<Vec<u8>, String> {
    crate::encode(&Compact(value))
}

/// Deserializes a compact config or action into the game's normal Serde type.
///
/// Use [`decode_with_schema`] with a cached declaration when decoding
/// repeatedly or when the output borrows strings.
pub fn decode_compact<T: GameValue + DeserializeOwned>(bytes: &[u8]) -> Result<T, String> {
    decode_named(bytes, &T::schema()).map_err(|error| error.to_string())
}

/// Deserializes compact bytes with an existing declaration, borrowing strings when possible.
pub fn decode_with_schema<'de, T: Deserialize<'de>>(
    schema: &'de Schema,
    bytes: &'de [u8],
) -> Result<T, String> {
    decode_named(bytes, schema).map_err(|error| error.to_string())
}

/// Implements wire descriptors for primitive types with native Serde representations.
macro_rules! primitive {
    ($($ty:ty => $schema:ident),* $(,)?) => {$(
        impl GameValue for $ty {
            /// Declares the primitive's JSON shape.
            fn schema() -> Schema {
                Schema::$schema
            }

            /// Retains the primitive's ordinary wire representation.
            fn encode_compact<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                self.serialize(serializer)
            }
        }
    )*};
}

primitive! {
    () => Null, bool => Bool, i8 => I64, i16 => I64, i32 => I64, i64 => I64, isize => I64,
    u8 => U64, u16 => U64, u32 => U64, u64 => U64, usize => U64, f32 => F64, f64 => F64,
    str => String, String => String, char => String, Cow<'_, str> => String, serde_json::Value => Dynamic,
}

impl<T: GameValue + ?Sized> GameValue for &T {
    /// Describes the referenced type.
    fn schema() -> Schema {
        T::schema()
    }

    /// Delegates to the referenced value.
    fn encode_compact<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        T::encode_compact(self, serializer)
    }
}

impl<T: GameValue> GameValue for Option<T> {
    /// Declares the nullable inner shape.
    fn schema() -> Schema {
        Schema::Option(Box::new(T::schema()))
    }

    /// Emits a nil marker or the inner value.
    fn encode_compact<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Some(item) => serializer.serialize_some(&Compact(item)),
            None => serializer.serialize_none(),
        }
    }
}

impl<T: GameValue> GameValue for [T] {
    /// Declares the repeated element shape.
    fn schema() -> Schema {
        Schema::List(Box::new(T::schema()))
    }

    /// Streams the elements into a compact array.
    fn encode_compact<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut sequence = serializer.serialize_seq(Some(self.len()))?;
        for item in self {
            sequence.serialize_element(&Compact(item))?;
        }
        sequence.end()
    }
}

impl<T: GameValue> GameValue for Vec<T> {
    /// Declares the repeated element shape.
    fn schema() -> Schema {
        <[T]>::schema()
    }

    /// Serializes the backing slice without copying it.
    fn encode_compact<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.as_slice().encode_compact(serializer)
    }
}

impl<T: GameValue, const N: usize> GameValue for [T; N] {
    /// Declares a fixed tuple containing the common element shape.
    fn schema() -> Schema {
        Schema::Tuple((0..N).map(|_| T::schema()).collect())
    }

    /// Serializes the array without copying it.
    fn encode_compact<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.as_slice().encode_compact(serializer)
    }
}

/// Implements string-keyed component dictionaries for the standard map types.
macro_rules! string_map {
    ($map:ident) => {
        impl<T: GameValue> GameValue for $map<String, T> {
            /// Declares the common shape of all dictionary values.
            fn schema() -> Schema {
                Schema::Map(Box::new(T::schema()))
            }

            /// Streams each key and its compact value as a map entry.
            fn encode_compact<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                use serde::ser::SerializeMap;
                let mut map = serializer.serialize_map(Some(self.len()))?;
                for (key, value) in self {
                    map.serialize_entry(key, &Compact(value))?;
                }
                map.end()
            }
        }
    };
}

string_map!(BTreeMap);
string_map!(HashMap);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ComponentValue;

    /// Supplies names, omission behavior and primitive inheritance from one Rust type.
    #[derive(Debug, PartialEq, Serialize, Deserialize, crate::GameValue)]
    struct Projection<'a> {
        #[serde(rename = "playerRole")]
        role: &'a str,
        counts: [u64; 2],
        #[serde(skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    }

    /// Checks that one schema serves the compact producer and ordinary named consumers.
    #[test]
    fn projects_compact_payloads_without_losing_named_shape() {
        #[derive(Debug, PartialEq, Deserialize)]
        struct Role<'a> {
            #[serde(rename = "playerRole")]
            role: &'a str,
        }

        let input = Projection {
            role: "red",
            counts: [1, 2],
            note: None,
        };
        let schema = Arc::new(Projection::schema());
        let compact = encode_compact(&input).unwrap();
        assert_eq!(compact, [0x93, 0xa3, b'r', b'e', b'd', 0x92, 1, 2, 0xc0]);
        let value = ComponentValue::from_schema_bytes(compact, Arc::clone(&schema)).unwrap();
        assert_eq!(
            serde_json::to_value(&value).unwrap(),
            serde_json::to_value(&input).unwrap()
        );

        let actual: Role<'_> = value.decode().unwrap();
        assert_eq!(actual, Role { role: "red" });
        assert_eq!(actual.role.as_ptr(), value.as_messagepack()[2..].as_ptr());

        let named = ComponentValue::from_typed(&input).unwrap();
        let converted = named.to_schema_messagepack(&schema).unwrap();
        assert_eq!(converted.as_ref(), value.as_messagepack());
        assert!(matches!(
            value.to_schema_messagepack(&schema).unwrap(),
            Cow::Borrowed(_)
        ));
    }

    /// Exercises unit-only, externally tagged, internally tagged and adjacent enums.
    #[test]
    fn decodes_the_declared_enum_representation() {
        #[derive(Debug, PartialEq, Serialize, Deserialize, crate::GameValue)]
        #[serde(rename_all = "snake_case")]
        enum Unit {
            TopFloor,
            Basement,
        }

        #[derive(Debug, PartialEq, Serialize, Deserialize, crate::GameValue)]
        enum External {
            Unit,
            Tuple(u8, bool),
            Struct { text: String },
            Newtype(String),
        }

        #[derive(Debug, PartialEq, Serialize, Deserialize, crate::GameValue)]
        #[serde(tag = "type", rename_all = "snake_case")]
        enum Internal {
            Wait,
            MoveTo { floor: u8 },
        }

        #[derive(Debug, PartialEq, Serialize, Deserialize, crate::GameValue)]
        #[serde(tag = "type", content = "data", rename_all = "snake_case")]
        enum Adjacent {
            Wait,
            Label(String),
            Detail(u8, bool),
        }

        /// Compares generated bytes, typed output, and both JSON representations.
        fn roundtrip<T: GameValue + Serialize + DeserializeOwned + std::fmt::Debug + PartialEq>(
            values: &[T],
        ) {
            let schema = Arc::new(T::schema());
            for input in values {
                let compact = encode_compact(input).unwrap();
                let named = ComponentValue::from_typed(input).unwrap();
                assert_eq!(
                    named.to_schema_messagepack(&schema).unwrap().as_ref(),
                    compact.as_slice()
                );
                assert_eq!(&decode_compact::<T>(&compact).unwrap(), input);
                let received =
                    ComponentValue::from_schema_bytes(compact, Arc::clone(&schema)).unwrap();
                assert_eq!(
                    serde_json::to_value(received).unwrap(),
                    serde_json::to_value(input).unwrap()
                );
            }
        }

        roundtrip(&[Unit::TopFloor, Unit::Basement]);
        roundtrip(&[
            External::Unit,
            External::Tuple(2, true),
            External::Struct { text: "a".into() },
            External::Newtype("b".into()),
        ]);
        roundtrip(&[Internal::Wait, Internal::MoveTo { floor: 7 }]);
        roundtrip(&[
            Adjacent::Wait,
            Adjacent::Label("B".into()),
            Adjacent::Detail(1, false),
        ]);
    }

    /// Stops repeated declared text from creating an unbounded named projection.
    #[test]
    fn bounds_synthetic_projection_text() {
        let schema = Schema::List(Box::new(Schema::Struct(vec![Field {
            name: "x".repeat(BYTE_LIMIT / 2 + 1),
            schema: Schema::Null,
            omit_none: false,
        }])));
        let bytes = vec![0x92, 0x91, 0xc0, 0x91, 0xc0];
        let value = ComponentValue::from_schema_bytes(bytes, Arc::new(schema)).unwrap();
        assert!(value
            .decode::<serde_json::Value>()
            .unwrap_err()
            .to_string()
            .contains("expansion limit"));
        assert!(serde_json::to_value(&value)
            .unwrap_err()
            .to_string()
            .contains("expansion limit"));
    }

    /// Skips unused scalar contents but continues to reject malformed wire structure.
    #[test]
    fn structurally_skips_unrequested_fields() {
        #[derive(Debug, Deserialize, PartialEq)]
        struct Selected {
            accepted: bool,
        }

        let schema = Arc::new(Schema::Struct(vec![
            Field {
                name: "ignored".into(),
                schema: Schema::String,
                omit_none: false,
            },
            Field {
                name: "accepted".into(),
                schema: Schema::Bool,
                omit_none: false,
            },
        ]));
        let invalid_utf8 = vec![0x92, 0xa1, 0xff, 0xc3];
        assert!(ComponentValue::from_messagepack(invalid_utf8.clone()).is_err());
        let projected =
            ComponentValue::from_schema_bytes(invalid_utf8, Arc::clone(&schema)).unwrap();
        assert_eq!(
            projected.decode::<Selected>().unwrap(),
            Selected { accepted: true }
        );
        assert!(serde_json::to_value(&projected).is_err());
        for broken in [vec![0x92, 0xa3, 0x61, 0xc3], vec![0x92, 0xc1, 0xc3]] {
            let value = ComponentValue::from_schema_bytes(broken, Arc::clone(&schema)).unwrap();
            assert!(value.decode::<Selected>().is_err());
        }
    }

    /// Rejects inconsistent declarations and malformed compact payloads when read.
    #[test]
    fn validates_declarations_and_compact_access() {
        let field = Field {
            name: "x".into(),
            schema: Schema::String,
            omit_none: false,
        };
        let make = |view| {
            GameSchema::new(
                Schema::Dynamic,
                Schema::Dynamic,
                view,
                Schema::Dynamic,
                Schema::Null,
                Schema::Null,
            )
        };
        let game = make(Schema::Struct(vec![field.clone()]));
        assert_eq!(
            GameSchema::from_description(game.to_description().unwrap().as_bytes()).unwrap(),
            game
        );
        assert!(make(Schema::Struct(vec![field.clone(), field]))
            .to_description()
            .is_err());

        for bytes in [vec![0x90], vec![0x91, 0xc0], vec![0x91, 0xa0, 0xc0]] {
            let value = ComponentValue::from_schema_bytes(bytes, Arc::clone(&game.view_a)).unwrap();
            assert!(value.decode::<serde_json::Value>().is_err());
            assert!(serde_json::to_value(&value).is_err());
        }
    }
}
