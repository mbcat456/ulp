use crate::dict::{DedupCount, FoldHasher, SeenSet, dedup_write};
use crate::domain::contains;
use crate::format::{corrupt, r32, segments_of, truncated};
use crate::merge::bin_field_len;
use crate::mmap::MappedFile;
use crate::reader::Reader;
use crate::varint::read_varint;
use std::collections::HashMap;
use std::io::Write;

#[inline]
fn col_u32(s: &[u8], base: usize, i: usize) -> std::io::Result<u32> {
    let o = base
        .checked_add(
            i.checked_mul(4)
                .ok_or_else(|| corrupt("column index overflows: corrupt .ulp"))?,
        )
        .ok_or_else(|| corrupt("column offset overflows: corrupt .ulp"))?;
    r32(s, o).ok_or_else(|| truncated("column value out of range: corrupt .ulp"))
}

#[inline]
fn sep_bit(s: &[u8], base: usize, idx: u64) -> std::io::Result<u8> {
    let off = base
        .checked_add((idx as usize) / 8)
        .ok_or_else(|| corrupt("record index overflows: corrupt .ulp"))?;
    let b = *s
        .get(off)
        .ok_or_else(|| truncated("separator bit out of range: corrupt .ulp"))?;
    Ok(if (b >> (idx % 8)) & 1 == 1 {
        b'|'
    } else {
        b':'
    })
}

#[inline]
fn set_bit(bits: &mut [u8], id: usize) -> std::io::Result<()> {
    let slot = bits
        .get_mut(id >> 3)
        .ok_or_else(|| corrupt("id exceeds bitmap: corrupt .ulp"))?;
    *slot |= 1 << (id & 7);
    Ok(())
}

#[inline]
fn arena_direct<'a>(arena: &'a [u8], offsets: &[u32], id: usize) -> std::io::Result<&'a [u8]> {
    let start = *offsets
        .get(id)
        .ok_or_else(|| corrupt("id exceeds offset table: corrupt .ulp"))? as usize;
    let end = *offsets
        .get(id + 1)
        .ok_or_else(|| corrupt("id exceeds offset table: corrupt .ulp"))? as usize;
    arena
        .get(start..end)
        .ok_or_else(|| corrupt("arena range out of bounds: corrupt .ulp"))
}

#[inline]
fn arena_selected<'a>(
    arena: &'a [u8],
    offsets: &[u32],
    remap: &HashMap<usize, u32>,
    id: usize,
) -> std::io::Result<&'a [u8]> {
    let pos = remap
        .get(&id)
        .copied()
        .ok_or_else(|| corrupt("id not selected in decode remap: corrupt .ulp"))?
        as usize;
    let start = *offsets
        .get(pos)
        .ok_or_else(|| corrupt("position exceeds offset table: corrupt .ulp"))?
        as usize;
    let end = *offsets
        .get(pos + 1)
        .ok_or_else(|| corrupt("position exceeds offset table: corrupt .ulp"))?
        as usize;
    arena
        .get(start..end)
        .ok_or_else(|| corrupt("arena range out of bounds: corrupt .ulp"))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RecordMode {
    Text,

    Frames,
}

fn push_record(
    out: &mut Vec<u8>,
    mode: RecordMode,
    url: &[u8],
    sep1: u8,
    user: &[u8],
    sep2: u8,
    pass: &[u8],
) -> std::io::Result<()> {
    match mode {
        RecordMode::Text => {
            out.extend_from_slice(url);
            out.push(sep1);
            out.extend_from_slice(user);
            out.push(sep2);
            out.extend_from_slice(pass);
            out.push(b'\n');
            Ok(())
        }
        RecordMode::Frames => {
            let ul = bin_field_len(url.len(), "url")?;
            let nl = bin_field_len(user.len(), "user")?;
            let pl = bin_field_len(pass.len(), "pass")?;
            out.extend_from_slice(&ul.to_le_bytes());
            out.extend_from_slice(url);
            out.extend_from_slice(&nl.to_le_bytes());
            out.extend_from_slice(user);
            out.extend_from_slice(&pl.to_le_bytes());
            out.extend_from_slice(pass);
            Ok(())
        }
    }
}

