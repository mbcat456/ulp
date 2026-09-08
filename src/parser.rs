#[inline]
fn scheme_end(line: &[u8]) -> usize {
    if line.len() < 3 {
        return 0;
    }
    let c0 = line[0];
    if !(c0.is_ascii_alphabetic()) {
        return 0;
    }
    let mut i = 1;
    while i < line.len() {
        let c = line[i];
        if c.is_ascii_alphanumeric() || c == b'+' || c == b'-' || c == b'.' {
            i += 1;
        } else {
            break;
        }
    }

    if i + 3 <= line.len() && line[i] == b':' && line[i + 1] == b'/' && line[i + 2] == b'/' {
        i + 3
    } else {
        0
    }
}

#[inline]
fn find_delim(line: &[u8], from: usize) -> Option<(usize, u8)> {
    let mut i = from;
    while i < line.len() {
        let c = line[i];
        if c == b':' || c == b'|' {
            return Some((i, c));
        }
        i += 1;
    }
    None
}

fn numeric_field(s: &[u8]) -> bool {
    !s.is_empty() && s.iter().all(|&c| c.is_ascii_digit())
}

fn first_cred_delim(line: &[u8], from: usize) -> Option<(usize, u8)> {
    if from > 0 && line[..from].ends_with(b"://") {
        let authority_start = from;
        let rest = &line[authority_start..];
        let authority_end = rest
            .iter()
            .position(|&c| matches!(c, b'/' | b'?' | b'#' | b';'))
            .unwrap_or(rest.len());
        let authority = &rest[..authority_end];
        let host_rel = authority
            .iter()
            .rposition(|&b| b == b'@')
            .map_or(0, |p| p + 1);
        let host = &authority[host_rel..];

        if host.starts_with(b"[") {
            let close = host.iter().position(|&b| b == b']')?;
            let after = close + 1;
            let search = if host.get(after) == Some(&b':') {
                let next = host[after + 1..]
                    .iter()
                    .position(|&b| b == b':' || b == b'|')
                    .unwrap_or(host.len() - after - 1);
                if numeric_field(&host[after + 1..after + 1 + next]) {
                    after + 1
                } else {
                    after
                }
            } else {
                after
            };
            return find_delim(line, authority_start + host_rel + search);
        }

        let mut col = 0;
        while col < host.len() {
            if host[col] == b':' {
                let next = host[col + 1..]
                    .iter()
                    .position(|&b| b == b':' || b == b'|')
                    .unwrap_or(host.len() - col - 1);
                if numeric_field(&host[col + 1..col + 1 + next]) {
                    return find_delim(line, authority_start + host_rel + col + 1);
                }
            }
            col += 1;
        }
        return find_delim(line, from);
    }

    if from == 0 && line.starts_with(b"[") {
        let close = line.iter().position(|&b| b == b']')?;
        let after = close + 1;
        let search = if line.get(after) == Some(&b':') {
            let next = line[after + 1..]
                .iter()
                .position(|&b| b == b':' || b == b'|')
                .unwrap_or(line.len() - after - 1);
            if numeric_field(&line[after + 1..after + 1 + next]) {
                after + 1
            } else {
                after
            }
        } else {
            after
        };
        return find_delim(line, search);
    }

    find_delim(line, from)
}

#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "unused in non-test builds since scan.rs was dropped; kept for the parse-line matrix unit tests"
    )
)]
pub fn has_scheme(line: &[u8]) -> bool {
    scheme_end(line) > 0
}

#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "unused in non-test builds: production callers use the buffer-reusing strip_inline_ad_into"
    )
)]
fn strip_inline_ad(line: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    strip_inline_ad_into(line, &mut out).map(|()| out)
}

