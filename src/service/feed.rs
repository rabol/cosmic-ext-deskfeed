// SPDX-License-Identifier: MIT

//! Parsing of the feed discovery document and tenant feeds (spec §7).
//!
//! Both are untrusted input. We parse with DTDs disallowed (no entity
//! expansion, no external loading), cap node and item counts, keep only
//! the fields we need, and clean up display strings.
//!
//! Schemas observed 2026-10-17:
//! - discovery: `http://schemas.microsoft.com/ts/2014/03/tswfdiscovery`,
//!   `<TenantFeedURLs><TenantFeedURL FeedURL TenantId WorkspaceName …/>`
//! - feed: `http://schemas.microsoft.com/ts/2007/05/tswf`, schema 2.1,
//!   `<ResourceCollection><Publisher><Resources><Resource …>` with
//!   `<Icons><Icon32/><IconRaw/></Icons>` and
//!   `<HostingTerminalServers><HostingTerminalServer><ResourceFile/>`.

use roxmltree::{Document, Node, ParsingOptions};
use url::Url;

const NS_DISCOVERY: &str = "http://schemas.microsoft.com/ts/2014/03/tswfdiscovery";
const NS_FEED: &str = "http://schemas.microsoft.com/ts/2007/05/tswf";

/// Generous limits; real documents are a few kilobytes.
const MAX_NODES: u32 = 200_000;
const MAX_TENANT_FEEDS: usize = 64;
const MAX_RESOURCES: usize = 2_000;
const MAX_TEXT_CHARS: usize = 256;
const MAX_ID_CHARS: usize = 128;
/// Real feeds nest about 8 levels. roxmltree recurses per level and
/// overflows the stack on deeply nested input (0.20 at ~10 000 levels,
/// 0.21 at ~1 000 in debug builds), so we check depth first.
const MAX_DEPTH: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FeedError {
    /// Not well-formed XML, a DTD, or over the node limit.
    Xml(String),
    /// Well-formed, but not the document we expected.
    UnexpectedDocument,
    TooManyItems,
}

impl std::fmt::Display for FeedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Xml(e) => write!(f, "malformed feed XML: {e}"),
            Self::UnexpectedDocument => write!(f, "unexpected feed document"),
            Self::TooManyItems => write!(f, "feed has too many entries"),
        }
    }
}

impl std::error::Error for FeedError {}

