// SPDX-License-Identifier: MIT

//! Line reading with a size limit (spec §§7–8: bounded input from other
//! processes). `BufRead::lines` keeps a whole line in memory before anyone
//! can check its length; a program that never sends a newline could fill
//! the memory. This keeps at most `max` bytes of a line and skips the
//! rest.
//!
//! Shared by the app and the sign-in helper (`#[path]` include), so it uses
//! only the standard library.

use std::io::BufRead;

/// One line, without its line ending.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    /// At most `max` bytes of it (invalid UTF-8 replaced).
    pub text: String,
    /// It was longer, and `text` is only its start.
    pub too_long: bool,
}

/// The lines of `reader`, each limited to `max` bytes. Ends at the end of
/// the input or on a read error.
pub fn lines<R: BufRead>(reader: R, max: usize) -> Lines<R> {
    Lines { reader, max }
}

pub struct Lines<R> {
    reader: R,
    max: usize,
}

impl<R: BufRead> Iterator for Lines<R> {
    type Item = Line;

    fn next(&mut self) -> Option<Line> {
        let mut buf = Vec::new();
        let mut too_long = false;
        let mut started = false;
        loop {
            let chunk = match self.reader.fill_buf() {
                Ok(chunk) => chunk,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => return None,
            };
            if chunk.is_empty() {
                if !started {
                    return None;
                }
                break; // last line without a newline
            }
            started = true;
            let newline = chunk.iter().position(|&b| b == b'\n');
            let part = &chunk[..newline.unwrap_or(chunk.len())];
            let room = self.max.saturating_sub(buf.len());
            if part.len() > room {
                too_long = true;
            }
            buf.extend_from_slice(&part[..part.len().min(room)]);
            let used = newline.map_or(chunk.len(), |i| i + 1);
            self.reader.consume(used);
            if newline.is_some() {
                break;
            }
        }
        if buf.last() == Some(&b'\r') {
            buf.pop();
        }
        Some(Line {
            text: String::from_utf8_lossy(&buf).into_owned(),
            too_long,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufReader, Read};

    fn all(input: &[u8], max: usize) -> Vec<Line> {
        // A tiny buffer, so lines span several reads.
        lines(BufReader::with_capacity(3, input), max).collect()
    }

    fn line(text: &str, too_long: bool) -> Line {
        Line {
            text: text.into(),
            too_long,
        }
    }

    #[test]
    fn splits_like_lines() {
        assert_eq!(
            all(b"one\r\ntwo\n\nlast", 100),
            [
                line("one", false),
                line("two", false),
                line("", false),
                line("last", false)
            ]
        );
        assert_eq!(all(b"", 100), Vec::new());
    }

    #[test]
    fn long_lines_are_cut_and_the_rest_skipped() {
        assert_eq!(
            all(b"abcdefgh\nok\n", 4),
            [line("abcd", true), line("ok", false)]
        );
    }

    #[test]
    fn a_producer_without_newlines_uses_bounded_memory() {
        // 64 MiB of input without a newline, generated on the fly.
        let endless = std::io::repeat(b'x').take(64 * 1024 * 1024);
        let mut it = lines(BufReader::new(endless), 16);
        assert_eq!(it.next(), Some(line("xxxxxxxxxxxxxxxx", true)));
        assert_eq!(it.next(), None);
    }
}
