//! Streams between a validated MessagePack payload and Serde data models.

use std::{
    cell::RefCell,
    collections::{hash_map::Entry, HashMap},
    fmt,
};

use rmp::encode;
use serde::{
    de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor},
    ser::{self, SerializeMap, SerializeSeq},
    Deserializer, Serialize, Serializer,
};

use crate::{
    encoder::Encoder,
    wire::{Decoder, Token, BYTE_LIMIT, DEPTH_LIMIT, NODE_LIMIT},
};

/// Serializes one value directly from the compact representation.
pub(crate) fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
    ReadValue {
        cursor: &RefCell::new(Decoder::new(bytes)),
        depth: 0,
    }
    .serialize(serializer)
}

/// Borrows the cursor shared by a value and the visitors for its children.
struct ReadValue<'cursor, 'wire> {
    cursor: &'cursor RefCell<Decoder<'wire>>,
    depth: usize,
}

impl Serialize for ReadValue<'_, '_> {
    /// Emits a value in the target format without allocating a decoded tree.
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let token = self
            .cursor
            .borrow_mut()
            .next()
            .map_err(ser::Error::custom)?;
        let child = Self {
            cursor: self.cursor,
            depth: self.depth + 1,
        };
        match token {
            Token::Null => serializer.serialize_unit(),
            Token::Bool(value) => serializer.serialize_bool(value),
            Token::Signed(value) => serializer.serialize_i64(value),
            Token::Unsigned(value) => serializer.serialize_u64(value),
            Token::Float(value) => serializer.serialize_f64(value),
            Token::String(value) => serializer.serialize_str(value),
            Token::Array(len) => {
                self.cursor
                    .borrow()
                    .check_container(len, self.depth, 1)
                    .map_err(ser::Error::custom)?;
                let mut seq = serializer.serialize_seq(Some(len))?;
                for _ in 0..len {
                    seq.serialize_element(&child)?;
                }
                seq.end()
            }
            Token::Map(len) => {
                self.cursor
                    .borrow()
                    .check_container(len, self.depth, 2)
                    .map_err(ser::Error::custom)?;
                let mut map = serializer.serialize_map(Some(len))?;
                for _ in 0..len {
                    let mut cursor = self.cursor.borrow_mut();
                    let marker = cursor.marker().map_err(ser::Error::custom)?;
                    let key = cursor.string(marker).map_err(ser::Error::custom)?;
                    drop(cursor);
                    map.serialize_entry(key, &child)?;
                }
                map.end()
            }
        }
    }
}

/// Transcodes one deserialized data value into a bounded MessagePack buffer.
pub(crate) fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
    let mut encoder = Encoder {
        bytes: Vec::new(),
        nodes_left: NODE_LIMIT,
    };
    WriteValue {
        encoder: &mut encoder,
        depth: 0,
    }
    .deserialize(deserializer)?;
    Ok(encoder.bytes)
}

/// Recursively emits a MessagePack value as Serde supplies the source tokens.
struct WriteValue<'a> {
    encoder: &'a mut Encoder,
    depth: usize,
}

