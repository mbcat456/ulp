use crate::format::BLOCK_SIZE;
use crate::frontcode;
use std::collections::HashSet;
use std::io::Write;

#[inline]
fn fnv1a(b: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &c in b {
        h ^= c as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

#[inline]
pub(crate) fn fnv1a128(b: &[u8]) -> u128 {
    let mut h1: u64 = 0xcbf29ce484222325;
    let mut h2: u64 = 0x9e3779b97f4a7c15;
    for &c in b {
        h1 ^= c as u64;
        h1 = h1.wrapping_mul(0x100000001b3);
        h2 ^= c as u64;
        h2 = h2.wrapping_mul(0x100000001b3);
    }
    ((h1 as u128) << 64) | (h2 as u128)
}

#[derive(Default)]
pub(crate) struct DedupCount {
    pub(crate) found: u64,
    pub(crate) dupes: u64,
    pub(crate) saved: u64,
}

#[inline]
pub(crate) fn dedup_write(
    seen: &mut SeenSet,
    lock: &mut dyn Write,
    record: &[u8],
    st: &mut DedupCount,
) -> std::io::Result<()> {
    st.found += 1;
    if seen.insert(fnv1a128(record)) {
        lock.write_all(record)?;
        st.saved += 1;
    } else {
        st.dupes += 1;
    }
    Ok(())
}

#[derive(Default)]
pub(crate) struct FoldHasher(pub(crate) u64);
impl std::hash::Hasher for FoldHasher {
    #[inline]
    fn finish(&self) -> u64 {
        self.0
    }
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = self.0.rotate_left(8) ^ b as u64;
        }
    }
    #[inline]
    fn write_u128(&mut self, i: u128) {
        self.0 = (i as u64) ^ ((i >> 64) as u64);
    }
}
impl std::hash::BuildHasher for FoldHasher {
    type Hasher = FoldHasher;
    #[inline]
    fn build_hasher(&self) -> FoldHasher {
        FoldHasher(0)
    }
}
pub(crate) type SeenSet = HashSet<u128, FoldHasher>;

#[inline]
pub(crate) fn cmp_rev(a: &[u8], b: &[u8]) -> std::cmp::Ordering {
    let mut i = a.len() as isize - 1;
    let mut j = b.len() as isize - 1;
    while i >= 0 && j >= 0 {
        match a[i as usize].cmp(&b[j as usize]) {
            std::cmp::Ordering::Equal => {}
            o => return o,
        }
        i -= 1;
        j -= 1;
    }
    if i < 0 && j < 0 {
        std::cmp::Ordering::Equal
    } else if i < 0 {
        std::cmp::Ordering::Less
    } else {
        std::cmp::Ordering::Greater
    }
}

pub(crate) struct Dedup {
    arena: Vec<u8>,
    hashes: Vec<u64>,
    slots: Vec<Slot>,
    mask: u64,
    pub(crate) count: u32,
}

#[derive(Clone, Copy, Default)]
struct Slot {
    off: u32,
    len: u32,
    ins: u32,
}

impl Dedup {
    pub(crate) fn new(est: usize) -> Self {
        let mut cap = 1024usize;

        while cap < est.saturating_mul(10) / 7 {
            cap <<= 1;
        }
        Dedup {
            arena: Vec::new(),
            hashes: vec![0u64; cap],
            slots: vec![Slot::default(); cap],
            mask: cap as u64 - 1,
            count: 0,
        }
    }

    #[inline]
    pub(crate) fn insert(&mut self, s: &[u8]) -> u32 {
        if self.count as usize * 2 >= self.hashes.len() {
            self.grow();
        }
        let mut h = fnv1a(s);
        if h == 0 {
            h = 1;
        }
        let mut i = (h & self.mask) as usize;
        loop {
            if self.hashes[i] == 0 {
                self.hashes[i] = h;
                self.slots[i] = Slot {
                    off: self.arena.len() as u32,
                    len: s.len() as u32,
                    ins: self.count,
                };
                self.arena.extend_from_slice(s);
                self.count += 1;
                return self.slots[i].ins;
            } else if self.hashes[i] == h {
                let slot = self.slots[i];
                let off = slot.off as usize;
                let len = slot.len as usize;
                if &self.arena[off..off + len] == s {
                    return slot.ins;
                }
                i = (i + 1) & self.mask as usize;
            } else {
                i = (i + 1) & self.mask as usize;
            }
        }
    }

