// SPDX-License-Identifier: MIT

//! The one HTTP client for discovery, feeds, connection files, icons and
//! token requests (spec §7, ADR 0008).
//!
//! - Every URL is checked against the [`EndpointPolicy`] for its role before
//!   any connection is made.
//! - Redirects are never followed; a 3xx response is an error, so a bearer
//!   token can never travel to another origin.
//! - Timeouts and body sizes are bounded; the size counts the body after
//!   decompression (`finish`).
//! - TLS uses rustls with the system trust store; there is no switch to
//!   turn verification off.
//! - Proxies come from the standard environment variables.
//!
//! Calls block; run them on `tokio::task::spawn_blocking`.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use ureq::Agent;
use ureq::tls::{RootCerts, TlsConfig, TlsProvider};
use url::Url;

use super::endpoint::{EndpointError, EndpointPolicy, Role};

/// The client string the AVD feed service requires (spec §3, §7). Owned
/// here so there is one reviewed value to update when the service changes.
pub const X_MS_USER_AGENT: &str = "com.microsoft.rdc.html/2.0.79.2 rdhtml-sdk/2.0.4";

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const TOTAL_TIMEOUT: Duration = Duration::from_secs(60);

/// An access token for the AVD service. Never printed.
#[derive(Clone)]
pub struct BearerToken(String);

impl BearerToken {
    pub fn new(token: String) -> Self {
        Self(token)
    }

    /// The token, for handing to FreeRDP over its private argument pipe.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for BearerToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("BearerToken(<redacted>)")
    }
}

#[derive(Debug)]
pub struct Response {
    pub body: Vec<u8>,
}

#[derive(Debug)]
pub enum HttpError {
    /// Refused before connecting.
    Endpoint(EndpointError),
    /// A bearer token was offered for a role that must not carry one.
    TokenNotAllowed,
    /// The server answered with a redirect; we never follow them.
    Redirected {
        status: u16,
    },
    /// A 4xx or 5xx answer. The body is kept for callers that parse
    /// structured errors (for example the token endpoint's JSON).
    Status {
        status: u16,
        body: Vec<u8>,
    },
    TooLarge,
    Timeout,
    /// DNS lookup failed (often: offline).
    HostNotFound,
    ConnectionFailed,
    Tls,
    Other(String),
}

