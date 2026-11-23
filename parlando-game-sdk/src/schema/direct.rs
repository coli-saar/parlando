//! Constructs declared Rust values directly from their compact wire slots.

use std::{
    borrow::Cow,
    collections::{BTreeMap, HashMap},
};

use anyhow::{bail, ensure, Result};

use crate::wire::{Decoder, Token, BYTE_LIMIT};

use super::{read::decode_dynamic, GameValue};

/// Reads the compact layout declared by a Rust component type.
///
/// Deriving [`GameValue`] also implements this trait when the fields support decoding. The decoder
/// constructs Rust values from field slots and numeric variant ordinals.
/// Public Serde field names and enum tagging conventions affect the schema
/// but do not participate in this decoding path.
pub trait DecodeGameValue<'de>: GameValue + Sized {
    /// Consumes exactly one value from the shared compact reader.
    #[doc(hidden)]
    fn read_compact(reader: &mut CompactReader<'de>) -> Result<Self>;
}

/// Deserializes one compact action or configuration directly into its declared Rust type.
///
/// String fields may borrow from the input. The decoder checks the component
/// byte, node and nesting limits, and rejects trailing data. For a dynamic
/// declaration or a type requiring ordinary Serde, use
/// [`crate::decode_with_schema`].
///
/// ```
/// use parlando_game_sdk::{decode_declared, encode_compact, GameValue};
///
/// #[derive(GameValue)]
/// enum Action<'a> {
///     Wait,
///     Label { text: &'a str },
/// }
///
/// let bytes = encode_compact(&Action::Label { text: "roof" })?;
/// let action: Action<'_> = decode_declared(&bytes)?;
/// assert!(matches!(action, Action::Label { text: "roof" }));
/// # Ok::<(), String>(())
/// ```
pub fn decode_declared<'de, T: DecodeGameValue<'de>>(bytes: &'de [u8]) -> Result<T, String> {
    fn read<'de, T: DecodeGameValue<'de>>(bytes: &'de [u8]) -> Result<T> {
        ensure!(
            bytes.len() <= BYTE_LIMIT,
            "component payload exceeds byte limit"
        );
        let mut reader = CompactReader {
            decoder: Decoder::new(bytes),
            depth: 0,
        };
        let value = T::read_compact(&mut reader)?;
        ensure!(reader.decoder.is_finished(), "trailing MessagePack bytes");
        Ok(value)
    }
    read(bytes).map_err(|error| error.to_string())
}

/// Gives generated decoders checked sequential access to compact slots.
#[doc(hidden)]
pub struct CompactReader<'de> {
    decoder: Decoder<'de>,
    depth: usize,
}

impl<'de> CompactReader<'de> {
    /// Reads a value using the field type inferred from the construction site.
    pub fn read<T: DecodeGameValue<'de>>(&mut self) -> Result<T> {
        T::read_compact(self)
    }

    /// Opens a checked array and returns its advertised slot count.
    pub fn array(&mut self) -> Result<usize> {
        let Token::Array(len) = self.decoder.next()? else {
            bail!("expected a compact array");
        };
        self.decoder.check_container(len, self.depth, 1)?;
        Ok(len)
    }

    /// Ensures that a record, tuple, or variant contains exactly its declared slots.
    pub fn check_slots(actual: usize, expected: usize) -> Result<()> {
        ensure!(
            actual == expected,
            "compact array length differs from its component schema"
        );
        Ok(())
    }

    /// Reads the discriminator used by derived enums.
    pub fn ordinal(&mut self) -> Result<u32> {
        self.unsigned()?.try_into().map_err(Into::into)
    }

    /// Reports an ordinal with no corresponding declared enum variant.
    pub fn invalid_ordinal<T>() -> Result<T> {
        bail!("invalid compact enum ordinal")
    }

    /// Reads container children at the next physical nesting level.
    pub fn nested<T>(&mut self, read: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        self.depth += 1;
        let result = read(self);
        self.depth -= 1;
        result
    }

    /// Reads an integer normalized to the signed wire domain.
    fn signed(&mut self) -> Result<i64> {
        match self.decoder.next()? {
            Token::Signed(value) => Ok(value),
            Token::Unsigned(value) => Ok(value.try_into()?),
            _ => bail!("expected a compact signed integer"),
        }
    }

    /// Reads an integer normalized to the unsigned wire domain.
    fn unsigned(&mut self) -> Result<u64> {
        match self.decoder.next()? {
            Token::Unsigned(value) => Ok(value),
            Token::Signed(value) => Ok(value.try_into()?),
            _ => bail!("expected a compact unsigned integer"),
        }
    }

    /// Borrows one UTF-8 string from the input.
    fn string(&mut self) -> Result<&'de str> {
        match self.decoder.next()? {
            Token::String(value) => Ok(value),
            _ => bail!("expected a compact string"),
        }
    }

    /// Reads a dictionary with distinct string keys and declared values.
    fn map<T: DecodeGameValue<'de>, M: Default>(
        &mut self,
        mut insert: impl FnMut(&mut M, String, T) -> Option<T>,
    ) -> Result<M> {
        let Token::Map(len) = self.decoder.next()? else {
            bail!("expected a compact map");
        };
        self.decoder.check_container(len, self.depth, 2)?;
        let mut values = M::default();
        self.nested(|reader| {
            for _ in 0..len {
                let key = reader.string()?.to_owned();
                let value = reader.read()?;
                ensure!(
                    insert(&mut values, key, value).is_none(),
                    "duplicate MessagePack object key"
                );
            }
            Ok(values)
        })
    }
}

