#![allow(clippy::unwrap_used, clippy::expect_used)]

fn fixture(name: &str) -> String {
    format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))
}

fn sorted_lines(bytes: &[u8]) -> Vec<Vec<u8>> {
    let mut v: Vec<Vec<u8>> = bytes
        .split(|&b| b == b'\n')
        .filter(|l| !l.is_empty())
        .map(<[u8]>::to_vec)
        .collect();
    v.sort();
    v
}

#[test]
fn build_dump_equals_norm() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("db.ulp");
    let db = db.to_str().unwrap();

    ulp::build(&[fixture("fixture.txt")], db, false).unwrap();

    let mut got = Vec::new();
    ulp::dump(db, &mut got).unwrap();

    let mut reference = Vec::new();
    ulp::norm_into(&[fixture("fixture.txt")], &mut reference).unwrap();

    assert!(!got.is_empty() && !reference.is_empty(), "empty sink(s)");
    assert_eq!(
        sorted_lines(&got),
        sorted_lines(&reference),
        "dump(build(fixture)) drifted from norm(fixture)"
    );
}
