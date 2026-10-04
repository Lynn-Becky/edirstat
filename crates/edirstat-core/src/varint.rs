/// Max bytes required to represent a 64-bit integer in LEB128 varint format.
/// Ceil(64 bits / 7 bits per byte) = 10 bytes.
pub const MAX_VARINT_BYTES: usize = 10;

/// Maps signed integers (`i64`) to unsigned integers (`u64`) such that
/// values with small absolute magnitudes (both positive and negative)
/// map to small unsigned values.
///
/// Positive values are mapped to even numbers: 0 -> 0, 1 -> 2, 2 -> 4...
/// Negative values are mapped to odd numbers: -1 -> 1, -2 -> 3, -3 -> 5...
#[must_use]
#[inline]
pub const fn zigzag_encode(val: i64) -> u64 {
    // Arithmetic right shift by 63 fills the register with the sign bit
    // (all 1s if negative, all 0s if positive).
    // XORing with this mask maps the negative values cleanly.
    ((val << 1) ^ (val >> 63)) as u64
}

/// Restores a signed integer (`i64`) from a ZigZag-encoded unsigned integer (`u64`).
#[must_use]
#[inline]
pub const fn zigzag_decode(val: u64) -> i64 {
    // Shifting right by 1 isolates the magnitude.
    // XORing with the negated lowest bit restores the original sign.
    ((val >> 1) as i64) ^ -((val & 1) as i64)
}

/// Writes an unsigned 64-bit integer to a stream using LEB128 varint encoding.
/// Returns the number of bytes written to the stream.
pub(super) fn write_u64_varint(buf: &mut Vec<u8>, mut val: u64) {
    loop {
        let mut byte = (val & 0x7F) as u8;
        val >>= 7;

        if val != 0 {
            byte |= 0x80;
            buf.push(byte);
        } else {
            buf.push(byte);
            break;
        }
    }
}

/// Reads an unsigned 64-bit integer from a stream using LEB128 varint decoding.
pub(super) const fn read_u64_varint(
    slice: &[u8],
    cursor: &mut usize,
) -> Result<u64, crate::EdirstatError> {
    let mut val = 0u64;
    let mut shift = 0;

    loop {
        if *cursor >= slice.len() {
            return Err(crate::EdirstatError::Decode(
                "varint stream ended unexpectedly",
            ));
        }

        let byte = slice[*cursor];
        *cursor += 1;
        let payload = (byte & 0x7F) as u64;

        // Prevent overflow attacks (larger than 64-bit shifts)
        if shift >= 64 {
            return Err(crate::EdirstatError::Decode(
                "overlong varint exceeds 64 bits",
            ));
        }

        val |= payload << shift;

        if (byte & 0x80) == 0 {
            break;
        }

        shift += 7;
    }
    Ok(val)
}

/// Writes a signed 64-bit integer using `ZigZag` and LEB128 varint encoding.
#[inline]
pub(super) fn write_i64_zigzag(buf: &mut Vec<u8>, val: i64) {
    write_u64_varint(buf, zigzag_encode(val));
}

/// Reads a signed 64-bit integer using `ZigZag` and LEB128 varint decoding.
#[inline]
pub(super) fn read_i64_zigzag(
    slice: &[u8],
    cursor: &mut usize,
) -> Result<i64, crate::EdirstatError> {
    let val = read_u64_varint(slice, cursor)?;
    Ok(zigzag_decode(val))
}

// =============================================================================
// Slice API (For In-Memory / Zero-Allocation Array Buffers)
// =============================================================================

