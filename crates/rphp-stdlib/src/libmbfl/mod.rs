//! A pure-Rust port of the parts of php-src's `libmbfl` (the "streamable
//! kanji code filter and converter" behind `ext/mbstring`) that `mbstring.rs`
//! and `iconv.rs` need (plan S8, ADR-032: no ICU, no libiconv).
//!
//! The model is php 8.5's: every text encoding decodes to a stream of
//! `u32` "wchars" (Unicode code points, or [`BAD_INPUT`] for an illegal byte
//! sequence) and encodes from such a stream into a [`ConvertBuf`] whose
//! `error_mode`/`replacement_char` implement `mb_substitute_character`
//! (`none` / a code point / `long` / `entity`). Code points the target
//! cannot represent go through [`illegal_output`], exactly as
//! `mb_illegal_output` in `mbfl_convert.c`.
//!
//! Codecs written here from the C sources: UTF-8 (php's validation rules),
//! UTF-16/32 with BOM detection, UCS-2/4, UTF-7 and UTF7-IMAP, the
//! single-byte charsets (ISO-8859-x, Windows-125x, KOI8, CP850/866,
//! ArmSCII-8), the byte codecs (BASE64, Quoted-Printable, UUENCODE,
//! HTML-ENTITIES, 7bit, 8bit) and the ISO-2022-JP/KR + HZ escape-sequence
//! layers, plus the whole Japanese family (Shift_JIS, CP932, EUC-JP,
//! eucJP-win, CP51932, ISO-2022-JP, JIS) over php's own JIS tables. The
//! Chinese and Korean double-byte tables (EUC-KR/UHC, GBK/GB2312/GB18030,
//! Big5) come from `encoding_rs`; the documented divergences from libmbfl's
//! own tables are listed in `COVERAGE.md`.

pub mod bytecodecs;
pub mod case;
pub mod cjk;
pub mod detect;
pub mod jis;
pub mod singlebyte;
pub mod tables;
pub mod unicode;
pub mod utf7;

/// `MBFL_BAD_INPUT`: the wchar a decoder emits for an illegal byte sequence.
pub const BAD_INPUT: u32 = 0xFFFF_FFFF;

/// `MBFL_ENCTYPE_SBCS`: one byte per code point.
pub const FLAG_SBCS: u32 = 0x1;
/// `MBFL_ENCTYPE_WCS2`: two bytes per code point.
pub const FLAG_WCS2: u32 = 0x2;
/// `MBFL_ENCTYPE_WCS4`: four bytes per code point.
pub const FLAG_WCS4: u32 = 0x4;
/// `MBFL_ENCTYPE_GL_UNSAFE`: bytes below 0x80 may be part of a multibyte
/// sequence (or the encoding is stateful).
pub const FLAG_GL_UNSAFE: u32 = 0x4000;

/// `MBFL_WCSPLANE_UTF32MAX`.
pub const UTF32_MAX: u32 = 0x11_0000;
/// `MBFL_WCSPLANE_UCS2MAX`.
pub const UCS2_MAX: u32 = 0x1_0000;

/// How an encoder reports a wchar it cannot represent
/// (`MBFL_OUTPUTFILTER_ILLEGAL_MODE_*`, i.e. `mb_substitute_character`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ErrorMode {
    /// Drop it.
    None,
    /// Emit the substitute character.
    Char,
    /// Emit `U+XXXX` (or the substitute character for bad input).
    Long,
    /// Emit `&#xXXXX;` (or the substitute character for bad input).
    Entity,
    /// Internal: emit the byte `0xFF`, which is illegal in UTF-8, so that
    /// error markers can never match real text when searching.
    BadUtf8,
}

/// `mb_convert_buf`: the output of an encoder plus its conversion state,
/// error count and substitution settings.
#[derive(Clone, Debug)]
pub struct ConvertBuf {
    /// The encoded bytes.
    pub out: Vec<u8>,
    /// Encoder state carried between calls (escape-sequence mode, pending
    /// bits, line position, …); `0` at the start of a string.
    pub state: u32,
    /// Number of illegal wchars seen (`mb_get_info('illegal_chars')`).
    pub errors: u32,
    /// The substitute character for [`ErrorMode::Char`].
    pub replacement_char: u32,
    /// The substitution mode.
    pub error_mode: ErrorMode,
}

impl ConvertBuf {
    /// `mb_convert_buf_init`.
    pub fn new(replacement_char: u32, error_mode: ErrorMode) -> ConvertBuf {
        ConvertBuf { out: Vec::new(), state: 0, errors: 0, replacement_char, error_mode }
    }

    /// A buffer with php's defaults (`?`, [`ErrorMode::Char`]).
    pub fn default_subst() -> ConvertBuf {
        ConvertBuf::new(b'?' as u32, ErrorMode::Char)
    }

    /// The bytes written so far.
    pub fn len(&self) -> usize {
        self.out.len()
    }

    /// Whether nothing was written yet.
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.out.is_empty()
    }

    /// `mb_convert_buf_reset`: truncate the output to `len` bytes.
    pub fn reset(&mut self, len: usize) {
        self.out.truncate(len);
    }

}

/// Which libmbfl encoding this is (`enum mbfl_no_encoding`), in php's
/// enumeration order — several checks in `mbstring.c` are range tests on
/// this order.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
#[allow(missing_docs)]
pub enum Id {
    Pass,
    Base64,
    Uuencode,
    HtmlEnt,
    QPrint,
    SevenBit,
    EightBit,
    CharsetMin,
    Ucs4,
    Ucs4Be,
    Ucs4Le,
    Ucs2,
    Ucs2Be,
    Ucs2Le,
    Utf32,
    Utf32Be,
    Utf32Le,
    Utf16,
    Utf16Be,
    Utf16Le,
    Utf8,
    Utf8Docomo,
    Utf8KddiA,
    Utf8KddiB,
    Utf8Sb,
    Utf7,
    Utf7Imap,
    Ascii,
    EucJp,
    EucJp2004,
    Sjis,
    EucJpWin,
    SjisDocomo,
    SjisKddi,
    SjisSb,
    SjisMac,
    Sjis2004,
    Cp932,
    SjisWin,
    Cp51932,
    Jis,
    Iso2022Jp,
    Iso2022Jp2004,
    Iso2022JpKddi,
    Iso2022JpMs,
    Gb18030,
    Gb18030_2022,
    Cp1252,
    Cp1254,
    Iso8859_1,
    Iso8859_2,
    Iso8859_3,
    Iso8859_4,
    Iso8859_5,
    Iso8859_6,
    Iso8859_7,
    Iso8859_8,
    Iso8859_9,
    Iso8859_10,
    Iso8859_13,
    Iso8859_14,
    Iso8859_15,
    EucCn,
    Cp936,
    EucTw,
    Big5,
    Cp950,
    EucKr,
    Iso2022Kr,
    Uhc,
    Hz,
    Cp1251,
    Cp866,
    Koi8R,
    Koi8U,
    Iso8859_16,
    ArmSCII8,
    Cp850,
    Cp50220,
    Cp50221,
    Cp50222,
}

