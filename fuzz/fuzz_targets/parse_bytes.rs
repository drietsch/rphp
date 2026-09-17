//! `parse_bytes`: arbitrary bytes through lexer + parser must neither panic
//! nor hang (libFuzzer's `-timeout` catches the latter), and every diagnostic
//! must render against the source map (spans stay inside the file).
#![no_main]

use libfuzzer_sys::fuzz_target;
use rphp_intern::Interner;
use rphp_source::SourceMap;

fuzz_target!(|data: &[u8]| {
    let mut sources = SourceMap::new();
    let id = sources.add("fuzz.php", data.to_vec());
    let mut interner = Interner::new();

    // The lexer on its own: token spans must lie within the input.
    let lexed = rphp_lexer::lex(data, id, &mut interner);
    for tok in &lexed.tokens {
        assert!(tok.span.lo <= tok.span.hi, "inverted span {:?}", tok.span);
        assert!(
            tok.span.hi as usize <= data.len(),
            "span past EOF {:?}",
            tok.span
        );
    }
    for d in &lexed.diagnostics {
        let _ = d.render(&sources);
    }

    // The parser (lexes again internally); the program may be partial but the
    // diagnostics must all be renderable.
    let (_program, diags) = rphp_parser::parse(data, id, &mut interner);
    for d in &diags {
        let _ = d.render(&sources);
    }
});