/// Safely transforms a raw, potentially unaligned `u8` slice into an aligned `Vec<u32>`
/// without pointer casting risks.
pub(super) fn u8_slice_to_u32_vec(bytes: &[u8]) -> Vec<u32> {
    let u32_size = std::mem::size_of::<u32>();
    let count = bytes.len() / u32_size;
    let mut vec = vec![0u32; count];

    let target_bytes = bytemuck::cast_slice_mut(&mut vec);
    target_bytes.copy_from_slice(bytes);
    vec
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_zigzag_encode_known_vectors() {
        assert_eq!(zigzag_encode(0), 0);
        assert_eq!(zigzag_encode(-1), 1);
        assert_eq!(zigzag_encode(1), 2);
        assert_eq!(zigzag_encode(-2), 3);
        assert_eq!(zigzag_encode(2), 4);
        assert_eq!(zigzag_encode(i64::MIN), u64::MAX);
        assert_eq!(zigzag_encode(i64::MAX), u64::MAX - 1);
    }

    #[test]
    fn test_zigzag_decode_known_vectors() {
        assert_eq!(zigzag_decode(0), 0);
        assert_eq!(zigzag_decode(1), -1);
        assert_eq!(zigzag_decode(2), 1);
        assert_eq!(zigzag_decode(3), -2);
        assert_eq!(zigzag_decode(4), 2);
        assert_eq!(zigzag_decode(u64::MAX), i64::MIN);
        assert_eq!(zigzag_decode(u64::MAX - 1), i64::MAX);
    }

    #[test]
    fn test_zigzag_roundtrip_boundaries() {
        for value in [
            0i64,
            1,
            -1,
            i64::MAX,
            i64::MIN,
            i64::from(i32::MAX),
            i64::from(i32::MIN),
            86400,
            -86400,
        ] {
            assert_eq!(zigzag_decode(zigzag_encode(value)), value);
        }
    }

    #[test]
    fn test_u64_varint_roundtrip_small_range() -> Result<(), crate::EdirstatError> {
        // 0..=300 covers the 127/128 single-byte -> two-byte boundary.
        for value in 0..=300u64 {
            let mut buf = Vec::new();
            write_u64_varint(&mut buf, value);
            let mut cursor = 0;
            assert_eq!(read_u64_varint(&buf, &mut cursor)?, value);
            assert_eq!(cursor, buf.len());
        }
        Ok(())
    }

    #[test]
    fn test_u64_varint_roundtrip_boundaries() -> Result<(), crate::EdirstatError> {
        for value in [
            u64::MAX,
            1u64 << 32,
            1u64 << 63,
            u64::from(u32::MAX),
            1u64 << 7,
            (1u64 << 7) - 1,
        ] {
            let mut buf = Vec::new();
            write_u64_varint(&mut buf, value);
            let mut cursor = 0;
            assert_eq!(read_u64_varint(&buf, &mut cursor)?, value);
            assert_eq!(cursor, buf.len());
        }
        Ok(())
    }

    #[test]
    fn test_u64_varint_exact_encoding_bytes() {
        let cases: [(u64, &[u8]); 4] = [
            (0, &[0x00]),
            (127, &[0x7F]),
            (128, &[0x80, 0x01]),
            (300, &[0xAC, 0x02]),
        ];
        for (value, expected) in cases {
            let mut buf = Vec::new();
            write_u64_varint(&mut buf, value);
            assert_eq!(buf, expected);
        }
    }

    #[test]
    fn test_read_u64_varint_truncated_stream() {
        // Empty slice: nothing can be consumed.
        let mut cursor = 0;
        let result = read_u64_varint(&[], &mut cursor);
        assert!(matches!(
            result,
            Err(crate::EdirstatError::Decode(
                "varint stream ended unexpectedly"
            ))
        ));
        assert_eq!(cursor, 0);

        // Lone continuation byte: consumed, but no terminator followed.
        let mut cursor = 0;
        let result = read_u64_varint(&[0x80], &mut cursor);
        assert!(matches!(
            result,
            Err(crate::EdirstatError::Decode(
                "varint stream ended unexpectedly"
            ))
        ));
        assert_eq!(cursor, 1);
    }

    #[test]
    fn test_read_u64_varint_overlong() {
        // An 11th byte would require a shift >= 64 bits.
        let buf = [0x80u8; MAX_VARINT_BYTES + 1];
        let mut cursor = 0;
        let result = read_u64_varint(&buf, &mut cursor);
        assert!(matches!(
            result,
            Err(crate::EdirstatError::Decode(
                "overlong varint exceeds 64 bits"
            ))
        ));
    }

    #[test]
    fn test_u64_max_uses_max_varint_bytes() -> Result<(), crate::EdirstatError> {
        let mut buf = Vec::new();
        write_u64_varint(&mut buf, u64::MAX);
        assert_eq!(buf.len(), MAX_VARINT_BYTES);
        let mut cursor = 0;
        assert_eq!(read_u64_varint(&buf, &mut cursor)?, u64::MAX);
        assert_eq!(cursor, buf.len());
        Ok(())
    }

    #[test]
    fn test_u64_varint_sequence_single_cursor() -> Result<(), crate::EdirstatError> {
        let values = [0u64, 127, 128, 300, u64::MAX];
        let mut buf = Vec::new();
        for &value in &values {
            write_u64_varint(&mut buf, value);
        }
        let mut cursor = 0;
        for &expected in &values {
            assert_eq!(read_u64_varint(&buf, &mut cursor)?, expected);
        }
        assert_eq!(cursor, buf.len());
        Ok(())
    }

    #[test]
    fn test_zigzag_stream_roundtrip() -> Result<(), crate::EdirstatError> {
        let values = [0i64, -1, 1, -86400, 86400, i64::MIN, i64::MAX];
        let mut buf = Vec::new();
        for &value in &values {
            write_i64_zigzag(&mut buf, value);
        }
        let mut cursor = 0;
        for &expected in &values {
            assert_eq!(read_i64_zigzag(&buf, &mut cursor)?, expected);
        }
        assert_eq!(cursor, buf.len());
        Ok(())
    }

    #[test]
    fn test_u8_slice_to_u32_vec() {
        assert_eq!(u8_slice_to_u32_vec(&[]), [] as [u32; 0]);

        // bytemuck cast => native-endian interpretation of each 4-byte group.
        let bytes: [u8; 8] = [0x01, 0x02, 0x03, 0x04, 0xAA, 0xBB, 0xCC, 0xDD];
        let words = u8_slice_to_u32_vec(&bytes);
        assert_eq!(words.len(), 2);
        assert_eq!(words[0], u32::from_ne_bytes([0x01, 0x02, 0x03, 0x04]));
        assert_eq!(words[1], u32::from_ne_bytes([0xAA, 0xBB, 0xCC, 0xDD]));
    }
}
