#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::process::Command;

fn fixture(name: &str) -> String {
    format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))
}

fn lines(bytes: &[u8]) -> Vec<&[u8]> {
    bytes
        .split(|&b| b == b'\n')
        .filter(|l| !l.is_empty())
        .collect()
}

fn decode_frames(bytes: &[u8]) -> Vec<(Vec<u8>, Vec<u8>, Vec<u8>)> {
    let mut v = Vec::new();
    let mut pos = 0usize;
    let field = |p: &mut usize| -> Vec<u8> {
        let len = u32::from_le_bytes(bytes[*p..*p + 4].try_into().unwrap()) as usize;
        *p += 4;
        let f = bytes[*p..*p + len].to_vec();
        *p += len;
        f
    };
    while pos < bytes.len() {
        let url = field(&mut pos);
        let user = field(&mut pos);
        let pass = field(&mut pos);
        v.push((url, user, pass));
    }
    v
}

fn dump_frames(db: &str) -> Vec<u8> {
    let mut out = Vec::new();
    ulp::dump_frames(db, &mut out).unwrap();
    out
}

fn match_frames(db: &str, keyword: &str) -> Vec<u8> {
    let mut out = Vec::new();
    ulp::match_url_frames(db, "contains", keyword, &mut out).unwrap();
    out
}

#[test]
fn frames_roundtrip_delimiter_bearing_fields_and_empty_url() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("db.ulp");
    let db = db.to_str().unwrap().to_string();

    ulp::build_typed(
        [
            (&b"a.com"[..], &b"us:er"[..], &b"pw:with:colons"[..]),
            (&b"b.com"[..], &b"u|s"[..], &b"pw|rd"[..]),
            (&b""[..], &b"eu"[..], &b"ep"[..]),
        ],
        &db,
    )
    .unwrap();

    let got = decode_frames(&dump_frames(&db));
    assert_eq!(
        got,
        vec![
            (b"".to_vec(), b"eu".to_vec(), b"ep".to_vec()),
            (
                b"a.com".to_vec(),
                b"us:er".to_vec(),
                b"pw:with:colons".to_vec()
            ),
            (b"b.com".to_vec(), b"u|s".to_vec(), b"pw|rd".to_vec()),
        ],
        "frames must round-trip exact field bytes in store order (empty url sorts first)"
    );
}

#[test]
fn frames_format_is_exact_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("db.ulp");
    let db = db.to_str().unwrap().to_string();
    ulp::build_typed([(&b""[..], &b"u"[..], &b"p"[..])], &db).unwrap();

    let mut want = Vec::new();
    want.extend_from_slice(&0u32.to_le_bytes());
    want.extend_from_slice(&1u32.to_le_bytes());
    want.push(b'u');
    want.extend_from_slice(&1u32.to_le_bytes());
    want.push(b'p');
    assert_eq!(dump_frames(&db), want);
}

#[test]
fn match_frames_emit_same_records_as_text_match() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("db.ulp");
    let db = db.to_str().unwrap().to_string();

    ulp::build_typed(
        [
            (&b"a.com"[..], &b"u1"[..], &b"p1"[..]),
            (&b"b.com"[..], &b"u2"[..], &b"p2"[..]),
        ],
        &db,
    )
    .unwrap();

    ulp::append_typed(
        &db,
        [
            (&b"a.com"[..], &b"u1"[..], &b"p1"[..]),
            (&b"ab.com"[..], &b"u3"[..], &b"p3"[..]),
        ],
    )
    .unwrap();

    let mut text = Vec::new();
    ulp::match_url(&db, "contains", "a", &mut text).unwrap();
    let text = lines(&text);
    assert_eq!(
        text,
        vec![b"a.com:u1:p1".as_slice(), b"ab.com:u3:p3".as_slice()],
        "text match dedups the duplicate across segments"
    );

    let frames = decode_frames(&match_frames(&db, "a"));
    assert_eq!(frames.len(), text.len(), "frames must dedup like text");
    for (frame, line) in frames.iter().zip(text.iter()) {
        let joined = format!(
            "{}:{}:{}",
            String::from_utf8_lossy(&frame.0),
            String::from_utf8_lossy(&frame.1),
            String::from_utf8_lossy(&frame.2)
        );
        assert_eq!(
            joined.as_bytes(),
            *line,
            "frame order must match text stream order"
        );
    }

    let mut text = Vec::new();
    ulp::match_url(&db, "contains", "zzz", &mut text).unwrap();
    assert!(text.is_empty());
    assert!(match_frames(&db, "zzz").is_empty());
}