fn strip_inline_ad_into(line: &[u8], out: &mut Vec<u8>) -> Option<()> {
    let colon = line.iter().position(|&b| b == b':')?;
    let scheme = &line[..colon];
    if !(scheme.eq_ignore_ascii_case(b"https") || scheme.eq_ignore_ascii_case(b"http")) {
        return None;
    }
    let rest = &line[colon + 1..];
    let path_at = rest.windows(2).position(|w| w == b"//").or_else(|| {
        rest.iter()
            .enumerate()
            .filter(|&(i, &b)| i > 0 && b == b'/' && rest[i - 1].is_ascii_whitespace())
            .map(|(i, _)| i)
            .next_back()
    })?;

    let seg_start = usize::from(rest.first() == Some(&b'/'));
    if path_at < seg_start {
        return None;
    }
    let junk = &rest[seg_start..path_at];
    if junk.is_empty()
        || junk.contains(&b'/')
        || !junk.contains(&b'@')
        || !junk.iter().any(u8::is_ascii_whitespace)
    {
        return None;
    }
    out.clear();
    out.extend_from_slice(scheme);
    out.push(b':');
    if seg_start == 1 {
        out.push(b'/');
    }
    out.extend_from_slice(&rest[path_at..]);
    Some(())
}

#[expect(
    clippy::type_complexity,
    reason = "lossless parse result tuple; every caller destructures it"
)]
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "unused in non-test builds: production callers use the buffer-reusing parse_line_into"
    )
)]
pub fn parse_line(line: &[u8]) -> Option<(Vec<u8>, Vec<u8>, Vec<u8>, u8, u8)> {
    let (mut url, mut user, mut pass, mut scratch) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let (sep1, sep2) = parse_line_into(line, &mut url, &mut user, &mut pass, &mut scratch)?;
    Some((url, user, pass, sep1, sep2))
}

