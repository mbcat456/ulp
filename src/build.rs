use crate::dict::{Dedup, serialize_dict};
use crate::domain::normalize_domain_into;
use crate::format::{HEADER_LEN, MAGIC, VERSION, read_segment_meta};
use crate::merge::{bin_pass, bin_rec_size, bin_url, bin_user, write_bin_rec};
use crate::parser::parse_line_into;
use crate::platform::peak_rss_gb;
use crate::varint::write_varint;
use std::io::{BufRead, BufReader, BufWriter, Read, Seek, Write};

#[expect(
    clippy::type_complexity,
    reason = "record callback mirrors the parsed tuple; a struct would churn callers"
)]
fn read_lines(
    r: &mut impl BufRead,
    conform: &mut dyn FnMut(&[u8], &[u8], &[u8], u8, u8) -> std::io::Result<()>,
    nonconform: &mut dyn FnMut(&[u8]) -> std::io::Result<()>,
) -> std::io::Result<(u64, bool)> {
    let mut count = 0u64;
    let mut buf: Vec<u8> = Vec::with_capacity(256);

    let (mut url, mut user, mut pass, mut scratch) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    loop {
        buf.clear();
        match r.read_until(b'\n', &mut buf) {
            Ok(0) => return Ok((count, true)),
            Err(_) => return Ok((count, false)),
            Ok(_) => {}
        }
        while buf.last() == Some(&b'\n') || buf.last() == Some(&b'\r') {
            buf.pop();
        }
        match parse_line_into(&buf, &mut url, &mut user, &mut pass, &mut scratch) {
            Some((s1, s2)) => {
                conform(&url, &user, &pass, s1, s2)?;
                count += 1;
            }
            None => nonconform(&buf)?,
        }
    }
}

fn incomplete_read_error(inputs: &[String]) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        format!(
            "input ended in a read error and was not fully indexed: {}",
            inputs.join(", ")
        ),
    )
}

#[expect(
    clippy::type_complexity,
    reason = "record callback mirrors the parsed tuple; a struct would churn callers"
)]
fn for_each_line(
    inputs: &[String],
    conform: &mut dyn FnMut(&[u8], &[u8], &[u8], u8, u8) -> std::io::Result<()>,
    nonconform: &mut dyn FnMut(&[u8]) -> std::io::Result<()>,
) -> std::io::Result<(u64, Vec<String>)> {
    let mut count = 0u64;
    let mut incomplete = Vec::new();
    for input in inputs {
        let f = std::fs::File::open(input)?;
        let mut r = BufReader::with_capacity(16 * 1024 * 1024, f);
        let (c, clean_eof) = read_lines(&mut r, conform, nonconform)?;
        count += c;
        if !clean_eof {
            incomplete.push(input.clone());
        }
    }
    Ok((count, incomplete))
}

#[derive(Clone, Copy)]
struct Record {
    url_id: u32,
    user_id: u32,
    pass_id: u32,
    sep1: u8,
    sep2: u8,
}

pub(crate) trait RecordSource {
    fn estimate_records(&self) -> usize;
    fn incomplete_inputs(&self) -> &[String] {
        &[]
    }
    #[expect(
        clippy::type_complexity,
        reason = "record callback mirrors the parsed tuple; a struct would churn callers"
    )]
    fn for_each(
        &mut self,
        conform: &mut dyn FnMut(&[u8], &[u8], &[u8], u8, u8) -> std::io::Result<()>,
        nonconform: &mut dyn FnMut(&[u8]) -> std::io::Result<()>,
    ) -> std::io::Result<u64>;
}

struct TxtSource {
    inputs: Vec<String>,

    incomplete: Vec<String>,
}
impl RecordSource for TxtSource {
    fn estimate_records(&self) -> usize {
        estimate_lines(&self.inputs)
    }
    fn incomplete_inputs(&self) -> &[String] {
        &self.incomplete
    }
    fn for_each(
        &mut self,
        c: &mut dyn FnMut(&[u8], &[u8], &[u8], u8, u8) -> std::io::Result<()>,
        n: &mut dyn FnMut(&[u8]) -> std::io::Result<()>,
    ) -> std::io::Result<u64> {
        let (count, incomplete) = for_each_line(&self.inputs, c, n)?;

        for p in incomplete {
            if !self.incomplete.contains(&p) {
                self.incomplete.push(p);
            }
        }
        Ok(count)
    }
}

fn estimate_lines(inputs: &[String]) -> usize {
    let mut bytes = 0u64;
    for p in inputs {
        if let Ok(m) = std::fs::metadata(p) {
            bytes += m.len();
        }
    }

    (bytes / 50) as usize + 1
}

pub type TypedRecord<'a> = (&'a [u8], &'a [u8], &'a [u8]);