impl fmt::Display for HttpError {
    // Short, sanitized text: no URLs, tokens or bodies.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Endpoint(e) => write!(f, "refused URL: {e:?}"),
            Self::TokenNotAllowed => write!(f, "token not allowed for this request"),
            Self::Redirected { status } => write!(f, "unexpected redirect (HTTP {status})"),
            Self::Status { status, .. } => write!(f, "HTTP {status}"),
            Self::TooLarge => write!(f, "response too large"),
            Self::Timeout => write!(f, "timed out"),
            Self::HostNotFound => write!(f, "host not found"),
            Self::ConnectionFailed => write!(f, "connection failed"),
            Self::Tls => write!(f, "TLS error"),
            Self::Other(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for HttpError {}

impl From<EndpointError> for HttpError {
    fn from(e: EndpointError) -> Self {
        Self::Endpoint(e)
    }
}

impl From<ureq::Error> for HttpError {
    fn from(e: ureq::Error) -> Self {
        match e {
            ureq::Error::Timeout(_) => Self::Timeout,
            ureq::Error::HostNotFound => Self::HostNotFound,
            ureq::Error::ConnectionFailed | ureq::Error::Io(_) => Self::ConnectionFailed,
            ureq::Error::Tls(_) | ureq::Error::Rustls(_) => Self::Tls,
            ureq::Error::BodyExceedsLimit(_) => Self::TooLarge,
            ureq::Error::RequireHttpsOnly(_) => Self::Endpoint(EndpointError::NotHttps),
            other => Self::Other(other.to_string()),
        }
    }
}

pub struct Http {
    agent: Agent,
    policy: EndpointPolicy,
}

impl Http {
    pub fn new(policy: EndpointPolicy) -> Self {
        Self::with_timeouts(policy, CONNECT_TIMEOUT, TOTAL_TIMEOUT)
    }

    /// Tests: shorter limits than the app's (a minute is too long to wait).
    #[cfg(test)]
    fn quick(policy: EndpointPolicy, total: Duration) -> Self {
        Self::with_timeouts(policy, total, total)
    }

    fn with_timeouts(policy: EndpointPolicy, connect: Duration, total: Duration) -> Self {
        let tls = TlsConfig::builder()
            .provider(TlsProvider::Rustls)
            .root_certs(RootCerts::PlatformVerifier)
            .unversioned_rustls_crypto_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .build();
        let config = Agent::config_builder()
            .tls_config(tls)
            .https_only(policy.https_only())
            .max_redirects(0)
            .max_redirects_will_error(false)
            .http_status_as_error(false)
            .timeout_connect(Some(connect))
            .timeout_global(Some(total))
            .user_agent(concat!("cosmic-ext-deskfeed/", env!("CARGO_PKG_VERSION")))
            .build();
        Self {
            agent: Agent::new_with_config(config),
            policy,
        }
    }

    pub fn policy(&self) -> &EndpointPolicy {
        &self.policy
    }

    /// GET for discovery, feeds, connection files and icons.
    pub fn get(
        &self,
        url: &Url,
        role: Role,
        token: Option<&BearerToken>,
    ) -> Result<Response, HttpError> {
        if let Err(e) = self.policy.check(url, role) {
            // Diagnostics for the endpoint rules; IDs and the query removed.
            crate::app_log!("refused {role:?} URL ({e:?}): {}", redacted(url));
            return Err(e.into());
        }
        if token.is_some() && !role.sends_avd_token() {
            return Err(HttpError::TokenNotAllowed);
        }
        let mut req = self.agent.get(url.as_str());
        if role.sends_avd_token() {
            req = req.header("X-MS-User-Agent", X_MS_USER_AGENT);
        }
        if let Some(BearerToken(t)) = token {
            req = req.header("Authorization", format!("Bearer {t}"));
        }
        finish(req.call()?, role)
    }

    /// Form POST, used for the token endpoint. Never carries a bearer token.
    pub fn post_form(
        &self,
        url: &Url,
        role: Role,
        form: &[(&str, &str)],
    ) -> Result<Response, HttpError> {
        self.policy.check(url, role)?;
        if role.sends_avd_token() {
            return Err(HttpError::TokenNotAllowed);
        }
        let resp = self
            .agent
            .post(url.as_str())
            .send_form(form.iter().copied())?;
        finish(resp, role)
    }
}

/// `scheme://host:port/path` with GUID-like and long path segments
/// replaced by `<id>`, and no query or fragment. For diagnostics only.
pub fn redacted(url: &Url) -> String {
    let path: Vec<&str> = url
        .path()
        .split('/')
        .map(|seg| {
            let hexish = seg.len() >= 16 && seg.chars().all(|c| c.is_ascii_hexdigit() || c == '-');
            if hexish || seg.len() > 40 {
                "<id>"
            } else {
                seg
            }
        })
        .collect();
    format!(
        "{}://{}:{}{}",
        url.scheme(),
        url.host_str().unwrap_or("?"),
        url.port_or_known_default().unwrap_or(0),
        path.join("/")
    )
}

fn finish(mut resp: ureq::http::Response<ureq::Body>, role: Role) -> Result<Response, HttpError> {
    let status = resp.status().as_u16();
    if (300..400).contains(&status) {
        return Err(HttpError::Redirected { status });
    }
    // The limit counts what we get after decompression. ureq's own `limit`
    // sits below its gzip decoder (ureq 3.4.2 `body/mod.rs`, `BodyReader`),
    // so it only counts the compressed bytes: a small compressed answer
    // could expand without bound (found by `compressed_answers_count_expanded`).
    let max = role.max_body();
    let mut body = Vec::new();
    std::io::Read::read_to_end(
        &mut std::io::Read::take(resp.body_mut().with_config().limit(max).reader(), max + 1),
        &mut body,
    )
    .map_err(ureq::Error::from)?;
    if body.len() as u64 > max {
        return Err(HttpError::TooLarge);
    }
    if status >= 400 {
        return Err(HttpError::Status { status, body });
    }
    Ok(Response { body })
}

#[cfg(test)]
mod redact_tests {
    use super::*;

    #[test]
    fn redacts_ids_and_query() {
        let u = Url::parse("https://rdweb.wvd.microsoft.com/api/arm/feeddiscovery/tenants/00000000-0000-0000-0000-000000000001/resource?x=secret").unwrap();
        assert_eq!(
            redacted(&u),
            "https://rdweb.wvd.microsoft.com:443/api/arm/feeddiscovery/tenants/<id>/resource"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::thread;

    /// Mocks for any GET or POST (the client's only methods), each
    /// expected never to arrive.
    fn never(server: &mut mockito::ServerGuard) -> [mockito::Mock; 2] {
        ["GET", "POST"].map(|method| {
            server
                .mock(method, mockito::Matcher::Any)
                .expect(0)
                .create()
        })
    }

    /// A mock server (mockito) and the policy for it.
    fn mock_server() -> (mockito::ServerGuard, Http, u16) {
        let server = mockito::Server::new();
        let port = crate::test_util::port_of(&server);
        (server, Http::new(EndpointPolicy::local_test(port)), port)
    }

    const FEED: &str = "/api/arm/feeddiscovery/tenants/t1/webfeed";

    fn token() -> BearerToken {
        BearerToken::new("secret-token".into())
    }

    #[test]
    fn feed_request_carries_token_and_client_header() {
        let (mut server, http, port) = mock_server();
        // Answered only with both headers; exactly one request.
        let feed = server
            .mock("GET", FEED)
            .match_header("authorization", "Bearer secret-token")
            .match_header("x-ms-user-agent", X_MS_USER_AGENT)
            .with_header("content-type", "text/xml")
            .with_body("<x/>")
            .expect(1)
            .create();
        let resp = http
            .get(&local(port, FEED), Role::Feed, Some(&token()))
            .unwrap();
        assert_eq!(resp.body, b"<x/>");
        feed.assert();
    }

    #[test]
    fn redirect_is_not_followed_and_token_does_not_leak() {
        let mut other = mockito::Server::new();
        let stolen = never(&mut other);
        let target = format!("{}{FEED}", other.url());
        let (mut server, http, port) = mock_server();
        let redirect = server
            .mock("GET", FEED)
            .with_status(302)
            .with_header("location", &target)
            .expect(1)
            .create();
        let err = http
            .get(&local(port, FEED), Role::Feed, Some(&token()))
            .unwrap_err();
        assert!(
            matches!(err, HttpError::Redirected { status: 302 }),
            "{err:?}"
        );
        redirect.assert();
        thread::sleep(Duration::from_millis(100));
        // The redirect target was never contacted.
        stolen.iter().for_each(mockito::Mock::assert);
    }

    #[test]
    fn disallowed_url_makes_no_request() {
        let (mut server, http, port) = mock_server();
        let any = never(&mut server);
        for path in ["/somewhere/else", "/api/arm/feeddiscoveryx"] {
            let err = http
                .get(&local(port, path), Role::Feed, Some(&token()))
                .unwrap_err();
            assert!(
                matches!(err, HttpError::Endpoint(EndpointError::NotAllowed)),
                "{err:?}"
            );
        }
        // Same host and path, but another port than the policy allows.
        let other_port = local(port + 1, FEED);
        assert!(http.get(&other_port, Role::Feed, Some(&token())).is_err());
        thread::sleep(Duration::from_millis(100));
        any.iter().for_each(mockito::Mock::assert);
    }

    #[test]
    fn real_policy_refuses_plain_http() {
        let http = Http::new(EndpointPolicy::azure_public());
        let url = Url::parse("http://rdweb.wvd.microsoft.com/api/arm/feeddiscovery").unwrap();
        let err = http
            .get(&url, Role::FeedDiscovery, Some(&token()))
            .unwrap_err();
        assert!(
            matches!(err, HttpError::Endpoint(EndpointError::NotHttps)),
            "{err:?}"
        );
    }

    /// One connection, handled by `act` after the request head was read:
    /// for servers that misbehave in ways `Server` can't.
    fn one_connection(act: impl FnOnce(std::net::TcpStream) + Send + 'static) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
            }
            act(stream);
        });
        port
    }

    fn local(port: u16, path: &str) -> Url {
        Url::parse(&format!("http://127.0.0.1:{port}{path}")).unwrap()
    }

    /// A small compressed answer that expands past the role's limit (a
    /// "gzip bomb"): the limit counts the expanded bytes.
    #[test]
    fn compressed_answers_count_expanded() {
        let bomb: &[u8] = include_bytes!("testdata/zeros-1mib.gz");
        let answer = move |mut s: std::net::TcpStream| {
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                bomb.len()
            );
            s.write_all(head.as_bytes()).unwrap();
            s.write_all(bomb).unwrap();
        };
        // 1 MiB expanded, over a connection file's 256 KiB.
        let port = one_connection(answer);
        let http = Http::new(EndpointPolicy::local_test(port));
        let url = local(port, "/api/arm/feeddiscovery/tenants/t1/rdps/x.rdp");
        let err = http
            .get(&url, Role::ConnectionFile, Some(&token()))
            .unwrap_err();
        assert!(matches!(err, HttpError::TooLarge), "{err:?}");
        // Under a feed's 4 MiB it expands: so it was decompressed, and the
        // limit above was on the expanded size.
        let port = one_connection(answer);
        let http = Http::new(EndpointPolicy::local_test(port));
        let url = local(port, "/api/arm/feeddiscovery/tenants/t1/webfeed");
        let body = http.get(&url, Role::Feed, Some(&token())).unwrap().body;
        assert_eq!(body.len(), 1024 * 1024);
    }