fn query_postings_fast_varint(
    seg: &[u8],
    blob: std::ops::Range<usize>,
    known_user: Option<&[u8]>,
    known_pass: Option<&[u8]>,
    lock: &mut dyn Write,
    seen: &mut SeenSet,
    st: &mut DedupCount,
) -> std::io::Result<()> {
    let r = Reader::new(seg)?;
    let user_col_base = r.user_col_base as usize;
    let pass_col_base = r.pass_col_base as usize;
    let sep_base = r.sep_base as usize;
    let sep2_base = r.sep2_base as usize;

    let mut url_wanted = vec![0u8; (r.hdr.url_count as usize).div_ceil(8)];
    let mut user_wanted = if known_user.is_none() {
        Some(vec![0u8; (r.hdr.user_count as usize).div_ceil(8)])
    } else {
        None
    };
    let mut pass_wanted = if known_pass.is_none() {
        Some(vec![0u8; (r.hdr.pass_count as usize).div_ceil(8)])
    } else {
        None
    };

    let mut pos = blob.start;
    let mut prev: u64 = 0;
    while pos < blob.end {
        let d = read_varint(seg, &mut pos)?;
        prev = prev
            .checked_add(d)
            .ok_or_else(|| corrupt("posting index overflows: corrupt .ulp"))?;
        let idx = prev;
        let url_id = r.url_id_of(idx)? as usize;
        set_bit(&mut url_wanted, url_id)?;
        if let Some(uw) = user_wanted.as_mut() {
            let user_id = col_u32(seg, user_col_base, idx as usize)? as usize;
            set_bit(uw, user_id)?;
        }
        if let Some(pw) = pass_wanted.as_mut() {
            let pass_id = col_u32(seg, pass_col_base, idx as usize)? as usize;
            set_bit(pw, pass_id)?;
        }
    }

    let mut url_arena: Vec<u8> = Vec::new();
    let mut url_off: Vec<u32> = Vec::new();
    let mut url_remap: HashMap<usize, u32> = HashMap::new();
    r.url_dict
        .decode_selected_into(&url_wanted, &mut url_arena, &mut url_off, &mut url_remap)?;
    let mut user_arena: Vec<u8> = Vec::new();
    let mut user_off: Vec<u32> = Vec::new();
    let mut user_remap: HashMap<usize, u32> = HashMap::new();
    if let Some(uw) = &user_wanted {
        r.user_dict
            .decode_selected_into(uw, &mut user_arena, &mut user_off, &mut user_remap)?;
    }
    let mut pass_arena: Vec<u8> = Vec::new();
    let mut pass_off: Vec<u32> = Vec::new();
    let mut pass_remap: HashMap<usize, u32> = HashMap::new();
    if let Some(pw) = &pass_wanted {
        r.pass_dict
            .decode_selected_into(pw, &mut pass_arena, &mut pass_off, &mut pass_remap)?;
    }

    let mut out: Vec<u8> = Vec::with_capacity(1 << 16);
    pos = blob.start;
    prev = 0;
    while pos < blob.end {
        let d = read_varint(seg, &mut pos)?;
        prev = prev
            .checked_add(d)
            .ok_or_else(|| corrupt("posting index overflows: corrupt .ulp"))?;
        let idx = prev;
        let url_id = r.url_id_of(idx)? as usize;
        let sep1 = sep_bit(seg, sep_base, idx)?;
        let sep2 = sep_bit(seg, sep2_base, idx)?;
        out.clear();
        out.extend_from_slice(arena_selected(&url_arena, &url_off, &url_remap, url_id)?);
        out.push(sep1);
        if let Some(u) = known_user {
            out.extend_from_slice(u);
        } else {
            let user_id = col_u32(seg, user_col_base, idx as usize)? as usize;
            out.extend_from_slice(arena_selected(
                &user_arena,
                &user_off,
                &user_remap,
                user_id,
            )?);
        }
        out.push(sep2);
        if let Some(p) = known_pass {
            out.extend_from_slice(p);
        } else {
            let pass_id = col_u32(seg, pass_col_base, idx as usize)? as usize;
            out.extend_from_slice(arena_selected(
                &pass_arena,
                &pass_off,
                &pass_remap,
                pass_id,
            )?);
        }
        out.push(b'\n');
        dedup_write(seen, lock, &out, st)?;
    }
    Ok(())
}

