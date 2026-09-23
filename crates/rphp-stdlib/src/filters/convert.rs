//! The `convert.*` stream filters (php-src `ext/standard/filters.c`, the
//! `php_conv_*` converters): base64 and quoted-printable, each way.
//!
//! These are ports of php's converters rather than calls to the string
//! functions, because a stream hands them its bytes a bucket at a time and
//! the answer must not depend on where the buckets were cut — a base64
//! quantum or a `=XX` escape may straddle two writes. Each converter keeps
//! exactly the state php's keeps between calls.
//!
//! php's own quirks are part of the contract and are kept: invalid base64
//! characters are skipped rather than rejected, an input that ends inside
//! a quantum or an escape ends silently (the converter's "unexpected end"
//! is swallowed by the filter), and the quoted-printable encoder's
//! trailing-whitespace look-ahead only sees the rest of the bucket it is
//! working through — which is why that one converter also emulates php's
//! output buffer, whose growth restarts the look-ahead.

use super::{Fault, Flush, NativeFilter};

/// What one converter call ran into (`php_conv_err_t`).
#[derive(PartialEq)]
enum Conv {
    Ok,
    /// The output buffer is full: php grows it and calls again.
    TooBig,
    Invalid,
}

/// php's `b64_tbl_enc`, indexed by six bits.
const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// A converter's options, read from the filter's `$params` array by the
/// caller (`filters.rs`), with php's defaulting already applied.
pub(crate) struct Options {
    /// `line-length`, or `0` for no line breaking.
    pub(crate) line_len: u32,
    /// `line-break-chars`; `None` unless `line-length` is at least 4.
    pub(crate) lbchars: Option<Vec<u8>>,
    pub(crate) binary: bool,
    pub(crate) force_encode_first: bool,
}

/// The message php's filter warns with when a converter reports an invalid
/// sequence.
fn invalid(name: &str) -> Fault {
    Fault::Warning(format!("Stream filter ({name}): invalid byte sequence"))
}

// ---- base64 ----------------------------------------------------------------------------

/// `convert.base64-encode`.
pub(crate) struct Base64Encode {
    line_len: u32,
    lbchars: Option<Vec<u8>>,
    line_ccnt: u32,
    /// The bytes of an unfinished three-byte group.
    erem: Vec<u8>,
}

impl Base64Encode {
    pub(crate) fn new(o: Options) -> Base64Encode {
        let (line_len, lbchars) = match o.lbchars {
            Some(lb) => (o.line_len, Some(lb)),
            None => (0, None),
        };
        Base64Encode { line_len, lbchars, line_ccnt: line_len, erem: Vec::new() }
    }

    /// A line break when the line is full, before a group goes out.
    fn line_break(&mut self, out: &mut Vec<u8>) {
        if let Some(lb) = &self.lbchars {
            if self.line_ccnt < 4 {
                out.extend_from_slice(lb);
                self.line_ccnt = self.line_len;
            }
        }
    }

    fn group(&mut self, g: &[u8], out: &mut Vec<u8>) {
        self.line_break(out);
        let b = [g[0], *g.get(1).unwrap_or(&0), *g.get(2).unwrap_or(&0)];
        out.push(B64[(b[0] >> 2) as usize]);
        out.push(B64[(((b[0] << 4) | (b[1] >> 4)) & 0x3f) as usize]);
        out.push(if g.len() > 1 { B64[(((b[1] << 2) | (b[2] >> 6)) & 0x3f) as usize] } else { b'=' });
        out.push(if g.len() > 2 { B64[(b[2] & 0x3f) as usize] } else { b'=' });
        self.line_ccnt = self.line_ccnt.wrapping_sub(4);
    }
}

impl NativeFilter for Base64Encode {
    fn filter(&mut self, input: &[u8], out: &mut Vec<u8>, flush: Flush) -> Result<(), Fault> {
        let mut data = std::mem::take(&mut self.erem);
        data.extend_from_slice(input);
        let whole = data.len() / 3 * 3;
        for g in data[..whole].chunks(3) {
            self.group(g, out);
        }
        self.erem = data[whole..].to_vec();
        if flush == Flush::Close && !self.erem.is_empty() {
            let tail = std::mem::take(&mut self.erem);
            self.group(&tail, out);
        }
        Ok(())
    }
}