/// The implementation behind an [`Encoding`].
#[derive(Clone, Copy, Debug)]
pub enum Codec {
    /// `pass`: bytes in, bytes out.
    Pass,
    /// One of the byte codecs in [`bytecodecs`].
    Base64,
    /// See [`bytecodecs`].
    Uuencode,
    /// See [`bytecodecs`].
    HtmlEnt,
    /// See [`bytecodecs`].
    QPrint,
    /// See [`bytecodecs`].
    SevenBit,
    /// See [`bytecodecs`].
    EightBit,
    /// UCS-4 with BOM detection (encodes big-endian).
    Ucs4,
    /// UCS-4BE.
    Ucs4Be,
    /// UCS-4LE.
    Ucs4Le,
    /// UCS-2 with BOM detection (encodes big-endian).
    Ucs2,
    /// UCS-2BE.
    Ucs2Be,
    /// UCS-2LE.
    Ucs2Le,
    /// UTF-32 with BOM detection (encodes big-endian).
    Utf32,
    /// UTF-32BE.
    Utf32Be,
    /// UTF-32LE.
    Utf32Le,
    /// UTF-16 with BOM detection (encodes big-endian).
    Utf16,
    /// UTF-16BE.
    Utf16Be,
    /// UTF-16LE.
    Utf16Le,
    /// UTF-8 (php's strict validation).
    Utf8,
    /// UTF-7 (RFC 2152).
    Utf7,
    /// UTF7-IMAP (RFC 3501 modified UTF-7).
    Utf7Imap,
    /// US-ASCII.
    Ascii,
    /// ISO-8859-1: bytes are code points.
    Latin1,
    /// A table-driven single-byte charset: bytes below `min` are ASCII,
    /// the rest map through `table` (`0` = unmapped).
    SingleByte {
        /// First byte covered by the table.
        min: u8,
        /// Code point per byte from `min` upward.
        table: &'static [u16],
    },
    /// Windows-1252 (php's special-cased C1 handling).
    Cp1252,
    /// ArmSCII-8 (php's special-cased punctuation).
    ArmSCII8,
    /// A stateless double-byte charset from `encoding_rs`.
    Rs(&'static encoding_rs::Encoding),
    /// EUC-CN: the GB 2312 subset of the GBK tables (lead `0xA1..=0xF7`,
    /// trail `0xA1..=0xFE`).
    EucCn,
    /// Shift_JIS (JIS X 0208 + JIS X 0201 kana).
    Sjis,
    /// CP932 (Windows-31J); `win` selects the `SJIS-win` encoder variant.
    Cp932 {
        /// Whether this is `SJIS-win`.
        win: bool,
    },
    /// EUC-JP (JIS X 0208 + 0212 + 0201 kana).
    EucJp,
    /// eucJP-win (EUC-JP with the CP932 extensions).
    EucJpWin,
    /// CP51932 (Microsoft's EUC-JP).
    Cp51932,
    /// ISO-2022-JP family: the escape-sequence layer over the JIS tables.
    /// `jis` selects the `JIS` variant (JIS X 0201 kana via `ESC ( I`).
    Iso2022Jp {
        /// Whether this is the `JIS` variant.
        jis: bool,
    },
    /// ISO-2022-KR over EUC-KR tables.
    Iso2022Kr,
    /// HZ-GB-2312 over GB2312 tables.
    Hz,
    /// A name php recognizes but rphp does not convert (EUC-TW).
    Unsupported,
}

/// A libmbfl text encoding: its names and the codec that implements it.
#[derive(Debug)]
pub struct Encoding {
    /// `enum mbfl_no_encoding`.
    pub id: Id,
    /// The canonical name (`mb_list_encodings`).
    pub name: &'static str,
    /// The preferred MIME charset name (`mb_preferred_mime_name`).
    pub mime_name: Option<&'static str>,
    /// Alternative names (`mb_encoding_aliases`).
    pub aliases: &'static [&'static str],
    /// Byte length of a character keyed by its first byte, for encodings
    /// where that is well defined (`mb_strcut`, `mb_str_split`).
    pub mblen_table: Option<&'static [u8; 256]>,
    /// `MBFL_ENCTYPE_*` flags.
    pub flags: u32,
    /// The implementation.
    pub codec: Codec,
}

impl PartialEq for Encoding {
    fn eq(&self, other: &Encoding) -> bool {
        self.id == other.id
    }
}

impl Encoding {
    /// Whether every code point has the same byte width (`SBCS`/`WCS2`/`WCS4`
    /// flag): the width, or `None`.
    pub fn fixed_width(&self) -> Option<usize> {
        match self.flags & (FLAG_SBCS | FLAG_WCS2 | FLAG_WCS4) {
            FLAG_SBCS => Some(1),
            FLAG_WCS2 => Some(2),
            FLAG_WCS4 => Some(4),
            _ => None,
        }
    }

    /// `no_encoding <= mbfl_no_encoding_charset_min`: not a text encoding at
    /// all (never returned by `mb_detect_encoding`).
    pub fn is_byte_encoding(&self) -> bool {
        self.id <= Id::CharsetMin
    }

    /// `php_mb_is_unsupported_no_encoding`: encodings `mb_ord`/`mb_chr`
    /// refuse (byte codecs, UTF-7, the ISO-2022-JP family).
    pub fn is_unsupported_for_ord_chr(&self) -> bool {
        self.id <= Id::QPrint
            || (self.id >= Id::Utf7 && self.id <= Id::Utf7Imap)
            || (self.id >= Id::Jis && self.id <= Id::Iso2022JpMs)
            || (self.id >= Id::Cp50220 && self.id <= Id::Cp50222)
    }

