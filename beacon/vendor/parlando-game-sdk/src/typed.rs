//! Encodes the JSON-shaped subset of Serde directly into bounded MessagePack.

use std::{borrow::Cow, collections::HashSet, fmt};

use rmp::encode::{self, ByteBuf};
use serde::{ser, Serialize, Serializer};

use crate::wire::{BYTE_LIMIT, DEPTH_LIMIT, NODE_LIMIT};

/// Encodes the value in one pass, enforcing the wire constraints during serialization.
pub(crate) fn encode<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>, String> {
    let mut encoder = Encoder {
        bytes: ByteBuf::new(),
        nodes_left: NODE_LIMIT,
    };
    value
        .serialize(Value {
            encoder: &mut encoder,
            depth: 0,
        })
        .map_err(|error| error.0.into_owned())?;
    if encoder.len() > BYTE_LIMIT {
        return Err("component payload exceeds byte limit".to_owned());
    }
    Ok(encoder.bytes.into_vec())
}

/// Bounds a potentially large string before writing its UTF-8 body.
#[inline]
fn write_string(bytes: &mut ByteBuf, value: &str) -> Result<(), Error> {
    if value.len() > BYTE_LIMIT.saturating_sub(bytes.as_slice().len()) {
        return Err(Error::wire("component payload exceeds byte limit"));
    }
    let _ = encode::write_str(bytes, value);
    Ok(())
}

/// A Serde error that can also carry a wire or quota error.
#[derive(Debug)]
struct Error(Cow<'static, str>);

impl Error {
    /// Creates an error with an allocation-free static violation message.
    fn wire(message: &'static str) -> Self {
        Self(Cow::Borrowed(message))
    }
}

impl fmt::Display for Error {
    /// Presents the underlying error for callers and the guest ABI.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl std::error::Error for Error {}

impl ser::Error for Error {
    /// Preserves messages supplied by custom Serialize implementations.
    fn custom<T: fmt::Display>(message: T) -> Self {
        Self(Cow::Owned(message.to_string()))
    }
}

/// Proof that this serializer consumed one complete value.
struct Written;

/// Owns the typed serialization buffer and remaining node quota.
struct Encoder {
    bytes: ByteBuf,
    nodes_left: usize,
}

impl Encoder {
    /// Counts a value or key toward the payload limit.
    /// Together with the bound on string writes, this limits working memory
    /// until the final byte-size check.
    #[inline]
    fn charge(&mut self) -> Result<(), Error> {
        self.nodes_left = self
            .nodes_left
            .checked_sub(1)
            .ok_or_else(|| Error::wire("component payload exceeds node limit"))?;
        Ok(())
    }

    /// Reports the encoded length so far.
    #[inline]
    fn len(&self) -> usize {
        self.bytes.as_slice().len()
    }

    /// Starts a container with a reserved 32-bit count.
    fn header(&mut self, marker: u8) -> usize {
        let start = self.len() + 1;
        self.bytes
            .as_mut_vec()
            .extend_from_slice(&[marker, 0, 0, 0, 0]);
        start
    }

    /// Stores the completed container's count in the reserved header.
    #[inline]
    fn finish(&mut self, start: usize, count: u32) {
        self.bytes.as_mut_vec()[start..start + 4].copy_from_slice(&count.to_be_bytes());
    }
}

/// Borrows the encoder for one value.
struct Value<'a> {
    encoder: &'a mut Encoder,
    depth: usize,
}

impl<'a> Value<'a> {
    /// Starts a container, reserving a count field for an unknown-length input.
    #[inline]
    fn container(self, len: Option<usize>, map: bool) -> Result<Compound<'a>, Error> {
        self.encoder.charge()?;
        if self.depth >= DEPTH_LIMIT {
            return Err(Error::wire("component payload exceeds nesting limit"));
        }
        let entries_left = self.encoder.nodes_left / if map { 2 } else { 1 };
        if len.is_some_and(|len| len > entries_left) {
            return Err(Error::wire("component payload exceeds node limit"));
        }
        let header = match len {
            Some(len) => {
                let write_len = if map {
                    encode::write_map_len
                } else {
                    encode::write_array_len
                };
                let _ = write_len(&mut self.encoder.bytes, len as u32);
                None
            }
            None => Some(self.encoder.header(if map { 0xdf } else { 0xdd })),
        };
        Ok(Compound {
            encoder: self.encoder,
            depth: self.depth + 1,
            len,
            header,
            count: 0,
            failed: false,
        })
    }

    /// Emits the wrapper of an externally tagged variant before its single payload.
    #[inline]
    fn variant(self, name: &'static str) -> Result<Self, Error> {
        let wrapper = self.container(Some(1), true)?;
        wrapper.encoder.charge()?;
        write_string(&mut wrapper.encoder.bytes, name)?;
        Ok(Self {
            encoder: wrapper.encoder,
            depth: wrapper.depth,
        })
    }
}

