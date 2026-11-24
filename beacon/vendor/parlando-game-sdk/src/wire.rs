//! Reads and validates the runtime's JSON-compatible subset of MessagePack.

use anyhow::{bail, ensure, Context, Result};
use rmp::Marker;
use std::collections::HashSet;

pub(crate) const BYTE_LIMIT: usize = 8 * 1024 * 1024;
pub(crate) const NODE_LIMIT: usize = 262_144;
pub(crate) const DEPTH_LIMIT: usize = 64;

/// Checks a complete message without building a value tree.
pub(crate) fn validate(bytes: &[u8]) -> Result<()> {
    ensure!(
        bytes.len() <= BYTE_LIMIT,
        "component payload exceeds byte limit"
    );
    let mut decoder = Decoder::new(bytes);
    decoder.check_value(0)?;
    ensure!(decoder.bytes.is_empty(), "trailing MessagePack bytes");
    Ok(())
}

/// Tracks the unchecked suffix of a single observation and its remaining node budget.
pub(crate) struct Decoder<'a> {
    bytes: &'a [u8],
    nodes_left: usize,
}

impl<'a> Decoder<'a> {
    /// Starts reading an independent payload with a fresh node budget.
    pub(crate) fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            nodes_left: NODE_LIMIT,
        }
    }

    /// Consumes the next complete field, leaving the cursor unchanged on truncation.
    fn take(&mut self, len: usize) -> Result<&'a [u8]> {
        let (field, remaining) = self
            .bytes
            .split_at_checked(len)
            .context("truncated MessagePack observation")?;
        self.bytes = remaining;
        Ok(field)
    }

    /// Reports whether the entire payload has been consumed.
    pub(crate) fn is_finished(&self) -> bool {
        self.bytes.is_empty()
    }

    /// Checks for nil without consuming the next node.
    pub(crate) fn next_is_nil(&self) -> bool {
        self.bytes.first() == Some(&0xc0)
    }

    /// Borrows one complete value for reordering a named record without copying its data.
    pub(crate) fn value_bytes(&mut self) -> Result<&'a [u8]> {
        let before = self.bytes;
        self.skip_bounded(0)?;
        Ok(&before[..before.len() - self.bytes.len()])
    }

    /// Advances over one codec-produced value without inspecting unused scalar data.
    ///
    /// Container shape, marker support, truncation, nesting and node limits
    /// still apply. Only a consumer that reads a skipped scalar needs UTF-8
    /// or finite-number validation.
    pub(crate) fn skip_bounded(&mut self, depth: usize) -> Result<()> {
        let marker = self.marker()?;
        let (entries, map) = match marker {
            Marker::FixArray(len) => (usize::from(len), false),
            Marker::Array16 => (usize::from(u16::from_be_bytes(self.fixed()?)), false),
            Marker::Array32 => (u32::from_be_bytes(self.fixed()?) as usize, false),
            Marker::FixMap(len) => (usize::from(len), true),
            Marker::Map16 => (usize::from(u16::from_be_bytes(self.fixed()?)), true),
            Marker::Map32 => (u32::from_be_bytes(self.fixed()?) as usize, true),
            Marker::FixPos(_) | Marker::FixNeg(_) | Marker::Null | Marker::True | Marker::False => {
                return Ok(())
            }
            _ => {
                let size = match marker {
                    Marker::U8 | Marker::I8 => 1,
                    Marker::U16 | Marker::I16 => 2,
                    Marker::U32 | Marker::I32 | Marker::F32 => 4,
                    Marker::U64 | Marker::I64 | Marker::F64 => 8,
                    Marker::FixStr(_) | Marker::Str8 | Marker::Str16 | Marker::Str32 => {
                        self.string_len(marker)?
                    }
                    _ => bail!("unsupported MessagePack marker in component payload: {marker:?}"),
                };
                self.take(size)?;
                return Ok(());
            }
        };
        self.check_container(entries, depth, if map { 2 } else { 1 })?;
        for _ in 0..entries {
            if map {
                let key = self.marker()?;
                let length = self
                    .string_len(key)
                    .context("invalid MessagePack object key")?;
                self.take(length)?;
            }
            self.skip_bounded(depth + 1)?;
        }
        Ok(())
    }

    /// Reads a marker and charges its value or map key against the node budget.
    pub(crate) fn marker(&mut self) -> Result<Marker> {
        self.nodes_left = self
            .nodes_left
            .checked_sub(1)
            .context("component payload exceeds node limit")?;
        Ok(Marker::from(self.take(1)?[0]))
    }

    /// Reads a fixed-width big-endian field after its marker.
    fn fixed<const N: usize>(&mut self) -> Result<[u8; N]> {
        Ok(self.take(N)?.try_into().expect("take returned exact width"))
    }

    /// Reads a string payload after its marker and validates UTF-8.
    pub(crate) fn string(&mut self, marker: Marker) -> Result<&'a str> {
        let len = self.string_len(marker)?;
        std::str::from_utf8(self.take(len)?).context("invalid UTF-8 in component payload")
    }

    /// Reads only the length metadata of a MessagePack string.
    fn string_len(&mut self, marker: Marker) -> Result<usize> {
        Ok(match marker {
            Marker::FixStr(len) => usize::from(len),
            Marker::Str8 => usize::from(self.take(1)?[0]),
            Marker::Str16 => usize::from(u16::from_be_bytes(self.fixed()?)),
            Marker::Str32 => u32::from_be_bytes(self.fixed()?) as usize,
            _ => bail!("expected a MessagePack string"),
        })
    }

    /// Checks an advertised container length using the minimum cost of each entry.
    pub(crate) fn check_container(
        &self,
        len: usize,
        depth: usize,
        nodes_per_entry: usize,
    ) -> Result<()> {
        ensure!(
            depth < DEPTH_LIMIT,
            "component payload exceeds nesting limit"
        );
        ensure!(
            len <= self.nodes_left / nodes_per_entry,
            "component payload exceeds node limit"
        );
        ensure!(
            len <= self.bytes.len() / nodes_per_entry,
            "truncated MessagePack container"
        );
        Ok(())
    }

    /// Validates the value and its descendants, counting map keys as nodes.
    fn check_value(&mut self, depth: usize) -> Result<()> {
        match self.next()? {
            Token::Array(len) => {
                self.check_container(len, depth, 1)?;
                for _ in 0..len {
                    self.check_value(depth + 1)?;
                }
            }
            Token::Map(len) => {
                self.check_container(len, depth, 2)?;
                let mut keys = HashSet::new();
                for _ in 0..len {
                    let marker = self.marker()?;
                    let key = self
                        .string(marker)
                        .context("invalid MessagePack object key")?;
                    ensure!(keys.insert(key), "duplicate MessagePack object key");
                    self.check_value(depth + 1)?;
                }
            }
            Token::Float(value) => ensure!(value.is_finite(), "nonfinite MessagePack number"),
            _ => {}
        }
        Ok(())
    }

    /// Reads a scalar or a container header. Container children remain unread.
    pub(crate) fn next(&mut self) -> Result<Token<'a>> {
        let marker = self.marker()?;
        Ok(match marker {
            Marker::Null => Token::Null,
            Marker::False => Token::Bool(false),
            Marker::True => Token::Bool(true),
            Marker::FixPos(value) => Token::Unsigned((value).into()),
            Marker::FixNeg(value) => Token::Signed((value).into()),
            Marker::U8 => Token::Unsigned((self.take(1)?[0]).into()),
            Marker::U16 => Token::Unsigned((u16::from_be_bytes(self.fixed()?)).into()),
            Marker::U32 => Token::Unsigned((u32::from_be_bytes(self.fixed()?)).into()),
            Marker::U64 => Token::Unsigned(u64::from_be_bytes(self.fixed()?)),
            Marker::I8 => Token::Signed((i8::from_be_bytes(self.fixed()?)).into()),
            Marker::I16 => Token::Signed((i16::from_be_bytes(self.fixed()?)).into()),
            Marker::I32 => Token::Signed((i32::from_be_bytes(self.fixed()?)).into()),
            Marker::I64 => Token::Signed(i64::from_be_bytes(self.fixed()?)),
            Marker::F32 => Token::Float(f32::from_be_bytes(self.fixed()?).into()),
            Marker::F64 => Token::Float(f64::from_be_bytes(self.fixed()?)),
            Marker::FixStr(_) | Marker::Str8 | Marker::Str16 | Marker::Str32 => {
                Token::String(self.string(marker)?)
            }
            Marker::FixArray(len) => Token::Array(len.into()),
            Marker::Array16 => {
                let len = u16::from_be_bytes(self.fixed()?);
                Token::Array(len.into())
            }
            Marker::Array32 => {
                let len = u32::from_be_bytes(self.fixed()?);
                Token::Array(len as usize)
            }
            Marker::FixMap(len) => Token::Map(len.into()),
            Marker::Map16 => {
                let len = u16::from_be_bytes(self.fixed()?);
                Token::Map(len.into())
            }
            Marker::Map32 => {
                let len = u32::from_be_bytes(self.fixed()?);
                Token::Map(len as usize)
            }
            _ => bail!("unsupported MessagePack marker in component payload: {marker:?}"),
        })
    }
}

