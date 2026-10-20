// SPDX-License-Identifier: MIT

//! Test helpers shared by Deskfeed and its sign-in helper (a dev-dependency
//! of both; never part of a program).

use std::time::{Duration, Instant};

use proptest::collection::vec;
use proptest::prelude::*;
use proptest::test_runner::{Config, RngAlgorithm, TestCaseError, TestError, TestRng, TestRunner};

/// Random-input testing for parsers of untrusted input (spec §13 item 10,
/// ADR 0023): many mangled versions of real samples, plus some pure noise.
/// Each input must be handled without a panic and quickly; `check` adds
/// the parser's own rules. Repeatable: the seed is fixed unless
/// `DESKFEED_RANDOM_SEED` sets another; `DESKFEED_RANDOM_ROUNDS` runs more
/// than the default. A failing input is shrunk by proptest to the fewest
/// and smallest edits that still fail, then reported with the seed.
///
/// # Panics
///
/// When `check` panics or takes a second or more on an input: that is the
/// test failing.
pub fn random_inputs(name: &str, samples: &[&[u8]], check: impl Fn(&[u8])) {
    let rounds: u32 = std::env::var("DESKFEED_RANDOM_ROUNDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1500);
    let seed: u64 = std::env::var("DESKFEED_RANDOM_SEED")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0x5eed_dee5_f00d_cafe)
        ^ name
            .bytes()
            .fold(0u64, |h, b| h.rotate_left(5) ^ u64::from(b));
    let input = |bytes: &[u8]| String::from_utf8_lossy(bytes).into_owned();
    // The samples first, as they are.
    for (i, sample) in samples.iter().enumerate() {
        if let Err(why) = timed(&check, sample) {
            panic!(
                "{name}: sample {i} (seed {seed}) {why} on {:?}",
                input(sample)
            );
        }
    }
    let config = Config {
        cases: rounds,
        failure_persistence: None,
        // Shrinking re-runs `check`; a slow input takes a second each time.
        max_shrink_time: 60_000,
        ..Config::default()
    };
    let mut seed_bytes = [0u8; 32];
    for chunk in seed_bytes.chunks_mut(8) {
        chunk.copy_from_slice(&seed.to_le_bytes());
    }
    let mut runner = TestRunner::new_with_rng(
        config,
        TestRng::from_seed(RngAlgorithm::ChaCha, &seed_bytes),
    );
    let test = |bytes: Vec<u8>| timed(&check, &bytes).map_err(TestCaseError::fail);
    let noise = vec(any::<u8>(), 0..512);
    let outcome = if samples.is_empty() {
        runner.run(&noise, test)
    } else {
        let samples: Vec<Vec<u8>> = samples.iter().map(|s| s.to_vec()).collect();
        let count = samples.len();
        let mangled =
            (0..count, vec(edit(), 1..=8)).prop_map(move |(i, edits)| mangle(&samples[i], &edits));
        runner.run(&prop_oneof![15 => mangled, 1 => noise], test)
    };
    match outcome {
        Ok(()) => {}
        Err(TestError::Fail(why, bytes)) => {
            panic!("{name}: (seed {seed}) {why} on {:?}", input(&bytes))
        }
        Err(TestError::Abort(why)) => panic!("{name}: (seed {seed}) {why}"),
    }
}

/// Runs `check` on one input: a panic or a second or more is an error.
fn timed(check: &impl Fn(&[u8]), input: &[u8]) -> Result<(), String> {
    let started = Instant::now();
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| check(input)));
    let took = started.elapsed();
    if let Err(panic) = outcome {
        let text = panic
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| panic.downcast_ref::<&str>().copied())
            .unwrap_or("");
        return Err(format!("panicked ({text})"));
    }
    if took >= Duration::from_secs(1) {
        return Err(format!("took {took:?} on {} bytes", input.len()));
    }
    Ok(())
}

/// Pieces that tend to upset parsers: markup, separators, encodings,
/// numbers at the edges, and the markers this app's parsers look for.
const PIECES: &[&[u8]] = &[
    b"<",
    b">",
    b"</",
    b"/>",
    b"&",
    b"&amp;",
    b"&#0;",
    b"&#xD800;",
    b"\"",
    b"'",
    b"=",
    b":",
    b";",
    b",",
    b"?",
    b"#",
    b"%",
    b"%00",
    b"%zz",
    b"\r\n",
    b"\n",
    b"\r",
    b"\t",
    b"\0",
    b"\xff\xfe",
    b"\xef\xbb\xbf",
    b"\xc3",
    b"\xe2\x80\xae",
    b"<!DOCTYPE x [<!ENTITY a \"b\">]>",
    b"<![CDATA[",
    b"]]>",
    b"<!--",
    b"-->",
    b"9999999999999999999999",
    b"-1",
    b"0",
    b"..",
    b"//",
    b"https://",
    b"||",
    b":i:",
    b":s:",
    b" --> ",
    b"[ERROR]",
    b"[WARN]",
    b"\tThumbprint: ",
    b"Certificate details for ",
    b"RAIL exec error: execResult=",
];

/// One edit to a sample. Positions are taken modulo the current length,
/// so every edit stays valid while proptest shrinks the others away.
#[derive(Debug, Clone)]
enum Edit {
    /// Replace one byte.
    Byte(usize, u8),
    /// Insert one of [`PIECES`].
    Piece(usize, usize),
    /// Delete up to 31 bytes.
    Delete(usize, usize),
    /// Copy up to 63 bytes to another place.
    Copy(usize, usize, usize),
    Truncate(usize),
    /// Insert one to nine random bytes.
    Noise(usize, Vec<u8>),
    /// Something long: up to 15 bytes repeated up to 299 times at the end.
    Repeat(usize, usize, usize),
}