    /// Whether this is one of the UTF-8 variants (`php_mb_is_no_encoding_utf8`).
    pub fn is_utf8(&self) -> bool {
        self.id >= Id::Utf8 && self.id <= Id::Utf8Sb
    }

    /// Decode a whole byte string into wchars ([`BAD_INPUT`] marks illegal
    /// sequences; a truncated trailing sequence is illegal).
    pub fn decode(&self, input: &[u8]) -> Vec<u32> {
        let mut out = Vec::with_capacity(input.len());
        self.decode_into(input, &mut out);
        out
    }

    /// [`Encoding::decode`] appending to `out`.
    pub fn decode_into(&self, input: &[u8], out: &mut Vec<u32>) {
        match self.codec {
            Codec::Pass | Codec::EightBit | Codec::Latin1 => out.extend(input.iter().map(|&b| b as u32)),
            Codec::Base64 => bytecodecs::decode_base64(input, out),
            Codec::Uuencode => bytecodecs::decode_uuencode(input, out),
            Codec::HtmlEnt => bytecodecs::decode_htmlent(input, out),
            Codec::QPrint => bytecodecs::decode_qprint(input, out),
            Codec::SevenBit | Codec::Ascii => {
                out.extend(input.iter().map(|&b| if b < 0x80 { b as u32 } else { BAD_INPUT }))
            }
            Codec::Ucs4 => unicode::decode_ucs4(input, out),
            Codec::Ucs4Be => unicode::decode_ucs4be(input, out),
            Codec::Ucs4Le => unicode::decode_ucs4le(input, out),
            Codec::Ucs2 => unicode::decode_ucs2(input, out),
            Codec::Ucs2Be => unicode::decode_ucs2be(input, out),
            Codec::Ucs2Le => unicode::decode_ucs2le(input, out),
            Codec::Utf32 => unicode::decode_utf32(input, out),
            Codec::Utf32Be => unicode::decode_utf32be(input, out),
            Codec::Utf32Le => unicode::decode_utf32le(input, out),
            Codec::Utf16 => unicode::decode_utf16(input, out),
            Codec::Utf16Be => unicode::decode_utf16be(input, out),
            Codec::Utf16Le => unicode::decode_utf16le(input, out),
            Codec::Utf8 => unicode::decode_utf8(input, out),
            Codec::Utf7 => utf7::decode_utf7(input, out),
            Codec::Utf7Imap => utf7::decode_utf7imap(input, out),
            Codec::SingleByte { min, table } => singlebyte::decode_table(input, min, table, out),
            Codec::Cp1252 => singlebyte::decode_cp1252(input, out),
            Codec::ArmSCII8 => singlebyte::decode_armscii8(input, out),
            Codec::Rs(enc) => cjk::decode_rs(enc, input, out),
            Codec::EucCn => cjk::decode_euccn(input, out),
            Codec::Sjis => jis::decode_sjis(input, out),
            Codec::Cp932 { .. } => jis::decode_cp932(input, out),
            Codec::EucJp => jis::decode_eucjp(input, out),
            Codec::EucJpWin => jis::decode_eucjpwin(input, false, out),
            Codec::Cp51932 => jis::decode_eucjpwin(input, true, out),
            Codec::Iso2022Jp { .. } => jis::decode_iso2022jp(input, out),
            Codec::Iso2022Kr => cjk::decode_iso2022kr(input, out),
            Codec::Hz => cjk::decode_hz(input, out),
            Codec::Unsupported => out.extend(input.iter().map(|_| BAD_INPUT)),
        }
    }

    /// Encode wchars into `buf` (`from_wchar`). `end` flushes any pending
    /// state (closing escape sequences, Base64 padding, …).
    pub fn encode(&self, input: &[u32], buf: &mut ConvertBuf, end: bool) {
        match self.codec {
            Codec::Pass | Codec::EightBit => bytecodecs::encode_8bit(self, input, buf),
            Codec::Base64 => bytecodecs::encode_base64(input, buf, end),
            Codec::Uuencode => bytecodecs::encode_uuencode(input, buf, end),
            Codec::HtmlEnt => bytecodecs::encode_htmlent(input, buf),
            Codec::QPrint => bytecodecs::encode_qprint(input, buf),
            Codec::SevenBit | Codec::Ascii => bytecodecs::encode_7bit(self, input, buf),
            Codec::Ucs4 | Codec::Ucs4Be => unicode::encode_ucs4be(self, input, buf),
            Codec::Ucs4Le => unicode::encode_ucs4le(self, input, buf),
            Codec::Ucs2 | Codec::Ucs2Be => unicode::encode_ucs2be(self, input, buf),
            Codec::Ucs2Le => unicode::encode_ucs2le(self, input, buf),
            Codec::Utf32 | Codec::Utf32Be => unicode::encode_utf32be(self, input, buf),
            Codec::Utf32Le => unicode::encode_utf32le(self, input, buf),
            Codec::Utf16 | Codec::Utf16Be => unicode::encode_utf16be(self, input, buf),
            Codec::Utf16Le => unicode::encode_utf16le(self, input, buf),
            Codec::Utf8 => unicode::encode_utf8(self, input, buf),
            Codec::Utf7 => utf7::encode_utf7(self, input, buf, end),
            Codec::Utf7Imap => utf7::encode_utf7imap(self, input, buf, end),
            Codec::Latin1 => singlebyte::encode_latin1(self, input, buf),
            Codec::SingleByte { min, table } => singlebyte::encode_table(self, min, table, input, buf),
            Codec::Cp1252 => singlebyte::encode_cp1252(self, input, buf),
            Codec::ArmSCII8 => singlebyte::encode_armscii8(self, input, buf),
            Codec::Rs(enc) => cjk::encode_rs(self, enc, input, buf),
            Codec::EucCn => cjk::encode_euccn(self, input, buf),
            Codec::Sjis => jis::encode_sjis(self, input, buf),
            Codec::Cp932 { win } => jis::encode_cp932(self, win, input, buf),
            Codec::EucJp => jis::encode_eucjp(self, input, buf),
            Codec::EucJpWin => jis::encode_eucjpwin(self, input, buf),
            Codec::Cp51932 => jis::encode_cp51932(self, input, buf),
            Codec::Iso2022Jp { jis } => jis::encode_iso2022jp(self, jis, input, buf, end),
            Codec::Iso2022Kr => cjk::encode_iso2022kr(self, input, buf, end),
            Codec::Hz => cjk::encode_hz(self, input, buf, end),
            Codec::Unsupported => {
                for &w in input {
                    illegal_output(w, self, buf);
                }
            }
        }
    }

