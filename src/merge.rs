use crate::build::RecordSource;
use crate::dict::{Dedup, cmp_rev, serialize_dict};
use crate::format::{BLOCK_SIZE, HEADER_LEN, MAGIC, VERSION, corrupt, r32, segments_of, truncated};
use crate::mmap::MappedFile;
use crate::progress::Progress;
use crate::reader::Reader;
use crate::varint::write_varint;
use std::io::{BufReader, BufWriter, Read, Seek, Write};

#[inline]
#[expect(
    clippy::unwrap_used,
    reason = "fixed 4-byte prefix of a self-written record"
)]
fn le_u32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

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
pub(crate) fn bin_field_len(len: usize, field: &str) -> std::io::Result<u32> {
    u32::try_from(len).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "{field} field of {len} bytes exceeds the 4 GiB record limit (u32 length prefix)"
            ),
        )
    })
}

#[inline]
pub(crate) fn write_bin_rec(
    out: &mut Vec<u8>,
    url: &[u8],
    user: &[u8],
    pass: &[u8],
    sep1: u8,
    sep2: u8,
) -> std::io::Result<()> {
    let ul = bin_field_len(url.len(), "url")?;
    let nl = bin_field_len(user.len(), "user")?;
    let pl = bin_field_len(pass.len(), "pass")?;
    out.extend_from_slice(&ul.to_le_bytes());
    out.extend_from_slice(&nl.to_le_bytes());
    out.extend_from_slice(&pl.to_le_bytes());
    out.extend_from_slice(url);
    out.extend_from_slice(user);
    out.extend_from_slice(pass);
    out.push(sep1);
    out.push(sep2);
    Ok(())
}
#[inline]
pub(crate) fn bin_url(rec: &[u8]) -> &[u8] {
    let ul = le_u32(rec, 0) as usize;
    &rec[12..12 + ul]
}
#[inline]
pub(crate) fn bin_user(rec: &[u8]) -> &[u8] {
    let ul = le_u32(rec, 0) as usize;
    let nl = le_u32(rec, 4) as usize;
    &rec[12 + ul..12 + ul + nl]
}
#[inline]
pub(crate) fn bin_pass(rec: &[u8]) -> &[u8] {
    let ul = le_u32(rec, 0) as usize;
    let nl = le_u32(rec, 4) as usize;
    let pl = le_u32(rec, 8) as usize;
    &rec[12 + ul + nl..12 + ul + nl + pl]
}
#[inline]
pub(crate) fn bin_rec_size(rec: &[u8]) -> usize {
    let ul = le_u32(rec, 0) as usize;
    let nl = le_u32(rec, 4) as usize;
    let pl = le_u32(rec, 8) as usize;
    12 + ul + nl + pl + 2
}
fn cmp_bin_rec(a: &[u8], b: &[u8]) -> std::cmp::Ordering {
    let c = bin_url(a).cmp(bin_url(b));
    if c != std::cmp::Ordering::Equal {
        return c;
    }
    let c = bin_user(a).cmp(bin_user(b));
    if c != std::cmp::Ordering::Equal {
        return c;
    }
    let c = bin_pass(a).cmp(bin_pass(b));
    if c != std::cmp::Ordering::Equal {
        return c;
    }

    a[a.len() - 2..].cmp(&b[b.len() - 2..])
}

#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "unused in non-test builds; kept verbatim and exercised by merge.rs unit tests"
    )
)]
struct BinSource {
    path: String,
    count: u64,
}
impl RecordSource for BinSource {
    fn estimate_records(&self) -> usize {
        self.count as usize
    }
    fn for_each(
        &mut self,
        c: &mut dyn FnMut(&[u8], &[u8], &[u8], u8, u8) -> std::io::Result<()>,
        _n: &mut dyn FnMut(&[u8]) -> std::io::Result<()>,
    ) -> std::io::Result<u64> {
        let f = std::fs::File::open(&self.path)?;
        let mut r = BufReader::with_capacity(16 << 20, f);
        let mut hdr = [0u8; 12];
        let mut n = 0u64;
        loop {
            if r.read_exact(&mut hdr).is_err() {
                break;
            }
            let ul = le_u32(&hdr, 0) as usize;
            let nl = le_u32(&hdr, 4) as usize;
            let pl = le_u32(&hdr, 8) as usize;
            let mut body = vec![0u8; ul + nl + pl + 2];
            if r.read_exact(&mut body).is_err() {
                break;
            }
            let sep1 = body[ul + nl + pl];
            let sep2 = body[ul + nl + pl + 1];
            c(
                &body[..ul],
                &body[ul..ul + nl],
                &body[ul + nl..ul + nl + pl],
                sep1,
                sep2,
            )?;
            n += 1;
        }
        Ok(n)
    }
}

