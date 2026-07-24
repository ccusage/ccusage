//! Minimal protobuf wire-format reader for Antigravity's `GeneratorMetadata`
//! and trajectory metadata blobs. Supports varint, fixed64, length-delimited,
//! and fixed32 fields; group wire types terminate parsing (Antigravity does
//! not emit them) and truncated input stops iteration instead of failing.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FieldValue<'a> {
    Varint(u64),
    Fixed64(u64),
    LengthDelimited(&'a [u8]),
    Fixed32(u32),
}

/// Reads all top-level fields of a protobuf message. Malformed or unsupported
/// wire types end the read and return the fields decoded so far.
pub(super) fn read_fields(bytes: &[u8]) -> Vec<(u32, FieldValue<'_>)> {
    let mut fields = Vec::new();
    let mut rest = bytes;
    while !rest.is_empty() {
        let Some((key, tail)) = read_varint(rest) else {
            break;
        };
        rest = tail;
        let field_number = u32::try_from(key >> 3).unwrap_or(u32::MAX);
        match key & 0x7 {
            0 => {
                let Some((value, tail)) = read_varint(rest) else {
                    break;
                };
                fields.push((field_number, FieldValue::Varint(value)));
                rest = tail;
            }
            1 => {
                if rest.len() < 8 {
                    break;
                }
                let (value, tail) = rest.split_at(8);
                fields.push((
                    field_number,
                    FieldValue::Fixed64(u64::from_le_bytes(value.try_into().unwrap_or([0; 8]))),
                ));
                rest = tail;
            }
            2 => {
                let Some((length, tail)) = read_varint(rest) else {
                    break;
                };
                let Ok(length) = usize::try_from(length) else {
                    break;
                };
                if tail.len() < length {
                    break;
                }
                let (value, tail) = tail.split_at(length);
                fields.push((field_number, FieldValue::LengthDelimited(value)));
                rest = tail;
            }
            5 => {
                if rest.len() < 4 {
                    break;
                }
                let (value, tail) = rest.split_at(4);
                fields.push((
                    field_number,
                    FieldValue::Fixed32(u32::from_le_bytes(value.try_into().unwrap_or([0; 4]))),
                ));
                rest = tail;
            }
            // Groups (3, 4) and padding wire types are unsupported; stop here.
            _ => break,
        }
    }
    fields
}

fn read_varint(bytes: &[u8]) -> Option<(u64, &[u8])> {
    let mut value = 0_u64;
    let mut shift = 0_u32;
    for (index, byte) in bytes.iter().enumerate() {
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Some((value, &bytes[index + 1..]));
        }
        shift += 7;
        if shift >= 64 {
            return None;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_varint_and_length_delimited_fields() {
        // field 1 varint 150, field 2 LEN "ab"
        let bytes = [0x08, 0x96, 0x01, 0x12, 0x02, b'a', b'b'];
        let fields = read_fields(&bytes);

        assert_eq!(
            fields,
            vec![
                (1, FieldValue::Varint(150)),
                (2, FieldValue::LengthDelimited(b"ab")),
            ]
        );
    }

    #[test]
    fn stops_on_group_wire_type() {
        // field 1 varint 1, then field 2 with wire type 3 (start group)
        let bytes = [0x08, 0x01, 0x13, 0x08, 0x02];
        let fields = read_fields(&bytes);

        assert_eq!(fields, vec![(1, FieldValue::Varint(1))]);
    }

    #[test]
    fn stops_on_truncated_length_delimited_field() {
        let bytes = [0x12, 0x05, b'a'];
        let fields = read_fields(&bytes);

        assert!(fields.is_empty());
    }

    #[test]
    fn stops_on_unterminated_varint() {
        let bytes = [0x08, 0x80];
        let fields = read_fields(&bytes);

        assert!(fields.is_empty());
    }

    #[test]
    fn reads_fixed_width_fields() {
        let mut bytes = vec![0x09];
        bytes.extend_from_slice(&42_u64.to_le_bytes());
        bytes.push(0x15);
        bytes.extend_from_slice(&7_u32.to_le_bytes());
        let fields = read_fields(&bytes);

        assert_eq!(
            fields,
            vec![(1, FieldValue::Fixed64(42)), (2, FieldValue::Fixed32(7))]
        );
    }
}