    /// `php_mb_check_encoding`: whether `input` is well-formed. Encodings
    /// with a dedicated validator (UTF-7, UTF7-IMAP, ISO-2022-JP, JIS) use
    /// it; the rest decode and look for [`BAD_INPUT`].
    pub fn check(&self, input: &[u8]) -> bool {
        match self.codec {
            Codec::Utf7 => utf7::check_utf7(input),
            Codec::Utf7Imap => utf7::check_utf7imap(input),
            Codec::Utf8 => unicode::check_utf8(input),
            Codec::Iso2022Jp { jis } => jis::check_iso2022jp(input, jis),
            _ => {
                let mut out = Vec::with_capacity(input.len());
                self.decode_into(input, &mut out);
                !out.contains(&BAD_INPUT)
            }
        }
    }

    /// The encoding-specific `cut` (`mb_strcut` for UTF-8, UTF-16 and
    /// GB18030), if there is one.
    pub fn cut(&self, s: &[u8], from: usize, len: usize) -> Option<Vec<u8>> {
        match self.codec {
            Codec::Utf8 => Some(unicode::cut_utf8(s, from, len)),
            Codec::Utf16 => Some(unicode::cut_utf16(s, from, len)),
            Codec::Utf16Be => Some(unicode::cut_utf16be(s, from, len)),
            Codec::Utf16Le => Some(unicode::cut_utf16le(s, from, len)),
            Codec::Rs(enc) if std::ptr::eq(enc, encoding_rs::GB18030) => Some(cjk::cut_gb18030(s, from, len)),
            _ => None,
        }
    }

    /// Convert bytes from this encoding to `to` (`mb_fast_convert`).
    pub fn convert_to(&self, input: &[u8], to: &Encoding, buf: &mut ConvertBuf) {
        let (from, to) = reroute_byte_codecs(self, to);
        let wchars = from.decode(input);
        to.encode(&wchars, buf, true);
    }
}

/// `mbfl_convert_filter_get_vtbl`'s special case: converting *to* BASE64 or
/// QPrint treats the input as raw bytes, converting *from* BASE64, QPrint or
/// UUENCODE produces raw bytes.
pub fn reroute_byte_codecs<'a>(from: &'a Encoding, to: &'a Encoding) -> (&'a Encoding, &'a Encoding) {
    if to.id == Id::Base64 || to.id == Id::QPrint {
        (&EIGHT_BIT, to)
    } else if from.id == Id::Base64 || from.id == Id::QPrint || from.id == Id::Uuencode {
        (from, &EIGHT_BIT)
    } else {
        (from, to)
    }
}

/// `mb_fast_convert`: transcode `input` with the given substitution
/// settings, returning the bytes and the number of illegal characters.
pub fn convert(input: &[u8], from: &Encoding, to: &Encoding, replacement_char: u32, mode: ErrorMode) -> (Vec<u8>, u32) {
    let mut buf = ConvertBuf::new(replacement_char, mode);
    from.convert_to(input, to, &mut buf);
    let errors = buf.errors;
    (buf.out, errors)
}

/// `convert_cp_to_hex`: upper-case hex digits of `cp` without leading zeros
/// (`0` for zero).
fn push_hex(cp: u32, out: &mut Vec<u32>) {
    let mut nonzero = false;
    let mut shift = 28i32;
    while shift >= 0 {
        let n = (cp >> shift) & 0xF;
        if n != 0 || nonzero {
            nonzero = true;
            out.push(b"0123456789ABCDEF"[n as usize] as u32);
        }
        shift -= 4;
    }
    if !nonzero {
        out.push(b'0' as u32);
    }
}

/// `mb_illegal_marker`: the wchars that stand in for `bad` under `mode`.
fn illegal_marker(bad: u32, mode: ErrorMode, replacement: u32) -> Vec<u32> {
    let mut out = Vec::with_capacity(12);
    if bad == BAD_INPUT {
        if mode != ErrorMode::None {
            out.push(replacement);
        }
    } else {
        match mode {
            ErrorMode::Char => out.push(replacement),
            ErrorMode::Long => {
                out.push(b'U' as u32);
                out.push(b'+' as u32);
                push_hex(bad, &mut out);
            }
            ErrorMode::Entity => {
                out.push(b'&' as u32);
                out.push(b'#' as u32);
                out.push(b'x' as u32);
                push_hex(bad, &mut out);
                out.push(b';' as u32);
            }
            ErrorMode::None | ErrorMode::BadUtf8 => {}
        }
    }
    out
}

/// `mb_illegal_output`: record an illegal wchar and emit its marker through
/// the same encoder (with the error mode downgraded so a marker the target
/// cannot represent either does not recurse forever).
pub fn illegal_output(bad: u32, enc: &Encoding, buf: &mut ConvertBuf) {
    buf.errors += 1;
    if buf.error_mode == ErrorMode::BadUtf8 {
        buf.out.push(0xFF);
        return;
    }
    let marker = illegal_marker(bad, buf.error_mode, buf.replacement_char);
    let repl = buf.replacement_char;
    let mode = buf.error_mode;
    if mode == ErrorMode::Char && repl != b'?' as u32 {
        buf.replacement_char = b'?' as u32;
    } else {
        buf.error_mode = ErrorMode::None;
    }
    enc.encode(&marker, buf, false);
    buf.replacement_char = repl;
    buf.error_mode = mode;
}

// ---- the registry ------------------------------------------------------------

macro_rules! enc {
    ($id:ident, $name:literal, $mime:expr, $aliases:expr, $mblen:expr, $flags:expr, $codec:expr) => {
        Encoding { id: Id::$id, name: $name, mime_name: $mime, aliases: $aliases, mblen_table: $mblen, flags: $flags, codec: $codec }
    };
}