fn read_one_bin_rec_into(r: &mut dyn Read, out: &mut Vec<u8>) -> std::io::Result<bool> {
    let mut hdr = [0u8; 12];
    match r.read_exact(&mut hdr) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(false),
        Err(e) => return Err(e),
    }
    let ul = le_u32(&hdr, 0) as usize;
    let nl = le_u32(&hdr, 4) as usize;
    let pl = le_u32(&hdr, 8) as usize;
    out.clear();
    out.extend_from_slice(&hdr);
    out.resize(12 + ul + nl + pl + 2, 0);
    r.read_exact(&mut out[12..])?;
    Ok(true)
}

fn read_one_run_rec_into(r: &mut dyn Read, out: &mut Vec<u8>) -> std::io::Result<bool> {
    let mut lenb = [0u8; 8];
    match r.read_exact(&mut lenb) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(false),
        Err(e) => return Err(e),
    }
    let len = u64::from_le_bytes(lenb) as usize;
    out.resize(len, 0);
    r.read_exact(out)?;
    Ok(true)
}

fn read_one_run_rec(r: &mut impl Read) -> std::io::Result<Option<Vec<u8>>> {
    let mut rec = Vec::new();
    Ok(read_one_run_rec_into(r, &mut rec)?.then_some(rec))
}

fn read_one_str_idx_into(r: &mut dyn Read, out: &mut Vec<u8>) -> std::io::Result<bool> {
    let mut hdr = [0u8; 4];
    match r.read_exact(&mut hdr) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(false),
        Err(e) => return Err(e),
    }
    let len = u32::from_le_bytes(hdr) as usize;
    out.clear();
    out.extend_from_slice(&hdr);
    out.resize(4 + len + 4, 0);
    r.read_exact(&mut out[4..])?;
    Ok(true)
}

fn write_str_idx(out: &mut Vec<u8>, s: &[u8], idx: u32) {
    out.extend_from_slice(&(s.len() as u32).to_le_bytes());
    out.extend_from_slice(s);
    out.extend_from_slice(&idx.to_le_bytes());
}

fn cmp_str_idx(a: &[u8], b: &[u8]) -> std::cmp::Ordering {
    let al = le_u32(a, 0) as usize;
    let bl = le_u32(b, 0) as usize;
    let c = a[4..4 + al].cmp(&b[4..4 + bl]);
    if c != std::cmp::Ordering::Equal {
        return c;
    }
    let ai = le_u32(a, 4 + al);
    let bi = le_u32(b, 4 + bl);
    ai.cmp(&bi)
}

fn cmp_str_idx_rev(a: &[u8], b: &[u8]) -> std::cmp::Ordering {
    let al = le_u32(a, 0) as usize;
    let bl = le_u32(b, 0) as usize;
    let c = cmp_rev(&a[4..4 + al], &b[4..4 + bl]);
    if c != std::cmp::Ordering::Equal {
        return c;
    }
    let ai = le_u32(a, 4 + al);
    let bi = le_u32(b, 4 + bl);
    ai.cmp(&bi)
}

type CmpFn = fn(&[u8], &[u8]) -> std::cmp::Ordering;
type ReadItemFn = fn(&mut dyn Read, &mut Vec<u8>) -> std::io::Result<bool>;