struct TypedSource {
    buf: Vec<u8>,
    count: u64,
}

impl TypedSource {
    fn collect<'a>(records: impl IntoIterator<Item = TypedRecord<'a>>) -> std::io::Result<Self> {
        let mut buf = Vec::new();
        let mut count = 0u64;
        for (url, user, pass) in records {
            write_bin_rec(&mut buf, url, user, pass, b':', b':')?;
            count += 1;
        }
        Ok(TypedSource { buf, count })
    }
}

impl RecordSource for TypedSource {
    fn estimate_records(&self) -> usize {
        self.count as usize
    }
    fn for_each(
        &mut self,
        conform: &mut dyn FnMut(&[u8], &[u8], &[u8], u8, u8) -> std::io::Result<()>,
        _nonconform: &mut dyn FnMut(&[u8]) -> std::io::Result<()>,
    ) -> std::io::Result<u64> {
        let mut pos = 0usize;
        let mut n = 0u64;
        while pos < self.buf.len() {
            let rec = &self.buf[pos..];
            conform(bin_url(rec), bin_user(rec), bin_pass(rec), b':', b':')?;
            pos += bin_rec_size(rec);
            n += 1;
        }
        Ok(n)
    }
}

pub fn build(inputs: &[String], out_path: &str, delete_raw: bool) -> std::io::Result<()> {
    let incomplete = build_into(inputs, out_path)?;
    if delete_raw {
        report_kept(&incomplete, "read error, not fully indexed");
        let (deleted, failed) = delete_raw_inputs(inputs, out_path, &incomplete);
        eprintln!(
            "[delete-raw] deleted {} input(s), {} failed",
            deleted, failed
        );
    }
    Ok(())
}

pub(crate) fn build_into(inputs: &[String], out_path: &str) -> std::io::Result<Vec<String>> {
    let mut src = TxtSource {
        inputs: inputs.to_vec(),
        incomplete: Vec::new(),
    };
    build_from_source(&mut src, out_path)?;
    if !src.incomplete.is_empty() {
        return Err(incomplete_read_error(&src.incomplete));
    }
    Ok(Vec::new())
}

pub fn build_typed<'a>(
    records: impl IntoIterator<Item = TypedRecord<'a>>,
    out_path: &str,
) -> std::io::Result<()> {
    let mut src = TypedSource::collect(records)?;
    build_from_source(&mut src, out_path)
}

pub(crate) fn report_kept(kept: &[String], reason: &str) {
    for p in kept {
        eprintln!("[delete-raw] keeping {}: {}", p, reason);
    }
}

