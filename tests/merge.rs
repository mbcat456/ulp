#![allow(clippy::unwrap_used, clippy::expect_used)]

fn fixture(name: &str) -> String {
    format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))
}

fn lines(bytes: &[u8]) -> Vec<&[u8]> {
    bytes
        .split(|&b| b == b'\n')
        .filter(|l| !l.is_empty())
        .collect()
}

fn query(db: &str, field: &str, value: &str) -> Vec<u8> {
    let mut out = Vec::new();
    ulp::query(db, field, value, &mut out).unwrap();
    out
}

const MISC_LINES: [&[u8]; 3] = [
    b"bare word line",
    b"http://example.com",
    b"bare append line",
];

#[test]
fn merge_dedups_union_and_fixes_user_dict_order() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("db.ulp");
    let db = db.to_str().unwrap().to_string();
    let merged = dir.path().join("merged.ulp");
    let merged = merged.to_str().unwrap().to_string();

    ulp::build(&[fixture("fixture.txt")], &db, false).unwrap();
    ulp::append(&db, &[fixture("fixture2.txt")], false).unwrap();
    ulp::merge(&db, &merged).unwrap();

    let mut dump = Vec::new();
    ulp::dump(&merged, &mut dump).unwrap();
    let mut ls = lines(&dump).to_vec();
    ls.sort();
    let n = ls.len();
    ls.dedup();
    assert_eq!(ls.len(), n, "merged dump contains duplicate lines");

    let mut pre = Vec::new();
    ulp::dump(&db, &mut pre).unwrap();
    let mut pre_ls = lines(&pre).to_vec();
    pre_ls.sort();
    pre_ls.dedup();
    pre_ls.retain(|l| !MISC_LINES.contains(l));
    assert_eq!(ls, pre_ls);

    assert_eq!(
        lines(&query(&merged, "user", "user042")),
        vec![b"example.org:user042:pass042".as_slice()]
    );
    assert_eq!(
        lines(&query(&merged, "user", "alpha@qq.com")),
        vec![b"example.com:alpha@qq.com:qpw1".as_slice()]
    );
    assert_eq!(
        lines(&query(&merged, "pass", "newpw1")),
        vec![b"example.com:newuser1:newpw1".as_slice()]
    );
}
