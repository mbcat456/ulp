#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::Write;
use std::path::Path;

const ROWS_A: &[(&str, &str, &str)] = &[
    ("alpha.com", "user_a1", "pass_a1"),
    ("beta.com", "user_a2", "pass_a2"),
];
const ROWS_B: &[(&str, &str, &str)] = &[
    ("gamma.com", "user_b1", "pass_b1"),
    ("delta.com", "user_b2", "pass_b2"),
];

fn typed_store(path: &Path, rows: &[(&str, &str, &str)]) -> String {
    let p = path.to_str().unwrap().to_string();
    ulp::build_typed(
        rows.iter()
            .map(|&(u, s, p)| (u.as_bytes(), s.as_bytes(), p.as_bytes())),
        &p,
    )
    .unwrap();
    p
}

fn misc_store(path: &Path, rows: &[&str]) -> String {
    let txt = path.with_extension("txt");
    let mut content = rows.join("\n");
    content.push_str("\nno-separator line\n");
    std::fs::write(&txt, content).unwrap();
    let out = path.to_str().unwrap().to_string();
    ulp::build(&[txt.to_str().unwrap().to_string()], &out, false).unwrap();
    out
}

fn two_segment_store(base: &Path, name: &str) -> String {
    let db = typed_store(&base.join(name), ROWS_A);
    ulp::append_typed(
        &db,
        ROWS_B
            .iter()
            .map(|&(u, s, p)| (u.as_bytes(), s.as_bytes(), p.as_bytes())),
    )
    .unwrap();
    db
}

fn segment_only(bytes: &[u8]) -> Vec<u8> {
    let n = bytes.len();
    let ptr = u64::from_le_bytes(bytes[n - 8..].try_into().unwrap());
    bytes[..ptr as usize].to_vec()
}

fn r64(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

fn minimal_segment() -> Vec<u8> {
    let mut seg = vec![0u8; 175];
    seg[0..8].copy_from_slice(b"ULPFMT01");
    seg[8..12].copy_from_slice(&1u32.to_le_bytes());
    seg[64..72].copy_from_slice(&128u64.to_le_bytes());
    seg[72..80].copy_from_slice(&129u64.to_le_bytes());
    seg[80..88].copy_from_slice(&130u64.to_le_bytes());
    seg[88..96].copy_from_slice(&131u64.to_le_bytes());
    seg[96..104].copy_from_slice(&143u64.to_le_bytes());
    seg[104..112].copy_from_slice(&155u64.to_le_bytes());
    seg[112..120].copy_from_slice(&167u64.to_le_bytes());
    seg[120..128].copy_from_slice(&175u64.to_le_bytes());
    seg
}

fn guard_ok(b: &[u8]) -> bool {
    if b.len() < 8 {
        return false;
    }
    let len = b.len() as u64;
    let ptr = r64(b, b.len() - 8);
    if ptr == 0 || ptr + 8 > len {
        return false;
    }
    let ptr = ptr as usize;
    let seg_count = r64(b, ptr);
    if seg_count == 0 || seg_count > 1_000_000 {
        return false;
    }
    if ptr as u64 + 16 + seg_count * 8 != len {
        return false;
    }
    let mut offs = Vec::with_capacity(seg_count as usize);
    for i in 0..seg_count as usize {
        offs.push(r64(b, ptr + 8 + i * 8));
    }
    offs.first() == Some(&0)
        && offs.windows(2).all(|w| w[0] < w[1])
        && offs.iter().all(|&o| o < ptr as u64)
}

fn query(db: &str, field: &str, value: &str) -> Vec<u8> {
    let mut out = Vec::new();
    ulp::query(db, field, value, &mut out).unwrap();
    out
}

#[test]
fn valid_store_is_noop_and_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let db = typed_store(&dir.path().join("a.ulp"), ROWS_A);
    ulp::append_typed(
        &db,
        ROWS_B
            .iter()
            .map(|&(u, s, p)| (u.as_bytes(), s.as_bytes(), p.as_bytes())),
    )
    .unwrap();

    let before = std::fs::read(&db).unwrap();
    assert!(guard_ok(&before), "fixture must already pass the guard");

    let out = ulp::repair(&db).unwrap();
    assert!(!out.changed);
    assert_eq!((out.segments_kept, out.dropped_bytes), (2, 0));
    assert_eq!(std::fs::read(&db).unwrap(), before);
}