/// `pass`: only reachable through `mb_http_output`/`mb_http_input`.
pub static PASS: Encoding = enc!(Pass, "pass", None, &[], None, 0, Codec::Pass);
/// `8bit`.
pub static EIGHT_BIT: Encoding = enc!(EightBit, "8bit", Some("8bit"), &["binary"], None, FLAG_SBCS, Codec::EightBit);
/// `UTF-8`.
pub static UTF8: Encoding = enc!(Utf8, "UTF-8", Some("UTF-8"), &["utf8"], Some(&unicode::MBLEN_UTF8), 0, Codec::Utf8);

use tables::singlebyte as sb;

/// Every encoding, in `mbfl_encoding_ptr_list` order (`mb_list_encodings`).
pub static ENCODINGS: &[Encoding] = &[
    enc!(Base64, "BASE64", Some("BASE64"), &[], None, FLAG_GL_UNSAFE, Codec::Base64),
    enc!(Uuencode, "UUENCODE", Some("x-uuencode"), &[], None, FLAG_SBCS, Codec::Uuencode),
    enc!(HtmlEnt, "HTML-ENTITIES", Some("HTML-ENTITIES"), &["HTML", "html"], None, FLAG_GL_UNSAFE, Codec::HtmlEnt),
    enc!(QPrint, "Quoted-Printable", Some("Quoted-Printable"), &["qprint"], None, FLAG_GL_UNSAFE, Codec::QPrint),
    enc!(SevenBit, "7bit", Some("7bit"), &[], None, FLAG_SBCS, Codec::SevenBit),
    enc!(EightBit, "8bit", Some("8bit"), &["binary"], None, FLAG_SBCS, Codec::EightBit),
    enc!(Ucs4, "UCS-4", Some("UCS-4"), &["ISO-10646-UCS-4", "UCS4"], None, FLAG_WCS4, Codec::Ucs4),
    enc!(Ucs4Be, "UCS-4BE", Some("UCS-4BE"), &["byte4be"], None, FLAG_WCS4, Codec::Ucs4Be),
    enc!(Ucs4Le, "UCS-4LE", Some("UCS-4LE"), &["byte4le"], None, FLAG_WCS4, Codec::Ucs4Le),
    enc!(Ucs2, "UCS-2", Some("UCS-2"), &["ISO-10646-UCS-2", "UCS2", "UNICODE"], None, FLAG_WCS2, Codec::Ucs2),
    enc!(Ucs2Be, "UCS-2BE", Some("UCS-2BE"), &["byte2be"], None, FLAG_WCS2, Codec::Ucs2Be),
    enc!(Ucs2Le, "UCS-2LE", Some("UCS-2LE"), &["byte2le"], None, FLAG_WCS2, Codec::Ucs2Le),
    enc!(Utf32, "UTF-32", Some("UTF-32"), &["utf32"], None, FLAG_WCS4, Codec::Utf32),
    enc!(Utf32Be, "UTF-32BE", Some("UTF-32BE"), &[], None, FLAG_WCS4, Codec::Utf32Be),
    enc!(Utf32Le, "UTF-32LE", Some("UTF-32LE"), &[], None, FLAG_WCS4, Codec::Utf32Le),
    enc!(Utf16, "UTF-16", Some("UTF-16"), &["utf16"], None, 0, Codec::Utf16),
    enc!(Utf16Be, "UTF-16BE", Some("UTF-16BE"), &[], None, 0, Codec::Utf16Be),
    enc!(Utf16Le, "UTF-16LE", Some("UTF-16LE"), &[], None, 0, Codec::Utf16Le),
    enc!(Utf8, "UTF-8", Some("UTF-8"), &["utf8"], Some(&unicode::MBLEN_UTF8), 0, Codec::Utf8),
    enc!(Utf7, "UTF-7", Some("UTF-7"), &["utf7"], None, FLAG_GL_UNSAFE, Codec::Utf7),
    enc!(Utf7Imap, "UTF7-IMAP", None, &["mUTF-7"], None, 0, Codec::Utf7Imap),
    enc!(
        Ascii,
        "ASCII",
        Some("US-ASCII"),
        &["ANSI_X3.4-1968", "iso-ir-6", "ANSI_X3.4-1986", "ISO_646.irv:1991", "US-ASCII", "ISO646-US", "us", "IBM367", "IBM-367", "cp367", "csASCII"],
        None,
        FLAG_SBCS,
        Codec::Ascii
    ),
    enc!(EucJp, "EUC-JP", Some("EUC-JP"), &["EUC", "EUC_JP", "eucJP", "x-euc-jp"], Some(&cjk::MBLEN_EUCJP), 0, Codec::EucJp),
    enc!(Sjis, "SJIS", Some("Shift_JIS"), &["x-sjis", "SHIFT-JIS"], Some(&cjk::MBLEN_SJIS), FLAG_GL_UNSAFE, Codec::Sjis),
    enc!(EucJpWin, "eucJP-win", Some("EUC-JP"), &["eucJP-open", "eucJP-ms"], Some(&cjk::MBLEN_EUCJP), 0, Codec::EucJpWin),
    enc!(EucJp2004, "EUC-JP-2004", Some("EUC-JP"), &["EUC_JP-2004"], Some(&cjk::MBLEN_EUCJP), 0, Codec::EucJp),
    enc!(SjisDocomo, "SJIS-Mobile#DOCOMO", Some("Shift_JIS"), &["SJIS-DOCOMO", "shift_jis-imode", "x-sjis-emoji-docomo"], Some(&cjk::MBLEN_SJIS_MOBILE), FLAG_GL_UNSAFE, Codec::Cp932 { win: false }),
    enc!(SjisKddi, "SJIS-Mobile#KDDI", Some("Shift_JIS"), &["SJIS-KDDI", "shift_jis-kddi", "x-sjis-emoji-kddi"], Some(&cjk::MBLEN_SJIS_MOBILE), FLAG_GL_UNSAFE, Codec::Cp932 { win: false }),
    enc!(SjisSb, "SJIS-Mobile#SOFTBANK", Some("Shift_JIS"), &["SJIS-SOFTBANK", "shift_jis-softbank", "x-sjis-emoji-softbank"], Some(&cjk::MBLEN_SJIS_MOBILE), FLAG_GL_UNSAFE, Codec::Cp932 { win: false }),
    enc!(SjisMac, "SJIS-mac", Some("Shift_JIS"), &["MacJapanese", "x-Mac-Japanese"], Some(&cjk::MBLEN_SJIS_MAC), FLAG_GL_UNSAFE, Codec::Sjis),
    enc!(Sjis2004, "SJIS-2004", Some("Shift_JIS"), &["SJIS2004", "Shift_JIS-2004"], Some(&cjk::MBLEN_SJIS_MOBILE), FLAG_GL_UNSAFE, Codec::Sjis),
    enc!(Utf8Docomo, "UTF-8-Mobile#DOCOMO", Some("UTF-8"), &["UTF-8-DOCOMO", "UTF8-DOCOMO"], Some(&unicode::MBLEN_UTF8), 0, Codec::Utf8),
    enc!(Utf8KddiA, "UTF-8-Mobile#KDDI-A", Some("UTF-8"), &[], Some(&unicode::MBLEN_UTF8), 0, Codec::Utf8),
    enc!(Utf8KddiB, "UTF-8-Mobile#KDDI-B", Some("UTF-8"), &["UTF-8-Mobile#KDDI", "UTF-8-KDDI", "UTF8-KDDI"], Some(&unicode::MBLEN_UTF8), 0, Codec::Utf8),
    enc!(Utf8Sb, "UTF-8-Mobile#SOFTBANK", Some("UTF-8"), &["UTF-8-SOFTBANK", "UTF8-SOFTBANK"], Some(&unicode::MBLEN_UTF8), 0, Codec::Utf8),
    enc!(Cp932, "CP932", Some("Shift_JIS"), &["MS932", "Windows-31J", "MS_Kanji"], Some(&cjk::MBLEN_SJISWIN), FLAG_GL_UNSAFE, Codec::Cp932 { win: false }),
    enc!(SjisWin, "SJIS-win", Some("Shift_JIS"), &["SJIS-ms", "SJIS-open"], Some(&cjk::MBLEN_SJISWIN), FLAG_GL_UNSAFE, Codec::Cp932 { win: true }),
    enc!(Cp51932, "CP51932", Some("CP51932"), &["cp51932"], Some(&cjk::MBLEN_EUCJP), 0, Codec::Cp51932),
    enc!(Jis, "JIS", Some("ISO-2022-JP"), &[], None, FLAG_GL_UNSAFE, Codec::Iso2022Jp { jis: true }),
    enc!(Iso2022Jp, "ISO-2022-JP", Some("ISO-2022-JP"), &[], None, FLAG_GL_UNSAFE, Codec::Iso2022Jp { jis: false }),
    enc!(Iso2022JpMs, "ISO-2022-JP-MS", Some("ISO-2022-JP"), &["ISO2022JPMS"], None, FLAG_GL_UNSAFE, Codec::Iso2022Jp { jis: false }),
    enc!(Gb18030, "GB18030", Some("GB18030"), &["gb-18030", "gb-18030-2000"], None, FLAG_GL_UNSAFE, Codec::Rs(encoding_rs::GB18030)),
    enc!(Gb18030_2022, "GB18030-2022", Some("GB18030-2022"), &[], None, FLAG_GL_UNSAFE, Codec::Rs(encoding_rs::GB18030)),
    enc!(Cp1252, "Windows-1252", Some("Windows-1252"), &["cp1252"], None, FLAG_SBCS, Codec::Cp1252),
    enc!(Cp1254, "Windows-1254", Some("Windows-1254"), &["CP1254", "CP-1254", "WINDOWS-1254"], None, FLAG_SBCS, Codec::SingleByte { min: 0x80, table: sb::CP1254 }),
    enc!(Iso8859_1, "ISO-8859-1", Some("ISO-8859-1"), &["ISO8859-1", "latin1"], None, FLAG_SBCS, Codec::Latin1),
    enc!(Iso8859_2, "ISO-8859-2", Some("ISO-8859-2"), &["ISO8859-2", "latin2"], None, FLAG_SBCS, Codec::SingleByte { min: 0xA0, table: sb::ISO8859_2 }),
    enc!(Iso8859_3, "ISO-8859-3", Some("ISO-8859-3"), &["ISO8859-3", "latin3"], None, FLAG_SBCS, Codec::SingleByte { min: 0xA0, table: sb::ISO8859_3 }),
    enc!(Iso8859_4, "ISO-8859-4", Some("ISO-8859-4"), &["ISO8859-4", "latin4"], None, FLAG_SBCS, Codec::SingleByte { min: 0xA0, table: sb::ISO8859_4 }),
    enc!(Iso8859_5, "ISO-8859-5", Some("ISO-8859-5"), &["ISO8859-5", "cyrillic"], None, FLAG_SBCS, Codec::SingleByte { min: 0xA0, table: sb::ISO8859_5 }),
    enc!(Iso8859_6, "ISO-8859-6", Some("ISO-8859-6"), &["ISO8859-6", "arabic"], None, FLAG_SBCS, Codec::SingleByte { min: 0xA0, table: sb::ISO8859_6 }),
    enc!(Iso8859_7, "ISO-8859-7", Some("ISO-8859-7"), &["ISO8859-7", "greek"], None, FLAG_SBCS, Codec::SingleByte { min: 0xA0, table: sb::ISO8859_7 }),
    enc!(Iso8859_8, "ISO-8859-8", Some("ISO-8859-8"), &["ISO8859-8", "hebrew"], None, FLAG_SBCS, Codec::SingleByte { min: 0xA0, table: sb::ISO8859_8 }),
    enc!(Iso8859_9, "ISO-8859-9", Some("ISO-8859-9"), &["ISO8859-9", "latin5"], None, FLAG_SBCS, Codec::SingleByte { min: 0xA0, table: sb::ISO8859_9 }),
    enc!(Iso8859_10, "ISO-8859-10", Some("ISO-8859-10"), &["ISO8859-10", "latin6"], None, FLAG_SBCS, Codec::SingleByte { min: 0xA0, table: sb::ISO8859_10 }),
    enc!(Iso8859_13, "ISO-8859-13", Some("ISO-8859-13"), &["ISO8859-13"], None, FLAG_SBCS, Codec::SingleByte { min: 0xA0, table: sb::ISO8859_13 }),
    enc!(Iso8859_14, "ISO-8859-14", Some("ISO-8859-14"), &["ISO8859-14", "latin8"], None, FLAG_SBCS, Codec::SingleByte { min: 0xA0, table: sb::ISO8859_14 }),
    enc!(Iso8859_15, "ISO-8859-15", Some("ISO-8859-15"), &["ISO8859-15"], None, FLAG_SBCS, Codec::SingleByte { min: 0xA0, table: sb::ISO8859_15 }),
    enc!(Iso8859_16, "ISO-8859-16", Some("ISO-8859-16"), &["ISO8859-16"], None, FLAG_SBCS, Codec::SingleByte { min: 0xA0, table: sb::ISO8859_16 }),
    enc!(EucCn, "EUC-CN", Some("CN-GB"), &["CN-GB", "EUC_CN", "eucCN", "x-euc-cn", "gb2312"], Some(&cjk::MBLEN_EUCCN), 0, Codec::EucCn),
    enc!(Cp936, "CP936", Some("CP936"), &["CP-936", "GBK"], Some(&cjk::MBLEN_81_TO_FE), FLAG_GL_UNSAFE, Codec::Rs(encoding_rs::GBK)),
    enc!(Hz, "HZ", Some("HZ-GB-2312"), &[], None, FLAG_GL_UNSAFE, Codec::Hz),
    enc!(EucTw, "EUC-TW", Some("EUC-TW"), &["EUC_TW", "eucTW", "x-euc-tw"], Some(&cjk::MBLEN_EUCCN), 0, Codec::Unsupported),
    enc!(Big5, "BIG-5", Some("BIG5"), &["CN-BIG5", "BIG-FIVE", "BIGFIVE"], Some(&cjk::MBLEN_81_TO_FE), FLAG_GL_UNSAFE, Codec::Rs(encoding_rs::BIG5)),
    enc!(Cp950, "CP950", Some("BIG5"), &[], Some(&cjk::MBLEN_81_TO_FE), FLAG_GL_UNSAFE, Codec::Rs(encoding_rs::BIG5)),
    enc!(EucKr, "EUC-KR", Some("EUC-KR"), &["EUC_KR", "eucKR", "x-euc-kr"], Some(&cjk::MBLEN_EUCCN), 0, Codec::Rs(encoding_rs::EUC_KR)),
    enc!(Uhc, "UHC", Some("UHC"), &["CP949"], Some(&cjk::MBLEN_81_TO_FE), 0, Codec::Rs(encoding_rs::EUC_KR)),
    enc!(Iso2022Kr, "ISO-2022-KR", Some("ISO-2022-KR"), &[], None, FLAG_GL_UNSAFE, Codec::Iso2022Kr),
    enc!(Cp1251, "Windows-1251", Some("Windows-1251"), &["CP1251", "CP-1251", "WINDOWS-1251"], None, FLAG_SBCS, Codec::SingleByte { min: 0x80, table: sb::CP1251 }),
    enc!(Cp866, "CP866", Some("CP866"), &["CP-866", "IBM866", "IBM-866"], None, FLAG_SBCS, Codec::SingleByte { min: 0x80, table: sb::CP866 }),
    enc!(Koi8R, "KOI8-R", Some("KOI8-R"), &["KOI8R"], None, FLAG_SBCS, Codec::SingleByte { min: 0x80, table: sb::KOI8R }),
    enc!(Koi8U, "KOI8-U", Some("KOI8-U"), &["KOI8U"], None, FLAG_SBCS, Codec::SingleByte { min: 0x80, table: sb::KOI8U }),
    enc!(ArmSCII8, "ArmSCII-8", Some("ArmSCII-8"), &["ArmSCII8", "ARMSCII-8", "ARMSCII8"], None, FLAG_SBCS, Codec::ArmSCII8),
    enc!(Cp850, "CP850", Some("CP850"), &["CP-850", "IBM850", "IBM-850"], None, FLAG_SBCS, Codec::SingleByte { min: 0x80, table: sb::CP850 }),
    enc!(Iso2022Jp2004, "ISO-2022-JP-2004", Some("ISO-2022-JP-2004"), &[], None, FLAG_GL_UNSAFE, Codec::Iso2022Jp { jis: false }),
    enc!(Iso2022JpKddi, "ISO-2022-JP-MOBILE#KDDI", Some("ISO-2022-JP"), &["ISO-2022-JP-KDDI"], None, FLAG_GL_UNSAFE, Codec::Iso2022Jp { jis: false }),
    enc!(Cp50220, "CP50220", Some("ISO-2022-JP"), &["cp50220raw", "cp50220-raw", "JIS-ms"], None, FLAG_GL_UNSAFE, Codec::Iso2022Jp { jis: false }),
    enc!(Cp50221, "CP50221", Some("ISO-2022-JP"), &[], None, FLAG_GL_UNSAFE, Codec::Iso2022Jp { jis: false }),
    enc!(Cp50222, "CP50222", Some("ISO-2022-JP"), &[], None, FLAG_GL_UNSAFE, Codec::Iso2022Jp { jis: false }),
];