struct HItem {
    rec: Vec<u8>,
    run: usize,
    cmp: CmpFn,
}
impl PartialEq for HItem {
    fn eq(&self, o: &Self) -> bool {
        (self.cmp)(&self.rec, &o.rec) == std::cmp::Ordering::Equal
    }
}
impl Eq for HItem {}
impl PartialOrd for HItem {
    fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for HItem {
    fn cmp(&self, o: &Self) -> std::cmp::Ordering {
        (self.cmp)(&o.rec, &self.rec)
    }
}

fn external_sort(
    in_path: &str,
    out_path: &str,
    read_item: ReadItemFn,
    cmp: CmpFn,
    dedup: bool,
    name: &'static str,
) -> std::io::Result<u64> {
    let total_bytes = std::fs::metadata(in_path)?.len();
    let mut prog = Progress::new(name, total_bytes);
    let mut runs: Vec<String> = Vec::new();
    let mut arena: Vec<u8> = Vec::new();
    let mut starts: Vec<usize> = Vec::new();
    let mut buf_bytes = 0usize;
    {
        let f = std::fs::File::open(in_path)?;
        let mut r = BufReader::with_capacity(16 << 20, f);
        let mut item = Vec::new();
        while read_item(&mut r, &mut item)? {
            prog.add(item.len() as u64);
            starts.push(arena.len());
            arena.extend_from_slice(&item);
            buf_bytes += item.len();
            if buf_bytes > 400 << 20 {
                let run = format!("{}.run{}", out_path, runs.len());
                write_sorted_arena_run(&run, &mut arena, &mut starts, cmp)?;
                runs.push(run);
                buf_bytes = 0;
            }
        }
    }
    if !arena.is_empty() {
        let run = format!("{}.run{}", out_path, runs.len());
        write_sorted_arena_run(&run, &mut arena, &mut starts, cmp)?;
        runs.push(run);
    }
    prog.finish();

    let mut prog = Progress::new("merge", total_bytes.max(1));
    let mut out = BufWriter::with_capacity(1 << 22, std::fs::File::create(out_path)?);
    let mut readers: Vec<BufReader<std::fs::File>> = runs
        .iter()
        .map(|p| Ok(BufReader::with_capacity(1 << 20, std::fs::File::open(p)?)))
        .collect::<std::io::Result<_>>()?;
    let mut heap = std::collections::BinaryHeap::new();
    for (i, rd) in readers.iter_mut().enumerate() {
        if let Some(rec) = read_one_run_rec(rd)? {
            heap.push(HItem { rec, run: i, cmp });
        }
    }
    let mut written = 0u64;
    let mut last: Option<Vec<u8>> = None;
    while let Some(item) = heap.pop() {
        let is_dup = if dedup {
            match &last {
                Some(l) => cmp(l, &item.rec) == std::cmp::Ordering::Equal,
                None => false,
            }
        } else {
            false
        };
        if !is_dup {
            out.write_all(&item.rec)?;
            written += 1;
            prog.add(item.rec.len() as u64);
            if dedup {
                last = Some(item.rec);
            }
        }
        if let Some(next) = read_one_run_rec(&mut readers[item.run])? {
            heap.push(HItem {
                rec: next,
                run: item.run,
                cmp,
            });
        }
    }
    out.flush()?;
    prog.finish();
    for p in &runs {
        let _ = std::fs::remove_file(p);
    }
    Ok(written)
}

fn write_sorted_arena_run(
    path: &str,
    arena: &mut Vec<u8>,
    starts: &mut Vec<usize>,
    cmp: CmpFn,
) -> std::io::Result<()> {
    let count = starts.len();
    let mut order: Vec<usize> = (0..count).collect();
    order.sort_unstable_by(|&a, &b| {
        let a_end = if a + 1 < count {
            starts[a + 1]
        } else {
            arena.len()
        };
        let b_end = if b + 1 < count {
            starts[b + 1]
        } else {
            arena.len()
        };
        cmp(&arena[starts[a]..a_end], &arena[starts[b]..b_end])
    });
    let mut w = BufWriter::with_capacity(1 << 20, std::fs::File::create(path)?);
    for &idx in &order {
        let start = starts[idx];
        let end = if idx + 1 < count {
            starts[idx + 1]
        } else {
            arena.len()
        };
        w.write_all(&((end - start) as u64).to_le_bytes())?;
        w.write_all(&arena[start..end])?;
    }
    w.flush()?;
    arena.clear();
    starts.clear();
    Ok(())
}

fn extract_to_runs(in_path: &str, prefix: &str) -> std::io::Result<(Vec<String>, u64)> {
    let m = MappedFile::open(in_path)?;
    let s = m.as_slice();
    let segs = segments_of(s)?;
    let mut total: u64 = 0;
    for seg in &segs {
        total += Reader::new(seg)?.hdr.record_count;
    }
    let mut prog = Progress::new("extract", total);
    let mut runs: Vec<String> = Vec::new();
    let mut arena: Vec<u8> = Vec::new();
    let mut starts: Vec<usize> = Vec::new();
    let mut buf_bytes = 0usize;
    let mut n = 0u64;
    for seg in &segs {
        let r = Reader::new(seg)?;
        let mut url_arena: Vec<u8> = Vec::new();
        let mut user_arena: Vec<u8> = Vec::new();
        let mut pass_arena: Vec<u8> = Vec::new();
        let mut url_off: Vec<u32> = Vec::new();
        let mut user_off: Vec<u32> = Vec::new();
        let mut pass_off: Vec<u32> = Vec::new();
        r.url_dict.decode_into(&mut url_arena, &mut url_off)?;
        r.user_dict.decode_into(&mut user_arena, &mut user_off)?;
        r.pass_dict.decode_into(&mut pass_arena, &mut pass_off)?;
        let u = r.hdr.url_count as usize;
        let url_offsets_base = r.url_offsets_base as usize;
        let user_col_base = r.user_col_base as usize;
        let pass_col_base = r.pass_col_base as usize;
        let sep_base = r.sep_base as usize;
        let sep2_base = r.sep2_base as usize;
        for url_id in 0..u {
            let start = col_u32(seg, url_offsets_base, url_id)? as u64;
            let end = col_u32(seg, url_offsets_base, url_id + 1)? as u64;
            let url = arena_direct(&url_arena, &url_off, url_id)?;
            for idx in start..end {
                let user_id = col_u32(seg, user_col_base, idx as usize)? as usize;
                let pass_id = col_u32(seg, pass_col_base, idx as usize)? as usize;
                let b1 = *seg
                    .get(sep_base + (idx as usize) / 8)
                    .ok_or_else(|| truncated("separator bit out of range: corrupt .ulp"))?;
                let sep1 = if (b1 >> (idx % 8)) & 1 == 1 {
                    b'|'
                } else {
                    b':'
                };
                let b2 = *seg
                    .get(sep2_base + (idx as usize) / 8)
                    .ok_or_else(|| truncated("separator bit out of range: corrupt .ulp"))?;
                let sep2 = if (b2 >> (idx % 8)) & 1 == 1 {
                    b'|'
                } else {
                    b':'
                };
                let start = arena.len();
                write_bin_rec(
                    &mut arena,
                    url,
                    arena_direct(&user_arena, &user_off, user_id)?,
                    arena_direct(&pass_arena, &pass_off, pass_id)?,
                    sep1,
                    sep2,
                )?;
                buf_bytes += arena.len() - start;
                starts.push(start);
                n += 1;
                prog.add(1);
                if buf_bytes > 400 << 20 {
                    let run = format!("{}.run{}", prefix, runs.len());
                    write_sorted_arena_run(&run, &mut arena, &mut starts, cmp_bin_rec)?;
                    runs.push(run);
                    buf_bytes = 0;
                }
            }
        }
    }
    if !arena.is_empty() {
        let run = format!("{}.run{}", prefix, runs.len());
        write_sorted_arena_run(&run, &mut arena, &mut starts, cmp_bin_rec)?;
        runs.push(run);
    }
    prog.finish();
    Ok((runs, n))
}

fn merge_runs(
    runs: &[String],
    out_path: &str,
    cmp: CmpFn,
    dedup: bool,
    total: u64,
    name: &'static str,
) -> std::io::Result<u64> {
    let mut prog = Progress::new(name, total.max(1));
    let mut out = BufWriter::with_capacity(1 << 22, std::fs::File::create(out_path)?);
    let mut readers: Vec<BufReader<std::fs::File>> = runs
        .iter()
        .map(|p| Ok(BufReader::with_capacity(1 << 20, std::fs::File::open(p)?)))
        .collect::<std::io::Result<_>>()?;
    let mut heap = std::collections::BinaryHeap::new();
    for (i, rd) in readers.iter_mut().enumerate() {
        if let Some(rec) = read_one_run_rec(rd)? {
            heap.push(HItem { rec, run: i, cmp });
        }
    }
    let mut written = 0u64;
    let mut last: Option<Vec<u8>> = None;
    while let Some(item) = heap.pop() {
        let is_dup = dedup
            && last
                .as_ref()
                .is_some_and(|l| cmp(l, &item.rec) == std::cmp::Ordering::Equal);
        if !is_dup {
            out.write_all(&item.rec)?;
            written += 1;
            prog.add(1);
            if dedup {
                last = Some(item.rec);
            }
        }
        if let Some(next) = read_one_run_rec(&mut readers[item.run])? {
            heap.push(HItem {
                rec: next,
                run: item.run,
                cmp,
            });
        }
    }
    out.flush()?;
    prog.finish();
    for p in runs {
        let _ = std::fs::remove_file(p);
    }
    Ok(written)
}

pub fn merge(in_path: &str, out_path: &str) -> std::io::Result<()> {
    let t0 = std::time::Instant::now();
    let tmp_dir = crate::temp::TempDir::new(&format!("{}.sorted.tmp", out_path))?;
    let tmp_sorted = tmp_dir.join_string("sorted");
    eprintln!("[merge] step 1/3: extract -> sorted runs ...");
    let (runs, n) = extract_to_runs(in_path, &tmp_sorted)?;
    eprintln!("[merge] {} records -> {} runs", n, runs.len());
    eprintln!("[merge] step 2/3: k-way merge + dedup ...");
    let n_uniq = merge_runs(&runs, &tmp_sorted, cmp_bin_rec, true, n, "merge")?;
    eprintln!(
        "[merge] {} unique after dedup ({:.1}% dupes removed)",
        n_uniq,
        100.0 * (1.0 - n_uniq as f64 / n.max(1) as f64)
    );
    eprintln!("[merge] step 3/3: rebuilding single segment (external dicts) ...");
    build_external(&tmp_sorted, out_path, n_uniq)?;
    eprintln!(
        "[merge] done in {:.1}s -> {} ({} -> {} records)",
        t0.elapsed().as_secs_f64(),
        out_path,
        n,
        n_uniq
    );
    Ok(())
}

fn build_external(sorted_path: &str, out_path: &str, n_records: u64) -> std::io::Result<()> {
    let tmp_dir = crate::temp::TempDir::new(&format!("{}.tmp", out_path))?;
    let users_tmp = tmp_dir.join_string("users");
    let passes_tmp = tmp_dir.join_string("passes");
    let users_sorted = tmp_dir.join_string("users.sorted");
    let passes_sorted = tmp_dir.join_string("passes.sorted");
    let users_uniq = tmp_dir.join_string("users.uniq");
    let passes_uniq = tmp_dir.join_string("passes.uniq");
    let users_postings = tmp_dir.join_string("users.postings");
    let passes_postings = tmp_dir.join_string("passes.postings");

    let mut url_dedup = Dedup::new((n_records / 10).max(1024) as usize);
    let mut user_ids: Vec<u32> = vec![0u32; n_records as usize];
    let mut pass_ids: Vec<u32> = vec![0u32; n_records as usize];
    let mut sep1_bits: Vec<u8> = vec![0u8; n_records.div_ceil(8) as usize];
    let mut sep2_bits: Vec<u8> = vec![0u8; n_records.div_ceil(8) as usize];
    let mut url_offsets: Vec<u32> = Vec::new();
    url_offsets.push(0u32);
    {
        let mut users_w = BufWriter::with_capacity(1 << 22, std::fs::File::create(&users_tmp)?);
        let mut passes_w = BufWriter::with_capacity(1 << 22, std::fs::File::create(&passes_tmp)?);
        let f = std::fs::File::open(sorted_path)?;
        let mut r = BufReader::with_capacity(16 << 20, f);
        let mut ubuf = Vec::with_capacity(64);
        let mut pbuf = Vec::with_capacity(64);
        let mut prev_url = Vec::new();
        let mut have_prev_url = false;
        let mut rec_idx = 0u64;
        let mut prog = Progress::new("extract", n_records);
        let mut rec = Vec::with_capacity(64);
        while read_one_bin_rec_into(&mut r, &mut rec)? {
            let url = bin_url(&rec);
            let user = bin_user(&rec);
            let pass = bin_pass(&rec);
            url_dedup.insert(url);
            if !have_prev_url || prev_url.as_slice() != url {
                if have_prev_url {
                    url_offsets.push(rec_idx as u32);
                }
                prev_url.clear();
                prev_url.extend_from_slice(url);
                have_prev_url = true;
            }
            ubuf.clear();
            write_str_idx(&mut ubuf, user, rec_idx as u32);
            users_w.write_all(&ubuf)?;
            pbuf.clear();
            write_str_idx(&mut pbuf, pass, rec_idx as u32);
            passes_w.write_all(&pbuf)?;
            let sep1 = rec[rec.len() - 2];
            let sep2 = rec[rec.len() - 1];
            if sep1 == b'|' {
                sep1_bits[(rec_idx / 8) as usize] |= 1 << (rec_idx % 8);
            }
            if sep2 == b'|' {
                sep2_bits[(rec_idx / 8) as usize] |= 1 << (rec_idx % 8);
            }
            rec_idx += 1;
            prog.add(1);
        }
        prog.finish();
        users_w.flush()?;
        passes_w.flush()?;
    }
    url_offsets.push(n_records as u32);
    let (url_section, _) = serialize_dict(&mut url_dedup, false);
    let url_count = url_dedup.count;

    eprintln!("[merge] sorting usernames ...");
    external_sort(
        &users_tmp,
        &users_sorted,
        read_one_str_idx_into,
        cmp_str_idx_rev,
        false,
        "sort-users",
    )?;
    let _ = std::fs::remove_file(&users_tmp);
    eprintln!("[merge] sorting passwords ...");
    external_sort(
        &passes_tmp,
        &passes_sorted,
        read_one_str_idx_into,
        cmp_str_idx,
        false,
        "sort-passes",
    )?;
    let _ = std::fs::remove_file(&passes_tmp);

    eprintln!("[merge] building user index ...");
    let mut user_offsets: Vec<u32> = Vec::new();
    let (user_count, user_blob_len) = build_field_index(
        &users_sorted,
        &users_uniq,
        &users_postings,
        &mut user_ids,
        &mut user_offsets,
    )?;
    eprintln!("[merge] {} unique users", user_count);
    let _ = std::fs::remove_file(&users_sorted);
    let mut pass_offsets: Vec<u32> = Vec::new();
    let (pass_count, pass_blob_len) = build_field_index(
        &passes_sorted,
        &passes_uniq,
        &passes_postings,
        &mut pass_ids,
        &mut pass_offsets,
    )?;
    eprintln!("[merge] {} unique passes", pass_count);
    let _ = std::fs::remove_file(&passes_sorted);

    let mut out = BufWriter::with_capacity(1 << 22, std::fs::File::create(out_path)?);
    out.write_all(&[0u8; HEADER_LEN])?;
    let mut off = HEADER_LEN as u64;

    let url_dict_off = off;
    out.write_all(&url_section)?;
    off += url_section.len() as u64;

    let user_dict_off = off;
    write_dict_streaming(&users_uniq, &mut out, true)?;
    off = out.stream_position()?;

    let pass_dict_off = off;
    write_dict_streaming(&passes_uniq, &mut out, false)?;
    off = out.stream_position()?;

    let records_off = off;
    out.write_all(&(url_count as u64).to_le_bytes())?;
    off += 8;
    for &o in &url_offsets {
        out.write_all(&o.to_le_bytes())?;
    }
    off += url_offsets.len() as u64 * 4;
    for &u in &user_ids {
        out.write_all(&u.to_le_bytes())?;
    }
    off += user_ids.len() as u64 * 4;
    for &p in &pass_ids {
        out.write_all(&p.to_le_bytes())?;
    }
    off += pass_ids.len() as u64 * 4;
    out.write_all(&sep1_bits)?;
    off += sep1_bits.len() as u64;
    out.write_all(&sep2_bits)?;
    off += sep2_bits.len() as u64;

    let user_idx_off = off;
    out.write_all(&user_count.to_le_bytes())?;
    off += 8;
    for &o in &user_offsets {
        out.write_all(&o.to_le_bytes())?;
    }
    off += user_offsets.len() as u64 * 4;
    let mut user_blob = BufReader::with_capacity(8 << 20, std::fs::File::open(&users_postings)?);
    let copied = std::io::copy(&mut user_blob, &mut out)?;
    if copied != user_blob_len {
        return Err(corrupt("user postings temp file is shorter than expected"));
    }
    off += copied;

    let pass_idx_off = off;
    out.write_all(&pass_count.to_le_bytes())?;
    off += 8;
    for &o in &pass_offsets {
        out.write_all(&o.to_le_bytes())?;
    }
    off += pass_offsets.len() as u64 * 4;
    let mut pass_blob = BufReader::with_capacity(8 << 20, std::fs::File::open(&passes_postings)?);
    let copied = std::io::copy(&mut pass_blob, &mut out)?;
    if copied != pass_blob_len {
        return Err(corrupt("pass postings temp file is shorter than expected"));
    }
    off += copied;

    let misc_off = off;
    out.write_all(&0u64.to_le_bytes())?;
    off += 8;

    let mut hdr = Vec::with_capacity(HEADER_LEN);
    hdr.extend_from_slice(&MAGIC);
    hdr.extend_from_slice(&VERSION.to_le_bytes());
    hdr.extend_from_slice(&0u32.to_le_bytes());
    hdr.extend_from_slice(&(n_records).to_le_bytes());
    hdr.extend_from_slice(&(url_count as u64).to_le_bytes());
    hdr.extend_from_slice(&(user_count).to_le_bytes());
    hdr.extend_from_slice(&(pass_count).to_le_bytes());
    hdr.extend_from_slice(&0u64.to_le_bytes());
    hdr.extend_from_slice(&0u64.to_le_bytes());
    hdr.extend_from_slice(&(url_dict_off).to_le_bytes());
    hdr.extend_from_slice(&(user_dict_off).to_le_bytes());
    hdr.extend_from_slice(&(pass_dict_off).to_le_bytes());
    hdr.extend_from_slice(&(records_off).to_le_bytes());
    hdr.extend_from_slice(&(user_idx_off).to_le_bytes());
    hdr.extend_from_slice(&(pass_idx_off).to_le_bytes());
    hdr.extend_from_slice(&(misc_off).to_le_bytes());
    hdr.extend_from_slice(&off.to_le_bytes());
    assert_eq!(
        hdr.len(),
        HEADER_LEN,
        "merge: assembled header is {} bytes, expected {}; the header writer over/underfilled the fixed fields",
        hdr.len(),
        HEADER_LEN
    );
    out.seek(std::io::SeekFrom::Start(0))?;
    out.write_all(&hdr)?;
    out.flush()?;

    out.seek(std::io::SeekFrom::Start(off))?;
    out.write_all(&1u64.to_le_bytes())?;
    out.write_all(&0u64.to_le_bytes())?;
    out.write_all(&off.to_le_bytes())?;
    out.flush()?;

    Ok(())
}

pub fn sortcount(in_path: &str) -> std::io::Result<()> {
    let t0 = std::time::Instant::now();
    let tmp_dir = crate::temp::TempDir::new(&format!("{}.sortedcount.tmp", in_path))?;
    let tmp_sorted = tmp_dir.join_string("sorted");
    let (runs, n) = extract_to_runs(in_path, &tmp_sorted)?;
    eprintln!("[dedup] {} records -> {} runs", n, runs.len());
    let n_uniq = merge_runs(&runs, &tmp_sorted, cmp_bin_rec, true, n, "merge")?;
    eprintln!(
        "[dedup] {} unique after dedup ({:.1}% dupes removed) in {:.1}s",
        n_uniq,
        100.0 * (1.0 - n_uniq as f64 / n.max(1) as f64),
        t0.elapsed().as_secs_f64()
    );
    Ok(())
}

#[inline]
fn common_prefix(a: &[u8], b: &[u8]) -> usize {
    let n = a.len().min(b.len());
    let mut i = 0;
    while i < n && a[i] == b[i] {
        i += 1;
    }
    i
}

fn write_varint_to(out: &mut dyn Write, mut v: u64) -> std::io::Result<()> {
    loop {
        let mut b = (v & 0x7f) as u8;
        v >>= 7;
        if v != 0 {
            b |= 0x80;
            out.write_all(&[b])?;
        } else {
            out.write_all(&[b])?;
            break;
        }
    }
    Ok(())
}

fn varint_len(mut v: u64) -> u64 {
    let mut n = 1u64;
    while v >= 0x80 {
        v >>= 7;
        n += 1;
    }
    n
}

fn write_len_prefixed(out: &mut dyn Write, s: &[u8]) -> std::io::Result<()> {
    out.write_all(&(s.len() as u32).to_le_bytes())?;
    out.write_all(s)?;
    Ok(())
}

fn read_len_prefixed_into(r: &mut dyn Read, out: &mut Vec<u8>) -> std::io::Result<bool> {
    let mut hdr = [0u8; 4];
    match r.read_exact(&mut hdr) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(false),
        Err(e) => return Err(e),
    }
    let len = u32::from_le_bytes(hdr) as usize;
    out.resize(len, 0);
    r.read_exact(out)?;
    Ok(true)
}

