use crate::build::{append_into, build_into, delete_raw_inputs, report_kept};
use crate::platform::{free_space_bytes, total_free_ram};
use std::io::{BufRead, BufReader, BufWriter, Write};

fn choose_chunk_bytes(total: u64, free: u64) -> u64 {
    let tg = total as f64 / 1073741824.0;
    let fg = free as f64 / 1073741824.0;
    let floor: f64 = if tg >= 32.0 {
        5.0
    } else if tg >= 24.0 {
        4.0
    } else if tg >= 16.0 {
        3.0
    } else if tg >= 8.0 {
        2.0
    } else {
        1.0
    };
    let margin: f64 = 4.0;
    let target = ((fg - margin) / 1.8).max(1.0);
    let gb = floor.max(target).min(16.0);
    (gb * 1073741824.0) as u64
}

fn collect_dir(dir: &std::path::Path, out: &mut Vec<String>) {
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            let Ok(file_type) = p.symlink_metadata().map(|m| m.file_type()) else {
                continue;
            };
            if file_type.is_dir() && !file_type.is_symlink() {
                collect_dir(&p, out);
            } else if file_type.is_file()
                && !file_type.is_symlink()
                && let Some(s) = p.to_str()
            {
                out.push(s.to_string());
            }
        }
    }
}

fn collect_files(inputs: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for p in inputs {
        let path = std::path::Path::new(p);
        if path.is_dir() {
            collect_dir(path, &mut out);
        } else {
            out.push(p.clone());
        }
    }
    out
}

fn split_file(
    path: &str,
    chunk_bytes: u64,
    tmp_dir: &str,
    part_prefix: &str,
) -> std::io::Result<Vec<String>> {
    let f = std::fs::File::open(path)?;
    let mut r = BufReader::with_capacity(16 << 20, f);
    let mut out_paths = Vec::new();
    let mut cur: Option<BufWriter<std::fs::File>> = None;
    let mut cur_size = 0u64;
    let mut idx = 0usize;
    let mut buf: Vec<u8> = Vec::with_capacity(1 << 16);
    loop {
        buf.clear();
        let n = r.read_until(b'\n', &mut buf)?;
        if n == 0 {
            break;
        }
        if cur.is_none() || cur_size + n as u64 > chunk_bytes {
            if let Some(mut w) = cur {
                w.flush()?;
            }
            idx += 1;
            let p = format!("{}/{}_{:04}.txt", tmp_dir, part_prefix, idx);
            cur = Some(BufWriter::with_capacity(
                1 << 20,
                std::fs::File::create(&p)?,
            ));
            out_paths.push(p);
            cur_size = 0;
        }
        #[expect(
            clippy::unwrap_used,
            reason = "cur is always Some here: the guard above creates a chunk writer when cur is None"
        )]
        cur.as_mut().unwrap().write_all(&buf)?;
        cur_size += n as u64;
    }
    if let Some(mut w) = cur {
        w.flush()?;
    }
    Ok(out_paths)
}

fn split_oversized(
    items: &[(String, u64)],
    chunk_bytes: u64,
    tmp_dir: &str,
) -> (Vec<(String, String)>, Vec<String>) {
    let mut part_origin = Vec::new();
    let mut split_failed = Vec::new();
    let mut input_idx = 0usize;
    for (f, sz) in items {
        if *sz > chunk_bytes {
            let _ = std::fs::create_dir_all(tmp_dir);
            let prefix = format!("f{input_idx:04}");
            match split_file(f, chunk_bytes, tmp_dir, &prefix) {
                Ok(parts) => {
                    for p in parts {
                        part_origin.push((p, f.clone()));
                    }
                }
                Err(e) => {
                    eprintln!("[guide] failed to split {}: {}", f, e);
                    split_failed.push(f.clone());
                }
            }
            input_idx += 1;
        }
    }
    (part_origin, split_failed)
}

fn originals_with_incomplete_chunks(
    origin_chunks: &[(String, Vec<String>)],
    incomplete: &[String],
) -> Vec<String> {
    origin_chunks
        .iter()
        .filter(|(_, chunks)| chunks.iter().any(|c| incomplete.contains(c)))
        .map(|(orig, _)| orig.clone())
        .collect()
}

