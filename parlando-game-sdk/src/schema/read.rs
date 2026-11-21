//! Presents compact positional data as a borrowed, named Serde value on demand.

use std::{
    cell::{Cell, RefCell},
    collections::HashSet,
};

use anyhow::{ensure, Result};
use serde::{
    de::{
        self,
        value::{BorrowedStrDeserializer, Error},
        DeserializeSeed, EnumAccess, MapAccess, SeqAccess, VariantAccess, Visitor,
    },
    forward_to_deserialize_any, Deserialize, Deserializer,
};

use crate::wire::{Decoder, Token, BYTE_LIMIT};

use super::{transcode, EnumRepresentation, Field, Schema, Variant, VariantPayload};

/// Reads one compact value into a regular Serde type, borrowing strings when requested.
pub(crate) fn decode_named<'de, T: Deserialize<'de>>(
    bytes: &'de [u8],
    schema: &'de Schema,
) -> Result<T> {
    ensure!(
        bytes.len() <= BYTE_LIMIT,
        "component payload exceeds byte limit"
    );
    let decoder = RefCell::new(Decoder::new(bytes));
    let synthetic_bytes = Cell::new(BYTE_LIMIT);
    let value = T::deserialize(Read {
        decoder: &decoder,
        synthetic_bytes: &synthetic_bytes,
        shape: Shape::Declared(schema),
        depth: 0,
    })?;
    ensure!(decoder.borrow().is_finished(), "trailing MessagePack bytes");
    Ok(value)
}

/// Decodes a dynamic slot while preserving the parent's physical node and depth budgets.
pub(crate) fn decode_dynamic<'de, T: Deserialize<'de>>(
    decoder: &mut Decoder<'de>,
    depth: usize,
) -> Result<T> {
    let cursor = RefCell::new(std::mem::replace(decoder, Decoder::new(&[])));
    let value = T::deserialize(Read {
        decoder: &cursor,
        synthetic_bytes: &Cell::new(BYTE_LIMIT),
        shape: Shape::Declared(&Schema::Dynamic),
        depth,
    });
    *decoder = cursor.into_inner();
    Ok(value?)
}

/// Writes the named form to the destination serializer without building a value tree.
pub(crate) fn serialize_named<S: serde::Serializer>(
    bytes: &[u8],
    schema: &Schema,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    use serde::ser::Error as _;
    if bytes.len() > BYTE_LIMIT {
        return Err(S::Error::custom("component payload exceeds byte limit"));
    }
    let decoder = RefCell::new(Decoder::new(bytes));
    let synthetic_bytes = Cell::new(BYTE_LIMIT);
    let output = transcode::serialize(
        Read {
            decoder: &decoder,
            synthetic_bytes: &synthetic_bytes,
            shape: Shape::Declared(schema),
            depth: 0,
        },
        serializer,
    )?;
    if !decoder.borrow().is_finished() {
        return Err(S::Error::custom("trailing MessagePack bytes"));
    }
    Ok(output)
}