pub fn query(path: &str, field: &str, value: &str, sink: &mut dyn Write) -> std::io::Result<()> {
    query_bytes(path, field, value.as_bytes(), sink)
}

pub fn query_bytes(
    path: &str,
    field: &str,
    value: &[u8],
    sink: &mut dyn Write,
) -> std::io::Result<()> {
    if !matches!(field, "url" | "user" | "pass") {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("unknown field '{field}' (expected url|user|pass)"),
        ));
    }
    let t0 = std::time::Instant::now();
    let m = MappedFile::open(path)?;
    let s = m.as_slice();
    let vb = value;
    let mut out: Vec<u8> = Vec::with_capacity(1 << 16);
    let mut seen: SeenSet = SeenSet::with_hasher(FoldHasher(0));
    let mut st = DedupCount::default();
    let mut any = false;
    let mut rev_scratch = Vec::new();
    for seg in segments_of(s)? {
        let r = Reader::new(seg)?;
        match field {
            "url" => {
                if let Some(id) = r.url_dict.search_with_scratch(vb, &mut rev_scratch)? {
                    any = true;
                    let base = r.url_offsets_base as usize;
                    let start = col_u32(seg, base, id as usize)? as u64;
                    let end = col_u32(seg, base, id as usize + 1)? as u64;
                    if end.saturating_sub(start) > 10_000 {
                        query_url_fast(seg, id, start, end, sink, &mut seen, &mut st)?;
                    } else {
                        for idx in start..end {
                            out.clear();
                            r.output_record(&mut out, idx)?;
                            dedup_write(&mut seen, sink, &out, &mut st)?;
                        }
                    }
                }
            }
            "user" => {
                if let Some(id) = r.user_dict.search_with_scratch(vb, &mut rev_scratch)? {
                    any = true;
                    let base = r.user_off_base as usize;
                    let start = col_u32(seg, base, id as usize)? as usize;
                    let end = col_u32(seg, base, id as usize + 1)? as usize;
                    let blob = r.user_blob_base as usize;
                    let blob_start = blob
                        .checked_add(start)
                        .ok_or_else(|| corrupt("posting offset overflows: corrupt .ulp"))?;
                    let blob_end = blob
                        .checked_add(end)
                        .ok_or_else(|| corrupt("posting offset overflows: corrupt .ulp"))?;

                    let mut pos = blob_start;
                    let mut count: usize = 0;
                    while pos < blob_end {
                        read_varint(seg, &mut pos)?;
                        count += 1;
                    }
                    if count > 10_000 {
                        query_postings_fast_varint(
                            seg,
                            blob_start..blob_end,
                            Some(vb),
                            None,
                            sink,
                            &mut seen,
                            &mut st,
                        )?;
                    } else {
                        let mut pos = blob_start;
                        let mut prev: u64 = 0;
                        while pos < blob_end {
                            let d = read_varint(seg, &mut pos)?;
                            prev = prev
                                .checked_add(d)
                                .ok_or_else(|| corrupt("posting index overflows: corrupt .ulp"))?;
                            out.clear();
                            r.output_record(&mut out, prev)?;
                            dedup_write(&mut seen, sink, &out, &mut st)?;
                        }
                    }
                }
            }
            "pass" => {
                if let Some(id) = r.pass_dict.search_with_scratch(vb, &mut rev_scratch)? {
                    any = true;
                    let base = r.pass_off_base as usize;
                    let start = col_u32(seg, base, id as usize)? as usize;
                    let end = col_u32(seg, base, id as usize + 1)? as usize;
                    let blob = r.pass_blob_base as usize;
                    let blob_start = blob
                        .checked_add(start)
                        .ok_or_else(|| corrupt("posting offset overflows: corrupt .ulp"))?;
                    let blob_end = blob
                        .checked_add(end)
                        .ok_or_else(|| corrupt("posting offset overflows: corrupt .ulp"))?;

                    let mut pos = blob_start;
                    let mut count: usize = 0;
                    while pos < blob_end {
                        read_varint(seg, &mut pos)?;
                        count += 1;
                    }
                    if count > 10_000 {
                        query_postings_fast_varint(
                            seg,
                            blob_start..blob_end,
                            None,
                            Some(vb),
                            sink,
                            &mut seen,
                            &mut st,
                        )?;
                    } else {
                        let mut pos = blob_start;
                        let mut prev: u64 = 0;
                        while pos < blob_end {
                            let d = read_varint(seg, &mut pos)?;
                            prev = prev
                                .checked_add(d)
                                .ok_or_else(|| corrupt("posting index overflows: corrupt .ulp"))?;
                            out.clear();
                            r.output_record(&mut out, prev)?;
                            dedup_write(&mut seen, sink, &out, &mut st)?;
                        }
                    }
                }
            }
            _ => unreachable!("field validated at the top of query"),
        }
    }
    sink.flush()?;
    if !any {
        eprintln!("no match in {:.3}s", t0.elapsed().as_secs_f64());
    } else {
        eprintln!(
            "query done: {} found, {} dupes, {} saved in {:.3}s",
            st.found,
            st.dupes,
            st.saved,
            t0.elapsed().as_secs_f64()
        );
    }
    Ok(())
}

