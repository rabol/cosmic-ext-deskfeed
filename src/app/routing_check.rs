// SPDX-License-Identifier: MIT

//! Checks that every message reaches a handler that handles it.
//!
//! The router (`AppModel::route`) lists every message, so the compiler
//! refuses a message it doesn't route. It can't check that the handler a
//! message is sent to has an arm for it: a mismatch only shows at run
//! time as "bug: message … routed to the wrong handler" and the action
//! does nothing (Quit in the menu, 2026-10-20). This test reads the
//! sources and compares the two, so the mismatch fails CI instead.

use std::collections::{BTreeMap, BTreeSet};

const MODEL: &str = include_str!("model.rs");
const ROUTER: &str = include_str!("mod.rs");
/// Each handler and the file it is in.
const HANDLERS: &[(&str, &str)] = &[
    ("account_message", include_str!("account_flow.rs")),
    ("connection_message", include_str!("connect.rs")),
    ("settings_message", include_str!("settings_ui.rs")),
    ("app_message", include_str!("mod.rs")),
];

/// The body of `fn name(…) { … }` in `src`.
fn body<'a>(src: &'a str, name: &str) -> &'a str {
    let start = src
        .find(&format!("fn {name}("))
        .unwrap_or_else(|| panic!("fn {name} not found"));
    // Signatures here have no braces: the first `{` opens the body.
    let open = start + src[start..].find('{').expect("body");
    let mut depth = 0;
    for (i, c) in src[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return &src[open + 1..open + i];
                }
            }
            _ => {}
        }
    }
    panic!("fn {name}: unbalanced braces")
}

/// `Message::Name` occurrences in `text`.
fn names(text: &str) -> BTreeSet<String> {
    text.match_indices("Message::")
        .map(|(i, m)| {
            text[i + m.len()..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect::<String>()
        })
        .filter(|n| !n.is_empty())
        .collect()
}

/// The patterns of the top-level arms of the first `match message {` in
/// `body` (what comes before each `=>` at the match's own depth).
fn arm_patterns(body: &str) -> Vec<String> {
    let start = body.find("match message {").expect("match message") + "match message {".len();
    let bytes: Vec<char> = body[start..].chars().collect();
    let (mut depth, mut current, mut arms) = (1, String::new(), Vec::new());
    let mut i = 0;
    while i < bytes.len() && depth > 0 {
        let c = bytes[i];
        match c {
            '{' | '(' | '[' => depth += 1,
            '}' | ')' | ']' => depth -= 1,
            _ => {}
        }
        if depth == 1 && c == '=' && bytes.get(i + 1) == Some(&'>') {
            arms.push(std::mem::take(&mut current));
            i += 2;
            continue;
        }
        if depth == 1 && c == ',' {
            current.clear();
        } else if depth == 1 {
            current.push(c);
        }
        i += 1;
    }
    arms
}

fn variants() -> BTreeSet<String> {
    let start = MODEL.find("pub enum Message {").expect("enum Message");
    let end = start + MODEL[start..].find("\n}\n").expect("end of enum");
    MODEL[start..end]
        .lines()
        .filter_map(|l| {
            let name = l.strip_prefix("    ")?;
            let name: String = name
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            name.starts_with(|c: char| c.is_ascii_uppercase())
                .then_some(name)
        })
        .collect()
}

/// Router: which messages go to which handler.
fn routes() -> BTreeMap<String, BTreeSet<String>> {
    let route = body(ROUTER, "route");
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let arms = arm_patterns(route);
    // The handler each arm calls: the text after its `=>`, up to `(`.
    let mut rest = &route[route.find("match message {").expect("match")..];
    for pattern in arms {
        let at = rest.find("=>").expect("arm");
        rest = &rest[at + 2..];
        let call = rest.trim_start();
        let handler = call
            .strip_prefix("self.")
            .map(|c| {
                c.chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                    .collect::<String>()
            })
            .unwrap_or_default();
        out.entry(handler).or_default().extend(names(&pattern));
    }
    out
}

#[test]
fn every_message_reaches_a_handler_that_handles_it() {
    let variants = variants();
    assert!(variants.len() > 50, "enum not read: {variants:?}");
    let table = routes();
    let all_sent: BTreeSet<String> = table.values().flatten().cloned().collect();
    let missing: Vec<_> = variants.difference(&all_sent).collect();
    assert!(missing.is_empty(), "not routed: {missing:?}");
    for (handler, src) in HANDLERS {
        let handled: BTreeSet<String> = arm_patterns(body(src, handler))
            .iter()
            .flat_map(|a| names(a))
            .collect();
        let sent = table.get(*handler).cloned().unwrap_or_default();
        let unhandled: Vec<_> = sent.difference(&handled).collect();
        assert!(
            unhandled.is_empty(),
            "{handler} gets messages it has no arm for: {unhandled:?}"
        );
        let elsewhere: Vec<_> = handled.difference(&sent).collect();
        assert!(
            elsewhere.is_empty(),
            "{handler} handles messages the router sends elsewhere: {elsewhere:?}"
        );
    }
}