/// One token from the supported MessagePack subset. Strings borrow the wire.
pub(crate) enum Token<'a> {
    Null,
    Bool(bool),
    Signed(i64),
    Unsigned(u64),
    Float(f64),
    String(&'a str),
    Array(usize),
    Map(usize),
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Checks JSON representation of containers, UTF-8, extrema, and floats.
    #[test]
    fn preserves_supported_values() {
        let bytes = [
            0x81, 0xa1, b'x', 0x97, 0xc0, 0xc3, 0xa2, 0xc3, 0xa9, 0xcf, 0xff, 0xff, 0xff, 0xff,
            0xff, 0xff, 0xff, 0xff, 0xd3, 0x80, 0, 0, 0, 0, 0, 0, 0, 0xca, 0x3f, 0x80, 0, 0, 0xcb,
            0x3f, 0xe0, 0, 0, 0, 0, 0, 0,
        ];
        assert!(validate(&bytes).is_ok());
    }

    /// Rejects malformed and non-JSON inputs without coercion.
    #[test]
    fn rejects_invalid_observations() {
        for bytes in [
            &[][..],
            &[0xc0, 0xc0],
            &[0x81, 0xa1, b'x'],
            &[0x81, 0x01, 0xc0],
            &[0x82, 0xa1, b'x', 0xc0, 0xa1, b'x', 0xc0],
            &[0xc4, 0],
            &[0xd4, 0, 0],
            &[0xc1],
            &[0xa1, 0xff],
            &[0xca, 0x7f, 0xc0, 0, 0],
            &[0xca, 0xff, 0x80, 0, 0],
            &[0xdb, 0xff, 0xff, 0xff, 0xff],
        ] {
            assert!(validate(bytes).is_err(), "accepted {bytes:02x?}");
        }
    }

    /// Validates boundary cases and rejects deceptive lengths before allocating.
    #[test]
    fn enforces_size_and_depth_limits() {
        let mut nested = vec![0x91; DEPTH_LIMIT];
        nested.push(0xc0);
        assert!(validate(&nested).is_ok());
        nested.insert(0, 0x91);
        assert!(validate(&nested)
            .unwrap_err()
            .to_string()
            .contains("nesting"));

        let mut wide = vec![0xdd];
        wide.extend_from_slice(&((NODE_LIMIT - 1) as u32).to_be_bytes());
        wide.resize(5 + NODE_LIMIT - 1, 0xc0);
        assert!(validate(&wide).is_ok());
        wide[1..5].copy_from_slice(&(NODE_LIMIT as u32).to_be_bytes());
        assert!(validate(&wide).unwrap_err().to_string().contains("node"));
        assert!(validate(&[0xdf, 0xff, 0xff, 0xff, 0xff]).is_err());

        let oversized = vec![0; BYTE_LIMIT + 1];
        assert!(validate(&oversized)
            .unwrap_err()
            .to_string()
            .contains("byte"));
    }
}
