use crate::domain::normalize_domain_into;
use crate::format::{Header, corrupt, parse_header, segments_of};
use crate::mmap::MappedFile;
use crate::parser::parse_line_into;
use crate::reader::Reader;
use std::io::{self, BufRead, BufReader, BufWriter, Write};

pub fn validate(path: &str) -> std::io::Result<()> {
    let m = MappedFile::open(path)?;
    let s = m.as_slice();
    let normalize = |e: io::Error| match e.kind() {
        io::ErrorKind::InvalidData | io::ErrorKind::UnexpectedEof => {
            crate::format::corrupt("invalid or truncated .ulp store")
        }
        _ => e,
    };
    for seg in segments_of(s).map_err(normalize)? {
        Reader::new(seg).map_err(normalize)?;
    }
    Ok(())
}

pub fn norm(inputs: &[String]) -> std::io::Result<()> {
    let stdout = std::io::stdout();
    let mut lock = BufWriter::with_capacity(8 << 20, stdout.lock());
    norm_into(inputs, &mut lock)
}

pub fn norm_into(inputs: &[String], out: &mut dyn Write) -> std::io::Result<()> {
    for input in inputs {
        let f = std::fs::File::open(input)?;
        let mut r = BufReader::with_capacity(16 * 1024 * 1024, f);
        let mut buf: Vec<u8> = Vec::with_capacity(256);

        let (mut url, mut user, mut pass, mut scratch, mut dom) =
            (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
        loop {
            buf.clear();
            match r.read_until(b'\n', &mut buf) {
                Ok(0) => break,
                Ok(_) => {}
                Err(e) => return Err(e),
            }
            while buf.last() == Some(&b'\n') || buf.last() == Some(&b'\r') {
                buf.pop();
            }
            match parse_line_into(&buf, &mut url, &mut user, &mut pass, &mut scratch) {
                Some((s1, s2)) => {
                    normalize_domain_into(&url, &mut dom);
                    out.write_all(&dom)?;
                    out.write_all(&[s1])?;
                    out.write_all(&user)?;
                    out.write_all(&[s2])?;
                    out.write_all(&pass)?;
                    out.write_all(b"\n")?;
                }
                None => {
                    out.write_all(&buf)?;
                    out.write_all(b"\n")?;
                }
            }
        }
    }
    out.flush()
}

pub fn bench(path: &str, field: &str, value: &str, n: u64) -> std::io::Result<()> {
    if !matches!(field, "url" | "user" | "pass") {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("unknown field '{field}' (expected url|user|pass)"),
        ));
    }
    if n == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "bench iteration count must be greater than zero",
        ));
    }
    let m = MappedFile::open(path)?;
    let s = m.as_slice();
    let segs = segments_of(s)?;
    let vb = value.as_bytes();
    let t0 = std::time::Instant::now();
    let mut found = 0u64;
    let mut rev_scratch = Vec::new();

    for seg in &segs {
        let r = Reader::new(seg)?;
        for _ in 0..n {
            let id = match field {
                "url" => r.url_dict.search_with_scratch(vb, &mut rev_scratch)?,
                "user" => r.user_dict.search_with_scratch(vb, &mut rev_scratch)?,
                _ => r.pass_dict.search_with_scratch(vb, &mut rev_scratch)?,
            };
            if id.is_some() {
                found += 1;
            }
        }
    }
    let dt = t0.elapsed();
    let lookups = n * segs.len() as u64;
    eprintln!(
        "{} in-process lookups ({}) in {:?} => {:.2} us/lookup (found {})",
        lookups,
        field,
        dt,
        dt.as_micros() as f64 / lookups as f64,
        found
    );
    Ok(())
}

fn fmt_size(b: u64) -> String {
    if b >= 1073741824 {
        format!("{:.2} GB", b as f64 / 1073741824.0)
    } else if b >= 1048576 {
        format!("{:.2} MB", b as f64 / 1048576.0)
    } else if b >= 1024 {
        format!("{:.2} KB", b as f64 / 1024.0)
    } else {
        format!("{} B", b)
    }
}

fn validate_header_offsets(h: &Header, seg_len: usize) -> std::io::Result<()> {
    let offs = [
        h.url_dict_off,
        h.user_dict_off,
        h.pass_dict_off,
        h.records_off,
        h.user_idx_off,
        h.pass_idx_off,
        h.misc_off,
    ];
    if offs.windows(2).any(|w| w[0] > w[1]) || h.misc_off > seg_len as u64 {
        return Err(corrupt("header section offsets out of order: corrupt .ulp"));
    }
    Ok(())
}

