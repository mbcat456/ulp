#![allow(clippy::unwrap_used, clippy::expect_used)]

fn fixture(name: &str) -> String {
    format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn build_preserves_preexisting_temp_named_file() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("in.txt");
    std::fs::copy(fixture("fixture.txt"), &input).unwrap();
    let db = dir.path().join("db.ulp");
    let victim = dir.path().join("db.ulp.tmp.urlins");
    std::fs::write(&victim, b"UNRELATED").unwrap();

    ulp::build(
        &[input.to_str().unwrap().to_string()],
        db.to_str().unwrap(),
        false,
    )
    .unwrap();

    assert_eq!(std::fs::read(&victim).unwrap(), b"UNRELATED");
}

#[test]
fn append_preserves_preexisting_appendtmp_file() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("in.txt");
    std::fs::copy(fixture("fixture.txt"), &input).unwrap();
    let db = dir.path().join("db.ulp");
    ulp::build(
        &[input.to_str().unwrap().to_string()],
        db.to_str().unwrap(),
        false,
    )
    .unwrap();
    let victim = dir.path().join("db.ulp.appendtmp");
    std::fs::write(&victim, b"UNRELATED").unwrap();

    ulp::append(db.to_str().unwrap(), &[fixture("fixture2.txt")], false).unwrap();

    assert_eq!(std::fs::read(&victim).unwrap(), b"UNRELATED");
}

#[test]
fn merge_and_sortcount_preserve_preexisting_temp_files() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("db.ulp");
    ulp::build(&[fixture("fixture.txt")], db.to_str().unwrap(), false).unwrap();

    let merged = dir.path().join("merged.ulp");
    let merge_victim = dir.path().join("merged.ulp.sorted.tmp");
    std::fs::write(&merge_victim, b"UNRELATED-MERGE").unwrap();
    ulp::merge(db.to_str().unwrap(), merged.to_str().unwrap()).unwrap();
    assert_eq!(std::fs::read(&merge_victim).unwrap(), b"UNRELATED-MERGE");

    let count_victim = dir.path().join("db.ulp.sortedcount.tmp");
    std::fs::write(&count_victim, b"UNRELATED-COUNT").unwrap();
    ulp::sortcount(db.to_str().unwrap()).unwrap();
    assert_eq!(std::fs::read(&count_victim).unwrap(), b"UNRELATED-COUNT");
}

#[test]
fn guide_small_input_preserves_preexisting_split_dir() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("guide.ulp");
    let victim = dir.path().join("guide.ulp.split.tmp");
    std::fs::create_dir(&victim).unwrap();
    std::fs::write(victim.join("marker"), b"UNRELATED").unwrap();

    ulp::guide(
        &[db.to_str().unwrap().to_string(), fixture("fixture.txt")],
        false,
    )
    .unwrap();

    assert_eq!(std::fs::read(victim.join("marker")).unwrap(), b"UNRELATED");
}
