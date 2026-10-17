// SPDX-License-Identifier: MIT

//! Which URLs may receive which requests (spec §7).
//!
//! Every request names its role. A URL is accepted only if it matches an
//! origin and path prefix reviewed for that role. Broad matching such as
//! `*.microsoft.com` is deliberately not used.
//!
//! Observed 2026-10-17 (reference app feeds, owner's tenant): discovery,
//! feeds, `.rdp` files and icons all live on `rdweb.wvd.microsoft.com`
//! under `/api/arm/feeddiscovery`. Regional `rdweb-g-*` hosts appear only
//! for diagnostics and hub discovery, which the app does not call.

use url::Url;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// Entra token endpoint (`/oauth2/v2.0/token`). No bearer token.
    Token,
    /// Feed discovery: the list of tenant feeds.
    FeedDiscovery,
    /// A tenant feed returned by discovery.
    Feed,
    /// A resource's `.rdp` connection file.
    ConnectionFile,
    /// A resource icon.
    Icon,
}

impl Role {
    /// Roles whose requests carry the AVD bearer token.
    pub fn sends_avd_token(self) -> bool {
        !matches!(self, Self::Token)
    }

    /// Largest accepted response body.
    pub fn max_body(self) -> u64 {
        match self {
            Self::Token => 64 * 1024,
            Self::FeedDiscovery | Self::Icon => 1024 * 1024,
            Self::Feed => 4 * 1024 * 1024,
            Self::ConnectionFile => 256 * 1024,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndpointError {
    NotHttps,
    /// User name or password in the URL.
    HasUserInfo,
    /// Host, port or path is not allowed for this role.
    NotAllowed,
}

#[derive(Debug, Clone)]
struct Rule {
    role: Role,
    scheme: &'static str,
    host: String,
    port: u16,
    /// The path must equal this, or start with it followed by `/`.
    path_prefix: &'static str,
}

/// Reviewed endpoint rules.
#[derive(Debug, Clone)]
pub struct EndpointPolicy {
    rules: Vec<Rule>,
}

const AVD_FEED_HOST: &str = "rdweb.wvd.microsoft.com";
const LOGIN_HOST: &str = "login.microsoftonline.com";

impl EndpointPolicy {
    /// Azure public cloud.
    pub fn azure_public() -> Self {
        Self::for_hosts("https", LOGIN_HOST, AVD_FEED_HOST, 443)
    }

    fn for_hosts(scheme: &'static str, login: &str, feed: &str, port: u16) -> Self {
        let rule = |role, host: &str, path_prefix| Rule {
            role,
            scheme,
            host: host.to_owned(),
            port,
            path_prefix,
        };
        Self {
            rules: vec![
                // Token endpoints are /{tenant}/oauth2/v2.0/token; checked in
                // `check` because the tenant segment varies.
                rule(Role::Token, login, "/"),
                rule(Role::FeedDiscovery, feed, "/api/arm/feeddiscovery"),
                rule(Role::Feed, feed, "/api/arm/feeddiscovery/tenants"),
                rule(Role::ConnectionFile, feed, "/api/arm/feeddiscovery/tenants"),
                rule(Role::Icon, feed, "/api/arm/feeddiscovery/tenants"),
            ],
        }
    }

    /// A policy for a local test server (plain HTTP on 127.0.0.1).
    #[cfg(test)]
    pub fn local_test(port: u16) -> Self {
        Self::for_hosts("http", "127.0.0.1", "127.0.0.1", port)
    }

    /// True when every rule requires HTTPS (the real policy).
    pub fn https_only(&self) -> bool {
        self.rules.iter().all(|r| r.scheme == "https")
    }

    /// The feed discovery URL.
    pub fn feed_discovery_url(&self) -> Url {
        let r = self
            .rules
            .iter()
            .find(|r| r.role == Role::FeedDiscovery)
            .expect("discovery rule");
        let mut url = Url::parse(&format!("{}://{}", r.scheme, r.host)).expect("valid origin");
        url.set_port(Some(r.port)).expect("port settable");
        url.set_path(r.path_prefix);
        url
    }

    /// Accepts `url` for `role`, or explains why not.
    pub fn check(&self, url: &Url, role: Role) -> Result<(), EndpointError> {
        if !url.username().is_empty() || url.password().is_some() {
            return Err(EndpointError::HasUserInfo);
        }
        let host = url
            .host_str()
            .ok_or(EndpointError::NotAllowed)?
            .to_ascii_lowercase();
        let port = url
            .port_or_known_default()
            .ok_or(EndpointError::NotAllowed)?;
        let path = url.path();
        // `Url` already resolves `.` and `..`; reject encoded ones too.
        if path.to_ascii_lowercase().contains("%2e") || path.contains("%2f") || path.contains("%2F")
        {
            return Err(EndpointError::NotAllowed);
        }

        let rules: Vec<&Rule> = self
            .rules
            .iter()
            .filter(|r| r.role == role && r.scheme == url.scheme())
            .collect();
        if rules.is_empty() {
            return Err(if url.scheme() == "http" {
                EndpointError::NotHttps
            } else {
                EndpointError::NotAllowed
            });
        }
        let allowed = rules.iter().any(|r| {
            let prefix_ok = r.path_prefix == "/"
                || path == r.path_prefix
                || path
                    .strip_prefix(r.path_prefix)
                    .is_some_and(|rest| rest.starts_with('/'));
            host == r.host && port == r.port && prefix_ok
        });
        if allowed && (role != Role::Token || is_token_path(path)) {
            Ok(())
        } else {
            Err(EndpointError::NotAllowed)
        }
    }
}

/// `/{tenant}/oauth2/v2.0/token` with a single plain tenant segment.
fn is_token_path(path: &str) -> bool {
    let mut parts = path.split('/');
    matches!(
        (parts.next(), parts.next(), parts.next(), parts.next(), parts.next(), parts.next()),
        (Some(""), Some(tenant), Some("oauth2"), Some("v2.0"), Some("token"), None)
            if !tenant.is_empty()
                && tenant.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.'))
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(url: &str, role: Role) -> Result<(), EndpointError> {
        EndpointPolicy::azure_public().check(&Url::parse(url).unwrap(), role)
    }

    const TENANT: &str = "https://rdweb.wvd.microsoft.com/api/arm/feeddiscovery/tenants/00000000-0000-0000-0000-000000000001";

    #[test]
    fn accepts_observed_service_urls() {
        assert_eq!(
            ok(
                "https://rdweb.wvd.microsoft.com/api/arm/feeddiscovery",
                Role::FeedDiscovery
            ),
            Ok(())
        );
        assert_eq!(ok(&format!("{TENANT}/webfeed?a=b"), Role::Feed), Ok(()));
        assert_eq!(
            ok(&format!("{TENANT}/rdps/x.rdp?a=b"), Role::ConnectionFile),
            Ok(())
        );
        assert_eq!(ok(&format!("{TENANT}/pngs/x.png?a=b"), Role::Icon), Ok(()));
        assert_eq!(
            ok(
                "https://RDWEB.wvd.microsoft.com:443/api/arm/feeddiscovery",
                Role::FeedDiscovery
            ),
            Ok(())
        );
        assert_eq!(
            ok(
                "https://login.microsoftonline.com/organizations/oauth2/v2.0/token",
                Role::Token
            ),
            Ok(())
        );
    }

    #[test]
    fn discovery_url_is_the_reviewed_one() {
        assert_eq!(
            EndpointPolicy::azure_public().feed_discovery_url().as_str(),
            "https://rdweb.wvd.microsoft.com/api/arm/feeddiscovery"
        );
    }

    #[test]
    fn rejects_lookalike_and_other_hosts() {
        use EndpointError::*;
        for (url, want) in [
            (
                "http://rdweb.wvd.microsoft.com/api/arm/feeddiscovery",
                NotHttps,
            ),
            (
                "https://rdweb.wvd.microsoft.com.evil.example/api/arm/feeddiscovery",
                NotAllowed,
            ),
            (
                "https://evilrdweb.wvd.microsoft.com/api/arm/feeddiscovery",
                NotAllowed,
            ),
            (
                "https://rdweb-g-eu-r1.wvd.microsoft.com/api/arm/feeddiscovery",
                NotAllowed,
            ),
            (
                "https://www.microsoft.com/api/arm/feeddiscovery",
                NotAllowed,
            ),
            (
                "https://rdweb.wvd.microsoft.com:8443/api/arm/feeddiscovery",
                NotAllowed,
            ),
            (
                "https://user:pw@rdweb.wvd.microsoft.com/api/arm/feeddiscovery",
                HasUserInfo,
            ),
            (
                "https://rdweb.wvd.microsoft.com/api/arm/feeddiscoveryx",
                NotAllowed,
            ),
            (
                "https://rdweb.wvd.microsoft.com/api/arm/DiagnosticEvents/v1",
                NotAllowed,
            ),
        ] {
            assert_eq!(ok(url, Role::FeedDiscovery), Err(want), "{url}");
        }
    }

    #[test]
    fn roles_are_not_interchangeable() {
        assert!(
            ok(
                "https://login.microsoftonline.com/organizations/oauth2/v2.0/token",
                Role::Feed
            )
            .is_err()
        );
        assert!(ok(&format!("{TENANT}/webfeed"), Role::Token).is_err());
        assert!(
            ok(
                "https://rdweb.wvd.microsoft.com/api/arm/feeddiscovery",
                Role::Icon
            )
            .is_err()
        );
    }

    #[test]
    fn rejects_traversal_and_bad_token_paths() {
        assert!(ok(&format!("{TENANT}/%2e%2e/%2e%2e/x"), Role::Feed).is_err());
        assert!(ok(&format!("{TENANT}/a%2Fb"), Role::Feed).is_err());
        // `..` is resolved by the URL parser, leaving the allowed prefix.
        assert!(
            ok(
                "https://rdweb.wvd.microsoft.com/api/arm/feeddiscovery/tenants/../../x",
                Role::Feed
            )
            .is_err()
        );
        for path in [
            "/oauth2/v2.0/token",
            "/a/b/oauth2/v2.0/token",
            "/organizations/oauth2/v2.0/authorize",
            "/organizations/oauth2/v2.0/token/x",
        ] {
            assert!(
                ok(
                    &format!("https://login.microsoftonline.com{path}"),
                    Role::Token
                )
                .is_err(),
                "{path}"
            );
        }
    }

    #[test]
    fn only_token_role_skips_bearer() {
        assert!(!Role::Token.sends_avd_token());
        assert!(Role::Feed.sends_avd_token());
    }
}