/// Positions up to the longest input.
const POSITION: std::ops::Range<usize> = 0..MAX_LEN;

/// Mangled inputs are cut to this length.
const MAX_LEN: usize = 64 * 1024;

fn edit() -> impl Strategy<Value = Edit> {
    prop_oneof![
        (POSITION, any::<u8>()).prop_map(|(at, b)| Edit::Byte(at, b)),
        (POSITION, 0..PIECES.len()).prop_map(|(at, p)| Edit::Piece(at, p)),
        (POSITION, 0..32usize).prop_map(|(at, n)| Edit::Delete(at, n)),
        (POSITION, 0..64usize, POSITION).prop_map(|(at, n, to)| Edit::Copy(at, n, to)),
        POSITION.prop_map(Edit::Truncate),
        (POSITION, vec(any::<u8>(), 1..=9)).prop_map(|(at, b)| Edit::Noise(at, b)),
        (POSITION, 0..16usize, 0..300usize).prop_map(|(at, n, k)| Edit::Repeat(at, n, k)),
    ]
}

/// `sample` with `edits` applied in order.
fn mangle(sample: &[u8], edits: &[Edit]) -> Vec<u8> {
    let mut out = sample.to_vec();
    for edit in edits {
        let len = out.len();
        let at = |p: usize| p % (len + 1);
        match edit {
            Edit::Byte(p, b) => {
                if len > 0 {
                    out[p % len] = *b;
                }
            }
            Edit::Piece(p, i) => {
                let at = at(*p);
                out.splice(at..at, PIECES[*i].iter().copied());
            }
            Edit::Delete(p, n) => {
                let at = at(*p);
                out.drain(at..(at + n).min(len));
            }
            Edit::Copy(p, n, to) => {
                let at = at(*p);
                let copy = out[at..(at + n).min(len)].to_vec();
                let to = to % (len + 1);
                out.splice(to..to, copy);
            }
            Edit::Truncate(p) => out.truncate(at(*p)),
            Edit::Noise(p, bytes) => {
                let at = at(*p);
                out.splice(at..at, bytes.iter().copied());
            }
            Edit::Repeat(p, n, times) => {
                let at = at(*p);
                let copy = out[at..(at + n).min(len)].to_vec();
                for _ in 0..*times {
                    out.extend_from_slice(&copy);
                }
            }
        }
        out.truncate(MAX_LEN);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::random_inputs;
    use std::sync::Mutex;

    /// The panic message of `f`, which must panic.
    fn failure(f: impl FnOnce()) -> String {
        let err = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).unwrap_err();
        err.downcast_ref::<String>().cloned().unwrap_or_default()
    }

    #[test]
    fn the_samples_themselves_are_checked() {
        let seen = Mutex::new(Vec::new());
        random_inputs("samples", &[b"first", b"second"], |input| {
            seen.lock().unwrap().push(input.to_vec());
        });
        let seen = seen.into_inner().unwrap();
        // First, as they are (a mangled copy can come out unchanged too).
        assert_eq!(seen[..2], [b"first".to_vec(), b"second".to_vec()]);
        // Then many other inputs.
        assert!(seen.len() > 1000, "{}", seen.len());
    }

    #[test]
    fn runs_repeat_exactly() {
        let run = || {
            let seen = Mutex::new(Vec::new());
            random_inputs("repeat", &[b"<a href=\"x\">text</a>"], |input| {
                seen.lock().unwrap().push(input.to_vec());
            });
            seen.into_inner().unwrap()
        };
        assert_eq!(run(), run());
    }

    #[test]
    fn a_panic_names_the_test_seed_and_input() {
        let message = failure(|| {
            random_inputs("panics", &[b"hello world"], |input| {
                assert!(!input.contains(&b'<'), "found <");
            });
        });
        assert!(message.starts_with("panics: "), "{message}");
        assert!(message.contains("seed"), "{message}");
        assert!(message.contains('<'), "{message}");
    }

    #[test]
    fn a_failing_input_is_shrunk() {
        // Long sample, many edits per input: only the one edit that matters
        // should be left.
        let sample = [b'a'; 2000];
        let last = Mutex::new(Vec::new());
        failure(|| {
            random_inputs("shrink", &[&sample], |input| {
                if input.contains(&b'<') {
                    last.lock().unwrap().clone_from(&input.to_vec());
                    panic!("found <");
                }
            });
        });
        let last = last.into_inner().unwrap();
        // Noise shrinks to "<"; a mangled sample to the sample with one
        // short piece inserted.
        let others = last.iter().filter(|&&b| b != b'a').count();
        assert!(
            last == b"<" || (others <= 4 && last.len().abs_diff(sample.len()) <= 4),
            "not shrunk: {} bytes {:?}",
            last.len(),
            String::from_utf8_lossy(&last)
        );
    }

    #[test]
    fn a_slow_input_fails() {
        // Slow once only, so shrinking doesn't take a second per try.
        let slept = std::sync::atomic::AtomicBool::new(false);
        let message = failure(|| {
            random_inputs("slow", &[b"hello world"], |input| {
                if input.contains(&b'<') && !slept.swap(true, std::sync::atomic::Ordering::Relaxed)
                {
                    std::thread::sleep(std::time::Duration::from_millis(1100));
                }
            });
        });
        assert!(message.starts_with("slow: "), "{message}");
        assert!(message.contains("took"), "{message}");
    }
}