/// Defines scalar serializer methods with the same node accounting and error conversion.
macro_rules! scalar {
    ($method:ident, $write:ident $(, $value:ident: $ty:ty)?) => {
        #[doc = "Emits the scalar in its MessagePack wire representation."]
        #[inline]
        fn $method(self $(, $value: $ty)?) -> Result<Written, Error> {
            self.encoder.charge()?;
            let _ = encode::$write(&mut self.encoder.bytes $(, $value)?);
            Ok(Written)
        }
    };
}

/// Defines narrow integer methods by widening into the ordinary integer encoding.
macro_rules! narrow_integer {
    ($method:ident, $ty:ty, $wide:ident) => {
        #[doc = "Emits an integer using the narrowest matching wire width."]
        #[inline]
        fn $method(self, value: $ty) -> Result<Written, Error> {
            self.$wide(value.into())
        }
    };
}

impl<'a> Serializer for Value<'a> {
    type Ok = Written;
    type Error = Error;
    type SerializeSeq = Compound<'a>;
    type SerializeTuple = Compound<'a>;
    type SerializeTupleStruct = Compound<'a>;
    type SerializeTupleVariant = Compound<'a>;
    type SerializeMap = Map<'a>;
    type SerializeStruct = Struct<'a>;
    type SerializeStructVariant = Struct<'a>;

    /// Selects the representation used for human-readable game fields.
    fn is_human_readable(&self) -> bool {
        true
    }

    scalar!(serialize_unit, write_nil);
    scalar!(serialize_bool, write_bool, value: bool);
    scalar!(serialize_i64, write_sint, value: i64);
    scalar!(serialize_u64, write_uint, value: u64);
    narrow_integer!(serialize_i8, i8, serialize_i64);
    narrow_integer!(serialize_i16, i16, serialize_i64);
    narrow_integer!(serialize_i32, i32, serialize_i64);
    narrow_integer!(serialize_u8, u8, serialize_u64);
    narrow_integer!(serialize_u16, u16, serialize_u64);
    narrow_integer!(serialize_u32, u32, serialize_u64);

    /// Emits a finite single-precision float.
    #[inline]
    fn serialize_f32(self, value: f32) -> Result<Written, Error> {
        self.encoder.charge()?;
        if !value.is_finite() {
            return Err(Error::wire("nonfinite MessagePack number"));
        }
        let _ = encode::write_f32(&mut self.encoder.bytes, value);
        Ok(Written)
    }

    /// Emits a finite double-precision float.
    #[inline]
    fn serialize_f64(self, value: f64) -> Result<Written, Error> {
        self.encoder.charge()?;
        if !value.is_finite() {
            return Err(Error::wire("nonfinite MessagePack number"));
        }
        let _ = encode::write_f64(&mut self.encoder.bytes, value);
        Ok(Written)
    }

    /// Encodes a single Unicode scalar as a string.
    fn serialize_char(self, value: char) -> Result<Written, Error> {
        self.serialize_str(value.encode_utf8(&mut [0; 4]))
    }