/// `convert.base64-decode`.
pub(crate) struct Base64Decode {
    name: String,
    /// Bits of the byte being assembled, and how many.
    acc: u32,
    nbits: u32,
    /// Padding has been seen: nothing but more padding or skipped bytes
    /// may follow.
    padded: bool,
}

impl Base64Decode {
    pub(crate) fn new(name: &str) -> Base64Decode {
        Base64Decode { name: name.to_string(), acc: 0, nbits: 0, padded: false }
    }
}

/// php's `b64_tbl_dec`: a six-bit value, `Pad` for `=`, `Skip` for the rest.
enum B64In {
    Value(u32),
    Pad,
    Skip,
}

fn b64_in(c: u8) -> B64In {
    match c {
        b'A'..=b'Z' => B64In::Value((c - b'A') as u32),
        b'a'..=b'z' => B64In::Value((c - b'a' + 26) as u32),
        b'0'..=b'9' => B64In::Value((c - b'0' + 52) as u32),
        b'+' => B64In::Value(62),
        b'/' => B64In::Value(63),
        b'=' => B64In::Pad,
        _ => B64In::Skip,
    }
}

impl NativeFilter for Base64Decode {
    fn filter(&mut self, input: &[u8], out: &mut Vec<u8>, _flush: Flush) -> Result<(), Fault> {
        // End of stream inside a quantum is php's "unexpected end", which
        // the filter lets pass: the whole bytes are already out.
        for &c in input {
            match b64_in(c) {
                B64In::Value(v) => {
                    if self.padded {
                        return Err(invalid(&self.name));
                    }
                    self.acc = (self.acc << 6) | v;
                    self.nbits += 6;
                    if self.nbits >= 8 {
                        self.nbits -= 8;
                        out.push((self.acc >> self.nbits) as u8);
                        self.acc &= (1 << self.nbits) - 1;
                    }
                }
                B64In::Skip if !self.padded => {}
                // Padding is only legal two or three characters into a
                // quantum — with four or two bits of a byte pending — and
                // past it only more padding or skipped bytes may follow.
                B64In::Pad | B64In::Skip => {
                    self.padded = true;
                    if self.nbits == 0 || self.nbits == 6 {
                        return Err(invalid(&self.name));
                    }
                }
            }
        }
        Ok(())
    }
}

// ---- quoted-printable ------------------------------------------------------------------

/// `convert.quoted-printable-encode` — php's `php_conv_qprint_encode`.
pub(crate) struct QpEncode {
    line_len: u32,
    lbchars: Option<Vec<u8>>,
    binary: bool,
    force_encode_first: bool,
    line_ccnt: u32,
    /// A line break partly matched (`lb_cnt` bytes of `lbchars`), and how
    /// much of that match has been given back as ordinary input.
    lb_ptr: usize,
    lb_cnt: usize,
}

impl QpEncode {
    pub(crate) fn new(o: Options) -> QpEncode {
        let (line_len, lbchars) = match o.lbchars {
            Some(lb) => (o.line_len, Some(lb)),
            None => (0, None),
        };
        QpEncode {
            line_len,
            lbchars,
            binary: o.binary,
            force_encode_first: o.force_encode_first,
            line_ccnt: line_len,
            lb_ptr: 0,
            lb_cnt: 0,
        }
    }

