// SPDX-License-Identifier: MIT

//! Feed discovery and workspace feeds (`src/service/feed.rs`): no panic, and
//! only web addresses come out.

#![no_main]

use cosmic_ext_deskfeed_fuzz::service::feed::{parse_discovery, parse_feed};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // The app turns the body into text first and refuses invalid UTF-8.
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let web = |url: &url::Url| matches!(url.scheme(), "https" | "http");
    if let Ok(feeds) = parse_discovery(text) {
        assert!(feeds.iter().all(|f| web(&f.feed_url)));
    }
    if let Ok(ws) = parse_feed(text) {
        for r in ws.resources {
            assert!(r.connection_file.iter().chain(&r.icon_png).all(web));
        }
    }
});
