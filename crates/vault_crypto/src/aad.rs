use uuid::Uuid;

/// Build canonical AAD bytes for vault object encryption (schema v1 framing).
///
/// Layout:
/// ```text
/// 0x01
/// car_id            16 bytes (UUID big-endian)
/// object_type_len   u16 BE
/// object_type       UTF-8
/// logical_id        16 bytes
/// chunk_tag         0x00 = none | 0x01 = some
/// chunk_index       i32 BE if some
/// schema_version    i32 BE
/// ```
pub fn aad_v1(
    car_id: Uuid,
    object_type: &str,
    logical_id: Uuid,
    chunk_index: Option<i32>,
    schema_version: i32,
) -> Vec<u8> {
    let ot = object_type.as_bytes();
    let mut out = Vec::with_capacity(1 + 16 + 2 + ot.len() + 16 + 1 + 4 + 4);
    out.push(0x01);
    out.extend_from_slice(car_id.as_bytes());
    out.extend_from_slice(&(ot.len() as u16).to_be_bytes());
    out.extend_from_slice(ot);
    out.extend_from_slice(logical_id.as_bytes());
    match chunk_index {
        None => out.push(0x00),
        Some(i) => {
            out.push(0x01);
            out.extend_from_slice(&i.to_be_bytes());
        }
    }
    out.extend_from_slice(&schema_version.to_be_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_and_distinct() {
        let car = Uuid::nil();
        let logical = Uuid::from_u128(1);
        let a = aad_v1(car, "track_points_chunk", logical, Some(0), 1);
        let b = aad_v1(car, "track_points_chunk", logical, Some(0), 1);
        let c = aad_v1(car, "track_points_chunk", logical, Some(1), 1);
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(a[0], 0x01);
    }
}
