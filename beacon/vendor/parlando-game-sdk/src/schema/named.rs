//! Projects named MessagePack input into the layout declared by a game.
//! Object entries borrow their original bytes so fields can be written in declaration order.

use super::{EnumRepresentation, Field, Schema, Variant, VariantPayload};
use crate::wire::{Decoder, Token};
use anyhow::{bail, ensure, Context, Result};
use serde::{
    ser::{Error, SerializeMap, SerializeTuple},
    Serialize, Serializer,
};
use std::collections::BTreeMap;

/// Transcodes a named action or configuration without constructing a JSON tree.
pub(crate) fn encode_named(bytes: &[u8], schema: &Schema) -> Result<Vec<u8>, String> {
    crate::encode(&Named { bytes, schema })
}

/// References an existing encoded value and the layout expected by its consumer.
struct Named<'a> {
    bytes: &'a [u8],
    schema: &'a Schema,
}

type Object<'a> = BTreeMap<&'a str, &'a [u8]>;

/// Splits one map into borrowed keys and bounded slices of their corresponding values.
fn object(bytes: &[u8]) -> Result<Object<'_>> {
    let mut d = Decoder::new(bytes);
    let Token::Map(n) = d.next()? else {
        bail!("expected a named object")
    };
    d.check_container(n, 0, 2)?;
    let mut result = Object::new();
    for _ in 0..n {
        let token = d.marker()?;
        let key = d.string(token)?;
        ensure!(
            result.insert(key, d.value_bytes()?).is_none(),
            "duplicate input key {key:?}"
        );
    }
    ensure!(d.is_finished(), "trailing named object data");
    Ok(result)
}

/// Returns the one scalar in an input field, rejecting trailing data.
fn scalar(bytes: &[u8]) -> Result<Token<'_>> {
    let mut d = Decoder::new(bytes);
    let token = d.next()?;
    ensure!(d.is_finished(), "expected a single scalar");
    Ok(token)
}

/// Borrows the public variant name without interning or allocating a new string.
fn label(bytes: &[u8]) -> Result<&str> {
    match scalar(bytes)? {
        Token::String(text) => Ok(text),
        _ => bail!("expected a string enum tag"),
    }
}

/// Streams an input array in order, using one repeated item schema or fixed tuple slots.
fn sequence<S: Serializer>(
    s: S,
    bytes: &[u8],
    repeated: Option<&Schema>,
    fixed: &[Schema],
) -> Result<S::Ok, S::Error> {
    let mut d = Decoder::new(bytes);
    let Token::Array(n) = d.next().map_err(S::Error::custom)? else {
        return Err(S::Error::custom("expected an array"));
    };
    d.check_container(n, 0, 1).map_err(S::Error::custom)?;
    if repeated.is_none() && n != fixed.len() {
        return Err(S::Error::custom("incorrect tuple length"));
    }
    let mut seq = s.serialize_tuple(n)?;
    for i in 0..n {
        let bytes = d.value_bytes().map_err(S::Error::custom)?;
        let schema = repeated
            .or_else(|| fixed.get(i))
            .ok_or_else(|| S::Error::custom("incorrect tuple length"))?;
        seq.serialize_element(&Named { bytes, schema })?;
    }
    if !d.is_finished() {
        return Err(S::Error::custom("trailing array data"));
    }
    seq.end()
}

/// Emits exactly the declared fields and permits absent option values.
fn fields<T: SerializeTuple>(
    seq: &mut T,
    mut map: Object<'_>,
    spec: &[Field],
) -> Result<(), T::Error> {
    for field in spec {
        let bytes = match map.remove(field.name.as_str()) {
            Some(value) => value,
            None if matches!(field.schema, Schema::Option(_)) => &[0xc0],
            None => {
                return Err(T::Error::custom(format_args!(
                    "missing component field {:?}",
                    field.name
                )))
            }
        };
        seq.serialize_element(&Named {
            bytes,
            schema: &field.schema,
        })?;
    }
    if let Some(key) = map.keys().next() {
        return Err(T::Error::custom(format_args!(
            "unknown component field {key:?}"
        )));
    }
    Ok(())
}

/// Holds only the source bytes or fields that belong to one selected variant.
enum Body<'a> {
    Absent,
    Value(&'a [u8]),
    Fields(Object<'a>),
}

/// Separates the tag from its payload according to the declared public enum convention.
fn variant<'a>(bytes: &'a [u8], repr: &EnumRepresentation) -> Result<(&'a str, Body<'a>)> {
    match repr {
        EnumRepresentation::External => {
            if let Ok(name) = label(bytes) {
                return Ok((name, Body::Absent));
            }
            let map = object(bytes)?;
            ensure!(map.len() == 1, "external enum requires one variant");
            let (key, data) = map.into_iter().next().unwrap();
            Ok((key, Body::Value(data)))
        }
        EnumRepresentation::Internal { tag } => {
            let mut map = object(bytes)?;
            let name = label(map.remove(tag.as_str()).context("missing enum tag")?)?;
            Ok((name, Body::Fields(map)))
        }
        EnumRepresentation::Adjacent { tag, content } => {
            let mut map = object(bytes)?;
            let name = label(map.remove(tag.as_str()).context("missing enum tag")?)?;
            let body = map
                .remove(content.as_str())
                .map_or(Body::Absent, Body::Value);
            ensure!(map.is_empty(), "unexpected adjacent enum field");
            Ok((name, body))
        }
        EnumRepresentation::Untagged => bail!("untagged component enum is unsupported"),
    }
}