    /// A server that never answers, and one that trickles: the time limit
    /// ends both.
    #[test]
    fn slow_servers_time_out() {
        let limit = Duration::from_secs(1);
        let silent = one_connection(|_s| thread::sleep(Duration::from_secs(5)));
        let trickle = one_connection(|mut s| {
            let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1000\r\n\r\n");
            for _ in 0..20 {
                if s.write_all(b"x").is_err() {
                    return;
                }
                thread::sleep(Duration::from_millis(250));
            }
        });
        for port in [silent, trickle] {
            let http = Http::quick(EndpointPolicy::local_test(port), limit);
            let started = std::time::Instant::now();
            let err = http
                .get(
                    &local(port, "/api/arm/feeddiscovery"),
                    Role::FeedDiscovery,
                    Some(&token()),
                )
                .unwrap_err();
            assert!(matches!(err, HttpError::Timeout), "{err:?}");
            assert!(started.elapsed() < Duration::from_secs(3));
        }
    }

    /// An answer cut off before its announced length is an error, never a
    /// short body taken as complete.
    #[test]
    fn a_cut_off_answer_is_an_error() {
        let port = one_connection(|mut s| {
            let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\nonly ten b");
        });
        let http = Http::quick(EndpointPolicy::local_test(port), Duration::from_secs(2));
        let result = http.get(
            &local(port, "/api/arm/feeddiscovery"),
            Role::FeedDiscovery,
            Some(&token()),
        );
        assert!(result.is_err(), "{result:?}");
    }

