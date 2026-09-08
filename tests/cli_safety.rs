#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::process::Command;

fn fixture(name: &str) -> String {
    format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))
}

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_ulp")
}

#[test]
fn corrupt_db_preserves_existing_output_file() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("db.ulp");
    ulp::build(&[fixture("fixture.txt")], db.to_str().unwrap(), false).unwrap();
    let mut bytes = std::fs::read(&db).unwrap();
    bytes[16..24].copy_from_slice(&u64::MAX.to_le_bytes());
    std::fs::write(&db, &bytes).unwrap();

    let out = dir.path().join("precious.txt");
    std::fs::write(&out, b"KEEP-ME").unwrap();
    let run = Command::new(bin())
        .args(["dump", db.to_str().unwrap(), "-o", out.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!run.status.success());
    assert_eq!(std::fs::read(&out).unwrap(), b"KEEP-ME");
}

#[test]
fn successful_output_replaces_existing_file() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("db.ulp");
    ulp::build(&[fixture("fixture.txt")], db.to_str().unwrap(), false).unwrap();
    let mut want = Vec::new();
    ulp::dump(db.to_str().unwrap(), &mut want).unwrap();

    let out = dir.path().join("dump.txt");
    std::fs::write(&out, b"OLD").unwrap();
    let run = Command::new(bin())
        .args(["dump", db.to_str().unwrap(), "-o", out.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(run.status.success());
    assert_eq!(std::fs::read(&out).unwrap(), want);
}

#[test]
fn usage_errors_exit_nonzero() {
    for args in [
        vec![],
        vec!["unknown"],
        vec!["build"],
        vec!["query"],
        vec!["dump"],
        vec!["guide"],
    ] {
        let out = Command::new(bin()).args(&args).output().unwrap();
        assert_eq!(out.status.code(), Some(2), "args {args:?}");
    }
}

#[test]
fn bench_rejects_nonpositive_and_non_numeric_counts() {
    for n in ["0", "nope"] {
        let out = Command::new(bin())
            .args(["bench", &fixture("fixture.ulp"), "url", "example.com", n])
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(2), "n={n}");
    }
}
