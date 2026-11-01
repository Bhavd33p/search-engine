//! Variable-byte (VByte) integer encoding.
//!
//! Each byte carries 7 bits of the number, least-significant group first. The
//! high bit of a byte says "another byte follows". Small numbers — and the gaps
//! between sorted doc ids and positions are mostly small — take one byte
//! instead of the four a fixed-width `u32` needs.
//!
//! ```text
//!   5   -> 0000_0101                  (1 byte)
//!   130 -> 1000_0010 0000_0001        (2 bytes: 130 = 2 + 1·128)
//! ```
//!
//! This is the same scheme as protobuf varints and LEB128.

use thiserror::Error;

const PAYLOAD_MASK: u8 = 0x7F;
const CONTINUATION_BIT: u8 = 0x80;
/// A `u32` needs at most ceil(32 / 7) = 5 bytes.
const MAX_BYTES_PER_U32: usize = 5;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum DecodeError {
    #[error("unexpected end of input at byte {0}")]
    UnexpectedEof(usize),
    #[error("variable-byte integer at byte {0} does not fit in a u32")]
    Overflow(usize),
    #[error("{0} unexpected trailing bytes")]
    TrailingBytes(usize),
    #[error("delta-decoded value does not fit in a u32")]
    GapOverflow,
}

/// Appends `value` to `out` in VByte form.
pub fn encode_u32(mut value: u32, out: &mut Vec<u8>) {
    while value >= u32::from(CONTINUATION_BIT) {
        out.push((value as u8 & PAYLOAD_MASK) | CONTINUATION_BIT);
        value >>= 7;
    }
    out.push(value as u8);
}

/// Number of bytes `value` occupies once encoded.
pub fn encoded_len(value: u32) -> usize {
    let significant_bits = 32 - value.leading_zeros() as usize;
    significant_bits.div_ceil(7).max(1)
}

/// Reads VByte integers one after another from a byte slice.
pub struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    pub fn read_u32(&mut self) -> Result<u32, DecodeError> {
        let start = self.offset;
        let mut value: u64 = 0;
        for i in 0..MAX_BYTES_PER_U32 {
            let byte = *self
                .bytes
                .get(self.offset)
                .ok_or(DecodeError::UnexpectedEof(self.offset))?;
            self.offset += 1;
            value |= u64::from(byte & PAYLOAD_MASK) << (7 * i);
            if byte & CONTINUATION_BIT == 0 {
                return u32::try_from(value).map_err(|_| DecodeError::Overflow(start));
            }
        }
        Err(DecodeError::Overflow(start))
    }

    /// Fails unless every byte has been consumed — catches corrupted blobs.
    pub fn finish(self) -> Result<(), DecodeError> {
        match self.bytes.len() - self.offset {
            0 => Ok(()),
            remaining => Err(DecodeError::TrailingBytes(remaining)),
        }
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn encode_all(values: &[u32]) -> Vec<u8> {
        let mut out = Vec::new();
        for &v in values {
            encode_u32(v, &mut out);
        }
        out
    }

    fn decode_all(bytes: &[u8], count: usize) -> Result<Vec<u32>, DecodeError> {
        let mut reader = Reader::new(bytes);
        let values = (0..count).map(|_| reader.read_u32()).collect::<Result<Vec<_>, _>>()?;
        reader.finish()?;
        Ok(values)
    }

    #[test]
    fn encodes_known_values() {
        assert_eq!(encode_all(&[0]), [0x00]);
        assert_eq!(encode_all(&[5]), [0x05]);
        assert_eq!(encode_all(&[127]), [0x7F]);
        assert_eq!(encode_all(&[128]), [0x80, 0x01]);
        assert_eq!(encode_all(&[130]), [0x82, 0x01]);
        assert_eq!(encode_all(&[16_384]), [0x80, 0x80, 0x01]);
        assert_eq!(encode_all(&[u32::MAX]), [0xFF, 0xFF, 0xFF, 0xFF, 0x0F]);
    }

    #[test]
    fn encoded_len_matches_encoding() {
        for value in [0, 1, 127, 128, 16_383, 16_384, 2_097_151, 2_097_152, u32::MAX] {
            assert_eq!(encoded_len(value), encode_all(&[value]).len(), "value {value}");
        }
    }

    #[test]
    fn truncated_input_is_an_error() {
        assert_eq!(decode_all(&[0x80], 1), Err(DecodeError::UnexpectedEof(1)));
        assert_eq!(decode_all(&[], 1), Err(DecodeError::UnexpectedEof(0)));
    }

    #[test]
    fn values_wider_than_u32_are_rejected() {
        assert_eq!(
            decode_all(&[0xFF, 0xFF, 0xFF, 0xFF, 0x1F], 1),
            Err(DecodeError::Overflow(0))
        );
        assert_eq!(
            decode_all(&[0x80, 0x80, 0x80, 0x80, 0x80, 0x01], 1),
            Err(DecodeError::Overflow(0))
        );
    }

    #[test]
    fn trailing_bytes_are_rejected() {
        assert_eq!(decode_all(&[0x01, 0x02], 1), Err(DecodeError::TrailingBytes(1)));
    }

    proptest! {
        #[test]
        fn round_trips_any_sequence(values in proptest::collection::vec(any::<u32>(), 0..200)) {
            let bytes = encode_all(&values);
            prop_assert_eq!(decode_all(&bytes, values.len()), Ok(values));
        }
    }
}