/// One entry from feed discovery: a tenant's feed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TenantFeed {
    pub tenant_id: String,
    pub feed_url: Url,
    pub workspace_name: Option<String>,
    pub tenant_display_name: Option<String>,
    pub consent_status: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceKind {
    Desktop,
    RemoteApp,
    /// A type we don't know; shown but not connectable until supported.
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resource {
    /// Stable identifier from the feed. Storage keys combine it with the
    /// account and tenant; titles are never used as identity (spec §7).
    pub id: String,
    pub title: String,
    pub kind: ResourceKind,
    pub show_by_default: bool,
    pub connection_file: Option<Url>,
    pub icon_png: Option<Url>,
    pub icon_ico: Option<Url>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    pub publisher_id: Option<String>,
    pub name: String,
    pub resources: Vec<Resource>,
    /// Entries left out because they were malformed or duplicated.
    pub skipped: usize,
}

/// Rejects documents nested deeper than `limit`, and any DTD, with a
/// linear scan that never recurses. Exact for well-formed XML; for
/// malformed XML the parser stops at the first error anyway.
fn check_depth(xml: &str, limit: usize) -> Result<(), FeedError> {
    let bytes = xml.as_bytes();
    let find = |from: usize, pat: &[u8]| {
        bytes
            .get(from..)?
            .windows(pat.len())
            .position(|w| w == pat)
            .map(|p| from + p + pat.len())
    };
    let unterminated = || FeedError::Xml("unterminated markup".into());
    let mut depth = 0usize;
    let mut pos = 0;
    while let Some(off) = bytes[pos..].iter().position(|&c| c == b'<') {
        let start = pos + off;
        let rest = &bytes[start..];
        pos = if rest.starts_with(b"<!--") {
            find(start + 4, b"-->").ok_or_else(unterminated)?
        } else if rest.starts_with(b"<![CDATA[") {
            find(start + 9, b"]]>").ok_or_else(unterminated)?
        } else if rest.starts_with(b"<?") {
            find(start + 2, b"?>").ok_or_else(unterminated)?
        } else if rest.starts_with(b"<!") {
            return Err(FeedError::Xml("DTD not allowed".into()));
        } else if rest.starts_with(b"</") {
            depth = depth.saturating_sub(1);
            find(start + 2, b">").ok_or_else(unterminated)?
        } else {
            // Start tag: find its closing '>' outside quoted values.
            let mut end = start + 1;
            let mut quote = None;
            loop {
                let c = *bytes.get(end).ok_or_else(unterminated)?;
                match (quote, c) {
                    (None, b'"' | b'\'') => quote = Some(c),
                    (Some(q), _) if c == q => quote = None,
                    (None, b'>') => break,
                    _ => {}
                }
                end += 1;
            }
            if bytes[end - 1] != b'/' {
                depth += 1;
                if depth > limit {
                    return Err(FeedError::Xml("nested too deeply".into()));
                }
            }
            end + 1
        };
    }
    Ok(())
}

fn parse_doc(xml: &str) -> Result<Document<'_>, FeedError> {
    check_depth(xml, MAX_DEPTH)?;
    let opts = ParsingOptions {
        allow_dtd: false,
        nodes_limit: MAX_NODES,
    };
    Document::parse_with_options(xml, opts).map_err(|e| FeedError::Xml(e.to_string()))
}

fn is(node: Node<'_, '_>, ns: &str, name: &str) -> bool {
    node.is_element() && node.tag_name().name() == name && node.tag_name().namespace() == Some(ns)
}

fn child<'a, 'i>(node: Node<'a, 'i>, ns: &str, name: &str) -> Option<Node<'a, 'i>> {
    node.children().find(|c| is(*c, ns, name))
}

fn children<'a, 'i>(
    node: Node<'a, 'i>,
    ns: &'a str,
    name: &'a str,
) -> impl Iterator<Item = Node<'a, 'i>> + 'a {
    node.children().filter(move |c| is(*c, ns, name))
}

/// Display text: control characters removed, whitespace collapsed, length
/// capped. `None` if nothing is left.
fn clean_text(s: &str) -> Option<String> {
    let cleaned: String = s
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .filter(|c| !c.is_control() && !is_bidi_override(*c))
        .take(MAX_TEXT_CHARS)
        .collect();
    (!cleaned.is_empty()).then_some(cleaned)
}

