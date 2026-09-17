//! `tokenize_bytes`: the php-exact tokenizer must be lossless on arbitrary
//! bytes in both `short_open_tag` modes — tokens tile the input contiguously,
//! in order, without gaps, so concatenating their slices rebuilds the source.
//!
//! Built only with `--features tokenizer` until F5 lands
//! `rphp_tokenizer::tokenize(src, Options) -> Vec<RawToken { id, lo, hi, line }>`.
#![no_main]

use libfuzzer_sys::fuzz_target;
use rphp_tokenizer::{tokenize, Options};

fuzz_target!(|data: &[u8]| {
    for short_open_tag in [false, true] {
        let opts = Options {
            short_open_tag,
            ..Default::default()
        };
        let tokens = tokenize(data, opts);

        let mut pos = 0usize;
        let mut rebuilt = Vec::with_capacity(data.len());
        for (i, t) in tokens.iter().enumerate() {
            let (lo, hi) = (t.lo as usize, t.hi as usize);
            assert_eq!(
                lo, pos,
                "token {i} (id {}) starts at {lo}, expected {pos}",
                t.id
            );
            assert!(hi >= lo, "token {i} (id {}) has hi {hi} < lo {lo}", t.id);
            assert!(
                hi <= data.len(),
                "token {i} (id {}) ends past EOF: {hi} > {}",
                t.id,
                data.len()
            );
            rebuilt.extend_from_slice(&data[lo..hi]);
            pos = hi;
        }
        assert_eq!(
            pos,
            data.len(),
            "tokens stop at {pos}, input has {} bytes",
            data.len()
        );
        assert_eq!(
            rebuilt, data,
            "concatenated token slices differ from the input"
        );

        // Line numbers are 1-based and never decrease.
        let mut last_line = 1u64;
        for t in &tokens {
            let line = t.line as u64;
            assert!(line >= 1, "token id {} has line 0", t.id);
            assert!(
                line >= last_line,
                "line numbers went backwards: {line} < {last_line}"
            );
            last_line = line;
        }
    }
});
