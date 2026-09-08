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
fn guide_builds_a_small_dir_like_plain_build() {
    let dir = tempfile::tempdir().unwrap();
    let listdir = dir.path().join("lists");
    std::fs::create_dir(&listdir).unwrap();
    std::fs::copy(fixture("fixture.txt"), listdir.join("a.txt")).unwrap();
    std::fs::copy(fixture("fixture2.txt"), listdir.join("b.txt")).unwrap();

    let db = dir.path().join("guide.ulp");
    let db = db.to_str().unwrap().to_string();

    ulp::guide(&[db.clone(), listdir.to_str().unwrap().to_string()], false).unwrap();

    let refdb = dir.path().join("ref.ulp");
    let refdb = refdb.to_str().unwrap().to_string();
    ulp::build(
        &[
            listdir.join("a.txt").to_str().unwrap().to_string(),
            listdir.join("b.txt").to_str().unwrap().to_string(),
        ],
        &refdb,
        false,
    )
    .unwrap();

    let mut g = Vec::new();
    ulp::dump(&db, &mut g).unwrap();
    let mut r = Vec::new();
    ulp::dump(&refdb, &mut r).unwrap();
    assert_eq!(
        sorted_lines(&g),
        sorted_lines(&r),
        "guide db drifted from a plain build of the same files"
    );
}