    /// Encodes a UTF-8 string value.
    #[inline]
    fn serialize_str(self, value: &str) -> Result<Written, Error> {
        self.encoder.charge()?;
        write_string(&mut self.encoder.bytes, value)?;
        Ok(Written)
    }

    /// Rejects the binary marker; byte arrays use ordinary sequences.
    fn serialize_bytes(self, _: &[u8]) -> Result<Written, Error> {
        Err(Error::wire("unsupported MessagePack binary value"))
    }

    /// Preserves the rejection of rmp-serde's binary representation for i128.
    fn serialize_i128(self, _: i128) -> Result<Written, Error> {
        Err(Error::wire("unsupported 128-bit MessagePack integer"))
    }

    /// Preserves the rejection of rmp-serde's binary representation for u128.
    fn serialize_u128(self, _: u128) -> Result<Written, Error> {
        Err(Error::wire("unsupported 128-bit MessagePack integer"))
    }

    /// Encodes an absent optional field as null.
    #[inline]
    fn serialize_none(self) -> Result<Written, Error> {
        self.serialize_unit()
    }

    /// Serializes a present option using its contained value's representation.
    #[inline]
    fn serialize_some<T: Serialize + ?Sized>(self, value: &T) -> Result<Written, Error> {
        value.serialize(self)
    }

    /// Encodes a unit struct as an empty array, matching the ABI representation.
    fn serialize_unit_struct(self, _: &'static str) -> Result<Written, Error> {
        self.serialize_tuple(0)?.finish()
    }

    /// Encodes a unit variant as its Serde name.
    #[inline]
    fn serialize_unit_variant(
        self,
        _: &'static str,
        _: u32,
        variant: &'static str,
    ) -> Result<Written, Error> {
        self.serialize_str(variant)
    }

    /// Serializes a transparent wrapper and rejects reserved MessagePack extension wrappers.
    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        name: &'static str,
        value: &T,
    ) -> Result<Written, Error> {
        if name == rmp_serde::MSGPACK_EXT_STRUCT_NAME {
            return Err(Error::wire("unsupported MessagePack extension value"));
        }
        value.serialize(self)
    }

    /// Encodes an externally tagged newtype variant with one named entry.
    #[inline]
    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _: &'static str,
        _: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<Written, Error> {
        value.serialize(self.variant(variant)?)
    }

    /// Opens a sequence with an optional advertised length.
    #[inline]
    fn serialize_seq(self, len: Option<usize>) -> Result<Compound<'a>, Error> {
        self.container(len, false)
    }

    /// Opens a fixed-length array.
    #[inline]
    fn serialize_tuple(self, len: usize) -> Result<Compound<'a>, Error> {
        self.serialize_seq(Some(len))
    }

    /// Encodes a tuple struct as a fixed-length array.
    fn serialize_tuple_struct(self, _: &'static str, len: usize) -> Result<Compound<'a>, Error> {
        self.serialize_tuple(len)
    }

    /// Encodes an externally tagged tuple variant as a named array.
    fn serialize_tuple_variant(
        self,
        _: &'static str,
        _: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<Compound<'a>, Error> {
        self.variant(variant)?.serialize_tuple(len)
    }

    /// Opens a map whose keys must be distinct strings.
    #[inline]
    fn serialize_map(self, len: Option<usize>) -> Result<Map<'a>, Error> {
        let core = self.container(len, true)?;
        Ok(Map {
            core,
            keys: Keys::Small {
                keys: Default::default(),
                len: 0,
            },
            pending_key: false,
        })
    }

    /// Encodes a named struct as a map with its Serde field names.
    #[inline]
    fn serialize_struct(self, _: &'static str, len: usize) -> Result<Struct<'a>, Error> {
        let core = self.container(Some(len), true)?;
        let keys = if len <= 32 {
            StaticKeys::Small {
                mask: 0,
                keys: [""; 32],
            }
        } else {
            StaticKeys::Large(HashSet::new())
        };
        Ok(Struct { core, keys })
    }

    /// Encodes an externally tagged struct variant as a named map.
    fn serialize_struct_variant(
        self,
        _: &'static str,
        _: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<Struct<'a>, Error> {
        self.variant(variant)?.serialize_struct("", len)
    }
}