/// Describes an actual wire node or a virtual JSON node implied by the declaration.
#[derive(Clone, Copy)]
enum Shape<'de> {
    Declared(&'de Schema),
    Text(&'de str),
    Payload(&'de VariantPayload),
}

/// Holds an independent view of the schema and a shared sequential wire cursor.
#[derive(Clone, Copy)]
struct Read<'cursor, 'de> {
    decoder: &'cursor RefCell<Decoder<'de>>,
    synthetic_bytes: &'cursor Cell<usize>,
    shape: Shape<'de>,
    depth: usize,
}

impl<'de> Read<'_, 'de> {
    /// Reuses the cursor at the next nested wire level.
    fn child(self, shape: Shape<'de>) -> Self {
        Self {
            shape,
            depth: self.depth + 1,
            ..self
        }
    }

    /// Reuses the cursor at the current wire level for an option or synthetic wrapper.
    fn with_shape(self, shape: Shape<'de>) -> Self {
        Self { shape, ..self }
    }

    /// Exposes a newtype payload directly to typed Serde entry points.
    fn unwrap_payload(self) -> Self {
        match self.shape {
            Shape::Payload(VariantPayload::Newtype(schema)) => {
                self.with_shape(Shape::Declared(schema))
            }
            _ => self,
        }
    }

    /// Charges declared text that would otherwise expand beyond the physical byte limit.
    #[inline]
    fn charge_text(self, text: &str) -> Result<(), Error> {
        let remaining = self
            .synthetic_bytes
            .get()
            .checked_sub(text.len())
            .ok_or_else(|| {
                de::Error::custom("component output exceeds declared text expansion limit")
            })?;
        self.synthetic_bytes.set(remaining);
        Ok(())
    }

    /// Consumes one token under the shared node budget.
    fn token(self) -> Result<Token<'de>, Error> {
        self.decoder.borrow_mut().next().map_err(de::Error::custom)
    }

    /// Checks depth and minimum resources for an advertised collection.
    fn container(self, count: usize, per_entry: usize) -> Result<(), Error> {
        self.decoder
            .borrow()
            .check_container(count, self.depth, per_entry)
            .map_err(de::Error::custom)
    }

    /// Opens an array whose declared number of slots matches the schema.
    fn fixed_array(self, count: usize) -> Result<(), Error> {
        match self.token()? {
            Token::Array(actual) if actual == count => self.container(actual, 1),
            _ => Err(de::Error::custom(
                "compact array length differs from its component schema",
            )),
        }
    }

    /// Reads a nonnegative enum ordinal and resolves the matching variant.
    fn variant(self, variants: &'de [Variant]) -> Result<&'de Variant, Error> {
        let all_unit = variants
            .iter()
            .all(|variant| matches!(variant.payload, VariantPayload::Unit));
        let token = self.token()?;
        let (ordinal, count) = if all_unit {
            (token, 0)
        } else if let Token::Array(count) = token {
            self.container(count, 1)?;
            if count == 0 {
                return Err(de::Error::custom(
                    "compact enum missing its variant ordinal",
                ));
            }
            (self.token()?, count - 1)
        } else {
            return Err(de::Error::custom("expected a compact enum array"));
        };
        let index = match ordinal {
            Token::Unsigned(index) => usize::try_from(index).ok(),
            Token::Signed(index) => usize::try_from(index).ok(),
            _ => None,
        };
        let variant = index
            .and_then(|index| variants.get(index))
            .ok_or_else(|| de::Error::custom("invalid compact enum ordinal"))?;
        if variant.payload.len() != count {
            return Err(de::Error::custom(
                "compact enum field count differs from its component schema",
            ));
        }
        Ok(variant)
    }

    /// Presents an implicitly nested enum payload as the corresponding public JSON value.
    fn payload<V: Visitor<'de>>(
        self,
        payload: &'de VariantPayload,
        visitor: V,
    ) -> Result<V::Value, Error> {
        match payload {
            VariantPayload::Unit => visitor.visit_unit(),
            VariantPayload::Newtype(schema) => self
                .with_shape(Shape::Declared(schema))
                .deserialize_any(visitor),
            VariantPayload::Tuple(items) => visitor.visit_seq(Sequence {
                read: self,
                items: Items::Fixed(items),
            }),
            VariantPayload::Struct(fields) => {
                visitor.visit_map(Object::new(self, fields, None, None))
            }
        }
    }

    /// Projects the selected variant using the declaration's JSON tagging convention.
    fn enum_any<V: Visitor<'de>>(
        self,
        variant: &'de Variant,
        representation: &'de EnumRepresentation,
        visitor: V,
    ) -> Result<V::Value, Error> {
        let read = self.child(Shape::Payload(&variant.payload));
        match representation {
            EnumRepresentation::External if matches!(variant.payload, VariantPayload::Unit) => {
                read.charge_text(&variant.name)?;
                visitor.visit_borrowed_str(&variant.name)
            }
            EnumRepresentation::External => visitor.visit_map(Object::new(
                read,
                &[],
                Some((&variant.name, read.shape)),
                None,
            )),
            EnumRepresentation::Internal { tag } => {
                let fields = match &variant.payload {
                    VariantPayload::Struct(fields) => fields.as_slice(),
                    VariantPayload::Unit => &[],
                    _ => {
                        return Err(de::Error::custom(
                            "invalid internally tagged component variant",
                        ))
                    }
                };
                visitor.visit_map(Object::new(
                    read,
                    fields,
                    Some((tag, Shape::Text(&variant.name))),
                    None,
                ))
            }
            EnumRepresentation::Adjacent { tag, content } => {
                let payload = (!matches!(variant.payload, VariantPayload::Unit))
                    .then_some((content.as_str(), read.shape));
                visitor.visit_map(Object::new(
                    read,
                    &[],
                    Some((tag, Shape::Text(&variant.name))),
                    payload,
                ))
            }
            EnumRepresentation::Untagged => Err(de::Error::custom(
                "untagged component enum is not supported",
            )),
        }
    }

    /// Streams an ordinary dynamic MessagePack value with the usual JSON constraints.
    fn dynamic<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        match self.token()? {
            Token::Null => visitor.visit_unit(),
            Token::Bool(value) => visitor.visit_bool(value),
            Token::Unsigned(value) => visitor.visit_u64(value),
            Token::Signed(value) => visitor.visit_i64(value),
            Token::Float(value) if value.is_finite() => visitor.visit_f64(value),
            Token::Float(_) => Err(de::Error::custom("nonfinite MessagePack number")),
            Token::String(value) => visitor.visit_borrowed_str(value),
            Token::Array(count) => {
                self.container(count, 1)?;
                visitor.visit_seq(Sequence {
                    read: self.child(Shape::Declared(&Schema::Dynamic)),
                    items: Items::Repeat(&Schema::Dynamic, count),
                })
            }
            Token::Map(count) => {
                self.container(count, 2)?;
                visitor.visit_map(DynamicMap {
                    read: self.child(Shape::Declared(&Schema::Dynamic)),
                    remaining: count,
                    keys: HashSet::new(),
                })
            }
        }
    }

    /// Advances over a synthetic or physical value that a caller did not request.
    fn skip(self) -> Result<(), Error> {
        match self.shape {
            Shape::Text(_) | Shape::Payload(VariantPayload::Unit) => Ok(()),
            Shape::Payload(payload) => {
                for _ in 0..payload.len() {
                    self.decoder
                        .borrow_mut()
                        .skip_bounded(self.depth)
                        .map_err(de::Error::custom)?;
                }
                Ok(())
            }
            Shape::Declared(_) => self
                .decoder
                .borrow_mut()
                .skip_bounded(self.depth)
                .map_err(de::Error::custom),
        }
    }
}

