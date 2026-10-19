// SPDX-License-Identifier: MIT

//! Feed discovery: list the account's tenant feeds, then fetch each feed.
//! A failing feed is reported for its workspace; the others still load
//! (spec §10). Blocking; run on `spawn_blocking`.

use super::endpoint::Role;
use super::feed::{self, FeedError, TenantFeed, Workspace};
use super::http::{BearerToken, Http, HttpError};

#[derive(Debug)]
pub struct WorkspaceFeed {
    pub tenant: TenantFeed,
    pub result: Result<Workspace, FetchError>,
}

#[derive(Debug)]
pub enum FetchError {
    Http(HttpError),
    Feed(FeedError),
    NotUtf8,
    /// No longer wanted (its account signed out): stopped before the next
    /// request.
    Cancelled,
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Http(e) => e.fmt(f),
            Self::Feed(e) => e.fmt(f),
            Self::NotUtf8 => f.write_str("feed is not UTF-8"),
            Self::Cancelled => f.write_str("cancelled"),
        }
    }
}

fn fetch_text(
    http: &Http,
    url: &url::Url,
    role: Role,
    token: &BearerToken,
) -> Result<String, FetchError> {
    let resp = http.get(url, role, Some(token)).map_err(FetchError::Http)?;
    let mut text = String::from_utf8(resp.body).map_err(|_| FetchError::NotUtf8)?;
    if text.starts_with('\u{feff}') {
        text.remove(0);
    }
    Ok(text)
}