    /// One call of php's converter over `inp[*ps..]`, into at most `*ocnt`
    /// more bytes of `out`.
    fn convert(&mut self, inp: &[u8], ps: &mut usize, out: &mut Vec<u8>, ocnt: &mut usize) -> Conv {
        let mut line_ccnt = self.line_ccnt;
        let mut lb_ptr = self.lb_ptr;
        let mut lb_cnt = self.lb_cnt;
        let mut trail_ws: u32 = 0;
        let lb: Option<&[u8]> = self.lbchars.as_deref();
        let line_len = self.line_len;
        let mut err = Conv::Ok;

        /// The soft line break (`=` + the line break) a full line needs.
        fn soft_break(lb: &[u8], out: &mut Vec<u8>, ocnt: &mut usize) -> bool {
            if *ocnt < lb.len() + 1 {
                return false;
            }
            out.push(b'=');
            out.extend_from_slice(lb);
            *ocnt -= lb.len() + 1;
            true
        }

        loop {
            let icnt = inp.len() - *ps;
            if !self.binary {
                if let Some(l) = lb.filter(|l| !l.is_empty()) {
                    if icnt > 0 && inp[*ps] == l[lb_cnt] {
                        lb_cnt += 1;
                        if lb_cnt >= l.len() {
                            if *ocnt < lb_cnt {
                                lb_cnt -= 1;
                                err = Conv::TooBig;
                                break;
                            }
                            out.extend_from_slice(&l[..lb_cnt]);
                            *ocnt -= lb_cnt;
                            line_ccnt = line_len;
                            lb_ptr = 0;
                            lb_cnt = 0;
                        }
                        *ps += 1;
                        continue;
                    }
                }
            }
            if lb_ptr >= lb_cnt && icnt == 0 {
                break;
            }
            let c = match lb {
                Some(l) if lb_ptr < lb_cnt => l[lb_ptr],
                _ => inp[*ps],
            };
            // php's `CONSUME_CHAR`: a replayed line-break byte, or the input.
            macro_rules! consume {
                () => {
                    if lb_ptr < lb_cnt {
                        lb_ptr += 1;
                    } else {
                        lb_cnt = 0;
                        lb_ptr = 0;
                        *ps += 1;
                    }
                };
            }
            if !self.binary && trail_ws == 0 && (c == b'\t' || c == b' ') {
                match lb {
                    Some(l) if line_ccnt < 2 => {
                        if !soft_break(l, out, ocnt) {
                            err = Conv::TooBig;
                            break;
                        }
                        line_ccnt = line_len;
                    }
                    _ => {
                        if *ocnt < 1 {
                            err = Conv::TooBig;
                            break;
                        }
                        // Whitespace that only runs up to a line break must be
                        // encoded; php looks ahead through what is left of
                        // this call's input, one byte short of its end.
                        if let Some(l) = lb {
                            trail_ws = 1;
                            let mut lb_cnt2 = 0usize;
                            let mut p2 = *ps;
                            let mut j = icnt.saturating_sub(1);
                            while j > 0 {
                                let b = inp[p2];
                                if b == l.get(lb_cnt2).copied().unwrap_or(0) {
                                    lb_cnt2 += 1;
                                    if lb_cnt2 >= l.len() {
                                        break;
                                    }
                                } else if lb_cnt2 != 0 || (b != b'\t' && b != b' ') {
                                    trail_ws = 0;
                                    break;
                                } else {
                                    trail_ws += 1;
                                }
                                j -= 1;
                                p2 += 1;
                            }
                        }
                        if trail_ws == 0 {
                            out.push(c);
                            *ocnt -= 1;
                            line_ccnt = line_ccnt.wrapping_sub(1);
                            consume!();
                        }
                    }
                }
            } else if (!self.force_encode_first || line_ccnt < line_len)
                && ((33..=60).contains(&c) || (62..=126).contains(&c))
            {
                if let Some(l) = lb.filter(|_| line_ccnt < 2) {
                    if !soft_break(l, out, ocnt) {
                        err = Conv::TooBig;
                        break;
                    }
                    line_ccnt = line_len;
                }
                if *ocnt < 1 {
                    err = Conv::TooBig;
                    break;
                }
                out.push(c);
                *ocnt -= 1;
                line_ccnt = line_ccnt.wrapping_sub(1);
                consume!();
            } else {
                if let Some(l) = lb.filter(|_| line_ccnt < 4) {
                    if !soft_break(l, out, ocnt) {
                        err = Conv::TooBig;
                        break;
                    }
                    line_ccnt = line_len;
                }
                if *ocnt < 3 {
                    err = Conv::TooBig;
                    break;
                }
                const HEX: &[u8; 16] = b"0123456789ABCDEF";
                out.extend_from_slice(&[b'=', HEX[(c >> 4) as usize], HEX[(c & 0x0f) as usize]]);
                *ocnt -= 3;
                line_ccnt = line_ccnt.wrapping_sub(3);
                trail_ws = trail_ws.saturating_sub(1);
                consume!();
            }
        }
        self.line_ccnt = line_ccnt;
        self.lb_ptr = lb_ptr;
        self.lb_cnt = lb_cnt;
        err
    }
}