pub fn parse_line_into(
    line: &[u8],
    url: &mut Vec<u8>,
    user: &mut Vec<u8>,
    pass: &mut Vec<u8>,
    scratch: &mut Vec<u8>,
) -> Option<(u8, u8)> {
    let line: &[u8] = match strip_inline_ad_into(line, scratch) {
        Some(()) => scratch,
        None => line,
    };
    let start = scheme_end(line);
    let (d1, sep1) = first_cred_delim(line, start)?;

    let url_field = &line[..d1];
    if !url_field.windows(3).any(|w| w == b"://") && url_field.contains(&b'@') {
        let after_at = url_field.rsplit(|&b| b == b'@').next().unwrap_or(b"");
        if after_at.contains(&b'.') {
            return None;
        }
    }
    let (d2, sep2) = find_delim(line, d1 + 1)?;
    url.clear();
    url.extend_from_slice(url_field);
    user.clear();
    user.extend_from_slice(&line[d1 + 1..d2]);
    pass.clear();
    pass.extend_from_slice(&line[d2 + 1..]);
    Some((sep1, sep2))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_line_matrix() {
        #[expect(
            clippy::type_complexity,
            reason = "parse matrix case table; the tuple is the parser output shape"
        )]
        let cases: &[(&[u8], Option<(&[u8], &[u8], &[u8], u8, u8)>)] = &[
            (
                b"a.com:user:pass",
                Some((b"a.com", b"user", b"pass", b':', b':')),
            ),
            (
                b"a.com|user|pass",
                Some((b"a.com", b"user", b"pass", b'|', b'|')),
            ),
            (
                b"a.com|user:pass",
                Some((b"a.com", b"user", b"pass", b'|', b':')),
            ),
            (
                b"a.com:user|pass",
                Some((b"a.com", b"user", b"pass", b':', b'|')),
            ),
            (
                b"https://a.com/login:user:pass",
                Some((b"https://a.com/login", b"user", b"pass", b':', b':')),
            ),
            (
                b"https://a.com/login|user|pass",
                Some((b"https://a.com/login", b"user", b"pass", b'|', b'|')),
            ),
            (
                b"a.com:u:p:with:colons",
                Some((b"a.com", b"u", b"p:with:colons", b':', b':')),
            ),
            (b"a.com:u:", Some((b"a.com", b"u", b"", b':', b':'))),
            (
                b"a.com:user:pass:",
                Some((b"a.com", b"user", b"pass:", b':', b':')),
            ),
            (b"a.com:u", None),
            (b"a.com", None),
            (b"nodelims", None),
            (b"", None),

            (b"http://example.com", None),


            (
                b"https: Engineer: @logsadm   //accounts.google.com:u:p",
                Some((
                    b"https://accounts.google.com",
                    b"u",
                    b"p",
                    b':',
                    b':',
                )),
            ),
            (
                b"https:/ \xF0\x9F\x91\xBF@russia_logs\xF0\x9F\x91\xBF   /sso.det.nsw.edu.au/sso/XUI/:carlos:New",
                Some((
                    b"https://sso.det.nsw.edu.au/sso/XUI/",
                    b"carlos",
                    b"New",
                    b':',
                    b':',
                )),
            ),


            (b"user@example.com:pa:ss", None),
            (b"user@example.com:pa:ss:more", None),

            (
                b"https://example.com:8080:alice:secret",
                Some((
                    b"https://example.com:8080",
                    b"alice",
                    b"secret",
                    b':',
                    b':',
                )),
            ),
            (
                b"https://10.0.0.1:9000:bob:pw2",
                Some((
                    b"https://10.0.0.1:9000",
                    b"bob",
                    b"pw2",
                    b':',
                    b':',
                )),
            ),

            (
                b"https://[2001:db8::1]:8080:alice:secret",
                Some((
                    b"https://[2001:db8::1]:8080",
                    b"alice",
                    b"secret",
                    b':',
                    b':',
                )),
            ),
            (
                b"https://[2001:db8::1]:alice:secret",
                Some((
                    b"https://[2001:db8::1]",
                    b"alice",
                    b"secret",
                    b':',
                    b':',
                )),
            ),
            (
                b"[2001:db8::1]:alice:secret",
                Some((b"[2001:db8::1]", b"alice", b"secret", b':', b':')),
            ),

            (
                b"a.com:8080:u:p",
                Some((b"a.com", b"8080", b"u:p", b':', b':')),
            ),
        ];
        for (line, want) in cases {
            let got = parse_line(line);
            match want {
                Some((u, us, p, s1, s2)) => {
                    assert_eq!(
                        got,
                        Some((u.to_vec(), us.to_vec(), p.to_vec(), *s1, *s2)),
                        "line {line:?}"
                    );
                }
                None => assert_eq!(got, None, "line {line:?}"),
            }
        }
    }

    #[test]
    fn strip_inline_ad_keeps_real_url_shapes() {
        assert_eq!(strip_inline_ad(b"https://normal.com/:u:p"), None);
        assert_eq!(strip_inline_ad(b"http:/path"), None);
        assert_eq!(strip_inline_ad(b"https:u@h//p"), None);
        assert_eq!(strip_inline_ad(b"https:user@host.com:login:pa//ss"), None);
        assert_eq!(strip_inline_ad(b"ftp: Engineer: @x   //host"), None);
        assert_eq!(strip_inline_ad(b"a.com:u:p"), None);
    }

    #[test]
    fn first_cred_delim_skips_numeric_port() {
        let line = b"https://example.com:8080:alice:secret";
        assert_eq!(first_cred_delim(line, 8), Some((24, b':')));
    }

    #[test]
    fn parse_line_keeps_single_label_userinfo_urls_indexed() {
        assert_eq!(
            parse_line(b"foo@bar:u:p"),
            Some((
                b"foo@bar".to_vec(),
                b"u".to_vec(),
                b"p".to_vec(),
                b':',
                b':'
            ))
        );
    }

    #[test]
    fn has_scheme_detects_prefixes() {
        assert!(has_scheme(b"https://a.com"));
        assert!(has_scheme(b"http://x:y:z"));
        assert!(has_scheme(b"androidapp://x"));
        assert!(!has_scheme(b"a.com:u:p"));
        assert!(!has_scheme(b"://a.com"));
        assert!(!has_scheme(b""));
    }
}