/// Applies the Rust numeric range after checking the underlying wire domain.
macro_rules! integer {
    ($method:ident; $($ty:ty),* $(,)?) => {$(
        impl<'de> DecodeGameValue<'de> for $ty {
            /// Reads a compact integer checked against the destination width.
            fn read_compact(reader: &mut CompactReader<'de>) -> Result<Self> {
                Ok(reader.$method()?.try_into()?)
            }
        }
    )*};
}

integer!(signed; i8, i16, i32, i64, isize);
integer!(unsigned; u8, u16, u32, u64, usize);

impl<'de> DecodeGameValue<'de> for () {
    /// Requires an exact nil marker for a unit field.
    fn read_compact(reader: &mut CompactReader<'de>) -> Result<Self> {
        match reader.decoder.next()? {
            Token::Null => Ok(()),
            _ => bail!("expected compact null"),
        }
    }
}

impl<'de> DecodeGameValue<'de> for bool {
    /// Requires an exact Boolean wire marker.
    fn read_compact(reader: &mut CompactReader<'de>) -> Result<Self> {
        match reader.decoder.next()? {
            Token::Bool(value) => Ok(value),
            _ => bail!("expected compact Boolean"),
        }
    }
}

impl<'de> DecodeGameValue<'de> for f64 {
    /// Reads only finite values from the floating-point wire domain.
    fn read_compact(reader: &mut CompactReader<'de>) -> Result<Self> {
        match reader.decoder.next()? {
            Token::Float(value) if value.is_finite() => Ok(value),
            _ => bail!("expected a finite compact float"),
        }
    }
}

impl<'de> DecodeGameValue<'de> for f32 {
    /// Narrows a valid wire float using Serde's typed float conversion.
    fn read_compact(reader: &mut CompactReader<'de>) -> Result<Self> {
        Ok(f64::read_compact(reader)? as f32)
    }
}

impl<'de: 'a, 'a> DecodeGameValue<'de> for &'a str {
    /// Borrows a checked UTF-8 string from the input byte buffer.
    fn read_compact(reader: &mut CompactReader<'de>) -> Result<Self> {
        reader.string()
    }
}

impl<'de> DecodeGameValue<'de> for String {
    /// Copies a checked UTF-8 string into owned storage.
    fn read_compact(reader: &mut CompactReader<'de>) -> Result<Self> {
        Ok(reader.string()?.to_owned())
    }
}

impl<'de: 'a, 'a> DecodeGameValue<'de> for Cow<'a, str> {
    /// Borrows a checked UTF-8 string, without allocating its owned alternative.
    fn read_compact(reader: &mut CompactReader<'de>) -> Result<Self> {
        Ok(Cow::Borrowed(reader.string()?))
    }
}

impl<'de> DecodeGameValue<'de> for char {
    /// Requires a UTF-8 string containing exactly one scalar value.
    fn read_compact(reader: &mut CompactReader<'de>) -> Result<Self> {
        let mut chars = reader.string()?.chars();
        match (chars.next(), chars.next()) {
            (Some(value), None) => Ok(value),
            _ => bail!("expected a compact character"),
        }
    }
}

impl<'de, T: DecodeGameValue<'de>> DecodeGameValue<'de> for Option<T> {
    /// Consumes nil or delegates to the inner type at the same wire level.
    fn read_compact(reader: &mut CompactReader<'de>) -> Result<Self> {
        if reader.decoder.next_is_nil() {
            reader.read::<()>()?;
            Ok(None)
        } else {
            reader.read().map(Some)
        }
    }
}

impl<'de, T: DecodeGameValue<'de>> DecodeGameValue<'de> for Vec<T> {
    /// Reads a checked list of declared elements.
    fn read_compact(reader: &mut CompactReader<'de>) -> Result<Self> {
        let len = reader.array()?;
        reader.nested(|reader| (0..len).map(|_| reader.read()).collect())
    }
}

impl<'de, T: DecodeGameValue<'de>, const N: usize> DecodeGameValue<'de> for [T; N] {
    /// Checks the advertised length before converting the elements to a fixed array.
    fn read_compact(reader: &mut CompactReader<'de>) -> Result<Self> {
        CompactReader::check_slots(reader.array()?, N)?;
        let values =
            reader.nested(|reader| (0..N).map(|_| reader.read()).collect::<Result<Vec<T>>>())?;
        values
            .try_into()
            .map_err(|_| anyhow::anyhow!("invalid compact array length"))
    }
}

impl<'de, T: DecodeGameValue<'de>> DecodeGameValue<'de> for BTreeMap<String, T> {
    /// Reads a checked string dictionary into sorted storage.
    fn read_compact(reader: &mut CompactReader<'de>) -> Result<Self> {
        reader.map(BTreeMap::insert)
    }
}

impl<'de, T: DecodeGameValue<'de>> DecodeGameValue<'de> for HashMap<String, T> {
    /// Reads a checked string dictionary into a hash table.
    fn read_compact(reader: &mut CompactReader<'de>) -> Result<Self> {
        reader.map(HashMap::insert)
    }
}

impl<'de> DecodeGameValue<'de> for serde_json::Value {
    /// Uses the ordinary dynamic reader under the containing value's limits.
    fn read_compact(reader: &mut CompactReader<'de>) -> Result<Self> {
        decode_dynamic(&mut reader.decoder, reader.depth)
    }
}

#[cfg(test)]
mod tests;