impl NativeFilter for QpEncode {
    fn filter(&mut self, input: &[u8], out: &mut Vec<u8>, _flush: Flush) -> Result<(), Fault> {
        // php's `strfilter_convert_append_bucket`: an output buffer the size
        // of the input, doubled each time the converter fills it.
        let mut cap = input.len();
        let mut ocnt = cap;
        let mut ps = 0;
        while ps < input.len() {
            if self.convert(input, &mut ps, out, &mut ocnt) == Conv::TooBig {
                ocnt += cap;
                cap *= 2;
            }
        }
        Ok(())
    }
}

/// `convert.quoted-printable-decode` — php's `php_conv_qprint_decode`.
pub(crate) struct QpDecode {
    name: String,
    /// `line-break-chars`; without it `\r\n`, `\r` and `\n` are all line
    /// breaks after `=`.
    lbchars: Option<Vec<u8>>,
    scan_stat: u8,
    next_char: u32,
    lb_ptr: usize,
    lb_cnt: usize,
}

impl QpDecode {
    pub(crate) fn new(name: &str, lbchars: Option<Vec<u8>>) -> QpDecode {
        QpDecode { name: name.to_string(), lbchars, scan_stat: 0, next_char: 0, lb_ptr: 0, lb_cnt: 0 }
    }

    fn convert(&mut self, inp: &[u8], out: &mut Vec<u8>) -> Conv {
        let lb: &[u8] = self.lbchars.as_deref().unwrap_or(&[]);
        let auto = self.lbchars.is_none();
        let mut ps = 0usize;
        loop {
            let at = inp.get(ps).copied();
            match self.scan_stat {
                0 => {
                    let Some(b) = at else { break };
                    if b == b'=' {
                        self.scan_stat = 1;
                    } else {
                        out.push(b);
                    }
                    ps += 1;
                }
                1 | 2 => {
                    let Some(b) = at else { break };
                    if self.scan_stat == 1 {
                        if b == b' ' || b == b'\t' {
                            self.scan_stat = 4;
                            ps += 1;
                            continue;
                        } else if auto && self.lb_cnt == 0 && b == b'\r' {
                            self.lb_cnt += 1;
                            self.scan_stat = 5;
                            ps += 1;
                            continue;
                        } else if auto && self.lb_cnt == 0 && b == b'\n' {
                            self.lb_cnt = 0;
                            self.lb_ptr = 0;
                            self.scan_stat = 0;
                            ps += 1;
                            continue;
                        } else if self.lb_cnt < lb.len() && b == lb[self.lb_cnt] {
                            self.lb_cnt += 1;
                            self.scan_stat = 5;
                            ps += 1;
                            continue;
                        }
                    }
                    if !b.is_ascii_hexdigit() {
                        return Conv::Invalid;
                    }
                    let digit = if b >= b'A' { b as u32 - 0x37 } else { b as u32 - 0x30 };
                    self.next_char = (self.next_char << 4) | digit;
                    self.scan_stat += 1;
                    ps += 1;
                    if self.scan_stat == 3 {
                        out.push(self.next_char as u8);
                        self.scan_stat = 0;
                    }
                }
                4 => {
                    let Some(b) = at else { break };
                    if self.lb_cnt < lb.len() && b == lb[self.lb_cnt] {
                        self.lb_cnt += 1;
                        self.scan_stat = 5;
                    } else if b != b'\t' && b != b' ' {
                        return Conv::Invalid;
                    }
                    ps += 1;
                }
                5 => {
                    if auto && self.lb_cnt == 1 && at == Some(b'\n') {
                        self.lb_cnt = 0;
                        self.lb_ptr = 0;
                        self.scan_stat = 0;
                        ps += 1;
                    } else if auto && self.lb_cnt > 0 || self.lb_cnt >= lb.len() {
                        self.lb_cnt = 0;
                        self.lb_ptr = 0;
                        self.scan_stat = 0;
                    } else if let Some(b) = at {
                        if b == lb[self.lb_cnt] {
                            self.lb_cnt += 1;
                            ps += 1;
                        } else {
                            self.scan_stat = 6;
                        }
                    } else {
                        break;
                    }
                }
                _ => {
                    if self.lb_ptr < self.lb_cnt {
                        out.push(lb[self.lb_ptr]);
                        self.lb_ptr += 1;
                    } else {
                        self.scan_stat = 0;
                        self.lb_cnt = 0;
                        self.lb_ptr = 0;
                    }
                }
            }
        }
        Conv::Ok
    }
}