#[test]
fn truncated_second_segment_recovers_first() {
    let dir = tempfile::tempdir().unwrap();

    let store_a = misc_store(
        &dir.path().join("a.ulp"),
        &["alpha.com:user_a1:pass_a1", "beta.com:user_a2:pass_a2"],
    );
    let store_b = misc_store(
        &dir.path().join("b.ulp"),
        &["gamma.com:user_b1:pass_b1", "delta.com:user_b2:pass_b2"],
    );
    let seg_a = segment_only(&std::fs::read(&store_a).unwrap());
    let seg_b = segment_only(&std::fs::read(&store_b).unwrap());

    let misc_off_b = r64(&seg_b, 112) as usize;
    assert_eq!(r64(&seg_b, 48), 1, "fixture: segment B has one misc entry");
    let cut = misc_off_b + 14;
    assert!(
        cut > 128 && cut < seg_b.len(),
        "fixture: cut {cut} lands inside segment B ({} bytes)",
        seg_b.len()
    );
    let mut torn = seg_a.clone();
    torn.extend_from_slice(&seg_b[..cut]);
    let db = dir.path().join("torn.ulp");
    let db = db.to_str().unwrap().to_string();
    std::fs::write(&db, &torn).unwrap();

    let out = ulp::repair(&db).unwrap();
    assert!(out.changed);
    assert_eq!(out.segments_kept, 1);
    assert_eq!(out.dropped_bytes, cut as u64);

    let fixed = std::fs::read(&db).unwrap();
    assert!(guard_ok(&fixed));
    assert_eq!(fixed.len(), seg_a.len() + 24);
    assert_eq!(fixed[..seg_a.len()], seg_a[..]);

    assert!(!query(&db, "user", "user_a1").is_empty());
    assert!(query(&db, "user", "user_b1").is_empty());
}

#[test]
fn garbage_tail_is_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let db = typed_store(&dir.path().join("a.ulp"), ROWS_A);
    ulp::append_typed(
        &db,
        ROWS_B
            .iter()
            .map(|&(u, s, p)| (u.as_bytes(), s.as_bytes(), p.as_bytes())),
    )
    .unwrap();

    let mut bytes = std::fs::read(&db).unwrap();
    let footer_off = bytes.len() - 32;
    bytes.extend_from_slice(&[0xABu8; 300]);
    std::fs::write(&db, &bytes).unwrap();

    let out = ulp::repair(&db).unwrap();
    assert!(out.changed);
    assert_eq!(out.segments_kept, 2);
    assert_eq!(out.dropped_bytes, 32 + 300);

    let fixed = std::fs::read(&db).unwrap();
    assert!(guard_ok(&fixed));
    assert_eq!(fixed.len(), footer_off + 32);
    assert!(!query(&db, "user", "user_a2").is_empty());
    assert!(!query(&db, "user", "user_b2").is_empty());
}

#[test]
fn partial_footer_is_rewritten() {
    let dir = tempfile::tempdir().unwrap();
    let db = typed_store(&dir.path().join("a.ulp"), ROWS_A);
    ulp::append_typed(
        &db,
        ROWS_B
            .iter()
            .map(|&(u, s, p)| (u.as_bytes(), s.as_bytes(), p.as_bytes())),
    )
    .unwrap();

    let mut bytes = std::fs::read(&db).unwrap();
    bytes.truncate(bytes.len() - 8);
    std::fs::write(&db, &bytes).unwrap();

    let out = ulp::repair(&db).unwrap();
    assert!(out.changed);
    assert_eq!(out.segments_kept, 2);
    assert_eq!(out.dropped_bytes, 24);

    let fixed = std::fs::read(&db).unwrap();
    assert!(guard_ok(&fixed));
    assert!(!query(&db, "user", "user_a1").is_empty());
    assert!(!query(&db, "user", "user_b1").is_empty());
}

