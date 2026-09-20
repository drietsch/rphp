//! `highlight_string()` / `highlight_file()` — php's syntax highlighter
//! (`Zend/zend_highlight.c`, the php 8.3+ `<pre><code>` form) over the
//! workspace's own scanner.
//!
//! php colours by token class: comments, strings, the tags and magic
//! constants, everything that carries a value (names, variables, numbers)
//! in `highlight.default`, and every valueless token — keywords, operators,
//! punctuation — in `highlight.keyword`. Whitespace never changes the
//! colour, so it lands inside the running `<span>`; inline HTML is printed
//! in the code block's own colour with no span at all. Tabs become four
//! spaces, `<`, `>` and `&` their entities.

use rphp_runtime::{nf, Ctx, NativeFn, NativeResult};
use rphp_tokenizer::{ids, Options};
use rphp_value::Value;

/// This extension's registry contribution (see `lib.rs`).
pub(crate) static FUNCTIONS: &[NativeFn] = &[
    nf!("highlight_string", 1, Some(2), highlight_string),
    nf!("highlight_file", 1, Some(2), highlight_file),
    nf!("show_source", 1, Some(2), highlight_file),
];

/// The `highlight.*` ini colours.
struct Colors {
    html: String,
    comment: String,
    default: String,
    keyword: String,
    string: String,
}

impl Colors {
    fn of(ctx: &Ctx) -> Colors {
        let get = |k: &str, d: &str| ctx.ini.get(k).filter(|s| !s.is_empty()).unwrap_or(d).to_string();
        Colors {
            html: get("highlight.html", "#000000"),
            comment: get("highlight.comment", "#FF8000"),
            default: get("highlight.default", "#0000BB"),
            keyword: get("highlight.keyword", "#007700"),
            string: get("highlight.string", "#DD0000"),
        }
    }
}

/// The tokens the scanner gives a value (php's zval): coloured as
/// `highlight.default`; any other token reaching the default arm is a
/// keyword or operator.
fn carries_value(id: u16) -> bool {
    matches!(
        id,
        ids::T_STRING
            | ids::T_NAME_QUALIFIED
            | ids::T_NAME_FULLY_QUALIFIED
            | ids::T_NAME_RELATIVE
            | ids::T_VARIABLE
            | ids::T_LNUMBER
            | ids::T_DNUMBER
            | ids::T_NUM_STRING
            | ids::T_STRING_VARNAME
            | ids::T_BAD_CHARACTER
    )
}

/// `zend_html_puts`: entities for `<`, `>`, `&`, four spaces for a tab.
fn html_puts(out: &mut Vec<u8>, text: &[u8]) {
    for &b in text {
        match b {
            b'<' => out.extend_from_slice(b"&lt;"),
            b'>' => out.extend_from_slice(b"&gt;"),
            b'&' => out.extend_from_slice(b"&amp;"),
            b'\t' => out.extend_from_slice(b"    "),
            other => out.push(other),
        }
    }
}

/// Highlight `src` into html.
pub(crate) fn highlight(ctx: &Ctx, src: &[u8]) -> Vec<u8> {
    let c = Colors::of(ctx);
    let tokens = rphp_tokenizer::tokenize(
        src,
        Options {
            short_open_tag: ctx.ini.bool("short_open_tag"),
        },
    );
    let mut out = Vec::with_capacity(src.len() * 2);
    out.extend_from_slice(format!("<pre><code style=\"color: {}\">", c.html).as_bytes());
    let mut last: &str = &c.html;
    for t in &tokens {
        let text = &src[t.lo as usize..t.hi as usize];
        let next: &str = match t.id {
            ids::T_INLINE_HTML => &c.html,
            ids::T_COMMENT | ids::T_DOC_COMMENT => &c.comment,
            ids::T_OPEN_TAG
            | ids::T_OPEN_TAG_WITH_ECHO
            | ids::T_CLOSE_TAG
            | ids::T_LINE
            | ids::T_FILE
            | ids::T_DIR
            | ids::T_TRAIT_C
            | ids::T_METHOD_C
            | ids::T_FUNC_C
            | ids::T_NS_C
            | ids::T_CLASS_C
            | ids::T_PROPERTY_C => &c.default,
            ids::T_ENCAPSED_AND_WHITESPACE | ids::T_CONSTANT_ENCAPSED_STRING => &c.string,
            b if b == u16::from(b'"') => &c.string,
            ids::T_WHITESPACE => {
                html_puts(&mut out, text);
                continue;
            }
            id if carries_value(id) => &c.default,
            _ => &c.keyword,
        };
        if last != next {
            if last != c.html {
                out.extend_from_slice(b"</span>");
            }
            last = next;
            if last != c.html {
                out.extend_from_slice(format!("<span style=\"color: {last}\">").as_bytes());
            }
        }
        html_puts(&mut out, text);
    }
    if last != c.html {
        out.extend_from_slice(b"</span>");
    }
    out.extend_from_slice(b"</code></pre>");
    out
}

/// `highlight_string(string $string, bool $return = false): string|bool`
fn highlight_string(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let src = args[0].to_php_bytes();
    let html = highlight(ctx, &src);
    if args.get(1).is_some_and(|v| v.deref().to_bool()) {
        return Ok(Value::string(&html));
    }
    ctx.echo(&html);
    Ok(Value::Bool(true))
}

/// `highlight_file(string $filename, bool $return = false): string|bool`
fn highlight_file(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let path = crate::filestat::arg_path(ctx, &args[0]);
    let shown = String::from_utf8_lossy(&args[0].to_php_bytes()).into_owned();
    let src = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) => {
            ctx.warn(&format!(
                "highlight_file({shown}): Failed to open stream: {}",
                crate::filestat::io_text(&e)
            ))?;
            ctx.warn(&format!("Failed opening '{shown}' for highlighting"))?;
            return Ok(Value::Bool(false));
        }
    };
    let html = highlight(ctx, &src);
    if args.get(1).is_some_and(|v| v.deref().to_bool()) {
        return Ok(Value::string(&html));
    }
    ctx.echo(&html);
    Ok(Value::Bool(true))
}

/// The `highlight.*` ini entries.
pub(crate) fn register_ini(r: &mut rphp_runtime::Registry) {
    for (k, v) in [
        ("highlight.html", "#000000"),
        ("highlight.comment", "#FF8000"),
        ("highlight.default", "#0000BB"),
        ("highlight.keyword", "#007700"),
        ("highlight.string", "#DD0000"),
    ] {
        r.interp().ini.register(k, v);
    }
}