fn build_from_source(source: &mut dyn RecordSource, out_path: &str) -> std::io::Result<()> {
    let t0 = std::time::Instant::now();
    let tmp_dir = crate::temp::TempDir::new(&format!("{}.tmp", out_path))?;
    let url_ins_path = tmp_dir.join_string("urlins");
    let user_ins_path = tmp_dir.join_string("userins");
    let pass_ins_path = tmp_dir.join_string("passins");
    let sep_path = tmp_dir.join_string("sep1");
    let sep2_path = tmp_dir.join_string("sep2");
    let misc_tmp_path = tmp_dir.join_string("misc");

    eprintln!("[1/4] building URL dictionary...");
    let est_lines = source.estimate_records();
    let mut url_dedup = Dedup::new(est_lines / 10);
    let mut misc_len: u64 = 0;
    let mut misc_count: u64 = 0;
    let record_count: u64;
    {
        let mut url_ins_w =
            BufWriter::with_capacity(1 << 22, std::fs::File::create(&url_ins_path)?);
        let mut sep_w = BufWriter::with_capacity(1 << 22, std::fs::File::create(&sep_path)?);
        let mut sep2_w = BufWriter::with_capacity(1 << 22, std::fs::File::create(&sep2_path)?);
        let mut misc_w = BufWriter::with_capacity(1 << 22, std::fs::File::create(&misc_tmp_path)?);
        let mut url_batch = Vec::with_capacity(1 << 20);
        let mut sep_batch = Vec::with_capacity(1 << 20);
        let mut sep2_batch = Vec::with_capacity(1 << 20);
        let mut misc_batch = Vec::with_capacity(1 << 20);

        let mut domain_buf: Vec<u8> = Vec::new();
        record_count = source.for_each(
            &mut |url, _user, _pass, sep1, sep2| {
                normalize_domain_into(url, &mut domain_buf);
                let id = url_dedup.insert(&domain_buf);
                url_batch.extend_from_slice(&id.to_le_bytes());
                if url_batch.len() >= 1 << 20 {
                    url_ins_w.write_all(&url_batch)?;
                    url_batch.clear();
                }
                sep_batch.push(sep1);
                if sep_batch.len() >= 1 << 20 {
                    sep_w.write_all(&sep_batch)?;
                    sep_batch.clear();
                }
                sep2_batch.push(sep2);
                if sep2_batch.len() >= 1 << 20 {
                    sep2_w.write_all(&sep2_batch)?;
                    sep2_batch.clear();
                }
                Ok(())
            },
            &mut |raw| {
                write_varint(&mut misc_batch, raw.len() as u64);
                misc_batch.extend_from_slice(raw);
                if misc_batch.len() >= 1 << 20 {
                    misc_w.write_all(&misc_batch)?;
                    misc_batch.clear();
                }
                misc_len += varint_len(raw.len() as u64) + raw.len() as u64;
                misc_count += 1;
                Ok(())
            },
        )?;
        url_ins_w.write_all(&url_batch)?;
        sep_w.write_all(&sep_batch)?;
        sep2_w.write_all(&sep2_batch)?;
        misc_w.write_all(&misc_batch)?;
        url_ins_w.flush()?;
        sep_w.flush()?;
        sep2_w.flush()?;
        misc_w.flush()?;
    }
    eprintln!(
        "  url dict: {} unique, {} records",
        url_dedup.count, record_count
    );

    eprintln!("[2/4] building USER dictionary...");
    let mut user_dedup = Dedup::new(est_lines * 6 / 10);
    {
        let mut user_ins_w =
            BufWriter::with_capacity(1 << 22, std::fs::File::create(&user_ins_path)?);
        let mut user_batch = Vec::with_capacity(1 << 20);
        let c = source.for_each(
            &mut |_url, user, _pass, _sep1, _sep2| {
                let id = user_dedup.insert(user);
                user_batch.extend_from_slice(&id.to_le_bytes());
                if user_batch.len() >= 1 << 20 {
                    user_ins_w.write_all(&user_batch)?;
                    user_batch.clear();
                }
                Ok(())
            },
            &mut |_raw| Ok(()),
        )?;
        user_ins_w.write_all(&user_batch)?;
        assert_eq!(
            c, record_count,
            "build: user pass read {} records, expected {}; input changed between passes — re-run on a stable input set",
            c, record_count
        );
        user_ins_w.flush()?;
        eprintln!("  user dict: {} unique", user_dedup.count);
    }

    eprintln!("[3/4] building PASS dictionary...");
    let mut pass_dedup = Dedup::new(est_lines * 6 / 10);
    {
        let mut pass_ins_w =
            BufWriter::with_capacity(1 << 22, std::fs::File::create(&pass_ins_path)?);
        let mut pass_batch = Vec::with_capacity(1 << 20);
        let c = source.for_each(
            &mut |_url, _user, pass, _sep1, _sep2| {
                let id = pass_dedup.insert(pass);
                pass_batch.extend_from_slice(&id.to_le_bytes());
                if pass_batch.len() >= 1 << 20 {
                    pass_ins_w.write_all(&pass_batch)?;
                    pass_batch.clear();
                }
                Ok(())
            },
            &mut |_raw| Ok(()),
        )?;
        pass_ins_w.write_all(&pass_batch)?;
        assert_eq!(
            c, record_count,
            "build: pass pass read {} records, expected {}; input changed between passes — re-run on a stable input set",
            c, record_count
        );
        pass_ins_w.flush()?;
        eprintln!("  pass dict: {} unique", pass_dedup.count);
    }

    let url_count = url_dedup.count;
    let user_count = user_dedup.count;
    let pass_count = pass_dedup.count;

    let (url_section, perm_url) = serialize_dict(&mut url_dedup, false);
    drop(url_dedup);
    let (user_section, perm_user) = serialize_dict(&mut user_dedup, true);
    drop(user_dedup);
    let (pass_section, perm_pass) = serialize_dict(&mut pass_dedup, false);
    drop(pass_dedup);

    let mut out = BufWriter::with_capacity(1 << 22, std::fs::File::create(out_path)?);
    out.write_all(&[0u8; HEADER_LEN])?;
    let mut off: u64 = HEADER_LEN as u64;

    let url_dict_off = off;
    out.write_all(&url_section)?;
    off += url_section.len() as u64;

    let user_dict_off = off;
    out.write_all(&user_section)?;
    off += user_section.len() as u64;

    let pass_dict_off = off;
    out.write_all(&pass_section)?;
    off += pass_section.len() as u64;

    eprintln!("[4/4] assembling {} records...", record_count);
    let url_ins = read_u32_file(&url_ins_path)?;
    let user_ins = read_u32_file(&user_ins_path)?;
    let pass_ins = read_u32_file(&pass_ins_path)?;
    let sep1 = read_u8_file(&sep_path)?;
    let sep2 = read_u8_file(&sep2_path)?;
    let rc = url_ins.len();
    assert_eq!(
        rc as u64,
        record_count,
        "build: url insertion ids hold {rc}, expected {record_count}; temp file mismatch - delete {} and re-run",
        tmp_dir.path().display()
    );
    assert_eq!(user_ins.len(), rc, "build: user insertion ids short");
    assert_eq!(pass_ins.len(), rc, "build: pass insertion ids short");
    assert_eq!(sep1.len(), rc, "build: sep1 bytes short");
    assert_eq!(sep2.len(), rc, "build: sep2 bytes short");

    let mut records: Vec<Record> = Vec::with_capacity(rc);
    for i in 0..rc {
        records.push(Record {
            url_id: perm_url[url_ins[i] as usize],
            user_id: perm_user[user_ins[i] as usize],
            pass_id: perm_pass[pass_ins[i] as usize],
            sep1: sep1[i],
            sep2: sep2[i],
        });
    }
    drop(url_ins);
    drop(user_ins);
    drop(pass_ins);
    drop(sep1);
    drop(sep2);

    records.sort_unstable_by_key(|r| r.url_id);

    let records_off = off;
    write_records_section(&mut out, &records, url_count)?;
    off += records_section_len(rc, url_count);

    let user_idx_off = off;
    let mut user_pairs: Vec<(u32, u32)> = Vec::with_capacity(rc);
    for (i, rec) in records.iter().enumerate() {
        user_pairs.push((rec.user_id, i as u32));
    }
    user_pairs.sort_unstable_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    off += write_postings_section(&mut out, user_count, &user_pairs)?;
    drop(user_pairs);

    let pass_idx_off = off;
    let mut pass_pairs: Vec<(u32, u32)> = Vec::with_capacity(rc);
    for (i, rec) in records.iter().enumerate() {
        pass_pairs.push((rec.pass_id, i as u32));
    }
    pass_pairs.sort_unstable_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    off += write_postings_section(&mut out, pass_count, &pass_pairs)?;
    drop(pass_pairs);
    drop(records);

    let misc_off = off;
    out.write_all(&(misc_count).to_le_bytes())?;
    off += 8;
    let mut misc_f =
        std::io::BufReader::with_capacity(1 << 22, std::fs::File::open(&misc_tmp_path)?);
    std::io::copy(&mut misc_f, &mut out)?;
    drop(misc_f);
    off += misc_len;

    out.flush()?;

    let mut hdr = Vec::with_capacity(HEADER_LEN);
    hdr.extend_from_slice(&MAGIC);
    hdr.extend_from_slice(&VERSION.to_le_bytes());
    hdr.extend_from_slice(&0u32.to_le_bytes());
    hdr.extend_from_slice(&record_count.to_le_bytes());
    hdr.extend_from_slice(&(url_count as u64).to_le_bytes());
    hdr.extend_from_slice(&(user_count as u64).to_le_bytes());
    hdr.extend_from_slice(&(pass_count as u64).to_le_bytes());
    hdr.extend_from_slice(&(misc_count).to_le_bytes());
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
        "build: assembled header is {} bytes, expected {}; the header writer over/underfilled the fixed fields",
        hdr.len(),
        HEADER_LEN
    );
    out.seek(std::io::SeekFrom::Start(0))?;
    out.write_all(&hdr)?;
    out.flush()?;

    out.seek(std::io::SeekFrom::Start(off))?;
    out.write_all(&1u64.to_le_bytes())?;
    out.write_all(&0u64.to_le_bytes())?;
    out.write_all(&(off).to_le_bytes())?;
    out.flush()?;

    eprintln!(
        "done: {} records, {} urls, {} users, {} passes, {} misc -> {} ({} bytes)",
        record_count, url_count, user_count, pass_count, misc_count, out_path, off
    );
    eprintln!(
        "peak RSS {:.2} GB, took {:.1}s",
        peak_rss_gb(),
        t0.elapsed().as_secs_f64()
    );
    Ok(())
}

