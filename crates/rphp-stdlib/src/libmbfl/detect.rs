//! Encoding detection (`mb_guess_encoding_for_strings` in `mbstring.c`,
//! php 8.1+): every candidate decodes the input; illegal sequences either
//! eliminate it (strict) or cost 1000 demerits, "rare" code points cost 30,
//! ASCII punctuation 6, everything else 1; earlier candidates get a small
//! bonus when the list order is significant; the cheapest candidate wins.

use super::tables::rare_cp::RARE;
use super::{Encoding, Id, BAD_INPUT};

/// `estimate_demerits`.
fn demerits(w: u32) -> u64 {
    if w > 0xFFFF {
        40
    } else if (0x21..=0x2F).contains(&w) {
        6
    } else if (RARE[(w >> 5) as usize] >> (w & 0x1F)) & 1 == 1 {
        30
    } else {
        1
    }
}

/// `start_string`'s BOM skipping for UTF-8 / UTF-16BE / UTF-16LE.
fn skip_bom<'a>(enc: &Encoding, s: &'a [u8]) -> &'a [u8] {
    match enc.id {
        Id::Utf8 if s.starts_with(b"\xEF\xBB\xBF") => &s[3..],
        Id::Utf16Be if s.starts_with(b"\xFE\xFF") => &s[2..],
        Id::Utf16Le if s.starts_with(b"\xFF\xFE") => &s[2..],
        _ => s,
    }
}

struct Candidate {
    enc: &'static Encoding,
    demerits: u64,
    multiplier: f64,
}

/// `mb_guess_encoding_for_strings`: the most plausible of `elist` for all of
/// `strings`, or `None` when strict detection eliminates every candidate.
pub fn guess(strings: &[&[u8]], elist: &[&'static Encoding], strict: bool, order_significant: bool) -> Option<&'static Encoding> {
    if elist.is_empty() {
        return None;
    }
    if elist.len() == 1 {
        if strict && !strings.iter().all(|s| elist[0].check(s)) {
            return None;
        }
        return Some(elist[0]);
    }
    if strings.len() == 1 && strings[0].is_empty() {
        return Some(elist[0]);
    }

    // init_candidate_array
    let mut cands: Vec<Candidate> = Vec::with_capacity(elist.len());
    let n = elist.len();
    for (i, &enc) in elist.iter().enumerate() {
        let mut d = 0u64;
        if has_check(enc) {
            let mut skip = false;
            for s in strings {
                if !enc.check(s) {
                    if strict {
                        skip = true;
                        break;
                    }
                    d += 500;
                }
            }
            if skip {
                continue;
            }
        }
        let multiplier = if order_significant { 1.0 + (0.3 * i as f64) / n as f64 } else { 1.0 };
        cands.push(Candidate { enc, demerits: d, multiplier });
    }

    // One string at a time (php processes them last-to-first; the total is
    // the same), decoding every remaining candidate.
    for s in strings.iter().rev() {
        let mut i = 0;
        while i < cands.len() {
            let enc = cands[i].enc;
            let input = skip_bom(enc, s);
            let wchars = enc.decode(input);
            let mut eliminated = false;
            let mut d = 0u64;
            for &w in &wchars {
                if w == BAD_INPUT {
                    if strict {
                        eliminated = true;
                        break;
                    }
                    d += 1000;
                } else {
                    d += demerits(w);
                }
            }
            if eliminated {
                cands.remove(i);
            } else {
                cands[i].demerits += d;
                i += 1;
            }
        }
        if cands.is_empty() {
            return None;
        }
    }

    for c in &mut cands {
        let d = c.demerits as f64 * c.multiplier;
        c.demerits = if d < u64::MAX as f64 { d as u64 } else { u64::MAX };
    }
    let mut best = 0;
    for i in 1..cands.len() {
        if cands[i].demerits < cands[best].demerits {
            best = i;
        }
    }
    Some(cands[best].enc)
}

/// Whether the encoding has a dedicated `check` function in libmbfl (only
/// those take part in `init_candidate_array`'s pre-filter).
fn has_check(enc: &Encoding) -> bool {
    matches!(enc.id, Id::Utf7 | Id::Utf7Imap | Id::Jis | Id::Iso2022Jp)
}

#[cfg(test)]
mod tests {
    use super::super::name2encoding;
    use super::*;

    fn e(n: &str) -> &'static Encoding {
        name2encoding(n.as_bytes()).unwrap()
    }

    #[test]
    fn picks_the_plausible_candidate() {
        let list = [e("ASCII"), e("UTF-8")];
        assert_eq!(guess(&["日本語".as_bytes()], &list, false, true).unwrap().name, "UTF-8");
        assert_eq!(guess(&[b"abc"], &list, false, true).unwrap().name, "ASCII");
        let list = [e("UTF-8"), e("ISO-8859-1")];
        assert_eq!(guess(&[b"caf\xe9"], &list, false, true).unwrap().name, "ISO-8859-1");
        assert_eq!(guess(&[b"caf\xe9"], &[e("UTF-8")], true, true), None);
        let list = [e("ASCII"), e("UTF-8"), e("SJIS")];
        assert_eq!(guess(&["日本語".as_bytes()], &list, true, true).unwrap().name, "UTF-8");
    }
}
