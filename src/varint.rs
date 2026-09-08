use crate::format::{corrupt, truncated};

#[inline]
pub fn write_varint(out: &mut Vec<u8>, mut v: u64) {
    loop {
        let mut b = (v & 0x7f) as u8;
        v >>= 7;
        if v != 0 {
            b |= 0x80;
            out.push(b);
        } else {
            out.push(b);
            break;
        }
    }
}

#[inline]
pub fn read_varint(data: &[u8], pos: &mut usize) -> std::io::Result<u64> {
    let mut v: u64 = 0;
    let mut shift = 0;
    loop {
        let b = *data
            .get(*pos)
            .ok_or_else(|| truncated("varint truncated: corrupt .ulp"))?;
        *pos += 1;

        if shift > 63 || (shift == 63 && (b & 0x7f) > 1) {
            return Err(corrupt("varint overflow: value exceeds 64 bits"));
        }
        v |= ((b & 0x7f) as u64) << shift;
        if b & 0x80 == 0 {
            break;
        }
        shift += 7;
    }
    Ok(v)
}

#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "unused in non-test builds; kept for the roundtrip unit tests"
    )
)]
#[inline]
pub fn read_varint_at(data: &[u8], pos: usize) -> std::io::Result<(u64, usize)> {
    let mut p = pos;
    let v = read_varint(data, &mut p)?;
    Ok((v, p))
}

#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "unused in non-test builds; kept for the roundtrip unit tests"
    )
)]
pub fn write_delta(out: &mut Vec<u8>, vals: &[u32]) {
    let mut prev: u64 = 0;
    for &v in vals {
        write_varint(out, v as u64 - prev);
        prev = v as u64;
    }
}

#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "unused in non-test builds; kept for the roundtrip unit tests"
    )
)]
pub fn read_delta_into(data: &[u8], pos: &mut usize, out: &mut Vec<u32>) -> std::io::Result<()> {
    let mut prev: u64 = 0;
    while *pos < data.len() {
        let d = read_varint(data, pos)?;
        prev += d;
        out.push(prev as u32);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varint_roundtrip() {
        for v in [0u64, 1, 127, 128, 16_383, 16_384, u32::MAX as u64, u64::MAX] {
            let mut buf = Vec::new();
            write_varint(&mut buf, v);
            let mut pos = 0;
            assert_eq!(read_varint(&buf, &mut pos).unwrap(), v);
            assert_eq!(pos, buf.len(), "v={v}");
        }
    }

    #[test]
    fn varint_truncated_is_err() {
        let buf = vec![0x80u8];
        let mut pos = 0;
        assert!(read_varint(&buf, &mut pos).is_err());
    }

    #[test]
    fn varint_overflow_is_err() {
        let mut buf = vec![0x80u8; 11];
        buf[10] = 0x00;
        let mut pos = 0;
        assert!(read_varint(&buf, &mut pos).is_err());
    }

    #[test]
    fn varint_overflow_at_10th_byte_is_err() {
        let mut buf = vec![0x80u8; 9];
        buf.push(0x02);
        let mut pos = 0;
        assert!(read_varint(&buf, &mut pos).is_err());
    }

    #[test]
    fn varint_read_at_roundtrip() {
        for v in [0u64, 1, 127, 128, 16_384, u64::MAX] {
            let mut buf = Vec::new();
            write_varint(&mut buf, v);
            write_varint(&mut buf, 7);
            let (a, p) = read_varint_at(&buf, 0).unwrap();
            let (b, _) = read_varint_at(&buf, p).unwrap();
            assert_eq!((a, b), (v, 7));
        }
    }

    #[test]
    fn delta_roundtrip() {
        let vals: Vec<u32> = vec![0, 1, 2, 5, 1000, 40_000, u32::MAX];
        let mut buf = Vec::new();
        write_delta(&mut buf, &vals);
        let mut got = Vec::new();
        let mut pos = 0;
        read_delta_into(&buf, &mut pos, &mut got).unwrap();
        assert_eq!(got, vals);
        assert_eq!(pos, buf.len());
    }
}