fn write_dict_streaming(
    uniq_path: &str,
    out: &mut BufWriter<std::fs::File>,
    reversed: bool,
) -> std::io::Result<u64> {
    let mut reverse_buf = Vec::new();
    let (count, data_len, offsets) = {
        let f = std::fs::File::open(uniq_path)?;
        let mut r = BufReader::new(f);
        let mut count = 0u64;
        let mut data_len = 0u64;
        let mut offsets: Vec<u64> = Vec::new();
        let mut prev: Vec<u8> = Vec::new();
        let mut input = Vec::new();
        while read_len_prefixed_into(&mut r, &mut input)? {
            let s = &input;
            let key: &[u8] = if reversed {
                reverse_buf.clear();
                reverse_buf.extend(s.iter().rev().copied());
                &reverse_buf
            } else {
                s
            };
            if count.is_multiple_of(BLOCK_SIZE as u64) {
                offsets.push(data_len);
                data_len += varint_len(key.len() as u64) + key.len() as u64;
            } else {
                let shared = common_prefix(&prev, key);
                data_len += varint_len(shared as u64)
                    + varint_len((key.len() - shared) as u64)
                    + (key.len() - shared) as u64;
            }
            prev.clear();
            prev.extend_from_slice(key);
            count += 1;
        }
        (count, data_len, offsets)
    };
    write_varint_to(out, count)?;
    write_varint_to(out, BLOCK_SIZE as u64)?;
    write_varint_to(out, offsets.len() as u64)?;
    write_varint_to(out, data_len)?;
    out.write_all(&[reversed as u8])?;
    for &o in &offsets {
        out.write_all(&o.to_le_bytes())?;
    }
    let f = std::fs::File::open(uniq_path)?;
    let mut r = BufReader::new(f);
    let mut prev: Vec<u8> = Vec::new();
    let mut input = Vec::new();
    let mut i = 0u64;
    while read_len_prefixed_into(&mut r, &mut input)? {
        let s = &input;
        let key: &[u8] = if reversed {
            reverse_buf.clear();
            reverse_buf.extend(s.iter().rev().copied());
            &reverse_buf
        } else {
            s
        };
        if i.is_multiple_of(BLOCK_SIZE as u64) {
            write_varint_to(out, key.len() as u64)?;
            out.write_all(key)?;
        } else {
            let shared = common_prefix(&prev, key);
            write_varint_to(out, shared as u64)?;
            write_varint_to(out, (key.len() - shared) as u64)?;
            out.write_all(&key[shared..])?;
        }
        prev.clear();
        prev.extend_from_slice(key);
        i += 1;
    }
    Ok(count)
}

