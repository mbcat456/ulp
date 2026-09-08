use crate::format::{corrupt, truncated};
use crate::varint::{read_varint, write_varint};
use std::cmp::Ordering;
use std::collections::HashMap;

#[inline]
fn common_prefix(a: &[u8], b: &[u8]) -> usize {
    let n = a.len().min(b.len());
    let mut i = 0;
    while i < n && a[i] == b[i] {
        i += 1;
    }
    i
}

#[inline]
fn common_suffix(a: &[u8], b: &[u8]) -> usize {
    a.iter()
        .rev()
        .zip(b.iter().rev())
        .take_while(|(x, y)| x == y)
        .count()
}

pub fn encode_keys<'k, I>(mut keys: I, n: usize, block_size: usize, reversed: bool) -> Vec<u8>
where
    I: Iterator<Item = &'k [u8]>,
{
    let num_blocks = if n == 0 { 0 } else { n.div_ceil(block_size) };

    let mut data = Vec::with_capacity(n * 4);
    let mut offsets: Vec<u64> = Vec::with_capacity(num_blocks);

    for b in 0..num_blocks {
        let s = b * block_size;
        let e = (s + block_size).min(n);
        offsets.push(data.len() as u64);

        #[expect(
            clippy::expect_used,
            reason = "n keys promised by serialize_dict; fewer keys is a programmer error"
        )]
        let first = keys.next().expect("encode_keys: ran out of keys before the promised count; serialize_dict under-counted its dictionary");
        write_varint(&mut data, first.len() as u64);
        if reversed {
            data.extend(first.iter().rev().copied());
        } else {
            data.extend_from_slice(first);
        }
        let mut prev: &'k [u8] = first;
        for _ in s + 1..e {
            #[expect(
                clippy::expect_used,
                reason = "n keys promised by serialize_dict; fewer keys is a programmer error"
            )]
            let cur: &[u8] = keys.next().expect("encode_keys: ran out of keys mid-block; serialize_dict under-counted its dictionary");
            let shared = if reversed {
                common_suffix(prev, cur)
            } else {
                common_prefix(prev, cur)
            };
            write_varint(&mut data, shared as u64);
            write_varint(&mut data, (cur.len() - shared) as u64);
            if reversed {
                data.extend(cur[..cur.len() - shared].iter().rev().copied());
            } else {
                data.extend_from_slice(&cur[shared..]);
            }
            prev = cur;
        }
    }

    let mut out = Vec::with_capacity(data.len() + num_blocks * 8 + 64);
    write_varint(&mut out, n as u64);
    write_varint(&mut out, block_size as u64);
    write_varint(&mut out, num_blocks as u64);
    write_varint(&mut out, data.len() as u64);
    out.push(if reversed { 1 } else { 0 });
    for &o in &offsets {
        out.extend_from_slice(&o.to_le_bytes());
    }
    out.extend_from_slice(&data);
    out
}

#[inline]
fn read_slice<'a>(data: &'a [u8], pos: &mut usize, len: usize) -> std::io::Result<&'a [u8]> {
    let end = (*pos)
        .checked_add(len)
        .ok_or_else(|| corrupt("dict slice length overflow: corrupt .ulp"))?;
    let s = data
        .get(*pos..end)
        .ok_or_else(|| truncated("dict data truncated: corrupt .ulp"))?;
    *pos = end;
    Ok(s)
}

pub struct Dict<'a> {
    pub count: u64,
    pub block_size: u64,
    pub num_blocks: u64,
    pub reversed: bool,
    block_offsets: &'a [u8],
    data: &'a [u8],
}