pub fn append(db_path: &str, inputs: &[String], delete_raw: bool) -> std::io::Result<()> {
    let incomplete = append_into(db_path, inputs)?;
    if delete_raw {
        report_kept(&incomplete, "read error, not fully indexed");
        let (deleted, failed) = delete_raw_inputs(inputs, db_path, &incomplete);
        eprintln!(
            "[delete-raw] deleted {} input(s), {} failed",
            deleted, failed
        );
    }
    Ok(())
}

pub(crate) fn append_into(db_path: &str, inputs: &[String]) -> std::io::Result<Vec<String>> {
    let mut src = TxtSource {
        inputs: inputs.to_vec(),
        incomplete: Vec::new(),
    };
    append_source(db_path, &mut src)?;
    if !src.incomplete.is_empty() {
        return Err(incomplete_read_error(&src.incomplete));
    }
    Ok(Vec::new())
}

pub fn append_typed<'a>(
    db_path: &str,
    records: impl IntoIterator<Item = TypedRecord<'a>>,
) -> std::io::Result<()> {
    let mut src = TypedSource::collect(records)?;
    append_source(db_path, &mut src)
}

fn append_source(db_path: &str, source: &mut dyn RecordSource) -> std::io::Result<()> {
    let t0 = std::time::Instant::now();
    if std::fs::metadata(db_path)?.len() == 0 {
        build_from_source(source, db_path)?;
        if !source.incomplete_inputs().is_empty() {
            let _ = std::fs::remove_file(db_path);
            return Err(incomplete_read_error(source.incomplete_inputs()));
        }
        return Ok(());
    }
    crate::inspect::validate(db_path)?;
    let (offsets, footer_off) = read_segment_meta(db_path)?;

    let tmp_dir = crate::temp::TempDir::new(&format!("{}.appendtmp", db_path))?;
    let tmp_path = tmp_dir.join_string("segment");
    build_from_source(source, &tmp_path)?;
    if !source.incomplete_inputs().is_empty() {
        return Err(incomplete_read_error(source.incomplete_inputs()));
    }
    let (_, tmp_footer_off) = read_segment_meta(&tmp_path)?;

    let mut db = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(db_path)?;
    db.set_len(footer_off)?;

    let src = std::fs::File::open(&tmp_path)?;
    let mut src_reader = BufReader::with_capacity(8 << 20, src.take(tmp_footer_off));
    db.seek(std::io::SeekFrom::Start(footer_off))?;
    let mut db_writer = BufWriter::with_capacity(8 << 20, &mut db);
    let copied = std::io::copy(&mut src_reader, &mut db_writer)?;
    db_writer.flush()?;
    drop(db_writer);
    if copied != tmp_footer_off {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "append temp segment shorter than expected",
        ));
    }

    let new_seg_count = offsets.len() as u64 + 1;
    let new_footer_off = footer_off + tmp_footer_off;
    db.seek(std::io::SeekFrom::Start(new_footer_off))?;
    db.write_all(&new_seg_count.to_le_bytes())?;
    for &o in &offsets {
        db.write_all(&o.to_le_bytes())?;
    }
    db.write_all(&footer_off.to_le_bytes())?;
    db.write_all(&new_footer_off.to_le_bytes())?;
    db.flush()?;
    drop(db);

    let total = new_footer_off + 8 + new_seg_count * 8 + 8;
    eprintln!(
        "appended -> {} segments, {} bytes total (peak RSS {:.2} GB, took {:.2}s)",
        new_seg_count,
        total,
        peak_rss_gb(),
        t0.elapsed().as_secs_f64()
    );
    Ok(())
}