fn bin() -> &'static str {
    let bin = env!("CARGO_BIN_EXE_ulp");

    let src = format!("{}/src/main.rs", env!("CARGO_MANIFEST_DIR"));
    assert!(
        std::fs::metadata(bin).unwrap().modified().unwrap()
            >= std::fs::metadata(&src).unwrap().modified().unwrap(),
        "stale binary at {bin} (older than src/main.rs)"
    );
    bin
}

fn run(args: &[&str]) -> (bool, Vec<u8>, String) {
    let out = Command::new(bin()).args(args).output().unwrap();
    (
        out.status.success(),
        out.stdout,
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn crafted_huge_misc_count_never_panics_and_warns_with_the_count() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("db.ulp");
    let db = db.to_str().unwrap().to_string();
    ulp::build(&[fixture("fixture.txt")], &db, false).unwrap();
    ulp::append(&db, &[fixture("fixture.txt")], false).unwrap();

    let mut bytes = std::fs::read(&db).unwrap();
    let footer_off = u64::from_le_bytes(bytes[bytes.len() - 8..].try_into().unwrap()) as usize;
    let seg_count =
        u64::from_le_bytes(bytes[footer_off..footer_off + 8].try_into().unwrap()) as usize;
    for i in 0..seg_count {
        let start = u64::from_le_bytes(
            bytes[footer_off + 8 + i * 8..footer_off + 16 + i * 8]
                .try_into()
                .unwrap(),
        ) as usize;
        bytes[start + 48..start + 56].copy_from_slice(&u64::MAX.to_le_bytes());
    }
    std::fs::write(&db, &bytes).unwrap();

    let (ok, _out, stderr) = run(&["dump", &db, "--frames"]);
    assert!(
        ok,
        "dump --frames over a crafted huge misc_count must not fail: {stderr}"
    );
    assert!(
        stderr.contains("frames dump skips 18446744073709551615 misc rows"),
        "warning must name the clamped count: {stderr}"
    );
}

#[test]
fn bin_frames_flag_matches_lib_and_leaves_text_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("db.ulp");
    let db = db.to_str().unwrap().to_string();
    ulp::build(&[fixture("fixture.txt")], &db, false).unwrap();

    ulp::append(&db, &[fixture("fixture.txt")], false).unwrap();

    let mut want = Vec::new();
    ulp::dump(&db, &mut want).unwrap();
    let (ok, out, stderr) = run(&["dump", &db]);
    assert!(ok, "dump: {stderr}");
    assert_eq!(out, want, "bin dump without --frames drifted from lib dump");

    let mut want = Vec::new();
    ulp::match_url(&db, "contains", "example", &mut want).unwrap();
    let (ok, out, stderr) = run(&["match", &db, "contains", "example"]);
    assert!(ok, "match: {stderr}");
    assert_eq!(
        out, want,
        "bin match without --frames drifted from lib match"
    );

    let mut want = Vec::new();
    ulp::dump_frames(&db, &mut want).unwrap();
    let (ok, out, stderr) = run(&["dump", &db, "--frames"]);
    assert!(ok, "dump --frames: {stderr}");
    assert_eq!(out, want);
    assert_eq!(
        stderr.matches("frames dump skips 6 misc rows").count(),
        1,
        "exactly one warning line: {stderr}"
    );

    let typed_db = dir.path().join("typed.ulp");
    let typed_db = typed_db.to_str().unwrap().to_string();
    ulp::build_typed([(&b"a.com"[..], &b"u"[..], &b"p"[..])], &typed_db).unwrap();
    let (ok, _out, stderr) = run(&["dump", &typed_db, "--frames"]);
    assert!(ok, "typed dump --frames: {stderr}");
    assert!(
        !stderr.contains("misc rows"),
        "a store without misc rows must not warn: {stderr}"
    );

    let mut want = Vec::new();
    ulp::match_url_frames(&db, "contains", "example", &mut want).unwrap();
    let (ok, out, stderr) = run(&["match", &db, "contains", "example", "--frames"]);
    assert!(ok, "match --frames: {stderr}");
    assert_eq!(out, want);
}
