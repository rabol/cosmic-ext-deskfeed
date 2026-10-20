// SPDX-License-Identifier: MIT

//! The proxy the sign-in window uses: the app's own (spec §7), from a
//! private file rather than the environment, since its address may carry a
//! user name and password. The app writes the file (0600, in a
//! private folder) and names it in `PROXY_FILE_ENV`; the variable holds only
//! the path. Without the variable the web view uses the desktop's proxy
//! settings.

use std::io::Read as _;

use common::helper::{PROXY_FILE_ENV, ProxyFile};
const MAX_FILE: u64 = 8 * 1024;
const MAX_URI: usize = 2048;
const MAX_IGNORED: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proxy {
    pub uri: String,
    /// Hosts that bypass it (`no_proxy`).
    pub ignore_hosts: Vec<String>,
}

/// The proxy to use: `Ok(None)` without the variable. An `Err` when the
/// variable is set but the file can't be used: then signing in must not go
/// on without the proxy the app uses.
pub fn from_env() -> Result<Option<Proxy>, ()> {
    let Some(path) = std::env::var_os(PROXY_FILE_ENV) else {
        return Ok(None);
    };
    let mut text = String::new();
    std::fs::File::open(path)
        .and_then(|f| f.take(MAX_FILE).read_to_string(&mut text))
        .map_err(|_| ())?;
    parse(&text).map(Some)
}

pub fn parse(text: &str) -> Result<Proxy, ()> {
    let file: ProxyFile = serde_json::from_str(text).map_err(|_| ())?;
    let printable = |s: &str| !s.chars().any(char::is_control);
    if file.uri.is_empty() || file.uri.len() > MAX_URI || !printable(&file.uri) {
        return Err(());
    }
    let ignore_hosts = file
        .no_proxy
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|h| !h.is_empty() && printable(h))
        .take(MAX_IGNORED)
        .map(str::to_owned)
        .collect();
    Ok(Proxy {
        uri: file.uri,
        ignore_hosts,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_proxy_and_its_exceptions() {
        let p = parse(r#"{"uri":"http://anna:s3cret@proxy.example:8080","no_proxy":"localhost, .corp.example,,"}"#)
            .unwrap();
        assert_eq!(p.uri, "http://anna:s3cret@proxy.example:8080");
        assert_eq!(p.ignore_hosts, ["localhost", ".corp.example"]);
        let p = parse(r#"{"uri":"socks5://proxy.example:1080"}"#).unwrap();
        assert_eq!(p.ignore_hosts, Vec::<String>::new());
    }

    #[test]
    fn refuses_what_it_cant_use() {
        for bad in [
            "",
            "not json",
            r#"{"uri":""}"#,
            r#"{"uri":"http://a\nb"}"#,
            r#"{"no_proxy":"x"}"#,
        ] {
            assert_eq!(parse(bad), Err(()), "{bad}");
        }
        assert_eq!(
            parse(&format!(r#"{{"uri":"http://{}"}}"#, "a".repeat(3000))),
            Err(())
        );
    }

    /// The proxy file, with random and mangled input (ADR 0028).
    #[test]
    fn random_proxy_files_never_break_the_reader() {
        let sample =
            r#"{"uri":"http://anna:pw@proxy.example:3128","no_proxy":"localhost,.corp.example"}"#;
        common::testkit::random_inputs("proxy file", &[sample.as_bytes()], |input| {
            if let Ok(p) = parse(&String::from_utf8_lossy(input)) {
                assert!(!p.uri.is_empty() && p.uri.len() <= MAX_URI);
                assert!(!p.uri.chars().any(char::is_control));
                assert!(p.ignore_hosts.len() <= MAX_IGNORED);
                assert!(
                    p.ignore_hosts
                        .iter()
                        .all(|h| !h.is_empty() && !h.chars().any(char::is_control))
                );
            }
        });
    }
}
