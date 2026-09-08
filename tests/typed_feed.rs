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

fn dump(db: &str) -> Vec<u8> {
    let mut out = Vec::new();
    ulp::dump(db, &mut out).unwrap();
    out
}

fn query(db: &str, field: &str, value: &str) -> Vec<u8> {
    let mut out = Vec::new();
    ulp::query(db, field, value, &mut out).unwrap();
    out
}

#[test]
fn delimiter_bearing_fields_roundtrip_through_query_and_dump() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("db.ulp");
    let db = db.to_str().unwrap().to_string();

    let rows: [(&[u8], &[u8], &[u8]); 4] = [
        (b"example.com", b"us:er", b"pw"),
        (b"example.com", b"u|s", b"pw"),
        (b"example.com", b"plain", b"pw:with:colons"),
        (b"example.com", b"plain2", b"pw|with|pipes"),
    ];
    ulp::build_typed(rows, &db).unwrap();

    let dumped = dump(&db);
    let got = lines(&dumped);
    assert!(got.contains(&b"example.com:us:er:pw".as_slice()));
    assert!(got.contains(&b"example.com:u|s:pw".as_slice()));
    assert!(got.contains(&b"example.com:plain:pw:with:colons".as_slice()));
    assert!(got.contains(&b"example.com:plain2:pw|with|pipes".as_slice()));

    assert_eq!(
        lines(&query(&db, "user", "us:er")),
        vec![b"example.com:us:er:pw".as_slice()]
    );
    assert_eq!(
        lines(&query(&db, "user", "u|s")),
        vec![b"example.com:u|s:pw".as_slice()]
    );
    assert_eq!(
        lines(&query(&db, "pass", "pw:with:colons")),
        vec![b"example.com:plain:pw:with:colons".as_slice()]
    );
    assert_eq!(
        lines(&query(&db, "pass", "pw|with|pipes")),
        vec![b"example.com:plain2:pw|with|pipes".as_slice()]
    );

    assert!(query(&db, "user", "us").is_empty());
    assert!(query(&db, "user", "u").is_empty());
}

#[test]
fn non_utf8_credentials_are_searchable_through_byte_api() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("db.ulp");
    let db = db.to_str().unwrap().to_string();
    let user = [0xff, b'u', 0xfe];
    ulp::build_typed([(&b"a.com"[..], user.as_slice(), &b"p"[..])], &db).unwrap();

    let mut out = Vec::new();
    ulp::query_bytes(&db, "user", &user, &mut out).unwrap();
    assert!(out.windows(3).any(|w| w == user));

    let mut matched = Vec::new();
    ulp::match_user_bytes(&db, "prefix", &user[..2], &mut matched).unwrap();
    assert!(matched.windows(3).any(|w| w == user));
}

#[test]
fn typed_build_is_byte_identical_to_text_build() {
    let dir = tempfile::tempdir().unwrap();
    let txt = dir.path().join("in.txt");
    std::fs::write(
        &txt,
        b"a.com:us:er:pw\nhttps://www.Example.co.uk/login:u:p\n:emptyu:p3\na.com:dupe:dupepw\na.com:dupe:dupepw\n",
    )
    .unwrap();
    let dbt = dir.path().join("db-text.ulp");
    let dbt = dbt.to_str().unwrap().to_string();
    ulp::build(&[txt.to_str().unwrap().to_string()], &dbt, false).unwrap();

    let rows: [(&[u8], &[u8], &[u8]); 5] = [
        (b"a.com", b"us", b"er:pw"),
        (b"https://www.Example.co.uk/login", b"u", b"p"),
        (b"", b"emptyu", b"p3"),
        (b"a.com", b"dupe", b"dupepw"),
        (b"a.com", b"dupe", b"dupepw"),
    ];
    let dby = dir.path().join("db-typed.ulp");
    let dby = dby.to_str().unwrap().to_string();
    ulp::build_typed(rows, &dby).unwrap();

    assert_eq!(
        std::fs::read(&dbt).unwrap(),
        std::fs::read(&dby).unwrap(),
        "typed and text builds of the same records must be byte-identical"
    );
}

#[test]
fn empty_url_survives_merge_and_dumps_as_colon_user_pass() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("db.ulp");
    let db = db.to_str().unwrap().to_string();

    ulp::build_typed(
        [
            (&b""[..], &b"eu"[..], &b"ep"[..]),
            (&b"a.com"[..], &b"u"[..], &b"p"[..]),
        ],
        &db,
    )
    .unwrap();

    let merged = dir.path().join("merged.ulp");
    let merged = merged.to_str().unwrap().to_string();
    ulp::merge(&db, &merged).unwrap();

    let dumped = dump(&merged);
    let got = lines(&dumped);
    assert!(got.contains(&b":eu:ep".as_slice()));
    assert!(got.contains(&b"a.com:u:p".as_slice()));
    assert_eq!(
        lines(&query(&merged, "user", "eu")),
        vec![b":eu:ep".as_slice()]
    );
    assert_eq!(
        lines(&query(&merged, "pass", "ep")),
        vec![b":eu:ep".as_slice()]
    );
}