impl<'a> Dict<'a> {
    pub fn parse(section: &'a [u8]) -> std::io::Result<Dict<'a>> {
        let mut p = 0usize;
        let count = read_varint(section, &mut p)?;
        let block_size = read_varint(section, &mut p)?;
        let num_blocks = read_varint(section, &mut p)?;
        let data_len = read_varint(section, &mut p)?;

        if block_size == 0 {
            return Err(corrupt("dict block_size is zero: corrupt .ulp"));
        }
        if num_blocks != count.div_ceil(block_size) {
            return Err(corrupt(
                "dict num_blocks does not match count/block_size: corrupt .ulp",
            ));
        }
        let reversed = match section.get(p) {
            Some(&1) => true,
            Some(&0) => false,
            Some(_) => return Err(corrupt("dict reversed flag is not 0/1: corrupt .ulp")),
            None => return Err(truncated("dict reversed flag missing: corrupt .ulp")),
        };
        p += 1;
        let offs_len = (num_blocks as usize)
            .checked_mul(8)
            .ok_or_else(|| corrupt("dict num_blocks overflows: corrupt .ulp"))?;
        let off_end = p
            .checked_add(offs_len)
            .ok_or_else(|| corrupt("dict block-offset table overflows: corrupt .ulp"))?;
        let block_offsets = section
            .get(p..off_end)
            .ok_or_else(|| truncated("dict block-offset table truncated: corrupt .ulp"))?;
        let data_end = off_end
            .checked_add(data_len as usize)
            .ok_or_else(|| corrupt("dict data length overflows: corrupt .ulp"))?;
        let data = section
            .get(off_end..data_end)
            .ok_or_else(|| truncated("dict data truncated: corrupt .ulp"))?;

        if count > data.len() as u64 {
            return Err(corrupt("dict count exceeds encoded data: corrupt .ulp"));
        }
        Ok(Dict {
            count,
            block_size,
            num_blocks,
            reversed,
            block_offsets,
            data,
        })
    }

    #[inline]
    fn block_start(&self, b: usize) -> std::io::Result<usize> {
        let o = b
            .checked_mul(8)
            .ok_or_else(|| corrupt("dict block index overflows: corrupt .ulp"))?;
        let s: [u8; 8] = self
            .block_offsets
            .get(o..o + 8)
            .and_then(|s| s.try_into().ok())
            .ok_or_else(|| truncated("dict block offset out of range: corrupt .ulp"))?;
        Ok(u64::from_le_bytes(s) as usize)
    }

    #[inline]
    fn block_bounds(&self, b: usize) -> std::io::Result<(u64, u64)> {
        let start = (b as u64)
            .checked_mul(self.block_size)
            .ok_or_else(|| corrupt("dict block bounds overflow: corrupt .ulp"))?;
        let end = start
            .checked_add(self.block_size)
            .ok_or_else(|| corrupt("dict block bounds overflow: corrupt .ulp"))?
            .min(self.count);
        Ok((start, end))
    }

    #[inline]
    fn read_first_into(&self, pos: &mut usize, out: &mut Vec<u8>) -> std::io::Result<()> {
        let len = read_varint(self.data, pos)? as usize;
        out.clear();
        out.extend_from_slice(read_slice(self.data, pos, len)?);
        Ok(())
    }

    fn read_first_append(&self, pos: &mut usize, out: &mut Vec<u8>) -> std::io::Result<()> {
        let len = read_varint(self.data, pos)? as usize;
        out.extend_from_slice(read_slice(self.data, pos, len)?);
        Ok(())
    }

    fn read_next_into(
        &self,
        pos: &mut usize,
        prev: &[u8],
        out: &mut Vec<u8>,
    ) -> std::io::Result<()> {
        let shared = read_varint(self.data, pos)? as usize;
        let slen = read_varint(self.data, pos)? as usize;
        let prefix = prev
            .get(..shared)
            .ok_or_else(|| corrupt("dict shared prefix exceeds previous key: corrupt .ulp"))?;
        let suffix = read_slice(self.data, pos, slen)?;
        out.clear();
        out.extend_from_slice(prefix);
        out.extend_from_slice(suffix);
        Ok(())
    }

    fn first_of_block_into(&self, b: usize, out: &mut Vec<u8>) -> std::io::Result<()> {
        let mut pos = self.block_start(b)?;
        self.read_first_into(&mut pos, out)
    }

    pub fn key_into(&self, i: u64, out: &mut Vec<u8>) -> std::io::Result<()> {
        let b = (i / self.block_size) as usize;
        let within = (i % self.block_size) as usize;
        let mut pos = self.block_start(b)?;
        let start = out.len();
        self.read_first_append(&mut pos, out)?;
        let mut prev_end = out.len();
        for _ in 0..within {
            let shared = read_varint(self.data, &mut pos)? as usize;
            let slen = read_varint(self.data, &mut pos)? as usize;
            if shared > prev_end - start {
                return Err(corrupt(
                    "dict shared prefix exceeds previous key: corrupt .ulp",
                ));
            }
            let suffix = read_slice(self.data, &mut pos, slen)?;
            out.truncate(start + shared);
            out.extend_from_slice(suffix);
            prev_end = start + shared + slen;
        }
        Ok(())
    }

    pub fn prefix_range(&self, prefix: &[u8]) -> std::io::Result<(u64, u64)> {
        if self.count == 0 || prefix.is_empty() {
            return Ok((0, 0));
        }

        let mut blo = 0usize;
        let mut bhi = self.num_blocks as usize;
        let mut block_key = Vec::new();
        while blo < bhi {
            let mid = (blo + bhi) / 2;
            self.first_of_block_into(mid, &mut block_key)?;
            if block_key.as_slice().cmp(prefix) == Ordering::Less {
                blo = mid + 1;
            } else {
                bhi = mid;
            }
        }
        let start_block = if blo == 0 { 0 } else { blo - 1 };

        let mut lo = self.count;
        let mut hi = self.count;
        let mut prev: Vec<u8> = Vec::new();
        'outer: for b in start_block..self.num_blocks as usize {
            let (s, e) = self.block_bounds(b)?;
            let mut pos = self.block_start(b)?;
            let len = read_varint(self.data, &mut pos)? as usize;
            prev.clear();
            prev.extend_from_slice(read_slice(self.data, &mut pos, len)?);
            let mut i = s;
            loop {
                if prev.as_slice().starts_with(prefix) {
                    if lo == self.count {
                        lo = i;
                    }
                    hi = i + 1;
                } else if prev.as_slice().cmp(prefix) == Ordering::Greater {
                    break 'outer;
                }
                i += 1;
                if i >= e {
                    break;
                }
                let shared = read_varint(self.data, &mut pos)? as usize;
                let slen = read_varint(self.data, &mut pos)? as usize;
                if shared > prev.len() {
                    return Err(corrupt(
                        "dict shared prefix exceeds previous key: corrupt .ulp",
                    ));
                }
                prev.truncate(shared);
                prev.extend_from_slice(read_slice(self.data, &mut pos, slen)?);
            }
        }
        Ok((lo, hi))
    }