pub fn dump(path: &str, sink: &mut dyn Write) -> std::io::Result<()> {
    dump_into(path, sink, RecordMode::Text)
}

pub fn dump_frames(path: &str, sink: &mut dyn Write) -> std::io::Result<()> {
    dump_into(path, sink, RecordMode::Frames)
}

fn dump_into(path: &str, sink: &mut dyn Write, mode: RecordMode) -> std::io::Result<()> {
    let t0 = std::time::Instant::now();
    let m = MappedFile::open(path)?;
    let s = m.as_slice();

    let mut misc_total = 0u64;
    for seg in segments_of(s)? {
        misc_total = misc_total.saturating_add(dump_seg(seg, sink, mode)?);
    }
    if mode == RecordMode::Frames && misc_total > 0 {
        eprintln!(
            "ulp: frames dump skips {misc_total} misc rows (no frame shape); text dump shows them"
        );
    }
    sink.flush()?;
    eprintln!("dump done in {:.3}s", t0.elapsed().as_secs_f64());
    Ok(())
}

fn dump_seg(s: &[u8], lock: &mut dyn Write, mode: RecordMode) -> std::io::Result<u64> {
    let r = Reader::new(s)?;

    let mut url_arena: Vec<u8> = Vec::new();
    let mut user_arena: Vec<u8> = Vec::new();
    let mut pass_arena: Vec<u8> = Vec::new();
    let mut url_off: Vec<u32> = Vec::new();
    let mut user_off: Vec<u32> = Vec::new();
    let mut pass_off: Vec<u32> = Vec::new();
    r.url_dict.decode_into(&mut url_arena, &mut url_off)?;
    r.user_dict.decode_into(&mut user_arena, &mut user_off)?;
    r.pass_dict.decode_into(&mut pass_arena, &mut pass_off)?;

    let mut out: Vec<u8> = Vec::with_capacity(1 << 16);
    let u = r.hdr.url_count as usize;
    let url_offsets_base = r.url_offsets_base as usize;
    let user_col_base = r.user_col_base as usize;
    let pass_col_base = r.pass_col_base as usize;
    let sep_base = r.sep_base as usize;
    let sep2_base = r.sep2_base as usize;

    for url_id in 0..u {
        let start = col_u32(s, url_offsets_base, url_id)? as u64;
        let end = col_u32(s, url_offsets_base, url_id + 1)? as u64;
        let url = arena_direct(&url_arena, &url_off, url_id)?;
        for idx in start..end {
            let user_id = col_u32(s, user_col_base, idx as usize)? as usize;
            let pass_id = col_u32(s, pass_col_base, idx as usize)? as usize;
            let sep1 = sep_bit(s, sep_base, idx)?;
            let sep2 = sep_bit(s, sep2_base, idx)?;
            out.clear();
            push_record(
                &mut out,
                mode,
                url,
                sep1,
                arena_direct(&user_arena, &user_off, user_id)?,
                sep2,
                arena_direct(&pass_arena, &pass_off, pass_id)?,
            )?;
            lock.write_all(&out)?;
        }
    }

    if mode == RecordMode::Text {
        let mut p = (r.hdr.misc_off as usize)
            .checked_add(8)
            .ok_or_else(|| corrupt("misc offset overflows: corrupt .ulp"))?;
        for _ in 0..r.hdr.misc_count {
            let len = read_varint(s, &mut p)? as usize;
            let line = s
                .get(p..)
                .ok_or_else(|| truncated("misc line offset out of range: corrupt .ulp"))?;
            let line = line
                .get(..len)
                .ok_or_else(|| truncated("misc line truncated: corrupt .ulp"))?;
            lock.write_all(line)?;
            lock.write_all(b"\n")?;
            p = p
                .checked_add(len)
                .ok_or_else(|| corrupt("misc offset overflows: corrupt .ulp"))?;
        }
    }

    Ok(r.hdr.misc_count)
}