pub(crate) fn delete_raw_inputs(inputs: &[String], out_path: &str, keep: &[String]) -> (u64, u64) {
    let out_canon = std::fs::canonicalize(out_path).ok();
    let mut deleted = 0u64;
    let mut failed = 0u64;
    for p in inputs {
        let path = std::path::Path::new(p);
        if path.is_dir() {
            continue;
        }
        if p == out_path {
            continue;
        }
        if let Some(out_c) = &out_canon
            && std::fs::canonicalize(p).is_ok_and(|in_c| in_c == *out_c)
        {
            continue;
        }
        if keep.iter().any(|k| k == p) {
            continue;
        }
        match std::fs::remove_file(p) {
            Ok(()) => deleted += 1,
            Err(e) => {
                eprintln!("[delete-raw] failed to delete {}: {}", p, e);
                failed += 1;
            }
        }
    }
    (deleted, failed)
}

fn read_u32_file(path: &str) -> std::io::Result<Vec<u32>> {
    let len = std::fs::metadata(path)?.len();
    let mut v = Vec::with_capacity((len / 4) as usize);
    let mut r = BufReader::with_capacity(16 << 20, std::fs::File::open(path)?);
    let mut b = [0u8; 4];
    loop {
        match r.read_exact(&mut b) {
            Ok(()) => v.push(u32::from_le_bytes(b)),
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(e),
        }
    }
    Ok(v)
}
fn read_u8_file(path: &str) -> std::io::Result<Vec<u8>> {
    std::fs::read(path)
}

fn records_section_len(record_count: usize, url_count: u32) -> u64 {
    8 + (url_count as u64 + 1) * 4
        + record_count as u64 * 4 * 2
        + 2 * (record_count as u64).div_ceil(8)
}