impl<'de> Deserializer<'de> for Read<'_, 'de> {
    type Error = Error;

    /// Reads a physical node or exposes its synthetic public JSON shape.
    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        let mut schema = match self.shape {
            Shape::Text(value) => {
                self.charge_text(value)?;
                return visitor.visit_borrowed_str(value);
            }
            Shape::Payload(payload) => return self.payload(payload, visitor),
            Shape::Declared(schema) => schema,
        };
        while let Schema::Option(inner) = schema {
            if self.decoder.borrow().next_is_nil() {
                self.token()?;
                return visitor.visit_unit();
            }
            schema = inner;
        }
        match schema {
            Schema::Dynamic => self.dynamic(visitor),
            Schema::Null => match self.token()? {
                Token::Null => visitor.visit_unit(),
                _ => Err(de::Error::custom("expected compact null")),
            },
            Schema::Bool => match self.token()? {
                Token::Bool(value) => visitor.visit_bool(value),
                _ => Err(de::Error::custom("expected compact Boolean")),
            },
            Schema::I64 => match self.token()? {
                Token::Signed(value) => visitor.visit_i64(value),
                Token::Unsigned(value) => {
                    visitor.visit_i64(i64::try_from(value).map_err(de::Error::custom)?)
                }
                _ => Err(de::Error::custom("expected a compact signed integer")),
            },
            Schema::U64 => match self.token()? {
                Token::Unsigned(value) => visitor.visit_u64(value),
                Token::Signed(value) => {
                    visitor.visit_u64(u64::try_from(value).map_err(de::Error::custom)?)
                }
                _ => Err(de::Error::custom("expected a compact unsigned integer")),
            },
            Schema::F64 => match self.token()? {
                Token::Float(value) if value.is_finite() => visitor.visit_f64(value),
                _ => Err(de::Error::custom("expected a finite compact float")),
            },
            Schema::String => match self.token()? {
                Token::String(value) => visitor.visit_borrowed_str(value),
                _ => Err(de::Error::custom("expected a compact string")),
            },
            Schema::Option(_) => unreachable!("transparent options were unwrapped"),
            Schema::List(element) => {
                let Token::Array(count) = self.token()? else {
                    return Err(de::Error::custom("expected a compact list"));
                };
                self.container(count, 1)?;
                visitor.visit_seq(Sequence {
                    read: self.child(Shape::Declared(element)),
                    items: Items::Repeat(element, count),
                })
            }
            Schema::Tuple(items) => {
                self.fixed_array(items.len())?;
                visitor.visit_seq(Sequence {
                    read: self.child(self.shape),
                    items: Items::Fixed(items),
                })
            }
            Schema::Map(element) => {
                let Token::Map(count) = self.token()? else {
                    return Err(de::Error::custom("expected a compact map"));
                };
                self.container(count, 2)?;
                visitor.visit_map(DynamicMap {
                    read: self.child(Shape::Declared(element)),
                    remaining: count,
                    keys: HashSet::new(),
                })
            }
            Schema::Struct(fields) => {
                self.fixed_array(fields.len())?;
                visitor.visit_map(Object::new(self.child(self.shape), fields, None, None))
            }
            Schema::Enum {
                representation,
                variants,
            } => self.enum_any(self.variant(variants)?, representation, visitor),
        }
    }

    /// Interprets nil as None and preserves the same wire position for a present value.
    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        let input = self.unwrap_payload();
        if input.decoder.borrow().next_is_nil()
            && matches!(
                input.shape,
                Shape::Declared(Schema::Dynamic | Schema::Option(_) | Schema::Null)
            )
        {
            input.token()?;
            return visitor.visit_none();
        }
        let read = match input.shape {
            Shape::Declared(Schema::Option(inner)) => input.with_shape(Shape::Declared(inner)),
            _ => input,
        };
        visitor.visit_some(read)
    }

    /// Deserializes an ordinary Rust newtype through its underlying wire shape.
    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        _: &'static str,
        visitor: V,
    ) -> Result<V::Value, Error> {
        visitor.visit_newtype_struct(self)
    }

    /// Resolves external enum variants for derived Rust enum visitors.
    fn deserialize_enum<V: Visitor<'de>>(
        self,
        _: &'static str,
        _: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Error> {
        let input = self.unwrap_payload();
        match input.shape {
            Shape::Text(name) => {
                input.charge_text(name)?;
                visitor.visit_enum(BorrowedStrDeserializer::new(name))
            }
            Shape::Declared(Schema::Enum {
                representation: EnumRepresentation::External,
                variants,
            }) => {
                let variant = input.variant(variants)?;
                input.charge_text(&variant.name)?;
                visitor.visit_enum(External {
                    name: &variant.name,
                    payload: input.child(Shape::Payload(&variant.payload)),
                })
            }
            Shape::Declared(Schema::Dynamic) => {
                let (name, shape) = match input.token()? {
                    Token::String(name) => (name, Shape::Payload(&VariantPayload::Unit)),
                    Token::Map(1) => {
                        input.container(1, 2)?;
                        let Token::String(name) = input.token()? else {
                            return Err(de::Error::custom("invalid external enum tag"));
                        };
                        (name, input.shape)
                    }
                    _ => return Err(de::Error::custom("expected an externally tagged enum")),
                };
                visitor.visit_enum(External {
                    name,
                    payload: input.child(shape),
                })
            }
            _ => input.deserialize_any(visitor),
        }
    }

    /// Skips unrequested fields without constructing or expanding JSON values.
    fn deserialize_ignored_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        self.skip()?;
        visitor.visit_unit()
    }

    forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string bytes byte_buf
        unit unit_struct seq tuple tuple_struct map struct identifier
    }
}