pub fn match_url(
    path: &str,
    mode: &str,
    keyword: &str,
    sink: &mut dyn Write,
) -> std::io::Result<()> {
    match_url_bytes(path, mode, keyword.as_bytes(), sink)
}

pub fn match_url_bytes(
    path: &str,
    mode: &str,
    keyword: &[u8],
    sink: &mut dyn Write,
) -> std::io::Result<()> {
    match_url_into(path, mode, keyword, sink, RecordMode::Text)
}

pub fn match_url_frames(
    path: &str,
    mode: &str,
    keyword: &str,
    sink: &mut dyn Write,
) -> std::io::Result<()> {
    match_url_frames_bytes(path, mode, keyword.as_bytes(), sink)
}

pub fn match_url_frames_bytes(
    path: &str,
    mode: &str,
    keyword: &[u8],
    sink: &mut dyn Write,
) -> std::io::Result<()> {
    match_url_into(path, mode, keyword, sink, RecordMode::Frames)
}

fn match_url_into(
    path: &str,
    mode: &str,
    keyword: &[u8],
    sink: &mut dyn Write,
    record_mode: RecordMode,
) -> std::io::Result<()> {
    if !matches!(mode, "contains" | "prefix") {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("unknown mode '{mode}' (expected contains|prefix)"),
        ));
    }
    let t0 = std::time::Instant::now();
    let m = MappedFile::open(path)?;
    let s = m.as_slice();
    let kw = keyword;
    let mut seen: SeenSet = SeenSet::with_hasher(FoldHasher(0));
    let mut st = DedupCount::default();
    let mut tu = 0u64;
    for seg in segments_of(s)? {
        tu += match_one(seg, mode, record_mode, kw, sink, &mut seen, &mut st)?;
    }
    sink.flush()?;
    eprintln!(
        "matched {} urls, {} found, {} dupes, {} saved in {:.3}s",
        tu,
        st.found,
        st.dupes,
        st.saved,
        t0.elapsed().as_secs_f64()
    );
    Ok(())
}