impl NativeFilter for QpDecode {
    fn filter(&mut self, input: &[u8], out: &mut Vec<u8>, _flush: Flush) -> Result<(), Fault> {
        // A stream that ends inside an escape ends quietly, as php's does.
        if self.convert(input, out) == Conv::Invalid {
            return Err(invalid(&self.name));
        }
        Ok(())
    }
}

/// `convert.iconv.<from>.<to>` over the iconv module's converter.
pub(crate) struct Iconv {
    from: String,
    to: String,
    conv: crate::iconv::StreamConv,
}

impl Iconv {
    /// The pair out of the filter's name: after `convert.iconv.`, the source
    /// charset runs to the first `.` or `/` and the target is the rest.
    pub(crate) fn named(name: &str) -> Option<Iconv> {
        let rest = name.splitn(3, '.').nth(2)?;
        let cut = rest.find(['.', '/'])?;
        let (from, to) = (&rest[..cut], &rest[cut + 1..]);
        // php's `ICONV_CSNMAXLEN`.
        if from.len() >= 64 || to.len() >= 64 {
            return None;
        }
        let conv = crate::iconv::StreamConv::open(from.as_bytes(), to.as_bytes())?;
        Some(Iconv { from: from.to_string(), to: to.to_string(), conv })
    }
}