    pub fn decode_into(&self, arena: &mut Vec<u8>, offsets: &mut Vec<u32>) -> std::io::Result<()> {
        offsets.clear();
        offsets.push(0u32);
        let mut prev: Vec<u8> = Vec::new();
        for b in 0..self.num_blocks {
            let (start, end) = self.block_bounds(b as usize)?;
            let mut pos = self.block_start(b as usize)?;

            let len = read_varint(self.data, &mut pos)? as usize;
            prev.clear();
            prev.extend_from_slice(read_slice(self.data, &mut pos, len)?);
            for i in start..end {
                if self.reversed {
                    let at = arena.len();
                    arena.extend_from_slice(&prev);
                    arena[at..].reverse();
                } else {
                    arena.extend_from_slice(&prev);
                }
                offsets.push(arena.len() as u32);
                if i + 1 < end {
                    let shared = read_varint(self.data, &mut pos)? as usize;
                    let slen = read_varint(self.data, &mut pos)? as usize;
                    if shared > prev.len() {
                        return Err(corrupt(
                            "dict shared prefix exceeds previous key: corrupt .ulp",
                        ));
                    }
                    prev.truncate(shared);
                    prev.extend_from_slice(read_slice(self.data, &mut pos, slen)?);
                }
            }
        }
        Ok(())
    }