fn match_user_one(
    s: &[u8],
    mode: &str,
    kw: &[u8],
    lock: &mut dyn Write,
    seen: &mut SeenSet,
    st: &mut DedupCount,
) -> std::io::Result<u64> {
    let r = Reader::new(s)?;
    let user_off_base = r.user_off_base as usize;
    let user_blob_base = r.user_blob_base as usize;
    let pass_col_base = r.pass_col_base as usize;
    let sep_base = r.sep_base as usize;
    let sep2_base = r.sep2_base as usize;

    let suffix_range = if mode == "suffix" {
        let rev_kw: Vec<u8> = kw.iter().rev().copied().collect();
        Some(r.user_dict.prefix_range(&rev_kw)?)
    } else {
        None
    };
    if suffix_range.is_some_and(|(lo, hi)| lo == hi) {
        return Ok(0);
    }

    let matched_ids: Vec<u64> = if suffix_range.is_some() {
        Vec::new()
    } else {
        let mut arena: Vec<u8> = Vec::new();
        let mut off: Vec<u32> = Vec::new();
        r.user_dict.decode_into(&mut arena, &mut off)?;
        let n = r.hdr.user_count as usize;
        let mut v = Vec::new();
        for i in 0..n {
            if arena_direct(&arena, &off, i)?.starts_with(kw) {
                v.push(i as u64);
            }
        }
        v
    };

    let mut user_wanted = vec![0u8; (r.hdr.user_count as usize).div_ceil(8)];
    let mut url_wanted = vec![0u8; (r.hdr.url_count as usize).div_ceil(8)];
    let mut pass_wanted = vec![0u8; (r.hdr.pass_count as usize).div_ceil(8)];
    let mut mark_user = |uid: u64| -> std::io::Result<()> {
        let u = uid as usize;
        set_bit(&mut user_wanted, u)?;
        let start = col_u32(s, user_off_base, u)? as usize;
        let end = col_u32(s, user_off_base, u + 1)? as usize;
        let mut pos = user_blob_base
            .checked_add(start)
            .ok_or_else(|| corrupt("posting offset overflows: corrupt .ulp"))?;
        let end_pos = user_blob_base
            .checked_add(end)
            .ok_or_else(|| corrupt("posting offset overflows: corrupt .ulp"))?;
        let mut prev = 0u64;
        while pos < end_pos {
            let d = read_varint(s, &mut pos)?;
            prev = prev
                .checked_add(d)
                .ok_or_else(|| corrupt("posting index overflows: corrupt .ulp"))?;
            let url_id = r.url_id_of(prev)? as usize;
            let pass_id = col_u32(s, pass_col_base, prev as usize)? as usize;
            set_bit(&mut url_wanted, url_id)?;
            set_bit(&mut pass_wanted, pass_id)?;
        }
        Ok(())
    };
    if let Some((lo, hi)) = suffix_range {
        for uid in lo..hi {
            mark_user(uid)?;
        }
    } else {
        for &uid in &matched_ids {
            mark_user(uid)?;
        }
    }

    let mut url_arena: Vec<u8> = Vec::new();
    let mut url_off: Vec<u32> = Vec::new();
    let mut url_remap: HashMap<usize, u32> = HashMap::new();
    r.url_dict
        .decode_selected_into(&url_wanted, &mut url_arena, &mut url_off, &mut url_remap)?;
    let mut pass_arena: Vec<u8> = Vec::new();
    let mut pass_off: Vec<u32> = Vec::new();
    let mut pass_remap: HashMap<usize, u32> = HashMap::new();
    r.pass_dict.decode_selected_into(
        &pass_wanted,
        &mut pass_arena,
        &mut pass_off,
        &mut pass_remap,
    )?;
    let mut user_arena: Vec<u8> = Vec::new();
    let mut user_off: Vec<u32> = Vec::new();
    let mut user_remap: HashMap<usize, u32> = HashMap::new();
    r.user_dict.decode_selected_into(
        &user_wanted,
        &mut user_arena,
        &mut user_off,
        &mut user_remap,
    )?;

    let mut out: Vec<u8> = Vec::with_capacity(1 << 16);
    let mut matched_users = 0u64;
    let mut emit_user = |uid: u64, matched_users: &mut u64| -> std::io::Result<()> {
        *matched_users += 1;
        let u = uid as usize;
        let user_str = arena_selected(&user_arena, &user_off, &user_remap, u)?;
        let start = col_u32(s, user_off_base, u)? as usize;
        let end = col_u32(s, user_off_base, u + 1)? as usize;
        let mut pos = user_blob_base
            .checked_add(start)
            .ok_or_else(|| corrupt("posting offset overflows: corrupt .ulp"))?;
        let end_pos = user_blob_base
            .checked_add(end)
            .ok_or_else(|| corrupt("posting offset overflows: corrupt .ulp"))?;
        let mut prev = 0u64;
        while pos < end_pos {
            let d = read_varint(s, &mut pos)?;
            prev = prev
                .checked_add(d)
                .ok_or_else(|| corrupt("posting index overflows: corrupt .ulp"))?;
            let idx = prev;
            let url_id = r.url_id_of(idx)? as usize;
            let pass_id = col_u32(s, pass_col_base, idx as usize)? as usize;
            let sep1 = sep_bit(s, sep_base, idx)?;
            let sep2 = sep_bit(s, sep2_base, idx)?;
            out.clear();
            out.extend_from_slice(arena_selected(&url_arena, &url_off, &url_remap, url_id)?);
            out.push(sep1);
            out.extend_from_slice(user_str);
            out.push(sep2);
            out.extend_from_slice(arena_selected(
                &pass_arena,
                &pass_off,
                &pass_remap,
                pass_id,
            )?);
            out.push(b'\n');
            dedup_write(seen, lock, &out, st)?;
        }
        Ok(())
    };
    if let Some((lo, hi)) = suffix_range {
        for uid in lo..hi {
            emit_user(uid, &mut matched_users)?;
        }
    } else {
        for &uid in &matched_ids {
            emit_user(uid, &mut matched_users)?;
        }
    }
    Ok(matched_users)
}