fn build_field_index(
    sorted_path: &str,
    uniq_path: &str,
    postings_path: &str,
    ids: &mut [u32],
    postings_offsets: &mut Vec<u32>,
) -> std::io::Result<(u64, u64)> {
    let f = std::fs::File::open(sorted_path)?;
    let mut r = BufReader::with_capacity(16 << 20, f);
    let mut uniq_w = BufWriter::with_capacity(1 << 22, std::fs::File::create(uniq_path)?);
    let mut postings_w = BufWriter::with_capacity(1 << 22, std::fs::File::create(postings_path)?);
    let mut prev_field = Vec::new();
    let mut have_prev_field = false;
    let mut cur_id: u32 = 0;
    let mut prev_rec: u64 = 0;
    postings_offsets.clear();
    postings_offsets.push(0u32);
    let mut postings_len = 0u64;
    let mut postings_batch = Vec::with_capacity(1 << 20);
    let mut count = 0u64;
    let mut item = Vec::new();
    while read_one_str_idx_into(&mut r, &mut item)? {
        let len = le_u32(&item, 0) as usize;
        let field = &item[4..4 + len];
        let rec_idx = le_u32(&item, 4 + len) as u64;
        let is_new = !have_prev_field || prev_field.as_slice() != field;
        if is_new {
            if have_prev_field {
                postings_offsets.push(postings_len as u32);
            }
            write_len_prefixed(&mut uniq_w, field)?;
            prev_field.clear();
            prev_field.extend_from_slice(field);
            have_prev_field = true;
            cur_id += 1;
            prev_rec = 0;
        }
        ids[rec_idx as usize] = cur_id - 1;
        let delta = rec_idx - prev_rec;
        write_varint(&mut postings_batch, delta);
        postings_len += varint_len(delta);
        if postings_batch.len() >= 1 << 20 {
            postings_w.write_all(&postings_batch)?;
            postings_batch.clear();
        }
        prev_rec = rec_idx;
        count += 1;
    }
    if have_prev_field {
        postings_offsets.push(postings_len as u32);
    }
    postings_w.write_all(&postings_batch)?;
    uniq_w.flush()?;
    postings_w.flush()?;
    let _ = count;
    Ok((cur_id as u64, postings_len))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binsource_reads_sorted_records() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("recs.bin");
        let mut recs: Vec<Vec<u8>> = Vec::new();
        for (url, user, pass) in [
            ("a.com", "u1", "p1"),
            ("b.com", "u2", "p2"),
            ("b.com", "u3", "p3"),
        ] {
            let mut r = Vec::new();
            write_bin_rec(
                &mut r,
                url.as_bytes(),
                user.as_bytes(),
                pass.as_bytes(),
                b':',
                b':',
            )
            .unwrap();
            recs.push(r);
        }
        recs.sort_by(|a, b| cmp_bin_rec(a, b));
        let mut file = Vec::new();
        for r in &recs {
            file.extend_from_slice(r);
        }
        std::fs::write(&p, &file).unwrap();
        let mut src = BinSource {
            path: p.to_str().unwrap().to_string(),
            count: recs.len() as u64,
        };
        assert_eq!(src.estimate_records(), 3);
        let mut conf = Vec::new();
        let n = src
            .for_each(
                &mut |u, us, pa, s1, s2| {
                    conf.push((u.to_vec(), us.to_vec(), pa.to_vec(), s1, s2));
                    Ok(())
                },
                &mut |_| Ok(()),
            )
            .unwrap();
        assert_eq!(n, 3);
        assert_eq!(
            conf[1],
            (
                b"b.com".to_vec(),
                b"u2".to_vec(),
                b"p2".to_vec(),
                b':',
                b':'
            )
        );
    }

    #[test]
    fn bin_record_field_len_guard() {
        for n in [0usize, 1, u32::MAX as usize] {
            assert!(bin_field_len(n, "user").is_ok(), "len {n} must encode");
        }
        for n in [u32::MAX as usize + 1, usize::MAX] {
            let e = bin_field_len(n, "user").unwrap_err();
            assert_eq!(e.kind(), std::io::ErrorKind::InvalidData);
            assert!(e.to_string().contains("4 GiB"), "{e}");
        }
        let mut buf = Vec::new();
        write_bin_rec(&mut buf, b"a", b"b", b"c", b':', b':').unwrap();
        assert_eq!(buf.len(), 12 + 3 + 2);
    }
}