/// Captures a key using only the Serde forms that encode as strings.
struct Key {
    bytes_left: usize,
}

/// Defines the remaining Serializer methods that cannot yield a string key.
macro_rules! reject_key {
    ($(fn $method:ident($($ty:ty),*) -> $result:ty;)*) => {
        $(
            #[doc = "Rejects a Serde form that does not represent a string key."]
            #[inline]
        fn $method(self, $(_: $ty),*) -> Result<$result, Error> {
                Err(Error::wire("invalid MessagePack object key"))
            }
        )*
    };
}

impl Serializer for Key {
    type Ok = String;
    type Error = Error;
    type SerializeSeq = ser::Impossible<String, Error>;
    type SerializeTuple = ser::Impossible<String, Error>;
    type SerializeTupleStruct = ser::Impossible<String, Error>;
    type SerializeTupleVariant = ser::Impossible<String, Error>;
    type SerializeMap = ser::Impossible<String, Error>;
    type SerializeStruct = ser::Impossible<String, Error>;
    type SerializeStructVariant = ser::Impossible<String, Error>;

    reject_key! {
        fn serialize_bool(bool) -> String;
        fn serialize_i8(i8) -> String;
        fn serialize_i16(i16) -> String;
        fn serialize_i32(i32) -> String;
        fn serialize_i64(i64) -> String;
        fn serialize_u8(u8) -> String;
        fn serialize_u16(u16) -> String;
        fn serialize_u32(u32) -> String;
        fn serialize_u64(u64) -> String;
        fn serialize_f32(f32) -> String;
        fn serialize_f64(f64) -> String;
        fn serialize_bytes(&[u8]) -> String;
        fn serialize_none() -> String;
        fn serialize_unit() -> String;
        fn serialize_unit_struct(&'static str) -> String;
        fn serialize_seq(Option<usize>) -> Self::SerializeSeq;
        fn serialize_tuple(usize) -> Self::SerializeTuple;
        fn serialize_tuple_struct(&'static str, usize) -> Self::SerializeTupleStruct;
        fn serialize_tuple_variant(&'static str, u32, &'static str, usize) -> Self::SerializeTupleVariant;
        fn serialize_map(Option<usize>) -> Self::SerializeMap;
        fn serialize_struct(&'static str, usize) -> Self::SerializeStruct;
        fn serialize_struct_variant(&'static str, u32, &'static str, usize) -> Self::SerializeStructVariant;
    }

    /// Owns an accepted key for duplicate detection in its containing map.
    #[inline]
    fn serialize_str(self, value: &str) -> Result<String, Error> {
        if value.len() > self.bytes_left {
            return Err(Error::wire("component payload exceeds byte limit"));
        }
        Ok(value.to_owned())
    }

    /// Represents a single Unicode scalar as a string key.
    fn serialize_char(self, value: char) -> Result<String, Error> {
        self.serialize_str(value.encode_utf8(&mut [0; 4]))
    }

    /// Represents a unit variant by its Serde name.
    #[inline]
    fn serialize_unit_variant(
        self,
        _: &'static str,
        _: u32,
        name: &'static str,
    ) -> Result<String, Error> {
        self.serialize_str(name)
    }

    /// Allows a present optional key when its inner value is a string.
    #[inline]
    fn serialize_some<T: Serialize + ?Sized>(self, value: &T) -> Result<String, Error> {
        value.serialize(self)
    }

    /// Allows transparent key wrappers except the reserved extension wrapper.
    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        name: &'static str,
        value: &T,
    ) -> Result<String, Error> {
        if name == rmp_serde::MSGPACK_EXT_STRUCT_NAME {
            return Err(Error::wire("invalid MessagePack object key"));
        }
        value.serialize(self)
    }

    /// Rejects a tagged newtype, whose wire representation is an object.
    #[inline]
    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: &T,
    ) -> Result<String, Error> {
        Err(Error::wire("invalid MessagePack object key"))
    }
}

/// Stores small objects' keys inline and uses hashing for larger objects.
enum Keys {
    Small { keys: [String; 4], len: usize },
    Large(HashSet<String>),
}

impl Keys {
    /// Inserts a distinct key, upgrading storage once inline capacity is exhausted.
    #[inline]
    fn insert(&mut self, key: String) -> bool {
        match self {
            Self::Small { keys, len } => {
                if keys[..*len].contains(&key) {
                    return false;
                }
                if *len < keys.len() {
                    keys[*len] = key;
                    *len += 1;
                } else {
                    let mut large = HashSet::from(std::mem::take(keys));
                    large.insert(key);
                    *self = Self::Large(large);
                }
                true
            }
            Self::Large(keys) => keys.insert(key),
        }
    }
}

/// Owns one open array or map and tracks progress against its header contract.
struct Compound<'a> {
    encoder: &'a mut Encoder,
    depth: usize,
    len: Option<usize>,
    header: Option<usize>,
    count: usize,
    failed: bool,
}