pub fn match_user(
    path: &str,
    mode: &str,
    keyword: &str,
    sink: &mut dyn Write,
) -> std::io::Result<()> {
    match_user_bytes(path, mode, keyword.as_bytes(), sink)
}

pub fn match_user_bytes(
    path: &str,
    mode: &str,
    keyword: &[u8],
    sink: &mut dyn Write,
) -> std::io::Result<()> {
    if !matches!(mode, "prefix" | "suffix") {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("unknown mode '{mode}' (expected prefix|suffix)"),
        ));
    }
    let t0 = std::time::Instant::now();
    let m = MappedFile::open(path)?;
    let s = m.as_slice();
    let kw = keyword;
    let mut seen: SeenSet = SeenSet::with_hasher(FoldHasher(0));
    let mut st = DedupCount::default();
    let mut tu = 0u64;
    for seg in segments_of(s)? {
        tu += match_user_one(seg, mode, kw, sink, &mut seen, &mut st)?;
    }
    sink.flush()?;
    eprintln!(
        "matched {} users, {} found, {} dupes, {} saved in {:.3}s",
        tu,
        st.found,
        st.dupes,
        st.saved,
        t0.elapsed().as_secs_f64()
    );
    Ok(())
}

fn match_one(
    s: &[u8],
    mode: &str,
    record_mode: RecordMode,
    kw: &[u8],
    lock: &mut dyn Write,
    seen: &mut SeenSet,
    st: &mut DedupCount,
) -> std::io::Result<u64> {
    let r = Reader::new(s)?;

    let mut url_arena: Vec<u8> = Vec::new();
    let mut url_off: Vec<u32> = Vec::new();
    r.url_dict.decode_into(&mut url_arena, &mut url_off)?;

    let u = r.hdr.url_count as usize;
    let url_offsets_base = r.url_offsets_base as usize;
    let user_col_base = r.user_col_base as usize;
    let pass_col_base = r.pass_col_base as usize;
    let sep_base = r.sep_base as usize;
    let sep2_base = r.sep2_base as usize;

    let mut user_wanted = vec![0u8; (r.hdr.user_count as usize).div_ceil(8)];
    let mut pass_wanted = vec![0u8; (r.hdr.pass_count as usize).div_ceil(8)];
    let mut hits: Vec<(usize, u64, u64)> = Vec::new();
    let mut matched_urls = 0u64;
    for url_id in 0..u {
        let url = arena_direct(&url_arena, &url_off, url_id)?;
        let hit = match mode {
            "prefix" => url.starts_with(kw),
            _ => contains(url, kw),
        };
        if !hit {
            continue;
        }
        matched_urls += 1;
        let start = col_u32(s, url_offsets_base, url_id)? as u64;
        let end = col_u32(s, url_offsets_base, url_id + 1)? as u64;
        for idx in start..end {
            let uid = col_u32(s, user_col_base, idx as usize)?;
            let pid = col_u32(s, pass_col_base, idx as usize)?;
            set_bit(&mut user_wanted, uid as usize)?;
            set_bit(&mut pass_wanted, pid as usize)?;
        }
        hits.push((url_id, start, end));
    }
    if hits.is_empty() {
        return Ok(0);
    }

    let mut user_arena: Vec<u8> = Vec::new();
    let mut user_off: Vec<u32> = Vec::new();
    let mut user_remap: HashMap<usize, u32> = HashMap::new();
    let mut pass_arena: Vec<u8> = Vec::new();
    let mut pass_off: Vec<u32> = Vec::new();
    let mut pass_remap: HashMap<usize, u32> = HashMap::new();
    r.user_dict.decode_selected_into(
        &user_wanted,
        &mut user_arena,
        &mut user_off,
        &mut user_remap,
    )?;
    r.pass_dict.decode_selected_into(
        &pass_wanted,
        &mut pass_arena,
        &mut pass_off,
        &mut pass_remap,
    )?;

    let mut out: Vec<u8> = Vec::with_capacity(1 << 16);
    for &(url_id, start, end) in &hits {
        let url = arena_direct(&url_arena, &url_off, url_id)?;
        for idx in start..end {
            let user_id = col_u32(s, user_col_base, idx as usize)? as usize;
            let pass_id = col_u32(s, pass_col_base, idx as usize)? as usize;
            let sep1 = sep_bit(s, sep_base, idx)?;
            let sep2 = sep_bit(s, sep2_base, idx)?;
            out.clear();
            push_record(
                &mut out,
                record_mode,
                url,
                sep1,
                arena_selected(&user_arena, &user_off, &user_remap, user_id)?,
                sep2,
                arena_selected(&pass_arena, &pass_off, &pass_remap, pass_id)?,
            )?;
            dedup_write(seen, lock, &out, st)?;
        }
    }
    Ok(matched_urls)
}

