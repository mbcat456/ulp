#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::process::Command;

fn bin() -> &'static str {
    let bin = env!("CARGO_BIN_EXE_ulp");

    let src = format!("{}/src/main.rs", env!("CARGO_MANIFEST_DIR"));
    assert!(
        std::fs::metadata(bin).unwrap().modified().unwrap()
            >= std::fs::metadata(&src).unwrap().modified().unwrap(),
        "stale binary at {bin} (older than src/main.rs)"
    );
    bin
}

fn run(args: &[&str]) -> (bool, String, String) {
    let out = Command::new(bin()).args(args).output().unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn version_prints_bin_crate_and_format_version() {
    let (ok, stdout, stderr) = run(&["--version"]);
    assert!(ok);
    assert_eq!(stdout, "ulp 1.58.0 (format 1)\n");
    assert!(stderr.is_empty());
}

#[test]
fn version_short_flag_and_subcommand_match_long_flag() {
    let (ok, long, _) = run(&["--version"]);
    assert!(ok);
    for arg in ["-V", "version"] {
        let (ok, stdout, stderr) = run(&[arg]);
        assert!(ok, "{arg}: exit != 0");
        assert_eq!(stdout, long, "{arg} diverges from --version");
        assert!(stderr.is_empty());
    }
}

#[test]
fn help_renders_to_stdout_exit_zero() {
    for arg in ["--help", "-h", "help"] {
        let (ok, stdout, stderr) = run(&[arg]);
        assert!(ok, "{arg}: exit != 0");
        assert!(stderr.is_empty(), "{arg}: help leaked to stderr: {stderr}");
        assert!(stdout.contains("usage:"), "{arg}: missing usage line");
        for sub in [
            "build",
            "append",
            "guide",
            "query",
            "match",
            "dump",
            "info",
            "bench",
            "norm",
            "merge",
            "sortcount",
        ] {
            assert!(stdout.contains(sub), "{arg}: help omits subcommand {sub}");
        }
        assert!(stdout.contains("--version"), "{arg}: help omits --version");
        assert!(stdout.contains("--json"), "{arg}: help omits info --json");
    }
}

#[test]
fn bin_info_json_flag_prints_machine_output() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("db.ulp");
    let db = db.to_str().unwrap();
    ulp::build(
        &[format!(
            "{}/tests/fixtures/fixture.txt",
            env!("CARGO_MANIFEST_DIR")
        )],
        db,
        false,
    )
    .unwrap();

    for args in [
        vec!["info", db, "--json"],
        vec!["info", "--json", db],
        vec!["info", db, "junk", "--json"],
    ] {
        let out = Command::new(bin()).args(&args).output().unwrap();
        assert!(out.status.success(), "args {args:?}");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.starts_with(r#"{"file":""#),
            "args {args:?} did not emit JSON: {stdout}"
        );
        assert!(stdout.contains(r#""format_version":1"#), "args {args:?}");
    }
}

#[test]
fn info_flag_only_opens_file_literally_named_dash_dash_json() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("--json");
    let db = db.to_str().unwrap();
    ulp::build(
        &[format!(
            "{}/tests/fixtures/fixture.txt",
            env!("CARGO_MANIFEST_DIR")
        )],
        db,
        false,
    )
    .unwrap();

    let out = Command::new(bin())
        .current_dir(dir.path())
        .args(["info", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("TOTAL records"),
        "human info expected: {stdout}"
    );
    assert!(
        !stdout.starts_with(r#"{"file":"#),
        "unexpected JSON: {stdout}"
    );
}

#[test]
fn info_flag_only_with_no_such_file_is_clean_error() {
    let dir = tempfile::tempdir().unwrap();
    let out = Command::new(bin())
        .current_dir(dir.path())
        .args(["info", "--json"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("ulp: info:"),
        "expected clean error, got: {stderr}"
    );
}
