//! Bounded output shared by the Serde serializers and visitors.

use std::io;

use crate::wire::BYTE_LIMIT;

/// Owns the output buffer and remaining quota while encoding JSON-shaped values.
pub(crate) struct Encoder {
    pub(crate) bytes: Vec<u8>,
    pub(crate) nodes_left: usize,
}

impl Encoder {
    /// Counts a value or map key toward the payload limit.
    pub(crate) fn charge(&mut self) -> io::Result<()> {
        self.nodes_left = self
            .nodes_left
            .checked_sub(1)
            .ok_or_else(|| io::Error::other("component payload exceeds node limit"))?;
        Ok(())
    }

    /// Begins a variable-length container whose count will be set after reading it.
    pub(crate) fn header(&mut self, marker: u8) -> io::Result<usize> {
        let start = self.bytes.len() + 1;
        io::Write::write_all(self, &[marker, 0, 0, 0, 0])?;
        Ok(start)
    }

    /// Writes the actual child or entry count into its reserved MessagePack header.
    pub(crate) fn finish(&mut self, start: usize, count: u32) {
        self.bytes[start..start + 4].copy_from_slice(&count.to_be_bytes());
    }
}

impl io::Write for Encoder {
    /// Refuses writes beyond the payload byte quota.
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > BYTE_LIMIT - self.bytes.len() {
            return Err(io::Error::other("component payload exceeds byte limit"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    /// Completes a write to the in-memory buffer.
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
