//! Version strings of the watcher (`0.2.0`, `v0.2.0`, `0.2.0-rc1`), compared the way a release
//! tag is: by its three numbers, nothing else. Shared by the watcher (is the release the server
//! names newer than me?) and the server (is this watcher older than the latest release?).

/// `major.minor.patch` of a version string; `None` for anything that is not shaped like one.
pub fn parse(s: &str) -> Option<[u64; 3]> {
    let core = s.trim().trim_start_matches(['v', 'V']);
    let core = core.split(['-', '+']).next()?;
    let mut parts = core.split('.');
    let mut out = [0u64; 3];
    for slot in &mut out {
        *slot = parts.next()?.parse().ok()?;
    }
    if parts.next().is_some() {
        return None;
    }
    Some(out)
}

/// Is `candidate` a later release than `current`? False when either does not parse.
pub fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse(candidate), parse(current)) {
        (Some(c), Some(now)) => c > now,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tags_and_plain_versions() {
        assert_eq!(parse("0.2.0"), Some([0, 2, 0]));
        assert_eq!(parse("v1.10.3"), Some([1, 10, 3]));
        assert_eq!(parse(" 0.2.0-rc1 "), Some([0, 2, 0]));
        assert_eq!(parse("0.2"), None);
        assert_eq!(parse("1.2.3.4"), None);
        assert_eq!(parse("abc"), None);
        assert_eq!(parse(""), None);
    }

    #[test]
    fn compares_release_numbers() {
        assert!(is_newer("0.2.0", "0.1.9"));
        assert!(is_newer("v0.10.0", "0.9.1"), "numeric, not lexical");
        assert!(!is_newer("0.2.0", "0.2.0"));
        assert!(!is_newer("0.1.0", "0.2.0"));
        assert!(!is_newer("lixo", "0.2.0") && !is_newer("0.2.0", "lixo"));
    }
}