/// `mbfl_name2encoding`: resolve a name case-insensitively against the
/// canonical names, then the MIME names, then the aliases.
pub fn name2encoding(name: &[u8]) -> Option<&'static Encoding> {
    if let Some(e) = ENCODINGS.iter().find(|e| e.name.as_bytes().eq_ignore_ascii_case(name)) {
        return Some(e);
    }
    if let Some(e) = ENCODINGS.iter().find(|e| e.mime_name.is_some_and(|m| m.as_bytes().eq_ignore_ascii_case(name))) {
        return Some(e);
    }
    ENCODINGS.iter().find(|e| e.aliases.iter().any(|a| a.as_bytes().eq_ignore_ascii_case(name)))
}

/// `mbfl_no2encoding`.
pub fn by_id(id: Id) -> &'static Encoding {
    if id == Id::Pass {
        return &PASS;
    }
    ENCODINGS.iter().find(|e| e.id == id).unwrap_or(&UTF8)
}

/// `mbfl_encoding_preferred_mime_name`.
pub fn preferred_mime_name(enc: &Encoding) -> Option<&'static str> {
    enc.mime_name.filter(|m| !m.is_empty())
}

// ---- languages ------------------------------------------------------------------

/// `mbfl_language`: a language with its default mail charset and transfer
/// encodings, and its default detect order (`php_mb_default_identify_list`).
#[derive(Debug)]
pub struct Language {
    /// The full name (`mb_language()`).
    pub name: &'static str,
    /// The short name.
    pub short: &'static str,
    /// Extra names.
    pub aliases: &'static [&'static str],
    /// Default `mail_charset`.
    pub mail_charset: Id,
    /// Default `mail_header_encoding`.
    pub mail_header: Id,
    /// Default `mail_body_encoding`.
    pub mail_body: Id,
    /// Default `mbstring.detect_order`.
    pub detect_order: &'static [Id],
}

