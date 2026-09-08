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

fn build_db(dir: &tempfile::TempDir) -> String {
    let db = dir.path().join("db.ulp");
    let db = db.to_str().unwrap().to_string();
    ulp::build(&[fixture("fixture.txt")], &db, false).unwrap();
    db
}

fn query(db: &str, field: &str, value: &str) -> Vec<u8> {
    let mut out = Vec::new();
    ulp::query(db, field, value, &mut out).unwrap();
    out
}

fn match_url(db: &str, mode: &str, keyword: &str) -> Vec<u8> {
    let mut out = Vec::new();
    ulp::match_url(db, mode, keyword, &mut out).unwrap();
    out
}

fn match_user(db: &str, mode: &str, keyword: &str) -> Vec<u8> {
    let mut out = Vec::new();
    ulp::match_user(db, mode, keyword, &mut out).unwrap();
    out
}

#[test]
fn exact_url_hits_and_misses() {
    let dir = tempfile::tempdir().unwrap();
    let db = build_db(&dir);

    let out = query(&db, "url", "example.com");
    let got = lines(&out);
    assert_eq!(got.len(), 12);
    assert!(got.contains(&b"example.com:user1:pw1".as_slice()));
    assert!(got.contains(&b"example.com:user4:hunter2".as_slice()));
    assert!(got.contains(&b"example.com:User7:PW7".as_slice()));
    assert!(got.contains(&b"example.com|user8:pw:with:colons".as_slice()));
    assert!(got.contains(&b"example.com:dupu:duppw".as_slice()));
    assert!(got.contains(&b"example.com:alpha@qq.com:qpw1".as_slice()));
    assert!(got.contains(&b"example.com:uni:\xc3\xbcserp\xc3\xa4ss".as_slice()));

    assert!(query(&db, "url", "Example.COM").is_empty());
    assert!(query(&db, "url", "site.example.com").is_empty());
    assert!(query(&db, "url", "nonexistent.invalid").is_empty());

    let out = query(&db, "url", "site.co.uk");
    let mut got = lines(&out).to_vec();
    got.sort();
    assert_eq!(
        got,
        vec![
            b"site.co.uk:ukuser:ukpw".as_slice(),
            b"site.co.uk:user2:pw2".as_slice()
        ]
    );
    assert_eq!(
        lines(&query(&db, "url", "10.0.0.1")),
        vec![b"10.0.0.1:ipuser:ippw".as_slice()]
    );

    assert!(query(&db, "url", "münchen.example.com").is_empty());
}

#[test]
fn exact_user_and_pass() {
    let dir = tempfile::tempdir().unwrap();
    let db = build_db(&dir);

    assert_eq!(
        lines(&query(&db, "user", "alpha@qq.com")),
        vec![b"example.com:alpha@qq.com:qpw1".as_slice()]
    );
    assert!(query(&db, "user", "nobody@qq.com").is_empty());

    assert_eq!(
        lines(&query(&db, "pass", "hunter2")),
        vec![b"example.com:user4:hunter2".as_slice()]
    );

    assert_eq!(
        lines(&query(&db, "pass", "pw:with:colons")),
        vec![b"example.com|user8:pw:with:colons".as_slice()]
    );
    assert!(query(&db, "pass", "not-a-pass").is_empty());
}

#[test]
fn match_modes() {
    let dir = tempfile::tempdir().unwrap();
    let db = build_db(&dir);

    let out = match_url(&db, "contains", "roblox");
    let got = lines(&out);
    assert_eq!(got.len(), 2);
    assert!(got.contains(&b"roblox.com|pipeuser|pipepass".as_slice()));
    assert!(got.contains(&b"roblox-hacks.net|pipeuser2|pipepass2".as_slice()));

    let out = match_url(&db, "prefix", "netflix");
    let got = lines(&out);
    assert_eq!(got.len(), 2);

    let out = match_user(&db, "suffix", "@qq.com");
    let got = lines(&out);
    assert_eq!(got.len(), 3);
    assert!(got.contains(&b"example.com:alpha@qq.com:qpw1".as_slice()));
    assert!(got.contains(&b"example.com:beta@qq.com:qpw2".as_slice()));
    assert!(got.contains(&b"example.com:gamma@qq.com:qpw3".as_slice()));

    let out = match_user(&db, "prefix", "admin");
    let got = lines(&out);
    assert_eq!(got.len(), 2);
    assert!(got.contains(&b"example.com:admin@corp.com:apw1".as_slice()));
    assert!(got.contains(&b"example.com:admin2@corp.com:apw2".as_slice()));

    assert!(match_url(&db, "contains", "zzz").is_empty());
    assert!(match_user(&db, "suffix", "@qq.com.").is_empty());
}

#[test]
fn fast_path_large_posting_list() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("many.txt");
    let mut text = String::with_capacity(10_005 * 40);
    for i in 0..10_005 {
        text.push_str(&format!("d{i}.com:shared_user:shared_pass\n"));
    }
    std::fs::write(&src, text).unwrap();
    let db = dir.path().join("db.ulp");
    let db = db.to_str().unwrap().to_string();
    ulp::build(&[src.to_str().unwrap().to_string()], &db, false).unwrap();

    let user_out = query(&db, "user", "shared_user");
    let by_user = lines(&user_out);
    assert_eq!(by_user.len(), 10_005);
    assert!(by_user.contains(&b"d0.com:shared_user:shared_pass".as_slice()));
    assert!(by_user.contains(&b"d10004.com:shared_user:shared_pass".as_slice()));

    let pass_out = query(&db, "pass", "shared_pass");
    let by_pass = lines(&pass_out);
    assert_eq!(by_pass.len(), 10_005);
    assert!(by_pass.contains(&b"d0.com:shared_user:shared_pass".as_slice()));
    assert!(by_pass.contains(&b"d10004.com:shared_user:shared_pass".as_slice()));
}