#[test]
fn append_typed_accumulates_across_calls() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("db.ulp");
    let db = db.to_str().unwrap().to_string();

    ulp::build_typed([(&b"a.com"[..], &b"u1"[..], &b"p1"[..])], &db).unwrap();
    ulp::append_typed(
        &db,
        [
            (&b"b.com"[..], &b"u2"[..], &b"p2"[..]),
            (&b"a.com"[..], &b"u1"[..], &b"p1"[..]),
        ],
    )
    .unwrap();
    ulp::append_typed(&db, [(&b"c.com"[..], &b"u:3"[..], &b"p3"[..])]).unwrap();

    let dumped = dump(&db);
    let got = lines(&dumped);
    assert!(got.contains(&b"a.com:u1:p1".as_slice()));
    assert!(got.contains(&b"b.com:u2:p2".as_slice()));
    assert!(got.contains(&b"c.com:u:3:p3".as_slice()));

    assert_eq!(
        lines(&query(&db, "user", "u1")),
        vec![b"a.com:u1:p1".as_slice()]
    );
    assert_eq!(
        lines(&query(&db, "user", "u:3")),
        vec![b"c.com:u:3:p3".as_slice()]
    );

    let mut info = Vec::new();
    ulp::info_json_into(&db, &mut info).unwrap();
    let info = String::from_utf8(info).unwrap();
    assert!(info.contains("\"segment_count\":3"), "{info}");
    assert!(info.contains("\"records\":4"), "{info}");
}

#[test]
fn append_typed_missing_db_fails_like_text_append() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing.ulp");
    let missing = missing.to_str().unwrap().to_string();

    let err_text = ulp::append(&missing, &[fixture("fixture.txt")], false).unwrap_err();
    let err_typed =
        ulp::append_typed(&missing, [(&b"a.com"[..], &b"u"[..], &b"p"[..])]).unwrap_err();
    assert_eq!(
        err_text.kind(),
        std::io::ErrorKind::NotFound,
        "text append on a missing db"
    );
    assert_eq!(
        err_typed.kind(),
        std::io::ErrorKind::NotFound,
        "typed append on a missing db"
    );
}

#[test]
fn empty_feed_builds_a_valid_empty_db() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("db.ulp");
    let db = db.to_str().unwrap().to_string();

    ulp::build_typed(std::iter::empty(), &db).unwrap();

    assert!(dump(&db).is_empty());
    let mut info = Vec::new();
    ulp::info_json_into(&db, &mut info).unwrap();
    let info = String::from_utf8(info).unwrap();
    assert!(info.contains("\"records\":0"), "{info}");
}

#[cfg(target_pointer_width = "64")]
#[test]
fn oversized_field_is_a_named_error_and_leaves_no_store() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("db.ulp");
    let db = db.to_str().unwrap().to_string();

    let big = vec![0u8; 1usize << 32];

    let err = ulp::build_typed([(&b"a.com"[..], big.as_slice(), &b"p"[..])], &db).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    assert!(err.to_string().contains("user field"), "{err}");
    assert!(
        !std::path::Path::new(&db).exists(),
        "a failed build must not create the store"
    );

    let existing = dir.path().join("existing.ulp");
    let existing = existing.to_str().unwrap().to_string();
    ulp::build(&[fixture("fixture.txt")], &existing, false).unwrap();
    let before = std::fs::read(&existing).unwrap();
    let err =
        ulp::append_typed(&existing, [(&b"a.com"[..], big.as_slice(), &b"p"[..])]).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    assert_eq!(
        std::fs::read(&existing).unwrap(),
        before,
        "a failed append must not touch the db"
    );
}

#[test]
fn append_typed_mirrors_text_append_on_tiny_db_files() {
    let dir = tempfile::tempdir().unwrap();
    for len in 0..8 {
        let db = dir.path().join(format!("tiny{len}.ulp"));
        let db = db.to_str().unwrap().to_string();
        let bytes = vec![b'x'; len];
        std::fs::write(&db, &bytes).unwrap();
        let r_text = ulp::append(&db, &[fixture("fixture.txt")], false);

        let db_typed = dir.path().join(format!("tiny{len}-typed.ulp"));
        let db_typed = db_typed.to_str().unwrap().to_string();
        std::fs::write(&db_typed, &bytes).unwrap();
        let r_typed = ulp::append_typed(&db_typed, [(&b"a.com"[..], &b"u"[..], &b"p"[..])]);

        if len == 0 {
            assert!(r_text.is_ok(), "text append on 0-byte db: {r_text:?}");
            assert!(r_typed.is_ok(), "typed append on 0-byte db: {r_typed:?}");
        } else {
            assert_eq!(
                r_text.unwrap_err().kind(),
                std::io::ErrorKind::InvalidData,
                "text append on {len}-byte db"
            );
            assert_eq!(
                r_typed.unwrap_err().kind(),
                std::io::ErrorKind::InvalidData,
                "typed append on {len}-byte db"
            );
            assert_eq!(std::fs::read(&db).unwrap(), bytes);
            assert_eq!(std::fs::read(&db_typed).unwrap(), bytes);
        }
    }

    let db = dir.path().join("tiny0-typed.ulp");
    let db = db.to_str().unwrap().to_string();
    let dumped = dump(&db);
    let got = lines(&dumped);
    assert_eq!(got, vec![b"a.com:u:p".as_slice()]);
}
