#![allow(clippy::unwrap_used, clippy::expect_used)]

fn fixture(name: &str) -> String {
    format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))
}

fn build_db(dir: &tempfile::TempDir) -> String {
    let db = dir.path().join("db.ulp");
    let db = db.to_str().unwrap().to_string();
    ulp::build(&[fixture("fixture.txt")], &db, false).unwrap();
    db
}

#[test]
fn info_missing_index_is_err() {
    assert!(ulp::info("does-not-exist.ulp").is_err());
}

#[test]
fn info_json_missing_index_is_err() {
    assert!(ulp::info_json("does-not-exist.ulp").is_err());
}

#[test]
fn bench_missing_index_is_err() {
    assert!(ulp::bench("does-not-exist.ulp", "url", "example.com", 1).is_err());
}

#[test]
fn bench_zero_iterations_is_err() {
    let err = ulp::bench("does-not-exist.ulp", "url", "example.com", 0).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
}

#[test]
fn norm_missing_input_is_err() {
    assert!(ulp::norm(&["does-not-exist.txt".to_string()]).is_err());
}

#[test]
fn norm_into_missing_input_is_err() {
    let mut out = Vec::new();
    assert!(ulp::norm_into(&["does-not-exist.txt".to_string()], &mut out).is_err());
}

#[test]
fn guide_missing_input_is_err() {
    let args = ["out.ulp".to_string(), "does-not-exist.txt".to_string()];
    assert!(ulp::guide(&args, false).is_err());
}

#[test]
fn truncated_ulp_returns_err_not_panic() {
    let dir = tempfile::tempdir().unwrap();
    let db = build_db(&dir);
    let bytes = std::fs::read(&db).unwrap();

    let records_off = u64::from_le_bytes(bytes[88..96].try_into().unwrap()) as usize;

    for cut in [100usize, 200, 500, bytes.len() / 2] {
        let trunc = dir.path().join(format!("trunc{cut}.ulp"));
        std::fs::write(&trunc, &bytes[..cut]).unwrap();
        let p = trunc.to_str().unwrap();

        let mut out = Vec::new();
        assert!(
            ulp::query(p, "url", "example.com", &mut out).is_err(),
            "query cut={cut}"
        );
        let mut out = Vec::new();
        assert!(ulp::dump(p, &mut out).is_err(), "dump cut={cut}");
        let merged = dir.path().join(format!("merged{cut}.ulp"));
        assert!(
            ulp::merge(p, merged.to_str().unwrap()).is_err(),
            "merge cut={cut}"
        );

        if cut < 128 {
            assert!(ulp::info(p).is_err(), "info cut={cut}");
        }

        if cut < records_off {
            assert!(
                ulp::bench(p, "url", "example.com", 1).is_err(),
                "bench cut={cut}"
            );
        }
    }
}

#[test]
fn garbage_ulp_returns_err_not_panic() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("garbage.ulp");

    let garbage = vec![b'x'; 200];
    std::fs::write(&p, &garbage).unwrap();
    let p = p.to_str().unwrap();

    let mut out = Vec::new();
    assert!(ulp::query(p, "url", "x", &mut out).is_err());
    let mut out = Vec::new();
    assert!(ulp::dump(p, &mut out).is_err());
    assert!(ulp::info(p).is_err());
    assert!(ulp::bench(p, "url", "x", 1).is_err());
    assert!(
        ulp::merge(p, dir.path().join("m.ulp").to_str().unwrap()).is_err(),
        "merge"
    );
}

#[test]
fn garbage_footer_pointer_returns_err_not_panic() {
    let dir = tempfile::tempdir().unwrap();
    let db = build_db(&dir);
    let orig = std::fs::read(&db).unwrap();
    let n = orig.len();

    let cases: [(&str, u64, Option<u64>); 2] =
        [("nearmax", u64::MAX - 3, None), ("midrange", 100, Some(0))];
    for (name, ptr, seg_count) in cases {
        let corrupt_db = dir.path().join(format!("{name}.ulp"));
        let mut bytes = orig.clone();
        bytes[n - 8..].copy_from_slice(&ptr.to_le_bytes());
        if let Some(sc) = seg_count {
            bytes[100..108].copy_from_slice(&sc.to_le_bytes());
        }
        std::fs::write(&corrupt_db, &bytes).unwrap();
        let p = corrupt_db.to_str().unwrap();

        let err = ulp::append(p, &[fixture("fixture.txt")], false).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData, "{name}");
        assert_eq!(
            std::fs::read(&corrupt_db).unwrap(),
            bytes,
            "a failed append must not touch the store, {name}"
        );
    }
}

#[test]
fn crafted_header_counts_are_clean_errors() {
    let dir = tempfile::tempdir().unwrap();
    let db = build_db(&dir);
    let mut bytes = std::fs::read(&db).unwrap();
    bytes[16..24].copy_from_slice(&u64::MAX.to_le_bytes());
    bytes[24..32].copy_from_slice(&u64::MAX.to_le_bytes());
    std::fs::write(&db, &bytes).unwrap();

    let mut out = Vec::new();
    let err = ulp::query(&db, "url", "example.com", &mut out).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    let err = ulp::dump(&db, &mut out).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
}

#[test]
fn malformed_footer_offset_table_is_an_error_not_a_partial_result() {
    let dir = tempfile::tempdir().unwrap();
    let db = build_db(&dir);
    ulp::append(&db, &[fixture("fixture2.txt")], false).unwrap();
    let mut bytes = std::fs::read(&db).unwrap();
    let n = bytes.len();
    let footer_off = u64::from_le_bytes(bytes[n - 8..].try_into().unwrap()) as usize;
    let second_off =
        u64::from_le_bytes(bytes[footer_off + 16..footer_off + 24].try_into().unwrap());
    bytes[footer_off + 8..footer_off + 16].copy_from_slice(&second_off.to_le_bytes());
    std::fs::write(&db, &bytes).unwrap();

    let mut out = Vec::new();
    let err = ulp::query(&db, "user", "user042", &mut out).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
}

struct FailingSink;

impl std::io::Write for FailingSink {
    fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
        Err(std::io::Error::other("disk full"))
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Err(std::io::Error::other("disk full"))
    }
}

#[test]
fn failing_write_sink_returns_err() {
    let dir = tempfile::tempdir().unwrap();
    let db = build_db(&dir);

    let mut sink = FailingSink;
    assert!(
        ulp::query(&db, "url", "example.com", &mut sink).is_err(),
        "query"
    );
    let mut sink = FailingSink;
    assert!(ulp::dump(&db, &mut sink).is_err(), "dump");
    let mut sink = FailingSink;
    assert!(
        ulp::match_url(&db, "contains", "example", &mut sink).is_err(),
        "match_url"
    );
    let mut sink = FailingSink;
    assert!(
        ulp::match_user(&db, "suffix", "@qq.com", &mut sink).is_err(),
        "match_user"
    );
    let mut sink = FailingSink;
    assert!(
        ulp::info_json_into(&db, &mut sink).is_err(),
        "info_json_into"
    );
}
