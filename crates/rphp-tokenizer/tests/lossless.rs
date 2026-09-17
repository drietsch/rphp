//! Structural invariants that must hold for *arbitrary* input: the tokens tile
//! the source exactly (lossless), and the tokenizer never panics. Checked over
//! the in-repo corpus and over deterministic random mutations of it.

use rphp_tokenizer::{tokenize, Options, RawToken};
use std::path::PathBuf;

fn corpus_files() -> Vec<PathBuf> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("tests/corpus exists")
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "php"))
        .collect();
    files.sort();
    assert!(!files.is_empty(), "empty corpus at {}", dir.display());
    files
}

/// `tokens[0].lo == 0`, adjacent tokens touch, the last ends at `len`; an empty
/// input has no tokens. Zero-length tokens are allowed (PHP emits them).
fn check_tiling(src: &[u8], tokens: &[RawToken], what: &str) {
    if src.is_empty() {
        assert!(tokens.is_empty(), "{what}: tokens for empty input");
        return;
    }
    assert!(
        !tokens.is_empty(),
        "{what}: no tokens for {} bytes",
        src.len()
    );
    assert_eq!(tokens[0].lo, 0, "{what}: first token does not start at 0");
    for w in tokens.windows(2) {
        assert_eq!(
            w[0].hi, w[1].lo,
            "{what}: gap/overlap between {:?} and {:?}",
            w[0], w[1]
        );
    }
    assert_eq!(
        tokens.last().unwrap().hi as usize,
        src.len(),
        "{what}: last token does not reach the end"
    );
    let mut prev_line = 1;
    for t in tokens {
        assert!(t.lo <= t.hi, "{what}: negative extent {t:?}");
        assert!(t.line >= prev_line, "{what}: line went backwards at {t:?}");
        prev_line = t.line;
    }
}

const MODES: [Options; 2] = [
    Options {
        short_open_tag: false,
    },
    Options {
        short_open_tag: true,
    },
];

#[test]
fn corpus_is_tiled_exactly() {
    for f in corpus_files() {
        let src = std::fs::read(&f).unwrap();
        for opts in MODES {
            let toks = tokenize(&src, opts);
            check_tiling(&src, &toks, &format!("{} ({opts:?})", f.display()));
            // Concatenation of the token texts reproduces the source byte for byte.
            let mut rebuilt = Vec::with_capacity(src.len());
            for t in &toks {
                rebuilt.extend_from_slice(t.text(&src));
            }
            assert_eq!(rebuilt, src, "{}: concat(tokens) != src", f.display());
        }
    }
}

/// xorshift64*: small, deterministic, dependency-free.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

/// Bytes that steer the scanner: quotes, dollars, braces, tag characters,
/// newlines, control bytes and high bytes, plus a uniformly random byte.
const INTERESTING: &[u8] = b"\"'`$\\{}[]<>?#/*\n\r\t \0\x0b\x7f\x80\xffbB0x_e.-&;=()";

#[test]
fn mutated_corpus_never_panics_and_stays_lossless() {
    let files = corpus_files();
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let picked: Vec<&PathBuf> = files.iter().take(20).collect();
    for f in picked {
        let original = std::fs::read(f).unwrap();
        for i in 0..200 {
            let mut src = original.clone();
            let byte = || 0u8;
            let _ = byte;
            let pick = |rng: &mut Rng| {
                if rng.below(2) == 0 {
                    INTERESTING[rng.below(INTERESTING.len())]
                } else {
                    (rng.next() & 0xff) as u8
                }
            };
            match rng.below(4) {
                0 if !src.is_empty() => {
                    let at = rng.below(src.len());
                    src[at] = pick(&mut rng);
                }
                1 => {
                    let at = rng.below(src.len() + 1);
                    src.insert(at, pick(&mut rng));
                }
                2 if !src.is_empty() => {
                    let at = rng.below(src.len());
                    src.remove(at);
                }
                _ => {
                    // truncate: exercises every "unterminated at EOF" path
                    let at = rng.below(src.len() + 1);
                    src.truncate(at);
                }
            }
            for opts in MODES {
                let toks = tokenize(&src, opts);
                check_tiling(
                    &src,
                    &toks,
                    &format!("{} mutation #{i} ({opts:?})", f.display()),
                );
            }
        }
    }
}