impl Compound<'_> {
    /// Records child failures so custom serializers cannot finish with partial bytes.
    #[inline]
    fn record(&mut self, result: Result<(), Error>) -> Result<(), Error> {
        self.failed |= result.is_err();
        result
    }

    /// Checks that an array or map can accept another element or entry.
    #[inline]
    fn reserve_entry(&self) -> Result<(), Error> {
        if self.len == Some(self.count) {
            return Err(Error::wire("too many MessagePack container entries"));
        }
        Ok(())
    }

    /// Encodes one array item or pending map value.
    #[inline]
    fn child<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Error> {
        let result = self.reserve_entry().and_then(|()| {
            value.serialize(Value {
                encoder: self.encoder,
                depth: self.depth,
            })?;
            self.count += 1;
            Ok(())
        });
        self.record(result)
    }

    /// Completes the header only after the actual length and map state are consistent.
    #[inline]
    fn finish(self) -> Result<Written, Error> {
        if self.failed || self.len.is_some_and(|len| len != self.count) {
            return Err(Error::wire("incomplete MessagePack container"));
        }
        if let Some(header) = self.header {
            self.encoder.finish(header, self.count as u32);
        }
        Ok(Written)
    }
}

/// Implements the equivalent Serde array protocols through their single-item interface.
macro_rules! array {
    ($trait:ident, $item:ident) => {
        impl ser::$trait for Compound<'_> {
            type Ok = Written;
            type Error = Error;

            /// Encodes the next array item.
            #[inline]
            fn $item<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Error> {
                self.child(value)
            }

            /// Ensures the array contains exactly its reported number of items.
            #[inline]
            fn end(self) -> Result<Written, Error> {
                self.finish()
            }
        }
    };
}

array!(SerializeSeq, serialize_element);
array!(SerializeTuple, serialize_element);
array!(SerializeTupleStruct, serialize_field);
array!(SerializeTupleVariant, serialize_field);

/// Owns the key set and key/value sequencing for a MessagePack object.
struct Map<'a> {
    core: Compound<'a>,
    keys: Keys,
    pending_key: bool,
}