/// Provides the expected shape of the next array slot without materializing it.
enum Items<'de> {
    Fixed(&'de [Schema]),
    Repeat(&'de Schema, usize),
}

/// Drives a visitor through positional values under collection limits.
struct Sequence<'cursor, 'de> {
    read: Read<'cursor, 'de>,
    items: Items<'de>,
}

impl<'de> SeqAccess<'de> for Sequence<'_, 'de> {
    type Error = Error;

    /// Reads the next slot under the appropriate element schema.
    fn next_element_seed<T: DeserializeSeed<'de>>(
        &mut self,
        seed: T,
    ) -> Result<Option<T::Value>, Error> {
        let shape = match &mut self.items {
            Items::Fixed(items) => {
                let Some((first, rest)) = items.split_first() else {
                    return Ok(None);
                };
                *items = rest;
                first
            }
            Items::Repeat(shape, remaining) => {
                if *remaining == 0 {
                    return Ok(None);
                }
                *remaining -= 1;
                *shape
            }
        };
        seed.deserialize(self.read.with_shape(Shape::Declared(shape)))
            .map(Some)
    }

    /// Reports the remaining number of positional slots.
    fn size_hint(&self) -> Option<usize> {
        Some(match self.items {
            Items::Fixed(items) => items.len(),
            Items::Repeat(_, len) => len,
        })
    }
}

/// Supplies declared field names without reading any bytes for those names.
struct Object<'cursor, 'de> {
    read: Read<'cursor, 'de>,
    fields: &'de [Field],
    prefix: [Option<(&'de str, Shape<'de>)>; 2],
    current: Option<Shape<'de>>,
}

impl<'cursor, 'de> Object<'cursor, 'de> {
    /// Builds a map projection, optionally adding virtual enum properties ahead of its fields.
    fn new(
        read: Read<'cursor, 'de>,
        fields: &'de [Field],
        first: Option<(&'de str, Shape<'de>)>,
        second: Option<(&'de str, Shape<'de>)>,
    ) -> Self {
        Self {
            read,
            fields,
            prefix: [first, second],
            current: None,
        }
    }