    fn grow(&mut self) {
        let new_cap = self.hashes.len() * 2;
        let old_hashes = std::mem::replace(&mut self.hashes, vec![0u64; new_cap]);
        let old_slots = std::mem::replace(&mut self.slots, vec![Slot::default(); new_cap]);
        self.mask = new_cap as u64 - 1;
        self.count = 0;
        for i in 0..old_hashes.len() {
            if old_hashes[i] != 0 {
                let h = old_hashes[i];
                let mut idx = (h & self.mask) as usize;
                loop {
                    if self.hashes[idx] == 0 {
                        self.hashes[idx] = h;
                        self.slots[idx] = old_slots[i];
                        self.count += 1;
                        break;
                    }
                    idx = (idx + 1) & self.mask as usize;
                }
            }
        }
    }

    #[inline]
    fn slice(&self, slot: usize) -> &[u8] {
        let s = self.slots[slot];
        let off = s.off as usize;
        let len = s.len as usize;
        &self.arena[off..off + len]
    }

    fn sorted_slots(&self, reversed: bool) -> Vec<u32> {
        let mut idx: Vec<u32> = Vec::with_capacity(self.count as usize);
        for i in 0..self.hashes.len() {
            if self.hashes[i] != 0 {
                idx.push(i as u32);
            }
        }
        idx.sort_unstable_by(|&a, &b| {
            let sa = self.slice(a as usize);
            let sb = self.slice(b as usize);
            if reversed {
                cmp_rev(sa, sb)
            } else {
                sa.cmp(sb)
            }
        });
        idx
    }
}

pub(crate) fn serialize_dict(dedup: &mut Dedup, reversed: bool) -> (Vec<u8>, Vec<u32>) {
    let slots = dedup.sorted_slots(reversed);
    let count = slots.len();
    let mut perm = vec![0u32; count];
    for (sorted_id, &slot) in slots.iter().enumerate() {
        perm[dedup.slots[slot as usize].ins as usize] = sorted_id as u32;
    }
    let section = frontcode::encode_keys(
        slots.iter().map(|&s| dedup.slice(s as usize)),
        count,
        BLOCK_SIZE,
        reversed,
    );
    (section, perm)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dedup_insert_and_grow() {
        let mut d = Dedup::new(10);
        let a = d.insert(b"alpha");
        assert_eq!(d.count, 1);
        assert_eq!(d.insert(b"alpha"), a, "same key -> same id");
        let b = d.insert(b"beta");
        assert_ne!(a, b);
        for i in 0..5000 {
            d.insert(format!("key{i}").as_bytes());
        }
        assert_eq!(d.count, 5002);
        let mut seen = std::collections::HashSet::new();
        for i in 0..5000 {
            seen.insert(d.insert(format!("key{i}").as_bytes()));
        }
        assert_eq!(seen.len(), 5000, "distinct keys must map to distinct ids");
    }

    #[test]
    fn dedup_sorted_slots_respect_reversed() {
        let mut d = Dedup::new(4);
        for k in [b"a@z.com".as_slice(), b"b@a.com", b"c@a.com"] {
            d.insert(k);
        }
        let fwd: Vec<Vec<u8>> = d
            .sorted_slots(false)
            .iter()
            .map(|&s| d.slice(s as usize).to_vec())
            .collect();
        assert_eq!(
            fwd,
            vec![
                b"a@z.com".to_vec(),
                b"b@a.com".to_vec(),
                b"c@a.com".to_vec()
            ]
        );
        let rev: Vec<Vec<u8>> = d
            .sorted_slots(true)
            .iter()
            .map(|&s| d.slice(s as usize).to_vec())
            .collect();
        assert_eq!(
            rev,
            vec![
                b"b@a.com".to_vec(),
                b"c@a.com".to_vec(),
                b"a@z.com".to_vec()
            ]
        );
    }

    #[test]
    fn serialize_dict_permutation() {
        let mut d = Dedup::new(4);
        for k in [b"b".as_slice(), b"a", b"c"] {
            d.insert(k);
        }
        let (_section, perm) = serialize_dict(&mut d, false);

        assert_eq!(perm, vec![1, 0, 2]);
    }
}
