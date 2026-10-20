// SPDX-License-Identifier: MIT

//! Connection files (`src/sessions/rdpfile.rs`): no panic, and an accepted
//! file gives FreeRDP printable lines only.

#![no_main]

use cosmic_ext_deskfeed_fuzz::sessions::rdpfile::ConnectionFile;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(file) = ConnectionFile::parse(data) else {
        return;
    };
    let _ = file.remote_app();
    // As written for FreeRDP, with the clipboard off and on.
    for clipboard in [false, true] {
        for line in file.to_freerdp_text_with(clipboard).split_terminator("\r\n") {
            assert!(!line.chars().any(char::is_control), "{line:?}");
            assert_eq!(line.splitn(3, ':').count(), 3, "{line:?}");
        }
    }
});