impl NativeFilter for Iconv {
    fn filter(&mut self, input: &[u8], out: &mut Vec<u8>, flush: Flush) -> Result<(), Fault> {
        let mut produced = Vec::new();
        match self.conv.push(input, &mut produced, flush == Flush::Close) {
            Ok(()) => {
                out.extend_from_slice(&produced);
                Ok(())
            }
            Err(()) => Err(Fault::Warning(format!(
                "iconv stream filter (\"{}\"=>\"{}\"): invalid multibyte sequence",
                self.from, self.to
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Feed `data` in pieces cut at `cuts`, then close.
    fn run(f: &mut dyn NativeFilter, data: &[u8], cuts: &[usize]) -> Result<Vec<u8>, Fault> {
        let mut out = Vec::new();
        let mut at = 0;
        for &c in cuts {
            let c = c.min(data.len()).max(at);
            f.filter(&data[at..c], &mut out, Flush::Normal)?;
            at = c;
        }
        f.filter(&data[at..], &mut out, Flush::Normal)?;
        f.filter(&[], &mut out, Flush::Close)?;
        Ok(out)
    }

    fn opts(line_len: u32, lb: Option<&[u8]>) -> Options {
        Options { line_len, lbchars: lb.map(<[u8]>::to_vec), binary: false, force_encode_first: false }
    }

    /// A small deterministic generator, so the property tests need no crate.
    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            self.0 >> 33
        }
    }

    fn cuts(rng: &mut Lcg, len: usize) -> Vec<usize> {
        let mut v: Vec<usize> = (0..rng.next() % 6).map(|_| (rng.next() as usize) % (len + 1)).collect();
        v.sort_unstable();
        v
    }

    #[test]
    fn base64_encode_matches_whole_input_at_any_cut() {
        let mut rng = Lcg(7);
        for _ in 0..500 {
            let len = (rng.next() % 40) as usize;
            let data: Vec<u8> = (0..len).map(|_| rng.next() as u8).collect();
            let whole = run(&mut Base64Encode::new(opts(0, None)), &data, &[]).unwrap();
            let split = run(&mut Base64Encode::new(opts(0, None)), &data, &cuts(&mut rng, len)).unwrap();
            assert_eq!(whole, split);
            let lined = run(&mut Base64Encode::new(opts(8, Some(b"\n"))), &data, &cuts(&mut rng, len)).unwrap();
            let unlined: Vec<u8> = lined.iter().copied().filter(|b| *b != b'\n').collect();
            assert_eq!(unlined, whole);
            let back = run(&mut Base64Decode::new("d"), &lined, &cuts(&mut rng, lined.len())).unwrap();
            assert_eq!(back, data);
        }
    }

    #[test]
    fn base64_matches_php() {
        let enc = run(&mut Base64Encode::new(opts(7, Some(b"|"))), &[b'x'; 19], &[2, 9, 10]).unwrap();
        assert_eq!(enc, b"eHh4|eHh4|eHh4|eHh4|eHh4|eHh4|eA==");
        let dec = |s: &[u8]| run(&mut Base64Decode::new("d"), s, &[]);
        assert_eq!(dec(b"YWJ").unwrap(), b"ab");
        assert_eq!(dec(b"YW*Jj").unwrap(), b"abc");
        assert_eq!(dec(b"YQ=\n=").unwrap(), b"a");
        assert!(dec(b"YWJj=").is_err());
        assert!(dec(b"Y===").is_err());
        assert!(dec(b"YQ==YQ==").is_err());
    }

    #[test]
    fn quoted_printable_round_trips_at_any_cut() {
        let mut rng = Lcg(11);
        let alphabet = b"ab =\t\r\n\xffZ";
        for _ in 0..500 {
            let len = (rng.next() % 50) as usize;
            let data: Vec<u8> = (0..len).map(|_| alphabet[(rng.next() % alphabet.len() as u64) as usize]).collect();
            let enc = run(&mut QpEncode::new(opts(0, None)), &data, &cuts(&mut rng, len)).unwrap();
            let whole = run(&mut QpEncode::new(opts(0, None)), &data, &[]).unwrap();
            assert_eq!(enc, whole);
            let dec = run(&mut QpDecode::new("q", None), &enc, &cuts(&mut rng, enc.len())).unwrap();
            assert_eq!(dec, data);
        }
    }

    #[test]
    fn quoted_printable_matches_php() {
        let enc = |o: Options, s: &[u8]| run(&mut QpEncode::new(o), s, &[]).unwrap();
        assert_eq!(enc(opts(6, Some(b"\n")), b"abc \n"), b"abc=\n=20\n");
        assert_eq!(enc(opts(6, Some(b"\n")), b"a \nb"), b"a=20\nb");
        let mut long = vec![b'a'; 74];
        long.extend_from_slice(b" b");
        assert_eq!(enc(opts(76, Some(b"\r\n")), &long).len(), 81);
        let dec = |lb: Option<&[u8]>, s: &[u8]| run(&mut QpDecode::new("q", lb.map(<[u8]>::to_vec)), s, &[]);
        assert_eq!(dec(None, b"=4a").unwrap(), b"j");
        assert_eq!(dec(None, b"a=\rb").unwrap(), b"ab");
        assert_eq!(dec(Some(b"\n"), b"=  \nx").unwrap(), b"x");
        assert!(dec(None, b"=  \r\nx").is_err());
        assert!(dec(Some(b"\n"), b"=\r\nx").is_err());
    }
}
