// SPDX-License-Identifier: MIT

//! The one HTTP client for discovery, feeds, connection files, icons and
//! token requests (spec §7, ADR 0008).
//!
//! - Every URL is checked against the [`EndpointPolicy`] for its role before
//!   any connection is made.
//! - Redirects are never followed; a 3xx response is an error, so a bearer
//!   token can never travel to another origin.
//! - Timeouts and (decompressed) body sizes are bounded.
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
}

impl fmt::Debug for BearerToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("BearerToken(<redacted>)")
    }
}

#[derive(Debug)]
#[allow(dead_code)] // Status and type are for callers that need them (icons).
pub struct Response {
    pub status: u16,
    pub content_type: Option<String>,
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
            .timeout_connect(Some(CONNECT_TIMEOUT))
            .timeout_global(Some(TOTAL_TIMEOUT))
            .user_agent(concat!("cosmic-ext-avd/", env!("CARGO_PKG_VERSION")))
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
        self.policy.check(url, role)?;
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

fn finish(mut resp: ureq::http::Response<ureq::Body>, role: Role) -> Result<Response, HttpError> {
    let status = resp.status().as_u16();
    if (300..400).contains(&status) {
        return Err(HttpError::Redirected { status });
    }
    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let body = resp
        .body_mut()
        .with_config()
        .limit(role.max_body())
        .read_to_vec()?;
    if status >= 400 {
        return Err(HttpError::Status { status, body });
    }
    Ok(Response {
        status,
        content_type,
        body,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::sync::Mutex;
    use std::thread;

    /// A one-thread HTTP server that records every request it receives and
    /// answers each with the next canned response.
    struct Server {
        port: u16,
        requests: Arc<Mutex<Vec<String>>>,
    }

    impl Server {
        fn start(responses: Vec<String>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            let requests = Arc::new(Mutex::new(Vec::new()));
            let seen = requests.clone();
            thread::spawn(move || {
                for (stream, response) in listener.incoming().zip(responses) {
                    let mut stream = stream.unwrap();
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut head = String::new();
                    let mut len = 0;
                    loop {
                        let mut line = String::new();
                        if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                            break;
                        }
                        if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                            len = v.trim().parse().unwrap();
                        }
                        head.push_str(&line);
                    }
                    let mut body = vec![0; len];
                    reader.read_exact(&mut body).unwrap();
                    head.push_str(&String::from_utf8_lossy(&body));
                    seen.lock().unwrap().push(head);
                    stream.write_all(response.as_bytes()).unwrap();
                }
            });
            Self { port, requests }
        }

        fn requests(&self) -> Vec<String> {
            self.requests.lock().unwrap().clone()
        }

        fn url(&self, path: &str) -> Url {
            Url::parse(&format!("http://127.0.0.1:{}{path}", self.port)).unwrap()
        }
    }

    fn reply(status: &str, extra: &str, body: &str) -> String {
        format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n{extra}\r\n{body}",
            body.len()
        )
    }

    const FEED: &str = "/api/arm/feeddiscovery/tenants/t1/webfeed";

    fn token() -> BearerToken {
        BearerToken::new("secret-token".into())
    }

    #[test]
    fn feed_request_carries_token_and_client_header() {
        let server = Server::start(vec![reply("200 OK", "Content-Type: text/xml\r\n", "<x/>")]);
        let http = Http::new(EndpointPolicy::local_test(server.port));
        let resp = http
            .get(&server.url(FEED), Role::Feed, Some(&token()))
            .unwrap();
        assert_eq!(resp.body, b"<x/>");
        assert_eq!(resp.content_type.as_deref(), Some("text/xml"));
        let reqs = server.requests();
        assert_eq!(reqs.len(), 1);
        let req = reqs[0].to_ascii_lowercase();
        assert!(req.contains("authorization: bearer secret-token"));
        assert!(req.contains(&format!(
            "x-ms-user-agent: {}",
            X_MS_USER_AGENT.to_ascii_lowercase()
        )));
    }

    #[test]
    fn redirect_is_not_followed_and_token_does_not_leak() {
        let other = Server::start(vec![reply("200 OK", "", "stolen")]);
        let target = other.url(FEED);
        let server = Server::start(vec![reply(
            "302 Found",
            &format!("Location: {target}\r\n"),
            "",
        )]);
        let http = Http::new(EndpointPolicy::local_test(server.port));
        let err = http
            .get(&server.url(FEED), Role::Feed, Some(&token()))
            .unwrap_err();
        assert!(
            matches!(err, HttpError::Redirected { status: 302 }),
            "{err:?}"
        );
        assert_eq!(server.requests().len(), 1);
        thread::sleep(Duration::from_millis(100));
        assert!(other.requests().is_empty(), "redirect target was contacted");
    }

    #[test]
    fn disallowed_url_makes_no_request() {
        let server = Server::start(vec![reply("200 OK", "", "")]);
        let http = Http::new(EndpointPolicy::local_test(server.port));
        for path in ["/somewhere/else", "/api/arm/feeddiscoveryx"] {
            let err = http
                .get(&server.url(path), Role::Feed, Some(&token()))
                .unwrap_err();
            assert!(
                matches!(err, HttpError::Endpoint(EndpointError::NotAllowed)),
                "{err:?}"
            );
        }
        // Same host and path, but another port than the policy allows.
        let other_port =
            Url::parse(&format!("http://127.0.0.1:{}{FEED}", server.port + 1)).unwrap();
        assert!(http.get(&other_port, Role::Feed, Some(&token())).is_err());
        thread::sleep(Duration::from_millis(100));
        assert_eq!(server.requests(), Vec::<String>::new());
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

    #[test]
    fn oversized_body_is_rejected() {
        let big = "x".repeat(usize::try_from(Role::ConnectionFile.max_body()).unwrap() + 1);
        let server = Server::start(vec![reply("200 OK", "", &big)]);
        let http = Http::new(EndpointPolicy::local_test(server.port));
        let url = server.url("/api/arm/feeddiscovery/tenants/t1/rdps/r.rdp");
        let err = http
            .get(&url, Role::ConnectionFile, Some(&token()))
            .unwrap_err();
        assert!(matches!(err, HttpError::TooLarge), "{err:?}");
    }

    #[test]
    fn error_status_keeps_body_for_parsing() {
        let server = Server::start(vec![reply(
            "400 Bad Request",
            "",
            "{\"error\":\"invalid_grant\"}",
        )]);
        let http = Http::new(EndpointPolicy::local_test(server.port));
        let url = server.url("/organizations/oauth2/v2.0/token");
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
        let server = Server::start(vec![reply("200 OK", "", "{}")]);
        let http = Http::new(EndpointPolicy::local_test(server.port));
        let url = server.url("/organizations/oauth2/v2.0/token");
        http.post_form(
            &url,
            Role::Token,
            &[("grant_type", "authorization_code"), ("code", "c")],
        )
        .unwrap();
        let req = server.requests()[0].to_ascii_lowercase();
        assert!(!req.contains("authorization:"));
        assert!(!req.contains("x-ms-user-agent"));
        assert!(req.contains("grant_type=authorization_code&code=c"));
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
