#![allow(clippy::unwrap_used, clippy::expect_used)]

fn fixture(name: &str) -> String {
    format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))
}

fn json_file_field(path: &str) -> String {
    let mut s = String::with_capacity(path.len());
    for c in path.chars() {
        match c {
            '\\' => {
                s.push('\\');
                s.push('\\');
            }
            '"' => {
                s.push('\\');
                s.push('"');
            }
            c => s.push(c),
        }
    }
    s
}

#[test]
fn info_json_matches_documented_schema() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("db.ulp");
    let db = db.to_str().unwrap();

    ulp::build(&[fixture("fixture.txt")], db, false).unwrap();

    let mut got = Vec::new();
    ulp::info_json_into(db, &mut got).unwrap();
    let got = String::from_utf8(got).unwrap();

    let expected = format!(
        r#"{{"file":"{}","size_bytes":3391,"format_version":1,"segment_count":1,"records":101,"urls":8,"users":100,"passes":100,"misc":3,"segments":[{{"records":101,"urls":8,"users":100,"passes":100,"misc":3}}]}}"#,
        json_file_field(db)
    ) + "\n";
    assert_eq!(got, expected);
}

#[test]
fn info_json_rejects_mixed_format_versions() {
    fn seg(version: u32) -> Vec<u8> {
        let mut h = vec![0u8; 128];
        h[..8].copy_from_slice(b"ULPFMT01");
        h[8..12].copy_from_slice(&version.to_le_bytes());
        h
    }
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("mixed.ulp");
    let mut f = seg(1);
    f.extend_from_slice(&seg(2));
    let footer_off = f.len() as u64;
    f.extend_from_slice(&2u64.to_le_bytes());
    f.extend_from_slice(&0u64.to_le_bytes());
    f.extend_from_slice(&128u64.to_le_bytes());
    f.extend_from_slice(&footer_off.to_le_bytes());
    std::fs::write(&p, &f).unwrap();

    let mut out = Vec::new();
    let err = ulp::info_json_into(p.to_str().unwrap(), &mut out).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
}

#[test]
fn info_json_handles_legacy_single_segment() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("legacy.ulp");
    let mut h = vec![0u8; 128];
    h[..8].copy_from_slice(b"ULPFMT01");
    h[8..12].copy_from_slice(&1u32.to_le_bytes());
    std::fs::write(&p, &h).unwrap();

    let mut out = Vec::new();
    ulp::info_json_into(p.to_str().unwrap(), &mut out).unwrap();
    let got = String::from_utf8(out).unwrap();
    assert!(got.contains(r#""format_version":1"#));
    assert!(got.contains(r#""segment_count":1"#));
    assert!(got.contains(r#""records":0"#));
}

#[cfg(unix)]
#[test]
fn info_json_schema_pin_survives_backslash_and_quote_in_path() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join(r#"we"ird\b.ulp"#);
    let db = db.to_str().unwrap();
    ulp::build(&[fixture("fixture.txt")], db, false).unwrap();

    let mut got = Vec::new();
    ulp::info_json_into(db, &mut got).unwrap();
    let got = String::from_utf8(got).unwrap();
    let expected = format!(
        r#"{{"file":"{}","size_bytes":3391,"format_version":1,"segment_count":1,"records":101,"urls":8,"users":100,"passes":100,"misc":3,"segments":[{{"records":101,"urls":8,"users":100,"passes":100,"misc":3}}]}}"#,
        json_file_field(db)
    ) + "\n";
    assert_eq!(got, expected);
}

#[cfg(unix)]
#[test]
fn info_json_escapes_quote_in_path() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("we\"ird.ulp");
    let db = db.to_str().unwrap();
    ulp::build(&[fixture("fixture.txt")], db, false).unwrap();

    let mut out = Vec::new();
    ulp::info_json_into(db, &mut out).unwrap();
    let got = String::from_utf8(out).unwrap();
    assert!(
        got.contains(r#"we\"ird.ulp"#),
        "quote in path unescaped: {got}"
    );
}