pub fn guide(args: &[String], delete_raw: bool) -> std::io::Result<()> {
    if args.len() < 2 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "usage: ulp guide <out.ulp> <folder-or-files...>",
        ));
    }
    let out_path = args[0].clone();
    let inputs = args[1..].to_vec();

    let (total_ram, free_ram) = total_free_ram();
    let chunk = choose_chunk_bytes(total_ram, free_ram);
    eprintln!(
        "[guide] RAM total {:.1} GiB, free {:.1} GiB -> chunk size {:.2} GiB",
        total_ram as f64 / 1073741824.0,
        free_ram as f64 / 1073741824.0,
        chunk as f64 / 1073741824.0
    );

    let files = collect_files(&inputs);
    let mut items: Vec<(String, u64)> = Vec::new();
    for f in files {
        if let Ok(m) = std::fs::metadata(&f)
            && m.is_file()
        {
            items.push((f, m.len()));
        }
    }
    items.sort_by_key(|b| std::cmp::Reverse(b.1));
    if items.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "no input files found",
        ));
    }
    let total_in: u64 = items.iter().map(|i| i.1).sum();
    eprintln!(
        "[guide] {} files, {:.2} GiB input",
        items.len(),
        total_in as f64 / 1073741824.0
    );

    if let Some(free_space) = free_space_bytes(&out_path) {
        let est_out = (total_in as f64 / 1.5) as u64;
        if est_out > free_space {
            eprintln!(
                "[guide] WARNING: est output ~{:.2} GiB but only {:.2} GiB free at {}",
                est_out as f64 / 1073741824.0,
                free_space as f64 / 1073741824.0,
                out_path
            );
        } else {
            eprintln!(
                "[guide] free space {:.2} GiB OK (est output ~{:.2} GiB)",
                free_space as f64 / 1073741824.0,
                est_out as f64 / 1073741824.0
            );
        }
    }

    let split_scratch = crate::temp::TempDir::new(&format!("{}.split.tmp", out_path))?;
    let tmp_dir = split_scratch.path_string();
    let (part_origin, mut split_failed) = split_oversized(&items, chunk, &tmp_dir);
    let need_split = !part_origin.is_empty() || !split_failed.is_empty();
    let mut chunks: Vec<Vec<String>> = Vec::new();
    let mut origin_chunks: Vec<(String, Vec<String>)> = Vec::new();
    for (part, orig) in part_origin {
        chunks.push(vec![part.clone()]);

        match origin_chunks.last_mut() {
            Some((o, parts)) if *o == orig => parts.push(part),
            _ => origin_chunks.push((orig, vec![part])),
        }
    }
    let mut bins: Vec<Vec<String>> = Vec::new();
    let mut bin_sizes: Vec<u64> = Vec::new();
    for (f, sz) in &items {
        if *sz > chunk {
            continue;
        }
        let mut placed = false;
        for i in 0..bins.len() {
            if bin_sizes[i] + sz <= chunk {
                bins[i].push(f.clone());
                bin_sizes[i] += sz;
                placed = true;
                break;
            }
        }
        if !placed {
            bins.push(vec![f.clone()]);
            bin_sizes.push(*sz);
        }
        origin_chunks.push((f.clone(), vec![f.clone()]));
    }
    for b in bins {
        chunks.push(b);
    }

    eprintln!("[guide] {} chunks", chunks.len());
    if chunks.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "nothing to do",
        ));
    }

    let t0 = std::time::Instant::now();
    let mut incomplete: Vec<String> = Vec::new();
    match build_into(&chunks[0], &out_path) {
        Ok(inc) => incomplete.extend(inc),
        Err(e) => {
            eprintln!("[guide] build failed: {}", e);
            return Err(e);
        }
    }
    for (i, c) in chunks.iter().enumerate().skip(1) {
        eprintln!("[guide] appending chunk {}/{}", i + 1, chunks.len());
        match append_into(&out_path, c) {
            Ok(inc) => incomplete.extend(inc),
            Err(e) => {
                eprintln!("[guide] append chunk {} failed: {}", i + 1, e);
                return Err(e);
            }
        }
    }
    eprintln!(
        "[guide] done -> {} in {:.1}s",
        out_path,
        t0.elapsed().as_secs_f64()
    );
    if need_split && !delete_raw {
        let kept = split_scratch.persist();
        eprintln!(
            "[guide] split parts left in {} (safe to delete)",
            kept.display()
        );
    }
    if delete_raw {
        dedup_strings(&mut split_failed);
        report_kept(&split_failed, "split failed, not indexed");

        let mut incomplete_origins = originals_with_incomplete_chunks(&origin_chunks, &incomplete);
        dedup_strings(&mut incomplete_origins);
        report_kept(&incomplete_origins, "chunk read error, not fully indexed");
        let mut keep = split_failed;
        keep.extend(incomplete_origins);
        let split_tmp = need_split.then_some(tmp_dir.as_str());
        let (deleted, failed) = delete_raw_cleanup(&items, &out_path, split_tmp, &keep);
        eprintln!(
            "[delete-raw] deleted {} input(s), {} failed",
            deleted, failed
        );
    }
    Ok(())
}