impl<'de> DeserializeSeed<'de> for WriteValue<'_> {
    type Value = ();

    /// Reads a scalar or container without requesting an intermediate value.
    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
        self.encoder.charge().map_err(de::Error::custom)?;
        deserializer.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for WriteValue<'_> {
    type Value = ();

    /// Describes the accepted Serde data model.
    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON-compatible value")
    }

    /// Emits null.
    fn visit_unit<E: de::Error>(self) -> Result<(), E> {
        encode::write_nil(self.encoder).map_err(E::custom)
    }

    /// Emits a Boolean.
    fn visit_bool<E: de::Error>(self, value: bool) -> Result<(), E> {
        encode::write_bool(self.encoder, value).map_err(E::custom)
    }

    /// Emits an integer using the narrowest matching wire width.
    fn visit_i64<E: de::Error>(self, value: i64) -> Result<(), E> {
        encode::write_sint(self.encoder, value)
            .map(|_| ())
            .map_err(E::custom)
    }

    /// Emits a nonnegative integer without narrowing it to the signed range.
    fn visit_u64<E: de::Error>(self, value: u64) -> Result<(), E> {
        encode::write_uint(self.encoder, value)
            .map(|_| ())
            .map_err(E::custom)
    }

    /// Emits a finite float.
    fn visit_f64<E: de::Error>(self, value: f64) -> Result<(), E> {
        if !value.is_finite() {
            return Err(E::custom("nonfinite MessagePack number"));
        }
        encode::write_f64(self.encoder, value).map_err(E::custom)
    }

    /// Writes the UTF-8 representation of a string.
    fn visit_str<E: de::Error>(self, value: &str) -> Result<(), E> {
        if value.len() > BYTE_LIMIT {
            return Err(E::custom("component payload exceeds byte limit"));
        }
        encode::write_str(self.encoder, value).map_err(E::custom)
    }

    /// Streams array values into one MessagePack buffer.
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
        if self.depth >= DEPTH_LIMIT {
            return Err(de::Error::custom("component payload exceeds nesting limit"));
        }
        let start = self.encoder.header(0xdd).map_err(de::Error::custom)?;
        let mut count = 0;
        loop {
            let before = self.encoder.bytes.len();
            let has_item = seq
                .next_element_seed(WriteValue {
                    encoder: self.encoder,
                    depth: self.depth + 1,
                })?
                .is_some();
            let wrote_item = self.encoder.bytes.len() != before;
            if has_item != wrote_item {
                return Err(de::Error::custom("inconsistent Serde sequence access"));
            }
            if !has_item {
                break;
            }
            count += 1;
        }
        self.encoder.finish(start, count);
        Ok(())
    }

    /// Streams string keys and their values into one MessagePack buffer.
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        if self.depth >= DEPTH_LIMIT {
            return Err(de::Error::custom("component payload exceeds nesting limit"));
        }
        let start = self.encoder.header(0xdf).map_err(de::Error::custom)?;
        let mut count = 0;
        let mut keys = HashMap::new();
        while let Some(key) = map.next_key::<String>()? {
            let Entry::Vacant(entry) = keys.entry(key) else {
                return Err(de::Error::custom("duplicate MessagePack object key"));
            };
            self.encoder.charge().map_err(de::Error::custom)?;
            WriteValue {
                encoder: self.encoder,
                depth: self.depth + 1,
            }
            .visit_str(entry.key())?;
            entry.insert(());
            let before = self.encoder.bytes.len();
            map.next_value_seed(WriteValue {
                encoder: self.encoder,
                depth: self.depth + 1,
            })?;
            if self.encoder.bytes.len() == before {
                return Err(de::Error::custom("inconsistent Serde map access"));
            }
            count += 1;
        }
        self.encoder.finish(start, count);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use serde::de::value::{Error, SeqAccessDeserializer, U64Deserializer};

    use super::*;

    /// Simulates a deserializer that emits a sequence child but reports the end of the sequence.
    struct DiscardedElement;

    impl<'de> SeqAccess<'de> for DiscardedElement {
        type Error = Error;

        /// Calls the child seed and deliberately misreports the number of emitted elements.
        fn next_element_seed<T: DeserializeSeed<'de>>(
            &mut self,
            seed: T,
        ) -> Result<Option<T::Value>, Self::Error> {
            seed.deserialize(U64Deserializer::new(1))?;
            Ok(None)
        }
    }

    /// Exercises the nested and wide limits through the incremental JSON visitor.
    #[test]
    fn bounds_deserialized_containers() {
        let nested = format!("{}null{}", "[".repeat(DEPTH_LIMIT), "]".repeat(DEPTH_LIMIT));
        assert!(serde_json::from_str::<crate::ComponentValue>(&nested).is_ok());
        let deeper = format!("[{nested}]");
        assert!(serde_json::from_str::<crate::ComponentValue>(&deeper)
            .unwrap_err()
            .to_string()
            .contains("nesting"));

        let wide = format!("[{}null]", "null,".repeat(NODE_LIMIT - 1));
        assert!(serde_json::from_str::<crate::ComponentValue>(&wide)
            .unwrap_err()
            .to_string()
            .contains("node"));
    }

    /// Ensures a custom access implementation cannot leave an orphaned wire token.
    #[test]
    fn rejects_unreported_deserialized_elements() {
        let input = SeqAccessDeserializer::new(DiscardedElement);
        assert!(deserialize(input)
            .unwrap_err()
            .to_string()
            .contains("inconsistent"));
    }
}
