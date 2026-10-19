// SPDX-License-Identifier: MIT

//! Test helpers shared by Deskfeed and its sign-in helper (a dev-dependency
//! of both; never part of a program). Standard library only.

/// Random-input testing for parsers of untrusted input (spec §13 item 10,
/// ADR 0023): many mangled versions of real samples, plus some pure noise.
/// Each input must be handled without a panic and quickly; `check` adds
/// the parser's own rules. Repeatable: the seed is fixed unless
/// `DESKFEED_RANDOM_SEED` sets another; `DESKFEED_RANDOM_ROUNDS` runs more
/// than the default. A failure names the round, seed and input.
///
/// # Panics
///
/// When `check` panics or takes a second or more on an input: that is the
/// test failing.
pub fn random_inputs(name: &str, samples: &[&[u8]], check: impl Fn(&[u8])) {
    let rounds: usize = std::env::var("DESKFEED_RANDOM_ROUNDS")
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
    let mut rng = Rng(seed | 1);
    let inputs = samples
        .iter()
        .map(|s| s.to_vec())
        .chain((0..rounds).map(|round| {
            if round % 16 == 0 || samples.is_empty() {
                let len = rng.below(512);
                (0..len).map(|_| rng.byte()).collect()
            } else {
                let sample = samples[rng.below(samples.len())];
                mutate(&mut rng, sample)
            }
        }));
    for (round, input) in inputs.enumerate() {
        let started = std::time::Instant::now();
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| check(&input)));
        let took = started.elapsed();
        assert!(
            outcome.is_ok(),
            "{name}: round {round} (seed {seed}) panicked on {:?}",
            String::from_utf8_lossy(&input)
        );
        assert!(
            took < std::time::Duration::from_secs(1),
            "{name}: round {round} (seed {seed}) took {took:?} on {} bytes",
            input.len()
        );
    }
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

/// One mangled copy of `sample`: one to eight random edits.
fn mutate(rng: &mut Rng, sample: &[u8]) -> Vec<u8> {
    const MAX_LEN: usize = 64 * 1024;
    let mut out = sample.to_vec();
    for _ in 0..=rng.below(8) {
        let at = rng.below(out.len() + 1);
        match rng.below(7) {
            0 if !out.is_empty() => {
                let i = rng.below(out.len());
                out[i] = rng.byte();
            }
            1 => {
                let piece = PIECES[rng.below(PIECES.len())];
                out.splice(at..at, piece.iter().copied());
            }
            2 => {
                let end = (at + rng.below(32)).min(out.len());
                out.drain(at..end);
            }
            3 => {
                let end = (at + rng.below(64)).min(out.len());
                let copy = out[at..end].to_vec();
                let to = rng.below(out.len() + 1);
                out.splice(to..to, copy);
            }
            4 => out.truncate(at),
            5 => {
                let noise: Vec<u8> = (0..=rng.below(8)).map(|_| rng.byte()).collect();
                out.splice(at..at, noise);
            }
            _ => {
                // Something long: a piece repeated many times.
                let end = (at + rng.below(16)).min(out.len());
                let copy = out[at..end].to_vec();
                let times = rng.below(300);
                for _ in 0..times {
                    out.extend_from_slice(&copy);
                }
            }
        }
        out.truncate(MAX_LEN);
    }
    out
}

/// xorshift64*: small, fast, and enough to pick edits.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            return 0;
        }
        usize::try_from(self.next() % n as u64).unwrap_or(0)
    }

    fn byte(&mut self) -> u8 {
        self.next().to_le_bytes()[0]
    }
}