    /// No kind of request follows a redirect, and its target is never
    /// contacted (`redirect_is_not_followed…` covers the
    /// feed with a token).
    #[test]
    fn no_role_follows_redirects() {
        let mut other = mockito::Server::new();
        let untouched = never(&mut other);
        let gets = [
            (Role::FeedDiscovery, "/api/arm/feeddiscovery", true),
            (
                Role::Feed,
                "/api/arm/feeddiscovery/tenants/t1/webfeed",
                true,
            ),
            (
                Role::ConnectionFile,
                "/api/arm/feeddiscovery/tenants/t1/rdps/x.rdp",
                true,
            ),
            (
                Role::Icon,
                "/api/arm/feeddiscovery/tenants/t1/pngs/x.png",
                true,
            ),
            (Role::SigningKeys, "/common/discovery/v2.0/keys", false),
        ];
        let location = format!("{}/stolen", other.url());
        for (role, path, with_token) in gets {
            let (mut server, http, port) = mock_server();
            server
                .mock("GET", path)
                .with_status(302)
                .with_header("location", &location)
                .create();
            let token = token();
            let err = http
                .get(&local(port, path), role, with_token.then_some(&token))
                .unwrap_err();
            assert!(
                matches!(err, HttpError::Redirected { status: 302 }),
                "{role:?}: {err:?}"
            );
        }
        let (mut server, http, port) = mock_server();
        server
            .mock("POST", "/organizations/oauth2/v2.0/token")
            .with_status(307)
            .with_header("location", &location)
            .create();
        let err = http
            .post_form(
                &local(port, "/organizations/oauth2/v2.0/token"),
                Role::Token,
                &[("grant_type", "refresh_token")],
            )
            .unwrap_err();
        assert!(
            matches!(err, HttpError::Redirected { status: 307 }),
            "{err:?}"
        );
        // No redirect target was contacted.
        untouched.iter().for_each(mockito::Mock::assert);
    }

