#![allow(clippy::unwrap_used, clippy::expect_used)]

fn fixture(name: &str) -> String {
    format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn build_matches_phase0_golden() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("db.ulp");
    let db = db.to_str().unwrap();

    ulp::build(&[fixture("fixture.txt")], db, false).unwrap();

    let built = std::fs::read(db).unwrap();
    let golden = std::fs::read(fixture("fixture.ulp")).unwrap();
    assert_eq!(
        built, golden,
        "built db bytes drifted from the phase-0 golden"
    );
}

#[test]
fn build_is_deterministic() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    let dba = a.path().join("db.ulp");
    let dbb = b.path().join("db.ulp");

    ulp::build(&[fixture("fixture.txt")], dba.to_str().unwrap(), false).unwrap();
    ulp::build(&[fixture("fixture.txt")], dbb.to_str().unwrap(), false).unwrap();

    assert_eq!(
        std::fs::read(&dba).unwrap(),
        std::fs::read(&dbb).unwrap(),
        "two builds of the same fixture produced different bytes"
    );
}
