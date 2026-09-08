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

#[test]
fn append_grows_and_dedups_across_segments() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("db.ulp");
    let db = db.to_str().unwrap().to_string();

    ulp::build(&[fixture("fixture.txt")], &db, false).unwrap();
    ulp::append(&db, &[fixture("fixture2.txt")], false).unwrap();

    ulp::append(&db, &[fixture("fixture2.txt")], false).unwrap();

    let mut dump = Vec::new();
    ulp::dump(&db, &mut dump).unwrap();
    let got = lines(&dump);
    assert!(got.contains(&b"example.com:newuser1:newpw1".as_slice()));
    assert!(got.contains(&b"netflix.com:newuser2:newpw2".as_slice()));
    assert!(got.contains(&b"roblox.com|newpipe|newpipepw".as_slice()));
    assert!(got.contains(&b"example.org:user090:pass090".as_slice()));
    assert!(got.contains(&b"example.org:user080:pass080".as_slice()));

    assert_eq!(
        lines(&query(&db, "user", "newuser1")),
        vec![b"example.com:newuser1:newpw1".as_slice()]
    );

    assert_eq!(
        lines(&query(&db, "user", "user042")),
        vec![b"example.org:user042:pass042".as_slice()]
    );
}