macro_rules! lang {
    ($name:literal, $short:literal, $aliases:expr, $cs:ident, $hdr:ident, $body:ident, $order:expr) => {
        Language { name: $name, short: $short, aliases: $aliases, mail_charset: Id::$cs, mail_header: Id::$hdr, mail_body: Id::$body, detect_order: $order }
    };
}

/// Every language, in `mbfl_language_ptr_table` order.
pub static LANGUAGES: &[Language] = &[
    lang!("uni", "uni", &["universal"], Utf8, Base64, Base64, &[Id::Ascii, Id::Utf8]),
    lang!("Japanese", "ja", &[], Iso2022Jp, Base64, SevenBit, &[Id::Ascii, Id::Jis, Id::Utf8, Id::EucJp, Id::Sjis]),
    lang!("Korean", "ko", &[], Iso2022Kr, Base64, SevenBit, &[Id::Ascii, Id::Utf8, Id::EucKr, Id::Uhc]),
    lang!("Simplified Chinese", "zh-cn", &[], Hz, Base64, SevenBit, &[Id::Ascii, Id::Utf8, Id::EucCn, Id::Cp936]),
    lang!("Traditional Chinese", "zh-tw", &[], Big5, Base64, EightBit, &[Id::Ascii, Id::Utf8, Id::EucTw, Id::Big5]),
    lang!("English", "en", &[], Iso8859_1, QPrint, EightBit, &[Id::Ascii, Id::Utf8]),
    lang!("German", "de", &["Deutsch"], Iso8859_15, QPrint, EightBit, &[Id::Ascii, Id::Utf8]),
    lang!("Russian", "ru", &[], Koi8R, QPrint, EightBit, &[Id::Ascii, Id::Utf8, Id::Koi8R, Id::Cp1251, Id::Cp866]),
    lang!("Ukrainian", "ua", &[], Koi8U, QPrint, EightBit, &[Id::Ascii, Id::Utf8, Id::Koi8U]),
    lang!("Armenian", "hy", &[], ArmSCII8, QPrint, EightBit, &[Id::Ascii, Id::Utf8, Id::ArmSCII8]),
    lang!("Turkish", "tr", &[], Iso8859_9, QPrint, EightBit, &[Id::Ascii, Id::Utf8, Id::Cp1254, Id::Iso8859_9]),
    lang!("neutral", "neutral", &[], Utf8, Base64, Base64, &[Id::Ascii, Id::Utf8]),
];