fn query_url_fast(
    s: &[u8],
    url_id: u64,
    start: u64,
    end: u64,
    lock: &mut dyn Write,
    seen: &mut SeenSet,
    st: &mut DedupCount,
) -> std::io::Result<()> {
    let r = Reader::new(s)?;
    let user_col_base = r.user_col_base as usize;
    let pass_col_base = r.pass_col_base as usize;
    let sep_base = r.sep_base as usize;
    let sep2_base = r.sep2_base as usize;

    let mut user_wanted = vec![0u8; (r.hdr.user_count as usize).div_ceil(8)];
    let mut pass_wanted = vec![0u8; (r.hdr.pass_count as usize).div_ceil(8)];
    for idx in start..end {
        let uid = col_u32(s, user_col_base, idx as usize)?;
        let pid = col_u32(s, pass_col_base, idx as usize)?;
        set_bit(&mut user_wanted, uid as usize)?;
        set_bit(&mut pass_wanted, pid as usize)?;
    }

    let mut user_arena: Vec<u8> = Vec::new();
    let mut user_off: Vec<u32> = Vec::new();
    let mut user_remap: HashMap<usize, u32> = HashMap::new();
    let mut pass_arena: Vec<u8> = Vec::new();
    let mut pass_off: Vec<u32> = Vec::new();
    let mut pass_remap: HashMap<usize, u32> = HashMap::new();
    r.user_dict.decode_selected_into(
        &user_wanted,
        &mut user_arena,
        &mut user_off,
        &mut user_remap,
    )?;
    r.pass_dict.decode_selected_into(
        &pass_wanted,
        &mut pass_arena,
        &mut pass_off,
        &mut pass_remap,
    )?;
    let mut url_buf = Vec::new();
    r.url_dict.key_into(url_id, &mut url_buf)?;

    let mut line: Vec<u8> = Vec::with_capacity(64);
    for idx in start..end {
        let user_id = col_u32(s, user_col_base, idx as usize)? as usize;
        let pass_id = col_u32(s, pass_col_base, idx as usize)? as usize;
        let sep1 = sep_bit(s, sep_base, idx)?;
        let sep2 = sep_bit(s, sep2_base, idx)?;
        line.clear();
        line.extend_from_slice(&url_buf);
        line.push(sep1);
        line.extend_from_slice(arena_selected(
            &user_arena,
            &user_off,
            &user_remap,
            user_id,
        )?);
        line.push(sep2);
        line.extend_from_slice(arena_selected(
            &pass_arena,
            &pass_off,
            &pass_remap,
            pass_id,
        )?);
        line.push(b'\n');
        dedup_write(seen, lock, &line, st)?;
    }
    Ok(())
}