fn dedup_strings(v: &mut Vec<String>) {
    let mut seen = std::collections::HashSet::with_capacity(v.len());
    v.retain(|s| {
        if seen.contains(s) {
            false
        } else {
            seen.insert(s.clone());
            true
        }
    });
}

fn delete_raw_cleanup(
    items: &[(String, u64)],
    out_path: &str,
    split_tmp: Option<&str>,
    keep: &[String],
) -> (u64, u64) {
    let mut files: Vec<String> = items.iter().map(|(f, _)| f.clone()).collect();
    dedup_strings(&mut files);
    let (deleted, mut failed) = delete_raw_inputs(&files, out_path, keep);
    if let Some(tmp_dir) = split_tmp
        && let Err(e) = std::fs::remove_dir_all(tmp_dir)
    {
        eprintln!("[delete-raw] failed to delete {}: {}", tmp_dir, e);
        failed += 1;
    }
    (deleted, failed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choose_chunk_bytes_table() {
        const GIB: u64 = 1073741824;
        let cases: &[((u64, u64), u64)] = &[
            ((32 * GIB, 4 * GIB), 5_368_709_120),
            ((24 * GIB, 20 * GIB), 9_544_371_768),
            ((16 * GIB, 6 * GIB), 3_221_225_472),
            ((8 * GIB, 100 * GIB), 17_179_869_184),
            ((4 * GIB, 8 * GIB), 2_386_092_942),
            ((4 * GIB, 2 * GIB), 1_073_741_824),
            ((64 * GIB, 200 * GIB), 17_179_869_184),
            ((GIB, GIB / 2), 1_073_741_824),
        ];
        for ((total, free), want) in cases {
            assert_eq!(
                choose_chunk_bytes(*total, *free),
                *want,
                "total={total} free={free}"
            );
        }
    }

    #[test]
    fn split_file_keeps_lines_whole() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("big.txt");
        let mut content = Vec::new();
        for i in 0..10 {
            content.extend_from_slice(format!("line{i}\n").as_bytes());
        }
        std::fs::write(&p, &content).unwrap();
        let tmp = dir.path().join("parts");
        std::fs::create_dir(&tmp).unwrap();
        let parts = split_file(p.to_str().unwrap(), 3, tmp.to_str().unwrap(), "f0").unwrap();
        let mut joined = Vec::new();
        for part in &parts {
            let b = std::fs::read(part).unwrap();
            assert!(
                b.is_empty() || b.last() == Some(&b'\n'),
                "{part}: not line-aligned"
            );
            joined.extend_from_slice(&b);
        }
        assert_eq!(joined, content);
    }

    #[test]
    fn delete_raw_cleanup_removes_files_and_split_tmp() {
        let dir = tempfile::tempdir().unwrap();
        let f1 = dir.path().join("a.txt");
        let f2 = dir.path().join("b.txt");
        std::fs::write(&f1, b"x").unwrap();
        std::fs::write(&f2, b"y").unwrap();
        let split = dir.path().join("db.ulp.split.tmp");
        std::fs::create_dir(&split).unwrap();
        std::fs::write(split.join("_split_0001.txt"), b"part").unwrap();
        let items = vec![
            (f1.to_str().unwrap().to_string(), 1u64),
            (f2.to_str().unwrap().to_string(), 1u64),
        ];
        let (deleted, failed) =
            delete_raw_cleanup(&items, "unused.ulp", Some(split.to_str().unwrap()), &[]);
        assert_eq!((deleted, failed), (2, 0));
        assert!(!f1.exists() && !f2.exists());
        assert!(!split.exists(), "split tmp dir not removed");
    }

    #[test]
    fn delete_raw_cleanup_dedups_duplicate_paths() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("a.txt");
        std::fs::write(&f, b"x").unwrap();
        let items = vec![
            (f.to_str().unwrap().to_string(), 1u64),
            (f.to_str().unwrap().to_string(), 1u64),
        ];
        let (deleted, failed) = delete_raw_cleanup(&items, "unused.ulp", None, &[]);
        assert_eq!((deleted, failed), (1, 0));
        assert!(!f.exists(), "duplicate input left behind or double-counted");
    }

    #[test]
    fn split_failure_excludes_input_from_deletion() {
        let dir = tempfile::tempdir().unwrap();
        let big = dir.path().join("big.txt");
        std::fs::write(&big, b"a.com:u1:p1\n").unwrap();
        let small = dir.path().join("small.txt");
        std::fs::write(&small, b"c.com:u3:p3\n").unwrap();

        let tmp = dir.path().join("db.ulp.split.tmp");
        std::fs::create_dir(&tmp).unwrap();
        std::fs::create_dir(tmp.join("f0000_0001.txt")).unwrap();

        let items = vec![
            (big.to_str().unwrap().to_string(), 100u64),
            (small.to_str().unwrap().to_string(), 5u64),
        ];
        let (part_origin, split_failed) = split_oversized(&items, 10, tmp.to_str().unwrap());
        assert!(part_origin.is_empty(), "failed split must yield no parts");
        assert_eq!(split_failed, vec![big.to_str().unwrap().to_string()]);

        let (deleted, failed) = delete_raw_cleanup(
            &items,
            "unused.ulp",
            Some(tmp.to_str().unwrap()),
            &split_failed,
        );
        assert_eq!((deleted, failed), (1, 0));
        assert!(big.exists(), "split-failed input deleted");
        assert!(!small.exists(), "indexed fixture not deleted");
    }

    #[test]
    fn originals_with_incomplete_chunks_keeps_only_affected() {
        let origin_chunks = vec![
            (
                "big.txt".to_string(),
                vec![
                    "parts/_split_0001.txt".to_string(),
                    "parts/_split_0002.txt".to_string(),
                ],
            ),
            (
                "ok.txt".to_string(),
                vec!["parts/_split_0003.txt".to_string()],
            ),
            ("small.txt".to_string(), vec!["small.txt".to_string()]),
        ];
        let incomplete = vec!["parts/_split_0002.txt".to_string(), "small.txt".to_string()];
        assert_eq!(
            originals_with_incomplete_chunks(&origin_chunks, &incomplete),
            vec!["big.txt".to_string(), "small.txt".to_string()]
        );
    }

    #[test]
    fn split_oversized_uses_distinct_names_per_input() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        std::fs::write(&a, b"aaaa\nbbbb\n").unwrap();
        std::fs::write(&b, b"1111\n2222\n").unwrap();
        let tmp = dir.path().join("parts");
        std::fs::create_dir(&tmp).unwrap();
        let items = vec![
            (a.to_str().unwrap().to_string(), 100u64),
            (b.to_str().unwrap().to_string(), 100u64),
        ];
        let (parts, failed) = split_oversized(&items, 4, tmp.to_str().unwrap());
        assert!(failed.is_empty(), "{failed:?}");
        assert_eq!(parts.len(), 4);
        assert_ne!(parts[0].0, parts[2].0, "part names collided across inputs");
        assert_eq!(std::fs::read(&parts[0].0).unwrap(), b"aaaa\n");
        assert_eq!(std::fs::read(&parts[1].0).unwrap(), b"bbbb\n");
        assert_eq!(std::fs::read(&parts[2].0).unwrap(), b"1111\n");
        assert_eq!(std::fs::read(&parts[3].0).unwrap(), b"2222\n");
    }
}