/// Writes the wire ordinal followed by all selected variant fields.
fn encode_enum<S: Serializer>(
    s: S,
    bytes: &[u8],
    repr: &EnumRepresentation,
    variants: &[Variant],
) -> Result<S::Ok, S::Error> {
    let (tag, body) = variant(bytes, repr).map_err(S::Error::custom)?;
    let (index, v) = variants
        .iter()
        .enumerate()
        .find(|(_, v)| v.name == tag)
        .ok_or_else(|| S::Error::custom(format_args!("unknown component variant {tag:?}")))?;
    if variants
        .iter()
        .all(|v| matches!(v.payload, VariantPayload::Unit))
    {
        if matches!(&body, Body::Absent) || matches!(&body,Body::Fields(m) if m.is_empty()) {
            return s.serialize_u32(index as u32);
        }
        return Err(S::Error::custom("unexpected unit variant payload"));
    }
    let mut seq = s.serialize_tuple(v.payload.len() + 1)?;
    seq.serialize_element(&(index as u32))?;
    match (&v.payload, body) {
        (VariantPayload::Unit, Body::Absent) => {}
        (VariantPayload::Unit, Body::Fields(m)) if m.is_empty() => {}
        (VariantPayload::Newtype(schema), Body::Value(bytes)) => {
            seq.serialize_element(&Named { bytes, schema })?
        }
        (VariantPayload::Tuple(items), Body::Value(bytes)) => {
            let mut d = Decoder::new(bytes);
            if !matches!(d.next().map_err(S::Error::custom)?,Token::Array(n) if n==items.len()) {
                return Err(S::Error::custom("incorrect enum tuple length"));
            }
            for schema in items {
                seq.serialize_element(&Named {
                    bytes: d.value_bytes().map_err(S::Error::custom)?,
                    schema,
                })?;
            }
            if !d.is_finished() {
                return Err(S::Error::custom("trailing enum tuple data"));
            }
        }
        (VariantPayload::Struct(spec), Body::Value(bytes)) => {
            fields(&mut seq, object(bytes).map_err(S::Error::custom)?, spec)?
        }
        (VariantPayload::Struct(spec), Body::Fields(m)) => fields(&mut seq, m, spec)?,
        _ => return Err(S::Error::custom("incorrect component variant payload")),
    }
    seq.end()
}

impl Serialize for Named<'_> {
    /// Writes primitive values directly and reorders object members only when declared.
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self.schema {
            Schema::Dynamic => crate::serde_bridge::serialize(self.bytes, s),
            Schema::Option(_) if self.bytes == [0xc0] => s.serialize_none(),
            Schema::Option(schema) => Named {
                bytes: self.bytes,
                schema,
            }
            .serialize(s),
            Schema::List(schema) => sequence(s, self.bytes, Some(schema), &[]),
            Schema::Tuple(items) => sequence(s, self.bytes, None, items),
            Schema::Struct(spec) => {
                let mut seq = s.serialize_tuple(spec.len())?;
                fields(
                    &mut seq,
                    object(self.bytes).map_err(S::Error::custom)?,
                    spec,
                )?;
                seq.end()
            }
            Schema::Map(schema) => {
                let items = object(self.bytes).map_err(S::Error::custom)?;
                let mut map = s.serialize_map(Some(items.len()))?;
                for (key, bytes) in items {
                    map.serialize_entry(key, &Named { bytes, schema })?;
                }
                map.end()
            }
            Schema::Enum {
                representation,
                variants,
            } => encode_enum(s, self.bytes, representation, variants),
            schema => match (schema, scalar(self.bytes).map_err(S::Error::custom)?) {
                (Schema::Null, Token::Null) => s.serialize_unit(),
                (Schema::Bool, Token::Bool(v)) => s.serialize_bool(v),
                (Schema::String, Token::String(v)) => s.serialize_str(v),
                (Schema::I64, Token::Signed(v)) => s.serialize_i64(v),
                (Schema::I64, Token::Unsigned(v)) => {
                    s.serialize_i64(v.try_into().map_err(S::Error::custom)?)
                }
                (Schema::U64, Token::Unsigned(v)) => s.serialize_u64(v),
                (Schema::U64, Token::Signed(v)) => {
                    s.serialize_u64(v.try_into().map_err(S::Error::custom)?)
                }
                (Schema::F64, Token::Float(v)) => s.serialize_f64(v),
                (Schema::F64, Token::Signed(v)) => s.serialize_f64(v as f64),
                (Schema::F64, Token::Unsigned(v)) => s.serialize_f64(v as f64),
                _ => Err(S::Error::custom(
                    "named value does not match its declared type",
                )),
            },
        }
    }
}