#[test]
fn garbage_footer_is_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let db = two_segment_store(dir.path(), "a.ulp");

    let mut bytes = std::fs::read(&db).unwrap();
    let n = bytes.len();
    bytes[n - 8..].copy_from_slice(&u64::MAX.to_le_bytes());
    std::fs::write(&db, &bytes).unwrap();

    let out = ulp::repair(&db).unwrap();
    assert!(out.changed);
    assert_eq!(out.segments_kept, 2);
    assert_eq!(out.dropped_bytes, 32);

    let fixed = std::fs::read(&db).unwrap();
    assert!(guard_ok(&fixed));
    assert!(!query(&db, "user", "user_a2").is_empty());
    assert!(!query(&db, "user", "user_b2").is_empty());

    let db = two_segment_store(dir.path(), "b.ulp");
    let mut bytes = std::fs::read(&db).unwrap();
    let n = bytes.len();
    let footer_off = n - 32;
    bytes[footer_off + 16..footer_off + 24].copy_from_slice(&0u64.to_le_bytes());
    std::fs::write(&db, &bytes).unwrap();

    let out = ulp::repair(&db).unwrap();
    assert!(out.changed);
    assert_eq!(out.segments_kept, 2);
    assert_eq!(out.dropped_bytes, 32);

    let fixed = std::fs::read(&db).unwrap();
    assert!(guard_ok(&fixed));
    assert!(!query(&db, "user", "user_a2").is_empty());
    assert!(!query(&db, "user", "user_b2").is_empty());
}

#[test]
fn footerless_store_gets_footer() {
    let dir = tempfile::tempdir().unwrap();
    let src = typed_store(&dir.path().join("a.ulp"), ROWS_A);
    let seg = segment_only(&std::fs::read(&src).unwrap());
    let db = dir.path().join("legacy.ulp");
    let db = db.to_str().unwrap().to_string();
    std::fs::write(&db, &seg).unwrap();

    let out = ulp::repair(&db).unwrap();
    assert!(out.changed);
    assert_eq!(out.segments_kept, 1);
    assert_eq!(out.dropped_bytes, 0);

    let fixed = std::fs::read(&db).unwrap();
    assert!(guard_ok(&fixed));
    assert_eq!(fixed.len(), seg.len() + 24);
    assert_eq!(fixed[..seg.len()], seg[..]);
    assert!(!query(&db, "user", "user_a1").is_empty());
}

#[test]
fn mixed_version_segment_is_refused_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let db = typed_store(&dir.path().join("a.ulp"), ROWS_A);
    ulp::append_typed(
        &db,
        ROWS_B
            .iter()
            .map(|&(u, s, p)| (u.as_bytes(), s.as_bytes(), p.as_bytes())),
    )
    .unwrap();

    let mut bytes = std::fs::read(&db).unwrap();
    let n = bytes.len();
    let footer_off = n - 32;
    let seg1_end = r64(&bytes, footer_off + 16) as usize;
    bytes[seg1_end + 8..seg1_end + 12].copy_from_slice(&2u32.to_le_bytes());
    std::fs::write(&db, &bytes).unwrap();

    let err = ulp::repair(&db).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    assert!(err.to_string().contains("declares format version"));
    assert_eq!(std::fs::read(&db).unwrap(), bytes);
}