/// Discovery and all tenant feeds. `Err` only when discovery itself fails.
/// The account's workspaces. `wanted` is asked before every request; once
/// it says no (the account signed out), no more requests are made (review
/// F08).
pub fn discover(
    http: &Http,
    token: &BearerToken,
    wanted: &dyn Fn() -> bool,
) -> Result<Vec<WorkspaceFeed>, FetchError> {
    let still = || {
        if wanted() {
            Ok(())
        } else {
            Err(FetchError::Cancelled)
        }
    };
    still()?;
    let url = http.policy().feed_discovery_url();
    let text = fetch_text(http, &url, Role::FeedDiscovery, token)?;
    let tenants = feed::parse_discovery(&text).map_err(FetchError::Feed)?;
    let mut feeds = Vec::with_capacity(tenants.len());
    for tenant in tenants {
        still()?;
        let result = fetch_text(http, &tenant.feed_url, Role::Feed, token)
            .and_then(|t| feed::parse_feed(&t).map_err(FetchError::Feed));
        feeds.push(WorkspaceFeed { tenant, result });
    }
    Ok(feeds)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::endpoint::EndpointPolicy;
    use std::collections::HashMap;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;

    /// Answers `count` requests with fixed bodies by path; 404 otherwise.
    fn serve(listener: TcpListener, routes: HashMap<String, (u16, String)>, count: usize) {
        std::thread::spawn(move || {
            for stream in listener.incoming().take(count) {
                let mut s = stream.unwrap();
                let mut r = BufReader::new(s.try_clone().unwrap());
                let mut first = String::new();
                r.read_line(&mut first).unwrap();
                loop {
                    let mut l = String::new();
                    r.read_line(&mut l).unwrap();
                    if l == "\r\n" {
                        break;
                    }
                }
                let path = first.split(' ').nth(1).unwrap().split('?').next().unwrap();
                let (status, body) = routes.get(path).cloned().unwrap_or((404, String::new()));
                write!(
                    s,
                    "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            }
        });
    }

    fn bind() -> (TcpListener, u16) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        (l, port)
    }

    #[test]
    fn stops_requesting_once_no_longer_wanted() {
        let (listener, port) = bind();
        let base = format!("http://127.0.0.1:{port}/api/arm/feeddiscovery/tenants");
        let discovery = format!(
            r#"<TenantFeedURLs xmlns="http://schemas.microsoft.com/ts/2014/03/tswfdiscovery">
                 <TenantFeedURL FeedURL="{base}/t1/webfeed" TenantId="t1"/>
               </TenantFeedURLs>"#
        );
        let routes = HashMap::from([("/api/arm/feeddiscovery".to_owned(), (200, discovery))]);
        // Answers only the discovery request: a feed request after it
        // would fail to connect, not end as Cancelled.
        serve(listener, routes, 1);
        let http = Http::new(EndpointPolicy::local_test(port));
        let asked = std::cell::Cell::new(0);
        let wanted = || {
            asked.set(asked.get() + 1);
            asked.get() == 1
        };
        let result = discover(&http, &BearerToken::new("t".into()), &wanted);
        assert!(matches!(result, Err(FetchError::Cancelled)), "{result:?}");
        assert_eq!(asked.get(), 2);
    }

    #[test]
    fn partial_failure_keeps_good_feeds() {
        let (listener, port) = bind();
        let base = format!("http://127.0.0.1:{port}/api/arm/feeddiscovery/tenants");
        let discovery = format!(
            r#"<TenantFeedURLs xmlns="http://schemas.microsoft.com/ts/2014/03/tswfdiscovery">
                 <TenantFeedURL FeedURL="{base}/t1/webfeed" TenantId="t1" WorkspaceName="Good"/>
                 <TenantFeedURL FeedURL="{base}/t2/webfeed" TenantId="t2" WorkspaceName="Broken"/>
               </TenantFeedURLs>"#
        );
        let good = r#"<ResourceCollection xmlns="http://schemas.microsoft.com/ts/2007/05/tswf">
              <Publisher Name="Good" ID="p"><Resources><Resource ID="r1" Title="Desk" Type="Desktop"/></Resources></Publisher>
            </ResourceCollection>"#;
        let routes = HashMap::from([
            (
                "/api/arm/feeddiscovery".to_owned(),
                (200, format!("\u{feff}{discovery}")),
            ),
            (
                "/api/arm/feeddiscovery/tenants/t1/webfeed".to_owned(),
                (200, good.to_owned()),
            ),
            (
                "/api/arm/feeddiscovery/tenants/t2/webfeed".to_owned(),
                (500, String::new()),
            ),
        ]);
        serve(listener, routes, 3);
        let http = Http::new(EndpointPolicy::local_test(port));
        let feeds = discover(&http, &BearerToken::new("t".into()), &|| true).unwrap();
        assert_eq!(feeds.len(), 2);
        assert_eq!(feeds[0].result.as_ref().unwrap().resources[0].title, "Desk");
        assert!(matches!(
            feeds[1].result,
            Err(FetchError::Http(HttpError::Status { status: 500, .. }))
        ));
        assert_eq!(feeds[1].tenant.workspace_name.as_deref(), Some("Broken"));
    }

    #[test]
    fn discovery_failure_is_an_error() {
        let (listener, port) = bind();
        serve(
            listener,
            HashMap::from([("/api/arm/feeddiscovery".to_owned(), (401, String::new()))]),
            1,
        );
        let http = Http::new(EndpointPolicy::local_test(port));
        let err = discover(&http, &BearerToken::new("t".into()), &|| true).unwrap_err();
        assert!(matches!(
            err,
            FetchError::Http(HttpError::Status { status: 401, .. })
        ));
    }

    #[test]
    fn feed_urls_outside_policy_are_not_fetched() {
        let (listener, port) = bind();
        let discovery = r#"<TenantFeedURLs xmlns="http://schemas.microsoft.com/ts/2014/03/tswfdiscovery">
                 <TenantFeedURL FeedURL="https://evil.example/api/arm/feeddiscovery/tenants/t1/webfeed" TenantId="t1"/>
               </TenantFeedURLs>"#;
        serve(
            listener,
            HashMap::from([(
                "/api/arm/feeddiscovery".to_owned(),
                (200, discovery.to_owned()),
            )]),
            1,
        );
        let http = Http::new(EndpointPolicy::local_test(port));
        let feeds = discover(&http, &BearerToken::new("t".into()), &|| true).unwrap();
        assert!(matches!(
            feeds[0].result,
            Err(FetchError::Http(HttpError::Endpoint(_)))
        ));
    }
}
