// SPDX-License-Identifier: MIT

//! Deskfeed's parsers, compiled from the app's own source files, so the
//! fuzzer tests exactly what ships. Only modules that need nothing but
//! their own crates are included, under the paths the app uses
//! (`crate::service::endpoint`, …).

#![allow(dead_code)]

#[path = "../../src/service/endpoint.rs"]
pub mod endpoint_rs;
#[path = "../../src/service/feed.rs"]
pub mod feed_rs;
#[path = "../../src/sessions/rdpfile.rs"]
pub mod rdpfile_rs;

pub mod service {
    pub use super::endpoint_rs as endpoint;
    pub use super::feed_rs as feed;
}

pub mod sessions {
    pub use super::rdpfile_rs as rdpfile;
}
