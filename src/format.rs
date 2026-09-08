use std::io::{Read, Seek};

pub(crate) const MAGIC: [u8; 8] = *b"ULPFMT01";

pub const VERSION: u32 = 1;
pub(crate) const BLOCK_SIZE: usize = 64;
pub(crate) const HEADER_LEN: usize = 128;

#[inline]
pub(crate) fn corrupt(msg: &'static str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, msg)
}

#[inline]
pub(crate) fn truncated(msg: &'static str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::UnexpectedEof, msg)
}

#[inline]
pub(crate) fn r64(b: &[u8], o: usize) -> Option<u64> {
    let s: [u8; 8] = b.get(o..o + 8)?.try_into().ok()?;
    Some(u64::from_le_bytes(s))
}

#[inline]
pub(crate) fn r32(b: &[u8], o: usize) -> Option<u32> {
    let s: [u8; 4] = b.get(o..o + 4)?.try_into().ok()?;
    Some(u32::from_le_bytes(s))
}

pub(crate) fn records_section_len_u64(record_count: u64, url_count: u64) -> Option<u64> {
    let url_offsets = url_count.checked_add(1)?.checked_mul(4)?;
    let columns = record_count.checked_mul(8)?;
    let sep_bits = record_count.div_ceil(8).checked_mul(2)?;
    8u64.checked_add(url_offsets)?
        .checked_add(columns)?
        .checked_add(sep_bits)
}

pub(crate) fn posting_section_min_len_u64(count: u64) -> Option<u64> {
    8u64.checked_add(count.checked_add(1)?.checked_mul(4)?)
}

pub(crate) fn segments_of(s: &[u8]) -> std::io::Result<Vec<&[u8]>> {
    if s.len() < 8 {
        return Ok(vec![s]);
    }
    let Some(footer_off) = r64(s, s.len() - 8) else {
        return Ok(vec![s]);
    };
    let footer_off = footer_off as usize;
    if footer_off == 0 || footer_off.checked_add(8).is_none_or(|v| v > s.len()) {
        return Ok(vec![s]);
    }
    let Some(seg_count) = r64(s, footer_off) else {
        return Ok(vec![s]);
    };
    let seg_count = seg_count as usize;
    if seg_count == 0 || seg_count > 1_000_000 {
        return Ok(vec![s]);
    }

    let table_fits = seg_count
        .checked_mul(8)
        .and_then(|t| t.checked_add(8))
        .and_then(|t| t.checked_add(footer_off))
        .is_some_and(|end| end <= s.len());
    if !table_fits {
        return Ok(vec![s]);
    }
    let mut offs = Vec::with_capacity(seg_count + 1);
    for i in 0..seg_count {
        match r64(s, footer_off + 8 + i * 8) {
            Some(o) => offs.push(o as usize),
            None => return Ok(vec![s]),
        }
    }
    offs.push(footer_off);
    if offs.first() != Some(&0) || offs.windows(2).any(|w| w[0] >= w[1]) {
        return Err(corrupt(
            "footer offset table is not strictly increasing: corrupt .ulp",
        ));
    }
    let mut segs = Vec::with_capacity(seg_count);
    for pair in offs.windows(2) {
        if pair[1] <= s.len() {
            segs.push(&s[pair[0]..pair[1]]);
        }
    }
    Ok(segs)
}

pub(crate) fn read_segment_meta(path: &str) -> std::io::Result<(Vec<u64>, u64)> {
    let mut f = std::fs::File::open(path)?;
    let len = f.metadata()?.len();
    if len < 8 {
        return Ok((vec![0], len));
    }
    let mut tail = [0u8; 8];
    f.seek(std::io::SeekFrom::Start(len - 8))?;
    f.read_exact(&mut tail)?;
    let footer_off = u64::from_le_bytes(tail);
    if footer_off == 0 {
        return Ok((vec![0], len));
    }

    let footer_end = footer_off
        .checked_add(8)
        .ok_or_else(|| corrupt("footer pointer overflows: corrupt .ulp"))?;
    if footer_end > len {
        return Ok((vec![0], len));
    }
    let mut c = [0u8; 8];
    f.seek(std::io::SeekFrom::Start(footer_off))?;
    f.read_exact(&mut c)?;
    let seg_count = u64::from_le_bytes(c);
    if seg_count == 0 || seg_count > 1_000_000 {
        return Err(corrupt("footer segment count is invalid: corrupt .ulp"));
    }

    let table_end = footer_end
        .checked_add(seg_count * 8)
        .ok_or_else(|| corrupt("footer offset table overflows: corrupt .ulp"))?;
    if table_end > len {
        return Err(corrupt("footer offset table is truncated: corrupt .ulp"));
    }
    let mut offsets = Vec::with_capacity(seg_count as usize);
    for _ in 0..seg_count {
        f.read_exact(&mut c)?;
        offsets.push(u64::from_le_bytes(c));
    }
    Ok((offsets, footer_off))
}