pub fn info(path: &str) -> std::io::Result<()> {
    let m = MappedFile::open(path)?;
    let s = m.as_slice();
    let meta = std::fs::metadata(path)?;
    let segs = segments_of(s)?;
    println!("file          : {}", path);
    println!(
        "size          : {} bytes ({:.2} GiB)",
        meta.len(),
        meta.len() as f64 / 1073741824.0
    );
    println!("segments      : {}", segs.len());
    let (mut tr, mut tu, mut tus, mut tp, mut tm) = (0u64, 0u64, 0u64, 0u64, 0u64);
    for (i, seg) in segs.iter().enumerate() {
        let h = parse_header(seg)?;
        validate_header_offsets(&h, seg.len())?;
        println!(
            "  seg {}: {} records, {} urls, {} users, {} passes, {} misc",
            i, h.record_count, h.url_count, h.user_count, h.pass_count, h.misc_count
        );
        println!(
            "       url {} | user {} | pass {} | rec {} | uidx {} | pidx {} | misc {}",
            fmt_size(h.user_dict_off - h.url_dict_off),
            fmt_size(h.pass_dict_off - h.user_dict_off),
            fmt_size(h.records_off - h.pass_dict_off),
            fmt_size(h.user_idx_off - h.records_off),
            fmt_size(h.pass_idx_off - h.user_idx_off),
            fmt_size(h.misc_off - h.pass_idx_off),
            fmt_size(seg.len() as u64 - h.misc_off)
        );
        tr += h.record_count;
        tu += h.url_count;
        tus += h.user_count;
        tp += h.pass_count;
        tm += h.misc_count;
    }
    println!("TOTAL records : {}", tr);
    println!("TOTAL urls    : {}", tu);
    println!("TOTAL users   : {}", tus);
    println!("TOTAL passes  : {}", tp);
    println!("TOTAL misc    : {}", tm);
    Ok(())
}

pub fn info_json(path: &str) -> std::io::Result<()> {
    let stdout = std::io::stdout();
    let mut lock = BufWriter::with_capacity(8 << 20, stdout.lock());
    info_json_into(path, &mut lock)
}

pub fn info_json_into(path: &str, out: &mut dyn Write) -> io::Result<()> {
    let m = MappedFile::open(path)?;
    let s = m.as_slice();
    let meta = std::fs::metadata(path)?;
    let headers: Vec<Header> = segments_of(s)?
        .iter()
        .map(|seg| parse_header(seg))
        .collect::<io::Result<_>>()?;
    let Some(first) = headers.first() else {
        return Err(corrupt("empty segment table: corrupt .ulp"));
    };
    let version = first.version;
    if headers[1..].iter().any(|h| h.version != version) {
        return Err(corrupt(
            "mixed format versions across segments: corrupt .ulp",
        ));
    }
    let (mut tr, mut tu, mut tus, mut tp, mut tm) = (0u64, 0u64, 0u64, 0u64, 0u64);
    for h in &headers {
        tr += h.record_count;
        tu += h.url_count;
        tus += h.user_count;
        tp += h.pass_count;
        tm += h.misc_count;
    }
    write!(out, "{{\"file\":")?;
    write_json_string(path, out)?;
    write!(out, ",\"size_bytes\":{}", meta.len())?;
    write!(out, ",\"format_version\":{version}")?;
    write!(out, ",\"segment_count\":{}", headers.len())?;
    write!(
        out,
        ",\"records\":{tr},\"urls\":{tu},\"users\":{tus},\"passes\":{tp},\"misc\":{tm}"
    )?;
    out.write_all(b",\"segments\":[")?;
    for (i, h) in headers.iter().enumerate() {
        if i > 0 {
            out.write_all(b",")?;
        }
        write!(
            out,
            "{{\"records\":{},\"urls\":{},\"users\":{},\"passes\":{},\"misc\":{}}}",
            h.record_count, h.url_count, h.user_count, h.pass_count, h.misc_count
        )?;
    }
    out.write_all(b"]}\n")?;
    out.flush()
}

fn write_json_string(s: &str, out: &mut dyn Write) -> io::Result<()> {
    out.write_all(b"\"")?;
    for c in s.chars() {
        match c {
            '"' => out.write_all(b"\\\"")?,
            '\\' => out.write_all(b"\\\\")?,
            '\n' => out.write_all(b"\\n")?,
            '\r' => out.write_all(b"\\r")?,
            '\t' => out.write_all(b"\\t")?,
            '\u{08}' => out.write_all(b"\\b")?,
            '\u{0c}' => out.write_all(b"\\f")?,
            c if (c as u32) < 0x20 => write!(out, "\\u{:04x}", c as u32)?,
            c => {
                let mut buf = [0u8; 4];
                out.write_all(c.encode_utf8(&mut buf).as_bytes())?;
            }
        }
    }
    out.write_all(b"\"")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn escaped(s: &str) -> String {
        let mut out = Vec::new();
        write_json_string(s, &mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn json_escape_quotes_backslashes_and_controls() {
        assert_eq!(escaped("plain"), r#""plain""#);
        assert_eq!(escaped(r#"a"b"#), r#""a\"b""#);
        assert_eq!(escaped(r"a\b"), r#""a\\b""#);
        assert_eq!(escaped("a\nb"), r#""a\nb""#);
        assert_eq!(escaped("a\rb"), r#""a\rb""#);
        assert_eq!(escaped("a\tb"), r#""a\tb""#);
        assert_eq!(escaped("a\x08b"), r#""a\bb""#);
        assert_eq!(escaped("a\x0cb"), r#""a\fb""#);
        assert_eq!(escaped("a\u{1}b"), r#""a\u0001b""#);
    }
}