fn write_records_section(
    out: &mut BufWriter<std::fs::File>,
    records: &[Record],
    url_count: u32,
) -> std::io::Result<()> {
    let r = records.len();
    out.write_all(&(url_count as u64).to_le_bytes())?;
    let mut batch = Vec::with_capacity(1 << 20);
    let mut write_batch = |bytes: &[u8]| -> std::io::Result<()> {
        batch.extend_from_slice(bytes);
        if batch.len() >= 1 << 20 {
            out.write_all(&batch)?;
            batch.clear();
        }
        Ok(())
    };

    let mut url_offsets = vec![0u32; url_count as usize + 1];
    let mut cur = 0usize;
    for (u, item) in url_offsets.iter_mut().enumerate().take(url_count as usize) {
        *item = cur as u32;
        while cur < r && records[cur].url_id == u as u32 {
            cur += 1;
        }
    }
    url_offsets[url_count as usize] = r as u32;
    for &o in &url_offsets {
        write_batch(&o.to_le_bytes())?;
    }

    for rec in records.iter() {
        write_batch(&rec.user_id.to_le_bytes())?;
    }
    for rec in records.iter() {
        write_batch(&rec.pass_id.to_le_bytes())?;
    }

    let mut byte = 0u8;
    let mut bit = 0u8;
    for rec in records.iter() {
        if rec.sep1 == b'|' {
            byte |= 1 << bit;
        }
        bit += 1;
        if bit == 8 {
            write_batch(&[byte])?;
            byte = 0;
            bit = 0;
        }
    }
    if bit != 0 {
        write_batch(&[byte])?;
    }

    byte = 0;
    bit = 0;
    for rec in records.iter() {
        if rec.sep2 == b'|' {
            byte |= 1 << bit;
        }
        bit += 1;
        if bit == 8 {
            write_batch(&[byte])?;
            byte = 0;
            bit = 0;
        }
    }
    if bit != 0 {
        write_batch(&[byte])?;
    }
    out.write_all(&batch)?;
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

fn write_postings_section(
    out: &mut BufWriter<std::fs::File>,
    key_count: u32,
    pairs: &[(u32, u32)],
) -> std::io::Result<u64> {
    let mut written: u64 = 0;
    out.write_all(&(key_count as u64).to_le_bytes())?;
    written += 8;
    let mut offsets: Vec<u32> = Vec::with_capacity(key_count as usize + 1);
    let mut blob_len: u64 = 0;
    offsets.push(0u32);
    let mut i = 0;
    let mut k: u32 = 0;
    while i < pairs.len() {
        let mut prev: u64 = 0;
        let mut j = i;
        while j < pairs.len() && pairs[j].0 == k {
            blob_len += varint_len(pairs[j].1 as u64 - prev);
            prev = pairs[j].1 as u64;
            j += 1;
        }
        offsets.push(blob_len as u32);
        i = j;
        k += 1;
    }
    while offsets.len() < key_count as usize + 1 {
        offsets.push(blob_len as u32);
    }
    let mut batch = Vec::with_capacity(1 << 20);
    for &o in &offsets {
        batch.extend_from_slice(&o.to_le_bytes());
        if batch.len() >= 1 << 20 {
            out.write_all(&batch)?;
            batch.clear();
        }
    }
    written += offsets.len() as u64 * 4;

    let mut i = 0;
    let mut k: u32 = 0;
    while i < pairs.len() {
        let mut prev: u64 = 0;
        let mut j = i;
        while j < pairs.len() && pairs[j].0 == k {
            write_varint(&mut batch, pairs[j].1 as u64 - prev);
            if batch.len() >= 1 << 20 {
                out.write_all(&batch)?;
                batch.clear();
            }
            prev = pairs[j].1 as u64;
            j += 1;
        }
        i = j;
        k += 1;
    }
    out.write_all(&batch)?;
    written += blob_len;
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct ErrorAfter {
        data: Vec<u8>,
        pos: usize,
        err_at: usize,
    }
    impl std::io::Read for ErrorAfter {
        fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
            if self.pos >= self.data.len() {
                return Ok(0);
            }
            if self.pos >= self.err_at {
                return Err(std::io::Error::other("injected read error"));
            }
            let take = out
                .len()
                .min(self.err_at - self.pos)
                .min(self.data.len() - self.pos);
            out[..take].copy_from_slice(&self.data[self.pos..self.pos + take]);
            self.pos += take;
            Ok(take)
        }
    }

    struct IncompleteRecordSource {
        data: Vec<u8>,
        err_at: usize,
        incomplete: Vec<String>,
    }

    impl RecordSource for IncompleteRecordSource {
        fn estimate_records(&self) -> usize {
            1
        }

        fn incomplete_inputs(&self) -> &[String] {
            &self.incomplete
        }

        fn for_each(
            &mut self,
            c: &mut dyn FnMut(&[u8], &[u8], &[u8], u8, u8) -> std::io::Result<()>,
            n: &mut dyn FnMut(&[u8]) -> std::io::Result<()>,
        ) -> std::io::Result<u64> {
            let mock = ErrorAfter {
                data: self.data.clone(),
                pos: 0,
                err_at: self.err_at,
            };
            let mut r = BufReader::new(mock);
            let (count, clean) = read_lines(&mut r, c, n)?;
            if !clean && !self.incomplete.iter().any(|p| p == "injected") {
                self.incomplete.push("injected".to_string());
            }
            Ok(count)
        }
    }

    #[test]
    fn read_lines_reports_midstream_read_error() {
        let data = b"a.com:u1:p1\nb.com:u2:p2\n".to_vec();
        let mock = ErrorAfter {
            data,
            pos: 0,
            err_at: 12,
        };
        let mut r = BufReader::new(mock);
        let mut conf = Vec::new();
        let (n, clean) = read_lines(
            &mut r,
            &mut |u, us, p, s1, s2| {
                conf.push((u.to_vec(), us.to_vec(), p.to_vec(), s1, s2));
                Ok(())
            },
            &mut |_| Ok(()),
        )
        .unwrap();
        assert_eq!(n, 1, "lines before the error must still be indexed");
        assert!(!clean, "midstream error must not read as clean EOF");
    }

    #[test]
    fn read_lines_clean_eof_is_complete() {
        let data = b"a.com:u1:p1\nb.com:u2:p2\n".to_vec();
        let mock = ErrorAfter {
            data: data.clone(),
            pos: 0,
            err_at: data.len(),
        };
        let mut r = BufReader::new(mock);
        let (n, clean) = read_lines(&mut r, &mut |_, _, _, _, _| Ok(()), &mut |_| Ok(())).unwrap();
        assert_eq!((n, clean), (2, true));
    }

    #[test]
    fn append_aborts_before_touching_db_when_input_read_is_incomplete() {
        let dir = tempfile::tempdir().unwrap();
        let txt = dir.path().join("in.txt");
        std::fs::write(&txt, b"a.com:u:p\n").unwrap();
        let db = dir.path().join("db.ulp");
        let db_str = db.to_str().unwrap().to_string();
        build(&[txt.to_str().unwrap().to_string()], &db_str, false).unwrap();
        let before = std::fs::read(&db).unwrap();

        let mut src = IncompleteRecordSource {
            data: b"broken\n".to_vec(),
            err_at: 2,
            incomplete: Vec::new(),
        };
        let err = append_source(&db_str, &mut src).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        assert_eq!(std::fs::read(&db).unwrap(), before, "db was modified");
    }

    #[test]
    fn for_each_line_splits_conforming_and_misc() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("in.txt");
        std::fs::write(&p, b"a.com:u1:p1\nb.com|u2|p2\nbare line\n").unwrap();
        let input = p.to_str().unwrap().to_string();
        let mut conf = Vec::new();
        let mut misc: Vec<Vec<u8>> = Vec::new();
        let (n, incomplete) = for_each_line(
            &[input],
            &mut |u, us, p, s1, s2| {
                conf.push((u.to_vec(), us.to_vec(), p.to_vec(), s1, s2));
                Ok(())
            },
            &mut |raw| {
                misc.push(raw.to_vec());
                Ok(())
            },
        )
        .unwrap();
        assert!(incomplete.is_empty());
        assert_eq!(n, 2);
        assert_eq!(conf.len(), 2);
        assert_eq!(
            conf[0],
            (
                b"a.com".to_vec(),
                b"u1".to_vec(),
                b"p1".to_vec(),
                b':',
                b':'
            )
        );
        assert_eq!(
            conf[1],
            (
                b"b.com".to_vec(),
                b"u2".to_vec(),
                b"p2".to_vec(),
                b'|',
                b'|'
            )
        );
        assert_eq!(misc, vec![b"bare line".to_vec()]);
    }

    #[test]
    fn typed_source_serves_identical_passes() {
        let rows: [(&[u8], &[u8], &[u8]); 3] = [
            (b"https://Example.com/login", b"us:er", b"pw|rd"),
            (b"", b"empty", b""),
            (b"a.com", b"u", b"p"),
        ];
        let mut src = TypedSource::collect(rows).unwrap();
        assert_eq!(src.estimate_records(), 3);
        let mut pass_a = Vec::new();
        let n1 = src
            .for_each(
                &mut |u, s, p, sep1, sep2| {
                    pass_a.push((u.to_vec(), s.to_vec(), p.to_vec(), sep1, sep2));
                    Ok(())
                },
                &mut |_| panic!("typed records never hit the nonconform callback"),
            )
            .unwrap();
        let mut pass_b = Vec::new();
        let n2 = src
            .for_each(
                &mut |u, s, p, sep1, sep2| {
                    pass_b.push((u.to_vec(), s.to_vec(), p.to_vec(), sep1, sep2));
                    Ok(())
                },
                &mut |_| panic!("typed records never hit the nonconform callback"),
            )
            .unwrap();
        assert_eq!((n1, n2), (3, 3));
        assert_eq!(pass_a, pass_b, "every pass must serve the same records");
        assert_eq!(
            pass_a,
            vec![
                (
                    b"https://Example.com/login".to_vec(),
                    b"us:er".to_vec(),
                    b"pw|rd".to_vec(),
                    b':',
                    b':'
                ),
                (b"".to_vec(), b"empty".to_vec(), b"".to_vec(), b':', b':'),
                (b"a.com".to_vec(), b"u".to_vec(), b"p".to_vec(), b':', b':'),
            ]
        );
    }

    #[test]
    fn typed_source_zero_records() {
        let rows: [(&[u8], &[u8], &[u8]); 0] = [];
        let mut src = TypedSource::collect(rows).unwrap();
        assert_eq!(src.estimate_records(), 0);
        let mut seen = 0u64;
        let n = src
            .for_each(
                &mut |_, _, _, _, _| {
                    seen += 1;
                    Ok(())
                },
                &mut |_| panic!("typed records never hit the nonconform callback"),
            )
            .unwrap();
        assert_eq!((n, seen), (0, 0));
    }

    #[test]
    fn delete_raw_inputs_skips_dirs_and_output() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("a.txt");
        std::fs::write(&f, b"x").unwrap();
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        let inputs = vec![
            f.to_str().unwrap().to_string(),
            sub.to_str().unwrap().to_string(),
            "out.ulp".to_string(),
        ];
        let (deleted, failed) = delete_raw_inputs(&inputs, "out.ulp", &[]);
        assert_eq!((deleted, failed), (1, 0));
        assert!(!f.exists());
        assert!(sub.exists());

        let missing = dir.path().join("missing.txt").to_str().unwrap().to_string();
        let (deleted, failed) = delete_raw_inputs(&[missing], "out.ulp", &[]);
        assert_eq!((deleted, failed), (0, 1));
    }

    #[test]
    fn delete_raw_inputs_keeps_unindexed_inputs() {
        let dir = tempfile::tempdir().unwrap();
        let indexed = dir.path().join("indexed.txt");
        std::fs::write(&indexed, b"x").unwrap();
        let broken = dir.path().join("broken.txt");
        std::fs::write(&broken, b"y").unwrap();
        let inputs = vec![
            indexed.to_str().unwrap().to_string(),
            broken.to_str().unwrap().to_string(),
        ];
        let keep = vec![broken.to_str().unwrap().to_string()];
        let (deleted, failed) = delete_raw_inputs(&inputs, "unused.ulp", &keep);
        assert_eq!((deleted, failed), (1, 0));
        assert!(!indexed.exists(), "fully indexed input not deleted");
        assert!(broken.exists(), "unindexed input deleted");
    }

    #[test]
    fn delete_raw_inputs_falls_back_to_exact_compare_when_output_uncanonicalizable() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real.txt");
        std::fs::write(&real, b"x").unwrap();
        let out = dir.path().join("no-such-parent").join("out.ulp");
        let out_str = out.to_str().unwrap().to_string();
        let inputs = vec![out_str.clone(), real.to_str().unwrap().to_string()];
        let (deleted, failed) = delete_raw_inputs(&inputs, &out_str, &[]);
        assert_eq!((deleted, failed), (1, 0));
        assert!(!real.exists(), "real input not deleted");
    }

    #[test]
    fn delete_raw_inputs_skips_aliased_output() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("db.ulp");
        std::fs::write(&out, b"x").unwrap();
        let raw = dir.path().join("raw.txt");
        std::fs::write(&raw, b"y").unwrap();

        std::fs::create_dir(dir.path().join("sub")).unwrap();
        let aliased = dir.path().join("sub").join("..").join("db.ulp");
        let inputs = vec![
            aliased.to_str().unwrap().to_string(),
            raw.to_str().unwrap().to_string(),
        ];
        let (deleted, failed) = delete_raw_inputs(&inputs, out.to_str().unwrap(), &[]);
        assert_eq!((deleted, failed), (1, 0));
        assert!(out.exists(), "aliased output deleted");
        assert!(!raw.exists(), "raw input not deleted");
    }
}