    pub fn decode_selected_into(
        &self,
        wanted: &[u8],
        arena: &mut Vec<u8>,
        offsets: &mut Vec<u32>,
        remap: &mut HashMap<usize, u32>,
    ) -> std::io::Result<()> {
        offsets.clear();
        offsets.push(0u32);
        remap.clear();
        let mut prev: Vec<u8> = Vec::new();
        let mut next_pos: u32 = 0;
        for b in 0..self.num_blocks {
            let (start, end) = self.block_bounds(b as usize)?;

            let w0 = (start / 8) as usize;
            let w1 = (end as usize).div_ceil(8);
            let any = (w0..w1).any(|wi| wanted.get(wi).is_some_and(|&byte| byte != 0));
            if !any {
                continue;
            }
            let mut pos = self.block_start(b as usize)?;
            let len = read_varint(self.data, &mut pos)? as usize;
            prev.clear();
            prev.extend_from_slice(read_slice(self.data, &mut pos, len)?);
            for i in start..end {
                let id = i as usize;
                let bit = *wanted
                    .get(id >> 3)
                    .ok_or_else(|| corrupt("wanted bitmap too short for dict count"))?;
                if (bit >> (id & 7)) & 1 == 1 {
                    if self.reversed {
                        let at = arena.len();
                        arena.extend_from_slice(&prev);
                        arena[at..].reverse();
                    } else {
                        arena.extend_from_slice(&prev);
                    }
                    offsets.push(arena.len() as u32);
                    remap.insert(id, next_pos);
                    next_pos += 1;
                }
                if i + 1 < end {
                    let shared = read_varint(self.data, &mut pos)? as usize;
                    let slen = read_varint(self.data, &mut pos)? as usize;
                    if shared > prev.len() {
                        return Err(corrupt(
                            "dict shared prefix exceeds previous key: corrupt .ulp",
                        ));
                    }
                    prev.truncate(shared);
                    prev.extend_from_slice(read_slice(self.data, &mut pos, slen)?);
                }
            }
        }
        Ok(())
    }

    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "allocating convenience wrapper used by the frontcode tests"
        )
    )]
    pub fn search(&self, query: &[u8]) -> std::io::Result<Option<u64>> {
        let mut scratch = Vec::new();
        self.search_with_scratch(query, &mut scratch)
    }

    pub fn search_with_scratch(
        &self,
        query: &[u8],
        scratch: &mut Vec<u8>,
    ) -> std::io::Result<Option<u64>> {
        if self.count == 0 {
            return Ok(None);
        }
        if self.reversed {
            scratch.clear();
            scratch.extend(query.iter().rev().copied());
            return self.search_key(scratch);
        }
        self.search_key(query)
    }

    fn search_key(&self, query: &[u8]) -> std::io::Result<Option<u64>> {
        let mut lo = 0usize;
        let mut hi = self.num_blocks as usize;
        let mut block_key = Vec::new();
        while lo < hi {
            let mid = (lo + hi) / 2;
            self.first_of_block_into(mid, &mut block_key)?;
            if block_key.as_slice().cmp(query) != Ordering::Greater {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        if lo == 0 {
            return Ok(None);
        }
        let b = lo - 1;
        let mut pos = self.block_start(b)?;
        let mut cur = Vec::new();
        let mut next = Vec::new();
        self.read_first_into(&mut pos, &mut cur)?;
        let (mut idx, block_end) = self.block_bounds(b)?;
        loop {
            match cur.as_slice().cmp(query) {
                Ordering::Equal => return Ok(Some(idx)),
                Ordering::Greater => return Ok(None),
                Ordering::Less => {}
            }
            idx += 1;
            if idx >= block_end {
                return Ok(None);
            }
            self.read_next_into(&mut pos, &cur, &mut next)?;
            std::mem::swap(&mut cur, &mut next);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decoded(d: &Dict<'_>) -> Vec<Vec<u8>> {
        let mut arena = Vec::new();
        let mut offs = Vec::new();
        d.decode_into(&mut arena, &mut offs).unwrap();
        (0..d.count as usize)
            .map(|i| arena[offs[i] as usize..offs[i + 1] as usize].to_vec())
            .collect()
    }

    fn sort_by_key_form(mut keys: Vec<&[u8]>, reversed: bool) -> Vec<&[u8]> {
        let sk = |s: &[u8]| -> Vec<u8> {
            if reversed {
                s.iter().rev().copied().collect()
            } else {
                s.to_vec()
            }
        };
        keys.sort_by_key(|a| sk(a));
        keys
    }

    #[test]
    fn encode_decode_roundtrip_small_blocks() {
        let keys: Vec<&[u8]> = vec![b"a", b"aa", b"ab", b"abc", b"b", b"bb", b"zz"];
        for reversed in [false, true] {
            let sorted = sort_by_key_form(keys.clone(), reversed);
            let section = encode_keys(sorted.clone().into_iter(), sorted.len(), 3, reversed);
            let dict = Dict::parse(&section).unwrap();
            assert_eq!(dict.count, 7);
            assert_eq!(dict.block_size, 3);
            assert_eq!(dict.num_blocks, 3);
            assert_eq!(dict.reversed, reversed);
            let originals = decoded(&dict);
            assert_eq!(
                originals,
                sorted.iter().map(|s| s.to_vec()).collect::<Vec<_>>(),
                "reversed={reversed}"
            );
        }
    }

    #[test]
    fn search_hit_and_miss() {
        let keys: Vec<&[u8]> = vec![b"a", b"aa", b"ab", b"abc", b"b", b"bb", b"zz"];
        let section = encode_keys(keys.clone().into_iter(), keys.len(), 3, false);
        let dict = Dict::parse(&section).unwrap();
        for (i, k) in keys.iter().enumerate() {
            assert_eq!(dict.search(k).unwrap(), Some(i as u64));
        }
        assert_eq!(dict.search(b"ac").unwrap(), None, "between ab and abc");
        assert_eq!(dict.search(b"").unwrap(), None, "smaller than first key");
        assert_eq!(dict.search(b"zzz").unwrap(), None, "beyond last key");
    }

    #[test]
    fn search_reversed_dict() {
        let keys: Vec<&[u8]> = vec![b"user1@qq.com", b"user2@qq.com", b"user3@gmail.com"];
        let sorted = sort_by_key_form(keys.clone(), true);
        let section = encode_keys(sorted.into_iter(), keys.len(), 3, true);
        let dict = Dict::parse(&section).unwrap();
        let hit = dict.search(b"user2@qq.com").unwrap().unwrap();
        let mut original = Vec::new();
        dict.key_into(hit, &mut original).unwrap();
        original.reverse();
        assert_eq!(original, b"user2@qq.com");
        let hit = dict.search(b"user3@gmail.com").unwrap().unwrap();
        let mut original = Vec::new();
        dict.key_into(hit, &mut original).unwrap();
        original.reverse();
        assert_eq!(original, b"user3@gmail.com");
        assert_eq!(dict.search(b"user4@qq.com").unwrap(), None);
    }

    #[test]
    fn key_into_matches_key() {
        let keys: Vec<Vec<u8>> = (0..20)
            .map(|i| format!("key{i:04}"))
            .map(String::into_bytes)
            .collect();
        let sorted = sort_by_key_form(keys.iter().map(Vec::as_slice).collect(), true);
        let expected: Vec<Vec<u8>> = sorted
            .iter()
            .map(|s| s.iter().rev().copied().collect())
            .collect();
        let section = encode_keys(sorted.into_iter(), 20, 4, true);
        let dict = Dict::parse(&section).unwrap();
        for i in 0..dict.count {
            let mut out = vec![0xAA];
            dict.key_into(i, &mut out).unwrap();
            assert_eq!(&out[1..], expected[i as usize].as_slice(), "entry {i}");
        }
    }

    #[test]
    fn prefix_range_bounds() {
        let keys: Vec<&[u8]> = vec![b"ab", b"abc", b"abd", b"b", b"bb"];
        let section = encode_keys(keys.clone().into_iter(), keys.len(), 2, false);
        let dict = Dict::parse(&section).unwrap();
        assert_eq!(dict.prefix_range(b"ab").unwrap(), (0, 3));
        assert_eq!(dict.prefix_range(b"b").unwrap(), (3, 5));
        assert_eq!(dict.prefix_range(b"bb").unwrap(), (4, 5));
        assert_eq!(dict.prefix_range(b"c").unwrap(), (5, 5));
        assert_eq!(dict.prefix_range(b"").unwrap(), (0, 0));
    }

    #[test]
    fn decode_selected_subset() {
        let keys: Vec<&[u8]> = vec![b"aa", b"ab", b"ac", b"ba", b"bb", b"bc", b"ca"];
        let section = encode_keys(keys.clone().into_iter(), keys.len(), 8, false);
        let dict = Dict::parse(&section).unwrap();
        let mut wanted = vec![0u8; 1];
        wanted[0] = (1 << 1) | (1 << 3) | (1 << 5);
        let (mut arena, mut offs, mut remap) = (Vec::new(), Vec::new(), HashMap::new());
        dict.decode_selected_into(&wanted, &mut arena, &mut offs, &mut remap)
            .unwrap();
        let strings: Vec<Vec<u8>> = (0..offs.len() - 1)
            .map(|i| arena[offs[i] as usize..offs[i + 1] as usize].to_vec())
            .collect();
        assert_eq!(
            strings,
            vec![b"ab".to_vec(), b"ba".to_vec(), b"bc".to_vec()]
        );
        assert_eq!(remap.get(&1), Some(&0));
        assert_eq!(remap.get(&3), Some(&1));
        assert_eq!(remap.get(&5), Some(&2));
        assert_eq!(remap.get(&0), None);
        assert_eq!(remap.get(&6), None);
    }

    #[test]
    fn decode_selected_skips_whole_blocks() {
        let keys: Vec<Vec<u8>> = (0..130)
            .map(|i| format!("key{i:04}"))
            .map(String::into_bytes)
            .collect();
        let section = encode_keys(keys.iter().map(|k| k.as_slice()), keys.len(), 64, false);
        let dict = Dict::parse(&section).unwrap();
        let mut wanted = vec![0u8; 130usize.div_ceil(8)];
        wanted[64 >> 3] |= 1 << (64 & 7);
        wanted[100 >> 3] |= 1 << (100 & 7);
        let (mut arena, mut offs, mut remap) = (Vec::new(), Vec::new(), HashMap::new());
        dict.decode_selected_into(&wanted, &mut arena, &mut offs, &mut remap)
            .unwrap();
        let strings: Vec<Vec<u8>> = (0..offs.len() - 1)
            .map(|i| arena[offs[i] as usize..offs[i + 1] as usize].to_vec())
            .collect();
        assert_eq!(strings, vec![b"key0064".to_vec(), b"key0100".to_vec()]);
        assert_eq!(remap.get(&64), Some(&0));
        assert_eq!(remap.get(&100), Some(&1));
        assert_eq!(remap.get(&0), None);
        assert_eq!(remap.get(&63), None);
        assert_eq!(remap.get(&65), None);
    }

    #[test]
    fn parse_truncated_is_err() {
        let keys: Vec<&[u8]> = vec![b"a", b"ab", b"b"];
        let section = encode_keys(keys.clone().into_iter(), keys.len(), 2, false);

        let cut = &section[..section.len() - 1];
        assert!(Dict::parse(cut).is_err());
    }

    #[test]
    fn parse_rejects_inconsistent_block_metadata() {
        let keys: Vec<&[u8]> = vec![b"a", b"ab", b"abc", b"b"];
        let section = encode_keys(keys.clone().into_iter(), keys.len(), 64, false);

        let mut patched = section.clone();
        patched[2] = 2;
        assert!(Dict::parse(&patched).is_err());

        let mut zero = section.clone();
        zero[1] = 0;
        assert!(Dict::parse(&zero).is_err());
    }

    #[test]
    fn decode_selected_skips_blocks_with_small_block_size() {
        let keys: Vec<Vec<u8>> = (0..10)
            .map(|i| format!("key{i:04}"))
            .map(String::into_bytes)
            .collect();
        let section = encode_keys(keys.iter().map(|k| k.as_slice()), keys.len(), 2, false);
        let dict = Dict::parse(&section).unwrap();
        let mut wanted = vec![0u8; 10usize.div_ceil(8)];
        wanted[7 >> 3] |= 1 << (7 & 7);
        wanted[9 >> 3] |= 1 << (9 & 7);
        let (mut arena, mut offs, mut remap) = (Vec::new(), Vec::new(), HashMap::new());
        dict.decode_selected_into(&wanted, &mut arena, &mut offs, &mut remap)
            .unwrap();
        let strings: Vec<Vec<u8>> = (0..offs.len() - 1)
            .map(|i| arena[offs[i] as usize..offs[i + 1] as usize].to_vec())
            .collect();
        assert_eq!(strings, vec![b"key0007".to_vec(), b"key0009".to_vec()]);
        assert_eq!(remap.get(&7), Some(&0));
        assert_eq!(remap.get(&9), Some(&1));
    }

    #[test]
    fn decode_rejects_shared_prefix_longer_than_key() {
        let mut data = Vec::new();
        write_varint(&mut data, 1);
        data.push(b'a');
        write_varint(&mut data, 2);
        write_varint(&mut data, 1);
        data.push(b'b');

        let mut sec = Vec::new();
        write_varint(&mut sec, 2);
        write_varint(&mut sec, 64);
        write_varint(&mut sec, 1);
        write_varint(&mut sec, data.len() as u64);
        sec.push(0);
        sec.extend_from_slice(&0u64.to_le_bytes());
        sec.extend_from_slice(&data);

        let dict = Dict::parse(&sec).unwrap();
        let (mut arena, mut offs) = (Vec::new(), Vec::new());
        assert!(dict.decode_into(&mut arena, &mut offs).is_err());
        let (mut arena2, mut offs2, mut remap) = (Vec::new(), Vec::new(), HashMap::new());
        assert!(
            dict.decode_selected_into(&[0xFF], &mut arena2, &mut offs2, &mut remap)
                .is_err()
        );
        assert!(dict.prefix_range(b"x").is_err());
    }
}