pub(crate) struct Header {
    pub(crate) version: u32,
    pub(crate) record_count: u64,
    pub(crate) url_count: u64,
    pub(crate) user_count: u64,
    pub(crate) pass_count: u64,
    pub(crate) misc_count: u64,
    pub(crate) url_dict_off: u64,
    pub(crate) user_dict_off: u64,
    pub(crate) pass_dict_off: u64,
    pub(crate) records_off: u64,
    pub(crate) user_idx_off: u64,
    pub(crate) pass_idx_off: u64,
    pub(crate) misc_off: u64,
}

pub(crate) fn parse_header(s: &[u8]) -> std::io::Result<Header> {
    if s.len() < HEADER_LEN {
        return Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            format!(
                "truncated header ({} bytes < {}): corrupt .ulp",
                s.len(),
                HEADER_LEN
            ),
        ));
    }
    if s[..8] != MAGIC {
        return Err(corrupt("bad magic: not a .ulp file"));
    }

    let field =
        |o: usize| r64(s, o).ok_or_else(|| truncated("header field out of bounds: corrupt .ulp"));
    Ok(Header {
        version: r32(s, 8).ok_or_else(|| truncated("header field out of bounds: corrupt .ulp"))?,
        record_count: field(16)?,
        url_count: field(24)?,
        user_count: field(32)?,
        pass_count: field(40)?,
        misc_count: field(48)?,
        url_dict_off: field(64)?,
        user_dict_off: field(72)?,
        pass_dict_off: field(80)?,
        records_off: field(88)?,
        user_idx_off: field(96)?,
        pass_idx_off: field(104)?,
        misc_off: field(112)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marker_bytes(seed: usize, len: usize) -> Vec<u8> {
        (0..len).map(|i| ((seed + i) % 251) as u8).collect()
    }

    #[test]
    fn byte_readers() {
        let b = [0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08];
        assert_eq!(r64(&b, 0), Some(u64::from_le_bytes(b)));
        assert_eq!(r32(&b, 0), Some(0x04030201));
        assert_eq!(r32(&b, 4), Some(0x08070605));
        assert_eq!(r32(&b, 5), None, "short read must not panic");
        assert_eq!(r64(&b, 1), None, "short read must not panic");
    }

    #[test]
    fn segments_of_valid_two_segment_file() {
        let s1 = marker_bytes(0, 100);
        let s2 = marker_bytes(100, 80);
        let mut f = s1.clone();
        f.extend_from_slice(&s2);
        let footer_off = f.len();
        f.extend_from_slice(&2u64.to_le_bytes());
        f.extend_from_slice(&0u64.to_le_bytes());
        f.extend_from_slice(&(s1.len() as u64).to_le_bytes());
        f.extend_from_slice(&(footer_off as u64).to_le_bytes());
        assert_eq!(segments_of(&f).unwrap(), vec![s1.as_slice(), s2.as_slice()]);
    }

    #[test]
    fn segments_of_degrades_to_legacy() {
        let data = marker_bytes(7, 200);

        assert_eq!(segments_of(&data[..7]).unwrap(), vec![&data[..7]]);

        let mut f = data.clone();
        f.extend_from_slice(&0u64.to_le_bytes());
        assert_eq!(segments_of(&f).unwrap(), vec![f.as_slice()]);

        let mut g = data.clone();
        g.extend_from_slice(&u64::MAX.to_le_bytes());
        assert_eq!(segments_of(&g).unwrap(), vec![g.as_slice()]);

        let mut h = data.clone();
        let fo = h.len();
        h.extend_from_slice(&0u64.to_le_bytes());
        h.extend_from_slice(&(fo as u64).to_le_bytes());
        assert_eq!(segments_of(&h).unwrap(), vec![h.as_slice()]);

        let mut k = data.clone();
        let fo = k.len();
        k.extend_from_slice(&3u64.to_le_bytes());
        k.extend_from_slice(&0u64.to_le_bytes());
        k.extend_from_slice(&(fo as u64).to_le_bytes());
        assert_eq!(segments_of(&k).unwrap(), vec![k.as_slice()]);
    }

    #[test]
    fn segments_of_rejects_bad_offset_tables() {
        let s1 = marker_bytes(0, 100);
        let s2 = marker_bytes(100, 80);
        for offsets in [vec![1u64, s1.len() as u64], vec![0u64, 0u64]] {
            let mut f = s1.clone();
            f.extend_from_slice(&s2);
            let footer_off = f.len() as u64;
            f.extend_from_slice(&2u64.to_le_bytes());
            for off in &offsets {
                f.extend_from_slice(&off.to_le_bytes());
            }
            f.extend_from_slice(&footer_off.to_le_bytes());
            assert!(
                segments_of(&f).is_err(),
                "offset table {offsets:?} must be refused, not silently skipped"
            );
        }
    }

    #[test]
    fn read_segment_meta_garbage_footer_never_panics() {
        let seg = marker_bytes(3, 200);
        let mut f = seg.clone();
        let footer_off = f.len();
        f.extend_from_slice(&1u64.to_le_bytes());
        f.extend_from_slice(&0u64.to_le_bytes());
        f.extend_from_slice(&(footer_off as u64).to_le_bytes());

        let dir = tempfile::tempdir().unwrap();
        let write = |name: &str, bytes: &[u8], ptr: u64, seg_count: Option<u64>| {
            let mut g = bytes.to_vec();
            let n = g.len();
            g[n - 8..].copy_from_slice(&ptr.to_le_bytes());
            if let Some(sc) = seg_count {
                g[ptr as usize..ptr as usize + 8].copy_from_slice(&sc.to_le_bytes());
            }
            let p = dir.path().join(name);
            std::fs::write(&p, &g).unwrap();
            p.to_str().unwrap().to_string()
        };

        let p = write("ok.ulp", &f, footer_off as u64, None);
        assert_eq!(read_segment_meta(&p).unwrap(), (vec![0], footer_off as u64));

        let p = write("nearmax.ulp", &f, u64::MAX - 3, None);
        let e = read_segment_meta(&p).unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::InvalidData);

        let p = write("midrange.ulp", &f, 100, Some(0));
        let e = read_segment_meta(&p).unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::InvalidData);

        let p = write("shorttable.ulp", &f, 100, Some(20));
        let e = read_segment_meta(&p).unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::InvalidData);

        let p = write("zero.ulp", &f, 0, None);
        assert_eq!(
            read_segment_meta(&p).unwrap(),
            (vec![0], footer_off as u64 + 24)
        );
        let p = write("beyond.ulp", &f, (footer_off + 24) as u64, None);
        assert_eq!(
            read_segment_meta(&p).unwrap(),
            (vec![0], footer_off as u64 + 24)
        );
    }

    fn make_header(record_count: u64) -> Vec<u8> {
        let mut hdr = Vec::with_capacity(HEADER_LEN);
        hdr.extend_from_slice(&MAGIC);
        hdr.extend_from_slice(&VERSION.to_le_bytes());
        hdr.extend_from_slice(&0u32.to_le_bytes());
        hdr.extend_from_slice(&record_count.to_le_bytes());
        hdr.extend_from_slice(&11u64.to_le_bytes());
        hdr.extend_from_slice(&22u64.to_le_bytes());
        hdr.extend_from_slice(&33u64.to_le_bytes());
        hdr.extend_from_slice(&44u64.to_le_bytes());
        hdr.extend_from_slice(&0u64.to_le_bytes());
        hdr.extend_from_slice(&1u64.to_le_bytes());
        hdr.extend_from_slice(&2u64.to_le_bytes());
        hdr.extend_from_slice(&3u64.to_le_bytes());
        hdr.extend_from_slice(&4u64.to_le_bytes());
        hdr.extend_from_slice(&5u64.to_le_bytes());
        hdr.extend_from_slice(&6u64.to_le_bytes());
        hdr.extend_from_slice(&7u64.to_le_bytes());
        hdr.resize(HEADER_LEN, 0);
        hdr
    }

    #[test]
    fn parse_header_fields() {
        let h = parse_header(&make_header(999)).unwrap();
        assert_eq!(h.version, 1, "format version read from header bytes 8..12");
        assert_eq!(h.record_count, 999);
        assert_eq!(h.url_count, 11);
        assert_eq!(h.user_count, 22);
        assert_eq!(h.pass_count, 33);
        assert_eq!(h.misc_count, 44);
        assert_eq!(h.url_dict_off, 1);
        assert_eq!(h.user_dict_off, 2);
        assert_eq!(h.pass_dict_off, 3);
        assert_eq!(h.records_off, 4);
        assert_eq!(h.user_idx_off, 5);
        assert_eq!(h.pass_idx_off, 6);
        assert_eq!(h.misc_off, 7);
    }

    #[test]
    fn parse_header_truncated_is_err() {
        assert!(parse_header(&[0u8; 64]).is_err());
    }

    #[test]
    fn parse_header_bad_magic_is_err() {
        let mut hdr = make_header(1);
        hdr[0] = b'X';
        assert!(parse_header(&hdr).is_err());
    }
}