    #[test]
    fn oversized_body_is_rejected() {
        let big = "x".repeat(usize::try_from(Role::ConnectionFile.max_body()).unwrap() + 1);
        let (mut server, http, port) = mock_server();
        let path = "/api/arm/feeddiscovery/tenants/t1/rdps/r.rdp";
        server.mock("GET", path).with_body(&big).create();
        let url = local(port, path);
        let err = http
            .get(&url, Role::ConnectionFile, Some(&token()))
            .unwrap_err();
        assert!(matches!(err, HttpError::TooLarge), "{err:?}");
    }

    #[test]
    fn error_status_keeps_body_for_parsing() {
        let (mut server, http, port) = mock_server();
        let path = "/organizations/oauth2/v2.0/token";
        server
            .mock("POST", path)
            .with_status(400)
            .with_body("{\"error\":\"invalid_grant\"}")
            .create();
        let url = local(port, path);
        let err = http
            .post_form(&url, Role::Token, &[("grant_type", "refresh_token")])
            .unwrap_err();
        match err {
            HttpError::Status { status: 400, body } => assert!(body.starts_with(b"{\"error\"")),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn token_request_has_no_bearer_or_feed_header() {
        let (mut server, http, port) = mock_server();
        let path = "/organizations/oauth2/v2.0/token";
        // Answered only without both headers and with exactly this body.
        let token_endpoint = server
            .mock("POST", path)
            .match_header("authorization", mockito::Matcher::Missing)
            .match_header("x-ms-user-agent", mockito::Matcher::Missing)
            .match_body("grant_type=authorization_code&code=c")
            .with_body("{}")
            .expect(1)
            .create();
        http.post_form(
            &local(port, path),
            Role::Token,
            &[("grant_type", "authorization_code"), ("code", "c")],
        )
        .unwrap();
        token_endpoint.assert();
    }

    #[test]
    fn token_cannot_be_attached_to_token_endpoint() {
        let http = Http::new(EndpointPolicy::local_test(1));
        let url = Url::parse("http://127.0.0.1:1/organizations/oauth2/v2.0/token").unwrap();
        assert!(matches!(
            http.get(&url, Role::Token, Some(&token())),
            Err(HttpError::TokenNotAllowed)
        ));
    }

    #[test]
    fn debug_and_display_hide_secrets() {
        assert!(!format!("{:?}", token()).contains("secret"));
        let e = HttpError::Status {
            status: 401,
            body: b"secret".to_vec(),
        };
        assert_eq!(e.to_string(), "HTTP 401");
    }

    /// Real TLS with the system trust store. No token is sent; the service
    /// answers 401. Run with `cargo test -- --ignored`.
    #[test]
    #[ignore = "needs the internet"]
    fn live_tls_to_feed_service_without_token() {
        let http = Http::new(EndpointPolicy::azure_public());
        let url = http.policy().feed_discovery_url();
        match http.get(&url, Role::FeedDiscovery, None) {
            Err(HttpError::Status { status, .. }) => assert!((400..500).contains(&status)),
            other => panic!("expected an HTTP 4xx, got {other:?}"),
        }
    }
}