/// Bidirectional override characters can make a title display differently
/// from what it is ("Trojan Source"); drop them.
fn is_bidi_override(c: char) -> bool {
    matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

fn clean_id(s: &str) -> Option<String> {
    let s = s.trim();
    (!s.is_empty() && s.chars().count() <= MAX_ID_CHARS && s.chars().all(|c| c.is_ascii_graphic()))
        .then(|| s.to_owned())
}

fn parse_url(s: Option<&str>) -> Option<Url> {
    let url = Url::parse(s?.trim()).ok()?;
    matches!(url.scheme(), "https" | "http").then_some(url)
}

pub fn parse_discovery(xml: &str) -> Result<Vec<TenantFeed>, FeedError> {
    let doc = parse_doc(xml)?;
    let root = doc.root_element();
    if !is(root, NS_DISCOVERY, "TenantFeedURLs") {
        return Err(FeedError::UnexpectedDocument);
    }
    let mut feeds = Vec::new();
    for el in children(root, NS_DISCOVERY, "TenantFeedURL") {
        let (Some(tenant_id), Some(feed_url)) = (
            el.attribute("TenantId").and_then(clean_id),
            parse_url(el.attribute("FeedURL")),
        ) else {
            continue;
        };
        if feeds.len() == MAX_TENANT_FEEDS {
            return Err(FeedError::TooManyItems);
        }
        feeds.push(TenantFeed {
            tenant_id,
            feed_url,
            workspace_name: el.attribute("WorkspaceName").and_then(clean_text),
            tenant_display_name: el.attribute("TenantDisplayName").and_then(clean_text),
            consent_status: el.attribute("ConsentStatus").and_then(clean_text),
        });
    }
    Ok(feeds)
}

pub fn parse_feed(xml: &str) -> Result<Workspace, FeedError> {
    let doc = parse_doc(xml)?;
    let root = doc.root_element();
    if !is(root, NS_FEED, "ResourceCollection") {
        return Err(FeedError::UnexpectedDocument);
    }
    let publisher = child(root, NS_FEED, "Publisher").ok_or(FeedError::UnexpectedDocument)?;
    let mut workspace = Workspace {
        publisher_id: publisher.attribute("ID").and_then(clean_id),
        name: publisher
            .attribute("Name")
            .and_then(clean_text)
            .unwrap_or_default(),
        resources: Vec::new(),
        skipped: 0,
    };
    let Some(resources) = child(publisher, NS_FEED, "Resources") else {
        return Ok(workspace);
    };
    for el in children(resources, NS_FEED, "Resource") {
        match parse_resource(el) {
            Some(r) if !workspace.resources.iter().any(|x| x.id == r.id) => {
                if workspace.resources.len() == MAX_RESOURCES {
                    return Err(FeedError::TooManyItems);
                }
                workspace.resources.push(r);
            }
            _ => workspace.skipped += 1,
        }
    }
    Ok(workspace)
}

fn parse_resource(el: Node<'_, '_>) -> Option<Resource> {
    let id = el.attribute("ID").and_then(clean_id)?;
    let title = el.attribute("Title").and_then(clean_text)?;
    let kind = match el.attribute("Type") {
        Some("Desktop") => ResourceKind::Desktop,
        Some("RemoteApp") => ResourceKind::RemoteApp,
        _ => ResourceKind::Other,
    };
    let icons = child(el, NS_FEED, "Icons");
    let icon = |name| {
        icons
            .and_then(|i| child(i, NS_FEED, name))
            .and_then(|i| parse_url(i.attribute("FileURL")))
    };
    let connection_file = child(el, NS_FEED, "HostingTerminalServers")
        .into_iter()
        .flat_map(|h| children(h, NS_FEED, "HostingTerminalServer"))
        .filter_map(|h| child(h, NS_FEED, "ResourceFile"))
        .find(|f| {
            f.attribute("FileExtension")
                .is_none_or(|e| e.eq_ignore_ascii_case(".rdp"))
        })
        .and_then(|f| parse_url(f.attribute("URL")));
    Some(Resource {
        id,
        title,
        kind,
        show_by_default: el.attribute("ShowByDefault") != Some("false"),
        connection_file,
        icon_png: icon("Icon32"),
        icon_ico: icon("IconRaw"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "https://rdweb.wvd.microsoft.com/api/arm/feeddiscovery/tenants/t1";

    fn discovery(entries: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="utf-8"?>
<TenantFeedURLs xmlns="{NS_DISCOVERY}" ARMResources="true" ClassicResources="false" UserIdType="OrgId">{entries}</TenantFeedURLs>"#
        )
    }

    fn feed(resources: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="utf-8"?>
<ResourceCollection xmlns="{NS_FEED}" PubDate="2026-10-17T00:00:00Z" SchemaVersion="2.1">
  <Publisher LastUpdated="2026-10-17T00:00:00Z" Name="Example Workspace" ID="pub-1" Description="" SupportsReconnect="false">
    <Resources>{resources}</Resources>
    <TerminalServers><TerminalServer ID="ts" Name="ts" LastUpdated="x"/></TerminalServers>
  </Publisher>
</ResourceCollection>"#
        )
    }

    fn desktop(id: &str, title: &str) -> String {
        format!(
            r#"<Resource ID="{id}" Alias="a" Title="{title}" LastUpdated="x" Type="Desktop" ShowByDefault="true">
  <Icons>
    <Icon32 Dimensions="32x32" FileType="Png" FileURL="{BASE}/pngs/{id}.png?v=1"/>
    <IconRaw FileType="Ico" FileURL="{BASE}/icons/{id}.ico?v=1"/>
  </Icons>
  <HostingTerminalServers><HostingTerminalServer>
    <ResourceFile FileExtension=".rdp" URL="{BASE}/rdps/{id}.rdp?v=1"/>
    <TerminalServerRef Ref="ts"/>
  </HostingTerminalServer></HostingTerminalServers>
</Resource>"#
        )
    }

    #[test]
    fn parses_discovery() {
        let xml = discovery(&format!(
            r#"<TenantFeedURL FeedURL="{BASE}/webfeed?x=1" TenantId="t1" WorkspaceName="Main" TenantDisplayName="Example Ltd" ConsentStatus="Accepted"/>
               <TenantFeedURL FeedURL="not a url" TenantId="t2"/>
               <TenantFeedURL TenantId="t3"/>"#
        ));
        let feeds = parse_discovery(&xml).unwrap();
        assert_eq!(feeds.len(), 1);
        assert_eq!(feeds[0].tenant_id, "t1");
        assert_eq!(feeds[0].feed_url.as_str(), format!("{BASE}/webfeed?x=1"));
        assert_eq!(feeds[0].workspace_name.as_deref(), Some("Main"));
        assert_eq!(feeds[0].consent_status.as_deref(), Some("Accepted"));
    }

    #[test]
    fn empty_discovery_means_no_assignments() {
        assert_eq!(parse_discovery(&discovery("")).unwrap(), vec![]);
    }

    #[test]
    fn parses_feed() {
        let ws = parse_feed(&feed(&desktop("r1", "Standard Desktop"))).unwrap();
        assert_eq!(ws.name, "Example Workspace");
        assert_eq!(ws.publisher_id.as_deref(), Some("pub-1"));
        assert_eq!(ws.skipped, 0);
        let r = &ws.resources[0];
        assert_eq!(r.id, "r1");
        assert_eq!(r.title, "Standard Desktop");
        assert_eq!(r.kind, ResourceKind::Desktop);
        assert!(r.show_by_default);
        assert_eq!(
            r.connection_file.as_ref().unwrap().as_str(),
            format!("{BASE}/rdps/r1.rdp?v=1")
        );
        assert!(
            r.icon_png
                .as_ref()
                .unwrap()
                .path()
                .ends_with("/pngs/r1.png")
        );
        assert!(
            r.icon_ico
                .as_ref()
                .unwrap()
                .path()
                .ends_with("/icons/r1.ico")
        );
    }

    #[test]
    fn duplicate_titles_are_kept_duplicate_ids_are_not() {
        let xml = feed(
            &[
                desktop("r1", "Desk"),
                desktop("r2", "Desk"),
                desktop("r1", "Other"),
            ]
            .concat(),
        );
        let ws = parse_feed(&xml).unwrap();
        let ids: Vec<_> = ws.resources.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["r1", "r2"]);
        assert_eq!(ws.skipped, 1);
    }

    #[test]
    fn malformed_resources_are_skipped_not_fatal() {
        let xml = feed(&format!(
            r#"{}<Resource Title="no id" Type="Desktop"/><Resource ID="bad id with spaces" Title="x"/><Resource ID="r9"/>"#,
            desktop("r1", "Ok")
        ));
        let ws = parse_feed(&xml).unwrap();
        assert_eq!(ws.resources.len(), 1);
        assert_eq!(ws.skipped, 3);
    }

    #[test]
    fn titles_are_cleaned() {
        let xml = feed(&desktop("r1", "  Evil&#x202E;txt.exe&#10;\tdesk  "));
        let ws = parse_feed(&xml).unwrap();
        assert_eq!(ws.resources[0].title, "Eviltxt.exe desk");
        let long = "a".repeat(1000);
        let ws = parse_feed(&feed(&desktop("r1", &long))).unwrap();
        assert_eq!(ws.resources[0].title.chars().count(), MAX_TEXT_CHARS);
    }

    #[test]
    fn unknown_type_and_non_http_urls() {
        let xml = feed(
            r#"<Resource ID="r1" Title="T" Type="Future">
                 <Icons><Icon32 FileURL="file:///etc/passwd"/></Icons>
                 <HostingTerminalServers><HostingTerminalServer>
                   <ResourceFile FileExtension=".exe" URL="https://example.com/x.exe"/>
                 </HostingTerminalServer></HostingTerminalServers>
               </Resource>"#,
        );
        let r = &parse_feed(&xml).unwrap().resources[0];
        assert_eq!(r.kind, ResourceKind::Other);
        assert_eq!(r.icon_png, None);
        assert_eq!(r.connection_file, None);
    }

    #[test]
    fn rejects_dtd_and_entities() {
        let bomb = format!(
            r#"<?xml version="1.0"?><!DOCTYPE x [<!ENTITY a "aaaaaaaaaa"><!ENTITY b "&a;&a;&a;&a;&a;">]>
<TenantFeedURLs xmlns="{NS_DISCOVERY}"><TenantFeedURL FeedURL="{BASE}/webfeed" TenantId="&b;"/></TenantFeedURLs>"#
        );
        assert!(matches!(parse_discovery(&bomb), Err(FeedError::Xml(_))));
        let external = format!(
            r#"<?xml version="1.0"?><!DOCTYPE x [<!ENTITY e SYSTEM "file:///etc/passwd">]>
<ResourceCollection xmlns="{NS_FEED}"><Publisher Name="&e;"/></ResourceCollection>"#
        );
        assert!(matches!(parse_feed(&external), Err(FeedError::Xml(_))));
    }

    #[test]
    fn rejects_wrong_documents() {
        assert_eq!(
            parse_feed(&discovery("")),
            Err(FeedError::UnexpectedDocument)
        );
        assert_eq!(
            parse_discovery(r#"<TenantFeedURLs xmlns="urn:other"/>"#),
            Err(FeedError::UnexpectedDocument)
        );
        assert!(matches!(parse_feed("<unclosed"), Err(FeedError::Xml(_))));
        assert_eq!(
            parse_feed(&format!(r#"<ResourceCollection xmlns="{NS_FEED}"/>"#)),
            Err(FeedError::UnexpectedDocument)
        );
    }

    #[test]
    fn caps_counts() {
        use std::fmt::Write as _;
        let mut many = String::new();
        for i in 0..=MAX_TENANT_FEEDS {
            write!(
                many,
                r#"<TenantFeedURL FeedURL="{BASE}/webfeed" TenantId="t{i}"/>"#
            )
            .unwrap();
        }
        assert_eq!(
            parse_discovery(&discovery(&many)),
            Err(FeedError::TooManyItems)
        );
    }

    #[test]
    fn deep_nesting_is_rejected_without_recursing() {
        for depth in [MAX_DEPTH + 1, 10_000, 300_000] {
            let deep = format!("{}{}", "<a>".repeat(depth), "</a>".repeat(depth));
            assert!(
                matches!(parse_feed(&deep), Err(FeedError::Xml(_))),
                "{depth}"
            );
        }
    }

    #[test]
    fn depth_scan_handles_markup_that_looks_like_tags() {
        let ok = |s: &str| check_depth(s, 2);
        assert!(ok("<a><b/></a><a><b></b></a>").is_ok());
        assert!(ok(r#"<a x="<<<" y='>'><b z=">"/></a>"#).is_ok());
        assert!(ok("<a><!-- <b><c><d> --><![CDATA[<b><c><d>]]><?pi <b><c>?></a>").is_ok());
        assert!(ok("<a><b><c/></b></a>").is_ok());
        assert!(ok("<a><b><c></c></b></a>").is_err());
        assert!(ok("<a><!DOCTYPE x></a>").is_err());
        assert!(ok("<a x=\"unterminated>").is_err());
        assert!(ok("<a><!-- never closed").is_err());
    }

    /// Parses real saved feeds from `AVD_FEED_DIR` (for example the
    /// reference app's folder) and prints only counts, never content.
    #[test]
    #[ignore = "needs AVD_FEED_DIR with real feed files"]
    fn real_saved_feeds_parse() {
        use crate::service::endpoint::{EndpointPolicy, Role};

        let dir = std::env::var("AVD_FEED_DIR").expect("set AVD_FEED_DIR");
        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        entries.sort();
        for path in entries
            .iter()
            .filter(|p| p.extension().is_some_and(|e| e == "xml"))
        {
            let xml = std::fs::read_to_string(path).unwrap();
            let name = path.file_name().unwrap().to_string_lossy();
            if name.starts_with("00_") {
                let feeds = parse_discovery(&xml).unwrap();
                let policy = EndpointPolicy::azure_public();
                let allowed = feeds
                    .iter()
                    .filter(|f| policy.check(&f.feed_url, Role::Feed).is_ok())
                    .count();
                println!(
                    "discovery: {} tenant feeds, {allowed} allowed by policy",
                    feeds.len()
                );
            } else {
                let ws = parse_feed(&xml).unwrap();
                let with_rdp = ws
                    .resources
                    .iter()
                    .filter(|r| r.connection_file.is_some())
                    .count();
                let with_icon = ws.resources.iter().filter(|r| r.icon_png.is_some()).count();
                let policy = EndpointPolicy::azure_public();
                let allowed = ws
                    .resources
                    .iter()
                    .flat_map(|r| {
                        [
                            r.connection_file
                                .as_ref()
                                .map(|u| (u, Role::ConnectionFile)),
                            r.icon_png.as_ref().map(|u| (u, Role::Icon)),
                            r.icon_ico.as_ref().map(|u| (u, Role::Icon)),
                        ]
                    })
                    .flatten()
                    .all(|(u, role)| policy.check(u, role).is_ok());
                println!("  all resource URLs allowed by policy: {allowed}");
                println!(
                    "feed: {} resources, {with_rdp} with .rdp, {with_icon} with icon, {} skipped",
                    ws.resources.len(),
                    ws.skipped
                );
            }
        }
    }

    #[test]
    fn random_feeds_never_break_the_parsers() {
        let disc = discovery(&format!(
            r#"<TenantFeedURL FeedURL="{BASE}/webfeed?x=1" TenantId="t1" WorkspaceName="Main"/>"#
        ));
        let one = feed(&desktop("r1", "Standard Desktop"));
        let two = feed(&format!("{}{}", desktop("r1", "One"), desktop("r2", "Two")));
        crate::test_util::random_inputs(
            "feed",
            &[disc.as_bytes(), one.as_bytes(), two.as_bytes()],
            |input| {
                let text = String::from_utf8_lossy(input);
                // Only web addresses; the shared HTTP policy then allows
                // nothing but HTTPS to Microsoft's hosts (`endpoint.rs`).
                let web = |url: &Url| matches!(url.scheme(), "https" | "http");
                if let Ok(feeds) = parse_discovery(&text) {
                    assert!(feeds.iter().all(|f| web(&f.feed_url)));
                }
                if let Ok(ws) = parse_feed(&text) {
                    for r in ws.resources {
                        for url in r.connection_file.iter().chain(&r.icon_png) {
                            assert!(web(url), "{url}");
                        }
                    }
                }
            },
        );
    }
}