    /// Retrieves the next visible field or virtual property.
    fn entry(&mut self) -> Result<Option<(&'de str, Shape<'de>)>, Error> {
        for prefix in &mut self.prefix {
            if let Some(entry) = prefix.take() {
                return Ok(Some(entry));
            }
        }
        while let Some((field, rest)) = self.fields.split_first() {
            self.fields = rest;
            if field.omit_none && self.read.decoder.borrow().next_is_nil() {
                self.read.token()?;
            } else {
                return Ok(Some((&field.name, Shape::Declared(&field.schema))));
            }
        }
        Ok(None)
    }
}

impl<'de> MapAccess<'de> for Object<'_, 'de> {
    type Error = Error;

    /// Offers the next public key to the requested Serde projection.
    fn next_key_seed<K: DeserializeSeed<'de>>(
        &mut self,
        seed: K,
    ) -> Result<Option<K::Value>, Error> {
        let Some((name, shape)) = self.entry()? else {
            return Ok(None);
        };
        self.read.charge_text(name)?;
        self.current = Some(shape);
        seed.deserialize(BorrowedStrDeserializer::new(name))
            .map(Some)
    }

    /// Decodes just the requested field value, which may be skipped by the caller.
    fn next_value_seed<V: DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value, Error> {
        let shape = self
            .current
            .take()
            .ok_or_else(|| de::Error::custom("component field value requested before its key"))?;
        seed.deserialize(self.read.with_shape(shape))
    }
}

/// Iterates wire-provided keys and checks distinctness for a dynamic dictionary.
struct DynamicMap<'cursor, 'de> {
    read: Read<'cursor, 'de>,
    remaining: usize,
    keys: HashSet<&'de str>,
}

impl<'de> MapAccess<'de> for DynamicMap<'_, 'de> {
    type Error = Error;

    /// Reads one checked UTF-8 dictionary key.
    fn next_key_seed<K: DeserializeSeed<'de>>(
        &mut self,
        seed: K,
    ) -> Result<Option<K::Value>, Error> {
        if self.remaining == 0 {
            return Ok(None);
        }
        let Token::String(key) = self.read.token()? else {
            return Err(de::Error::custom("invalid MessagePack object key"));
        };
        if !self.keys.insert(key) {
            return Err(de::Error::custom("duplicate MessagePack object key"));
        }
        self.remaining -= 1;
        seed.deserialize(BorrowedStrDeserializer::new(key))
            .map(Some)
    }

    /// Reads or skips the corresponding dictionary value.
    fn next_value_seed<V: DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value, Error> {
        seed.deserialize(self.read)
    }

    /// Reports the number of entries that remain to be consumed.
    fn size_hint(&self) -> Option<usize> {
        Some(self.remaining)
    }
}

/// Provides a named external variant and hands its compact payload to Serde.
struct External<'cursor, 'de> {
    name: &'de str,
    payload: Read<'cursor, 'de>,
}

impl<'cursor, 'de> EnumAccess<'de> for External<'cursor, 'de> {
    type Error = Error;
    type Variant = Read<'cursor, 'de>;

    /// Resolves the Rust variant identifier from the public variant name.
    fn variant_seed<V: DeserializeSeed<'de>>(
        self,
        seed: V,
    ) -> Result<(V::Value, Self::Variant), Error> {
        let tag = seed.deserialize(BorrowedStrDeserializer::new(self.name))?;
        Ok((tag, self.payload))
    }
}

impl<'de> VariantAccess<'de> for Read<'_, 'de> {
    type Error = Error;

    /// Reads a unit payload without consuming a synthetic node.
    fn unit_variant(self) -> Result<(), Error> {
        Deserialize::deserialize(self)
    }

    /// Reads the single inner value of an external newtype variant.
    fn newtype_variant_seed<T: DeserializeSeed<'de>>(self, seed: T) -> Result<T::Value, Error> {
        let read = match self.shape {
            Shape::Payload(VariantPayload::Newtype(schema)) => {
                self.with_shape(Shape::Declared(schema))
            }
            _ => self,
        };
        seed.deserialize(read)
    }

    /// Supplies the externally tagged tuple's positional fields.
    fn tuple_variant<V: Visitor<'de>>(self, _: usize, visitor: V) -> Result<V::Value, Error> {
        self.deserialize_any(visitor)
    }

    /// Supplies the externally tagged struct's named fields.
    fn struct_variant<V: Visitor<'de>>(
        self,
        _: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Error> {
        self.deserialize_any(visitor)
    }
}
