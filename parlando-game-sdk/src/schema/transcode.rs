//! Adapts a streaming named deserializer to Serde's serializer interfaces.

use std::{cell::RefCell, fmt};

use serde::{
    de::{DeserializeSeed, Error, MapAccess, SeqAccess, Visitor},
    ser::{Error as _, SerializeMap, SerializeSeq},
    Deserializer, Serialize, Serializer,
};

/// Streams one JSON-shaped value through the target serializer.
pub(crate) fn serialize<'de, D: Deserializer<'de>, S: Serializer>(
    input: D,
    target: S,
) -> Result<S::Ok, S::Error> {
    input
        .deserialize_any(Write(target))
        .map_err(S::Error::custom)
}

/// Receives decoded scalar or compound events for the next serialized value.
struct Write<S>(S);

impl<'de, S: Serializer> Visitor<'de> for Write<S> {
    type Value = S::Ok;

    /// Describes the value model handled by the transcoder.
    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON-shaped component value")
    }

    /// Writes nil.
    fn visit_unit<E: Error>(self) -> Result<Self::Value, E> {
        self.0.serialize_unit().map_err(E::custom)
    }

    /// Writes a Boolean.
    fn visit_bool<E: Error>(self, value: bool) -> Result<Self::Value, E> {
        self.0.serialize_bool(value).map_err(E::custom)
    }

    /// Writes a signed integer.
    fn visit_i64<E: Error>(self, value: i64) -> Result<Self::Value, E> {
        self.0.serialize_i64(value).map_err(E::custom)
    }

    /// Writes an unsigned integer.
    fn visit_u64<E: Error>(self, value: u64) -> Result<Self::Value, E> {
        self.0.serialize_u64(value).map_err(E::custom)
    }

    /// Writes a finite float.
    fn visit_f64<E: Error>(self, value: f64) -> Result<Self::Value, E> {
        self.0.serialize_f64(value).map_err(E::custom)
    }

    /// Writes a UTF-8 value.
    fn visit_str<E: Error>(self, value: &str) -> Result<Self::Value, E> {
        self.0.serialize_str(value).map_err(E::custom)
    }

    /// Writes child nodes one at a time through the target's sequence interface.
    fn visit_seq<A: SeqAccess<'de>>(self, mut input: A) -> Result<Self::Value, A::Error> {
        let mut seq = self
            .0
            .serialize_seq(input.size_hint())
            .map_err(A::Error::custom)?;
        while input.next_element_seed(SequenceItem(&mut seq))?.is_some() {}
        seq.end().map_err(A::Error::custom)
    }

    /// Writes borrowed keys and then streams their corresponding values.
    fn visit_map<A: MapAccess<'de>>(self, mut input: A) -> Result<Self::Value, A::Error> {
        let mut map = self
            .0
            .serialize_map(input.size_hint())
            .map_err(A::Error::custom)?;
        while let Some(key) = input.next_key::<&str>()? {
            map.serialize_key(key).map_err(A::Error::custom)?;
            input.next_value_seed(MapItem(&mut map))?;
        }
        map.end().map_err(A::Error::custom)
    }
}

/// Connects a sequence slot to the deserializer that knows how to fill it.
struct SequenceItem<'a, S>(&'a mut S);

impl<'de, S: SerializeSeq> DeserializeSeed<'de> for SequenceItem<'_, S> {
    type Value = ();

    /// Streams the child through the target serializer's element callback.
    fn deserialize<D: Deserializer<'de>>(self, input: D) -> Result<(), D::Error> {
        self.0
            .serialize_element(&Deferred(
                RefCell::new(Some(input)),
                std::marker::PhantomData,
            ))
            .map_err(D::Error::custom)
    }
}

/// Connects a map value to the deserializer that knows how to fill it.
struct MapItem<'a, S>(&'a mut S);

impl<'de, S: SerializeMap> DeserializeSeed<'de> for MapItem<'_, S> {
    type Value = ();

    /// Streams the child through the target serializer's value callback.
    fn deserialize<D: Deserializer<'de>>(self, input: D) -> Result<(), D::Error> {
        self.0
            .serialize_value(&Deferred(
                RefCell::new(Some(input)),
                std::marker::PhantomData,
            ))
            .map_err(D::Error::custom)
    }
}

/// Owns the next deserializer until the target asks to serialize this slot once.
struct Deferred<'de, D>(RefCell<Option<D>>, std::marker::PhantomData<&'de ()>);

impl<'de, D: Deserializer<'de>> Serialize for Deferred<'de, D> {
    /// Runs the child deserializer inside its target serializer callback.
    fn serialize<S: Serializer>(&self, target: S) -> Result<S::Ok, S::Error> {
        let input = self
            .0
            .borrow_mut()
            .take()
            .ok_or_else(|| S::Error::custom("component collection item serialized twice"))?;
        serialize(input, target)
    }
}