/// `mbfl_name2language`: by name, short name, then alias (all
/// case-insensitive).
pub fn name2language(name: &[u8]) -> Option<&'static Language> {
    LANGUAGES
        .iter()
        .find(|l| l.name.as_bytes().eq_ignore_ascii_case(name))
        .or_else(|| LANGUAGES.iter().find(|l| l.short.as_bytes().eq_ignore_ascii_case(name)))
        .or_else(|| LANGUAGES.iter().find(|l| l.aliases.iter().any(|a| a.as_bytes().eq_ignore_ascii_case(name))))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conv(s: &[u8], from: &str, to: &str) -> Vec<u8> {
        let f = name2encoding(from.as_bytes()).unwrap();
        let t = name2encoding(to.as_bytes()).unwrap();
        convert(s, f, t, b'?' as u32, ErrorMode::Char).0
    }

    #[test]
    fn names_resolve_like_php() {
        assert_eq!(name2encoding(b"utf8").unwrap().name, "UTF-8");
        assert_eq!(name2encoding(b"US-ASCII").unwrap().name, "ASCII");
        assert_eq!(name2encoding(b"latin1").unwrap().name, "ISO-8859-1");
        assert_eq!(name2encoding(b"Shift_JIS").unwrap().name, "SJIS");
        assert_eq!(name2encoding(b"gbk").unwrap().name, "CP936");
        assert_eq!(name2encoding(b"html").unwrap().name, "HTML-ENTITIES");
        assert!(name2encoding(b"nope").is_none());
        assert!(name2encoding(b"pass").is_none());
        // php 8.5 `count(mb_list_encodings())` is 79.
        assert_eq!(ENCODINGS.len(), 79);
    }

    #[test]
    fn utf8_round_trips_and_substitutes() {
        assert_eq!(conv("é".as_bytes(), "UTF-8", "ISO-8859-1"), b"\xe9");
        assert_eq!(conv(b"\xe9", "ISO-8859-1", "UTF-8"), "é".as_bytes());
        assert_eq!(conv("日".as_bytes(), "UTF-8", "ISO-8859-1"), b"?");
        assert_eq!(conv(b"a\xffb", "UTF-8", "UTF-8"), b"a?b");
        let ascii = name2encoding(b"ASCII").unwrap();
        let (out, errs) = convert("日x".as_bytes(), &UTF8, ascii, b'?' as u32, ErrorMode::Long);
        assert_eq!(out, b"U+65E5x");
        assert_eq!(errs, 1);
        let (out, _) = convert("日".as_bytes(), &UTF8, ascii, b'?' as u32, ErrorMode::Entity);
        assert_eq!(out, b"&#x65E5;");
        let (out, _) = convert(b"a\xffb", &UTF8, ascii, b'?' as u32, ErrorMode::None);
        assert_eq!(out, b"ab");
    }

    #[test]
    fn languages_resolve() {
        assert_eq!(name2language(b"ja").unwrap().name, "Japanese");
        assert_eq!(name2language(b"universal").unwrap().name, "uni");
        assert_eq!(name2language(b"NEUTRAL").unwrap().name, "neutral");
        assert!(name2language(b"xx").is_none());
    }
}