impl Map<'_> {
    /// Checks the map state, then encodes a distinct string key.
    #[inline]
    fn key(&mut self, key: String) -> Result<(), Error> {
        let result = self.core.reserve_entry().and_then(|()| {
            if self.pending_key {
                return Err(Error::wire("MessagePack map key is missing its value"));
            }
            self.core.encoder.charge()?;
            write_string(&mut self.core.encoder.bytes, &key)?;
            if !self.keys.insert(key) {
                return Err(Error::wire("duplicate MessagePack object key"));
            }
            self.pending_key = true;
            Ok(())
        });
        self.core.record(result)
    }

    /// Encodes a key's value and clears the pending-key flag.
    #[inline]
    fn value<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Error> {
        if !self.pending_key {
            return self
                .core
                .record(Err(Error::wire("MessagePack map value is missing its key")));
        }
        self.core.child(value)?;
        self.pending_key = false;
        Ok(())
    }

    /// Completes the map once all accepted keys have values.
    #[inline]
    fn finish(self) -> Result<Written, Error> {
        if self.pending_key {
            return Err(Error::wire("incomplete MessagePack container"));
        }
        self.core.finish()
    }
}

impl ser::SerializeMap for Map<'_> {
    type Ok = Written;
    type Error = Error;

    /// Captures only string-shaped keys from arbitrary Serialize implementations.
    #[inline]
    fn serialize_key<T: Serialize + ?Sized>(&mut self, key: &T) -> Result<(), Error> {
        match key.serialize(Key {
            bytes_left: BYTE_LIMIT.saturating_sub(self.core.encoder.len()),
        }) {
            Ok(key) => self.key(key),
            Err(error) => self.core.record(Err(error)),
        }
    }

    /// Encodes the value associated with the most recently supplied key.
    #[inline]
    fn serialize_value<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Error> {
        self.value(value)
    }

    /// Ensures all map entries are paired and the entry count is correct.
    #[inline]
    fn end(self) -> Result<Written, Error> {
        self.finish()
    }
}

/// Controls duplicate detection for maps with statically named fields.
#[allow(clippy::large_enum_variant)] // Keep the common key set allocation-free.
enum StaticKeys {
    Small { mask: u64, keys: [&'static str; 32] },
    Large(HashSet<&'static str>),
}

impl StaticKeys {
    /// Records the key, comparing strings only when the fingerprint may collide.
    #[inline]
    fn insert(&mut self, key: &'static str, count: usize) -> bool {
        match self {
            Self::Small { mask, keys } => {
                let bytes = key.as_bytes();
                let len = bytes.len();
                let fingerprint = if len == 0 {
                    0
                } else {
                    ((len & 63) * 7 + usize::from(bytes[0]) * 3 + usize::from(bytes[len - 1])) & 63
                };
                let bit = 1 << fingerprint;
                if *mask & bit != 0 && keys[..count].contains(&key) {
                    return false;
                }
                *mask |= bit;
                keys[count] = key;
                true
            }
            Self::Large(keys) => keys.insert(key),
        }
    }
}

/// Stores static named fields independently from maps with dynamic keys.
struct Struct<'a> {
    core: Compound<'a>,
    keys: StaticKeys,
}

impl Struct<'_> {
    /// Writes a field once its name and container capacity have been checked.
    #[inline]
    fn field<T: Serialize + ?Sized>(&mut self, key: &'static str, value: &T) -> Result<(), Error> {
        let result = self.core.reserve_entry().and_then(|()| {
            if !self.keys.insert(key, self.core.count) {
                return Err(Error::wire("duplicate MessagePack object key"));
            }
            self.core.encoder.charge()?;
            write_string(&mut self.core.encoder.bytes, key)?;
            self.core.child(value)
        });
        self.core.record(result)
    }
}

/// Implements the Serde named field protocols with allocation-free static field names.
macro_rules! fields {
    ($trait:ident) => {
        impl ser::$trait for Struct<'_> {
            type Ok = Written;
            type Error = Error;

            /// Encodes a named field and its value in the map.
            #[inline]
            fn serialize_field<T: Serialize + ?Sized>(
                &mut self,
                key: &'static str,
                value: &T,
            ) -> Result<(), Error> {
                self.field(key, value)
            }

            /// Completes the field map and checks its actual size.
            #[inline]
            fn end(self) -> Result<Written, Error> {
                self.core.finish()
            }
        }
    };
}

fields!(SerializeStruct);
fields!(SerializeStructVariant);

#[cfg(test)]
mod tests;
