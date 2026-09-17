//! Single-byte charsets (`mbfilter_singlebyte.c`): ISO-8859-1 (identity),
//! the table-driven ISO-8859-x / Windows-125x / KOI8 / CP850 / CP866 sets,
//! and the two php special-cases Windows-1252 (C1 bytes with no mapping
//! pass through) and ArmSCII-8 (ASCII punctuation is remapped).

use super::tables::singlebyte::{ARMSCII8, CP1252, UCS_ARMSCII8};
use super::{illegal_output, ConvertBuf, Encoding, BAD_INPUT};

/// Decode through a table starting at byte `min` (`0` entries are illegal).
pub fn decode_table(input: &[u8], min: u8, table: &[u16], out: &mut Vec<u32>) {
    for &c in input {
        if c < min {
            out.push(c as u32);
        } else {
            let w = table.get((c - min) as usize).copied().unwrap_or(0);
            out.push(if w == 0 { BAD_INPUT } else { w as u32 });
        }
    }
}

/// Encode through a table (reverse lookup, first match wins).
pub fn encode_table(enc: &Encoding, min: u8, table: &[u16], input: &[u32], buf: &mut ConvertBuf) {
    for &w in input {
        if w < min as u32 {
            buf.out.push(w as u8);
        } else if let Some(i) = table.iter().position(|&t| t as u32 == w) {
            buf.out.push(min + i as u8);
        } else {
            illegal_output(w, enc, buf);
        }
    }
}

/// `mb_wchar_to_8859_1`.
pub fn encode_latin1(enc: &Encoding, input: &[u32], buf: &mut ConvertBuf) {
    for &w in input {
        if w < 0x100 {
            buf.out.push(w as u8);
        } else {
            illegal_output(w, enc, buf);
        }
    }
}

/// `mb_cp1252_to_wchar`.
pub fn decode_cp1252(input: &[u8], out: &mut Vec<u32>) {
    for &c in input {
        if (0x80..0xA0).contains(&c) {
            let w = CP1252[(c - 0x80) as usize];
            out.push(if w == 0 { BAD_INPUT } else { w as u32 });
        } else {
            out.push(c as u32);
        }
    }
}

/// `mb_wchar_to_cp1252`.
pub fn encode_cp1252(enc: &Encoding, input: &[u32], buf: &mut ConvertBuf) {
    for &w in input {
        if w >= 0x100 {
            match CP1252.iter().position(|&t| t as u32 == w) {
                Some(i) => buf.out.push(0x80 + i as u8),
                None => illegal_output(w, enc, buf),
            }
        } else if w <= 0x7F || w >= 0xA0 || w == 0x81 || w == 0x8D || w == 0x8F || w == 0x90 || w == 0x9D {
            buf.out.push(w as u8);
        } else {
            illegal_output(w, enc, buf);
        }
    }
}

/// `mb_armscii8_to_wchar`.
pub fn decode_armscii8(input: &[u8], out: &mut Vec<u32>) {
    for &c in input {
        if c < 0xA0 {
            out.push(c as u32);
        } else {
            let w = ARMSCII8[(c - 0xA0) as usize];
            out.push(if w == 0 { BAD_INPUT } else { w as u32 });
        }
    }
}

/// `mb_wchar_to_armscii8`.
pub fn encode_armscii8(enc: &Encoding, input: &[u32], buf: &mut ConvertBuf) {
    for &w in input {
        if (0x28..=0x2F).contains(&w) {
            buf.out.push(UCS_ARMSCII8[(w - 0x28) as usize]);
        } else if w < 0xA0 {
            buf.out.push(w as u8);
        } else {
            match ARMSCII8.iter().position(|&t| t as u32 == w) {
                Some(i) => buf.out.push(0xA0 + i as u8),
                None => illegal_output(w, enc, buf),
            }
        }
    }
}