#[test]
fn over_cap_store_is_refused_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("many.ulp");
    let db = db.to_str().unwrap().to_string();

    let seg = minimal_segment();
    let file = std::fs::File::create(&db).unwrap();
    let mut w = std::io::BufWriter::new(file);
    for _ in 0..1_000_001 {
        w.write_all(&seg).unwrap();
    }
    w.flush().unwrap();
    drop(w);

    let err = ulp::repair(&db).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    assert!(err.to_string().contains("merge first"));
    assert_eq!(std::fs::metadata(&db).unwrap().len(), 1_000_001u64 * 175);
}

#[test]
fn empty_file_is_noop() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("empty.ulp");
    let db = db.to_str().unwrap().to_string();
    std::fs::write(&db, []).unwrap();

    let out = ulp::repair(&db).unwrap();
    assert!(!out.changed);
    assert_eq!((out.segments_kept, out.dropped_bytes), (0, 0));
    assert_eq!(std::fs::read(&db).unwrap(), b"");
}

#[test]
fn foreign_file_is_refused_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("foreign.ulp");
    let db = db.to_str().unwrap().to_string();
    let bytes = vec![0x5Au8; 300];
    std::fs::write(&db, &bytes).unwrap();

    let err = ulp::repair(&db).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    assert!(err.to_string().contains("not a .ulp store"));
    assert_eq!(std::fs::read(&db).unwrap(), bytes);

    let short = vec![0x11u8; 100];
    std::fs::write(&db, &short).unwrap();
    let err = ulp::repair(&db).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    assert!(err.to_string().contains("no valid segment"));
    assert_eq!(std::fs::read(&db).unwrap(), short);

    let zero = vec![0u8; 16];
    std::fs::write(&db, &zero).unwrap();
    let err = ulp::repair(&db).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    assert!(err.to_string().contains("no valid segment"));
    assert_eq!(std::fs::read(&db).unwrap(), zero);
}

#[test]
fn bin_repair_contract() {
    let dir = tempfile::tempdir().unwrap();
    let run = |path: &str| {
        std::process::Command::new(env!("CARGO_BIN_EXE_ulp"))
            .args(["repair", path])
            .output()
            .unwrap()
    };

    let db = typed_store(&dir.path().join("valid.ulp"), ROWS_A);
    let out = run(&db);
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stderr).contains("no changes"));

    let db = typed_store(&dir.path().join("torn.ulp"), ROWS_A);
    let mut bytes = std::fs::read(&db).unwrap();
    bytes.truncate(bytes.len() - 8);
    std::fs::write(&db, &bytes).unwrap();
    let out = run(&db);
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stderr).contains("footer rewritten"));

    let db = dir.path().join("foreign.ulp");
    let db = db.to_str().unwrap().to_string();
    std::fs::write(&db, vec![0x5Au8; 300]).unwrap();
    let out = run(&db);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("not a .ulp store"));
}

#[test]
fn torn_first_segment_is_refused_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let src = typed_store(&dir.path().join("a.ulp"), ROWS_A);
    let seg = segment_only(&std::fs::read(&src).unwrap());

    let cut = 200;
    assert!(
        cut > 128 && cut < seg.len(),
        "fixture: cut {cut} lands inside the segment ({} bytes)",
        seg.len()
    );
    let torn = &seg[..cut];
    let db = dir.path().join("tornfirst.ulp");
    let db = db.to_str().unwrap().to_string();
    std::fs::write(&db, torn).unwrap();

    let err = ulp::repair(&db).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    assert!(err.to_string().contains("no valid segment"));
    assert_eq!(std::fs::read(&db).unwrap(), torn);

    let mut overlong = seg.clone();
    let claimed = r64(&overlong, 120);
    overlong[120..128].copy_from_slice(&(claimed + 1).to_le_bytes());
    std::fs::write(&db, &overlong).unwrap();

    let err = ulp::repair(&db).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    assert!(err.to_string().contains("no valid segment"));
    assert_eq!(std::fs::read(&db).unwrap(), overlong);
}
