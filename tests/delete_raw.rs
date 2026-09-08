#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;

fn fixture(name: &str) -> String {
    format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))
}

fn copy_fixture(dir: &tempfile::TempDir, name: &str) -> String {
    let dst = dir.path().join(name);
    std::fs::copy(fixture(name), &dst).unwrap();
    dst.to_str().unwrap().to_string()
}

#[test]
fn build_flag_removes_inputs_and_keeps_db() {
    let dir = tempfile::tempdir().unwrap();
    let in1 = copy_fixture(&dir, "fixture.txt");
    let in2 = copy_fixture(&dir, "fixture2.txt");
    let db = dir.path().join("db.ulp");
    let db = db.to_str().unwrap().to_string();

    ulp::build(&[in1.clone(), in2.clone()], &db, true).unwrap();

    assert!(!Path::new(&in1).exists(), "input not deleted");
    assert!(!Path::new(&in2).exists(), "input not deleted");
    assert!(Path::new(&db).exists(), "db deleted");
    let mut dump = Vec::new();
    ulp::dump(&db, &mut dump).unwrap();
    assert!(!dump.is_empty());
}

#[test]
fn failed_build_leaves_inputs_intact() {
    let dir = tempfile::tempdir().unwrap();
    let input = copy_fixture(&dir, "fixture.txt");

    let db = dir.path().join("missing").join("db.ulp");
    let db = db.to_str().unwrap().to_string();

    assert!(ulp::build(std::slice::from_ref(&input), &db, true).is_err());
    assert!(
        Path::new(&input).exists(),
        "input deleted despite a failed build"
    );
    assert!(!Path::new(&db).exists());
}

#[test]
fn append_flag_removes_only_the_new_inputs() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("db.ulp");
    let db = db.to_str().unwrap().to_string();
    ulp::build(&[fixture("fixture.txt")], &db, false).unwrap();

    let extra = copy_fixture(&dir, "fixture2.txt");
    ulp::append(&db, std::slice::from_ref(&extra), true).unwrap();

    assert!(!Path::new(&extra).exists(), "append input not deleted");
    assert!(Path::new(&db).exists(), "db deleted");

    let mut out = Vec::new();
    ulp::query(&db, "user", "newuser1", &mut out).unwrap();
    assert!(!out.is_empty());
}

#[test]
fn bin_never_deletes_aliased_output() {
    let dir = tempfile::tempdir().unwrap();
    let raw = dir.path().join("raw.txt");
    std::fs::copy(fixture("fixture.txt"), &raw).unwrap();

    let bin = env!("CARGO_BIN_EXE_ulp");

    let src = format!("{}/src/build.rs", env!("CARGO_MANIFEST_DIR"));
    assert!(
        std::fs::metadata(bin).unwrap().modified().unwrap()
            >= std::fs::metadata(&src).unwrap().modified().unwrap(),
        "stale binary at {bin} (older than src/build.rs)"
    );

    let status = std::process::Command::new(bin)
        .current_dir(dir.path())
        .args(["build", "db.ulp", "raw.txt"])
        .status()
        .unwrap();
    assert!(status.success());

    let status = std::process::Command::new(bin)
        .current_dir(dir.path())
        .args(["build", "./db.ulp", "db.ulp", "raw.txt", "--delete-raw"])
        .status()
        .unwrap();
    assert!(status.success());
    assert!(dir.path().join("db.ulp").exists(), "aliased output deleted");
    assert!(!raw.exists(), "raw input not deleted");
}

#[test]
fn guide_flag_removes_inputs_and_leaves_no_split_dir() {
    let dir = tempfile::tempdir().unwrap();
    let listdir = dir.path().join("lists");
    std::fs::create_dir(&listdir).unwrap();
    let a = listdir.join("a.txt");
    let b = listdir.join("b.txt");
    std::fs::copy(fixture("fixture.txt"), &a).unwrap();
    std::fs::copy(fixture("fixture2.txt"), &b).unwrap();
    let db = dir.path().join("guide.ulp");
    let db = db.to_str().unwrap().to_string();

    ulp::guide(&[db.clone(), listdir.to_str().unwrap().to_string()], true).unwrap();

    assert!(!a.exists() && !b.exists(), "guide inputs not deleted");
    assert!(Path::new(&db).exists(), "db deleted");
    assert!(
        !dir.path().join("guide.ulp.split.tmp").exists(),
        "split tmp dir left behind"
    );

    assert!(listdir.exists());
}
