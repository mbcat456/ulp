#[inline]
pub(crate) fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    let Some((&first, _)) = needle.split_first() else {
        return true;
    };
    let mut pos = 0usize;
    while let Some(found) = haystack[pos..].iter().position(|&b| b == first) {
        let start = pos + found;
        if haystack[start..].starts_with(needle) {
            return true;
        }
        pos = start + 1;
    }
    false
}

fn is_sld(l: &[u8]) -> bool {
    match std::str::from_utf8(l) {
        Ok(s) => matches!(
            s,
            "com"
                | "co"
                | "org"
                | "net"
                | "gov"
                | "edu"
                | "ac"
                | "mil"
                | "gob"
                | "gouv"
                | "sch"
                | "gen"
                | "nom"
                | "asso"
                | "ne"
                | "or"
                | "go"
                | "tm"
                | "fm"
                | "ag"
                | "comm"
                | "ltd"
                | "plc"
                | "ed"
                | "ad"
                | "presse"
                | "med"
        ),
        Err(_) => false,
    }
}

#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "unused in non-test builds: production callers use the buffer-reusing normalize_domain_into"
    )
)]
pub(crate) fn normalize_domain(url: &[u8]) -> Vec<u8> {
    let mut h = Vec::new();
    normalize_domain_into(url, &mut h);
    h
}

pub(crate) fn normalize_domain_into(url: &[u8], h: &mut Vec<u8>) {
    h.clear();
    h.extend(
        url.iter()
            .map(|&c| if c.is_ascii_uppercase() { c + 32 } else { c }),
    );
    if let Some(p) = h.windows(3).position(|w| w == b"://") {
        h.drain(..p + 3);
    }
    if let Some(p) = h.iter().rposition(|&c| c == b'@') {
        h.drain(..p + 1);
    }
    if let Some(p) = h
        .iter()
        .position(|&c| c == b'/' || c == b'?' || c == b'#' || c == b';')
    {
        h.truncate(p);
    }

    if h.first() == Some(&b'[')
        && let Some(close) = h.iter().position(|&b| b == b']')
        && close > 1
        && h[1..close].contains(&b':')
    {
        h.copy_within(1..close, 0);
        h.truncate(close - 1);
        return;
    }
    if let Some(p) = h.iter().position(|&c| c == b':') {
        h.truncate(p);
    }

    let lead = h.iter().take_while(|&&c| c == b'.').count();
    if lead > 0 {
        h.drain(..lead);
    }
    while h.last() == Some(&b'.') {
        h.pop();
    }
    if h.is_empty() {
        return;
    }

    let mut r = h.len();
    let mut n = 0usize;
    let mut all_digits = true;
    let mut start2 = 0usize;
    let mut start3 = 0usize;
    let mut sld: &[u8] = b"";
    let mut tld: &[u8] = b"";
    while r > 0 {
        while r > 0 && h[r - 1] == b'.' {
            r -= 1;
        }
        if r == 0 {
            break;
        }
        let end = r;
        while r > 0 && h[r - 1] != b'.' {
            r -= 1;
        }
        let label = &h[r..end];
        if !label.iter().all(|&c| c.is_ascii_digit()) {
            all_digits = false;
        }
        n += 1;
        if n == 1 {
            tld = label;
        } else if n == 2 {
            sld = label;
            start2 = r;
        } else if n == 3 {
            start3 = r;
        }
    }
    if n == 0 {
        h.clear();
        return;
    }

    if all_digits {
        return;
    }
    let is_cc = tld.len() == 2 && tld.iter().all(|c| c.is_ascii_alphabetic());
    let k = if is_cc && n >= 3 && is_sld(sld) { 3 } else { 2 };
    let k = k.min(n);
    let keep_start = match k {
        3 => start3,
        2 => start2,
        _ => 0,
    };

    h.drain(..keep_start);
    let mut w = 0usize;
    let mut written = 0usize;
    let mut r = 0usize;
    while r < h.len() {
        while r < h.len() && h[r] == b'.' {
            r += 1;
        }
        if r >= h.len() {
            break;
        }
        let start = r;
        while r < h.len() && h[r] != b'.' {
            r += 1;
        }
        if written > 0 {
            h[w] = b'.';
            w += 1;
        }
        h.copy_within(start..r, w);
        w += r - start;
        written += 1;
    }
    h.truncate(w);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_sld_known_labels() {
        for l in [b"com".as_slice(), b"co", b"org", b"asso", b"med"] {
            assert!(is_sld(l), "{l:?}");
        }
        for l in [b"example".as_slice(), b"xyz", b"c0m"] {
            assert!(!is_sld(l), "{l:?}");
        }
    }

    #[test]
    fn normalize_domain_matrix() {
        let cases: &[(&[u8], &[u8])] = &[
            (b"example.com", b"example.com"),
            (b"EXAMPLE.COM", b"example.com"),
            (b"www.example.com", b"example.com"),
            (b"https://example.com", b"example.com"),
            (
                b"https://User@Sub.Example.co.uk:8080/path?q#f",
                b"example.co.uk",
            ),
            (b"example.com:8080", b"example.com"),
            (b"example.com/path", b"example.com"),
            (b"deep.site.com.mx", b"site.com.mx"),
            (b"api.site.co.uk", b"site.co.uk"),
            (b"co.uk", b"co.uk"),
            (b"a.b.c.d", b"c.d"),
            (b"1.2.3.4", b"1.2.3.4"),
            (b"10.0.0.1:8080", b"10.0.0.1"),
            (b"https://[2001:db8::1]:8080/path", b"2001:db8::1"),
            (b"[2001:db8::1]:8080", b"2001:db8::1"),
            (b"m\xc3\xbcnchen.example.com", b"example.com"),
            (b"", b""),
            (b".", b""),
        ];
        for (url, want) in cases {
            assert_eq!(normalize_domain(url), *want, "input {url:?}");
        }
    }
}
