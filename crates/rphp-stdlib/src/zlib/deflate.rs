//! zlib 1.2.12's compressor (`deflate.c` + `trees.c`), ported line for
//! line so that its output is the one php produces.
//!
//! **Why a port.** php links the system libz and calls `deflateInit2()`
//! with parameters `flate2` cannot pass on: `gzcompress()`, `gzdeflate()`,
//! `gzencode()`, `zlib_encode()` and the `zlib.deflate` stream filter use
//! memLevel 9 (`MAX_MEM_LEVEL`), and `deflate_init()` / `gzopen()` take a
//! memory level and a strategy (`ZLIB_FILTERED`, `ZLIB_RLE`, …) from the
//! script. `flate2` fixes memLevel at 8 and the strategy at the default,
//! and deflate's bytes depend on both — the memory level sizes the hash
//! table and the symbol buffer, and so where matches are found and where
//! blocks end. Reaching `deflateInit2()` directly would take `unsafe` FFI,
//! which this crate forbids, so the algorithm lives here instead: the same
//! state machine, the same tables, the same block decisions.
//!
//! **How it stays exact.** Every function below is the C function of the
//! same name, with its control flow kept — including the parts whose
//! output depends on the caller's buffer sizes (`deflate_stored()` sizes
//! stored blocks by `avail_out`), which is why each caller in `zlib.rs`
//! drives [`Deflate::deflate`] with the buffer sizes php uses. The unit
//! tests compare the output with the system libz itself (through
//! `flate2`, for the parameters it can reach) across levels, window sizes,
//! flush sequences and output-buffer sizes; the differential snippets
//! compare the rest with php.
//!
//! Only compression is here: inflate's output is fixed by its input, so
//! decompression stays on `flate2`.

use std::sync::OnceLock;

// ---- zlib.h ---------------------------------------------------------------

pub(crate) const Z_NO_FLUSH: i32 = 0;
pub(crate) const Z_PARTIAL_FLUSH: i32 = 1;
pub(crate) const Z_SYNC_FLUSH: i32 = 2;
pub(crate) const Z_FULL_FLUSH: i32 = 3;
pub(crate) const Z_FINISH: i32 = 4;
pub(crate) const Z_BLOCK: i32 = 5;

pub(crate) const Z_OK: i32 = 0;
pub(crate) const Z_STREAM_END: i32 = 1;
pub(crate) const Z_STREAM_ERROR: i32 = -2;
pub(crate) const Z_BUF_ERROR: i32 = -5;

pub(crate) const Z_FILTERED: i32 = 1;
pub(crate) const Z_HUFFMAN_ONLY: i32 = 2;
pub(crate) const Z_RLE: i32 = 3;
pub(crate) const Z_FIXED: i32 = 4;

/// The `OS` byte of a gzip header: zlib 1.2.12 writes 19 on Apple
/// platforms and 3 (Unix) elsewhere.
#[cfg(target_vendor = "apple")]
const OS_CODE: u8 = 19;
#[cfg(not(target_vendor = "apple"))]
const OS_CODE: u8 = 3;

// ---- deflate.h ------------------------------------------------------------

const LENGTH_CODES: usize = 29;
const LITERALS: usize = 256;
const L_CODES: usize = LITERALS + 1 + LENGTH_CODES;
const D_CODES: usize = 30;
const BL_CODES: usize = 19;
const HEAP_SIZE: usize = 2 * L_CODES + 1;
const MAX_BITS: usize = 15;
const MAX_BL_BITS: i32 = 7;
const END_BLOCK: usize = 256;
const REP_3_6: usize = 16;
const REPZ_3_10: usize = 17;
const REPZ_11_138: usize = 18;

const INIT_STATE: i32 = 42;
const GZIP_STATE: i32 = 57;
const BUSY_STATE: i32 = 113;
const FINISH_STATE: i32 = 666;

const MIN_MATCH: u32 = 3;
const MAX_MATCH: u32 = 258;
const MIN_LOOKAHEAD: u32 = MAX_MATCH + MIN_MATCH + 1;
const WIN_INIT: u64 = MAX_MATCH as u64;
const TOO_FAR: u32 = 4096;
const MAX_STORED: u32 = 65535;
const NIL: u32 = 0;
const PRESET_DICT: u32 = 0x20;

const STORED_BLOCK: u32 = 0;
const STATIC_TREES: u32 = 1;
const DYN_TREES: u32 = 2;

const EXTRA_LBITS: [u8; LENGTH_CODES] =
    [0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0];
const EXTRA_DBITS: [u8; D_CODES] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];
const EXTRA_BLBITS: [u8; BL_CODES] = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 3, 7];
const BL_ORDER: [u8; BL_CODES] = [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];

/// `ct_data`: C's two unions. `fc` is a node's frequency until
/// `gen_codes()` turns it into its code; `dl` is its father while the tree
/// is built and its bit length afterwards. The aliasing is load-bearing
/// (`scan_tree()` plants its guard in `dl`), so it is kept.
#[derive(Clone, Copy, Default)]
struct Ct {
    fc: u16,
    dl: u16,
}

/// `configuration_table`: good, lazy, nice, chain and the block function.
#[derive(Clone, Copy, PartialEq)]
enum Func {
    Stored,
    Fast,
    Slow,
}

const CONFIG: [(u32, u32, u32, u32, Func); 10] = [
    (0, 0, 0, 0, Func::Stored),
    (4, 4, 8, 4, Func::Fast),
    (4, 5, 16, 8, Func::Fast),
    (4, 6, 32, 32, Func::Fast),
    (4, 4, 16, 16, Func::Slow),
    (8, 16, 32, 32, Func::Slow),
    (8, 16, 128, 128, Func::Slow),
    (8, 32, 128, 256, Func::Slow),
    (32, 128, 258, 1024, Func::Slow),
    (32, 258, 258, 4096, Func::Slow),
];

#[derive(Clone, Copy, PartialEq)]
enum BlockState {
    NeedMore,
    BlockDone,
    FinishStarted,
    FinishDone,
}

/// The three trees `build_tree()` works on, with their `static_tree_desc`.
#[derive(Clone, Copy)]
enum Kind {
    L,
    D,
    Bl,
}

/// `trees.c`'s static tables, built the way `tr_static_init()` builds them.
struct Tables {
    static_ltree: [Ct; L_CODES + 2],
    static_dtree: [Ct; D_CODES],
    dist_code: [u8; 512],
    length_code: [u8; 256],
    base_length: [u32; LENGTH_CODES],
    base_dist: [u32; D_CODES],
}

fn tables() -> &'static Tables {
    static T: OnceLock<Tables> = OnceLock::new();
    T.get_or_init(|| {
        let mut t = Tables {
            static_ltree: [Ct::default(); L_CODES + 2],
            static_dtree: [Ct::default(); D_CODES],
            dist_code: [0; 512],
            length_code: [0; 256],
            base_length: [0; LENGTH_CODES],
            base_dist: [0; D_CODES],
        };
        let mut length = 0usize;
        let mut code = 0usize;
        while code < LENGTH_CODES - 1 {
            t.base_length[code] = length as u32;
            for _ in 0..(1 << EXTRA_LBITS[code]) {
                t.length_code[length] = code as u8;
                length += 1;
            }
            code += 1;
        }
        t.length_code[length - 1] = code as u8;
        let mut dist = 0usize;
        code = 0;
        while code < 16 {
            t.base_dist[code] = dist as u32;
            for _ in 0..(1 << EXTRA_DBITS[code]) {
                t.dist_code[dist] = code as u8;
                dist += 1;
            }
            code += 1;
        }
        dist >>= 7;
        while code < D_CODES {
            t.base_dist[code] = (dist << 7) as u32;
            for _ in 0..(1 << (EXTRA_DBITS[code] - 7)) {
                t.dist_code[256 + dist] = code as u8;
                dist += 1;
            }
            code += 1;
        }
        let mut bl_count = [0u16; MAX_BITS + 1];
        let mut n = 0;
        while n <= 143 {
            t.static_ltree[n].dl = 8;
            bl_count[8] += 1;
            n += 1;
        }
        while n <= 255 {
            t.static_ltree[n].dl = 9;
            bl_count[9] += 1;
            n += 1;
        }
        while n <= 279 {
            t.static_ltree[n].dl = 7;
            bl_count[7] += 1;
            n += 1;
        }
        while n <= 287 {
            t.static_ltree[n].dl = 8;
            bl_count[8] += 1;
            n += 1;
        }
        gen_codes(&mut t.static_ltree, L_CODES as i32 + 1, &bl_count);
        for (n, ct) in t.static_dtree.iter_mut().enumerate() {
            ct.dl = 5;
            ct.fc = bi_reverse(n as u32, 5) as u16;
        }
        t
    })
}

fn bi_reverse(mut code: u32, mut len: u32) -> u32 {
    let mut res = 0u32;
    loop {
        res |= code & 1;
        code >>= 1;
        res <<= 1;
        len -= 1;
        if len == 0 {
            break;
        }
    }
    res >> 1
}

fn gen_codes(tree: &mut [Ct], max_code: i32, bl_count: &[u16; MAX_BITS + 1]) {
    let mut next_code = [0u16; MAX_BITS + 1];
    let mut code: u32 = 0;
    for bits in 1..=MAX_BITS {
        code = (code + bl_count[bits - 1] as u32) << 1;
        next_code[bits] = code as u16;
    }
    for ct in tree.iter_mut().take((max_code + 1).max(0) as usize) {
        let len = ct.dl as usize;
        if len == 0 {
            continue;
        }
        ct.fc = bi_reverse(next_code[len] as u32, len as u32) as u16;
        next_code[len] = next_code[len].wrapping_add(1);
    }
}

fn adler32(adler: u32, buf: &[u8]) -> u32 {
    const BASE: u32 = 65521;
    let mut a = adler & 0xffff;
    let mut b = adler >> 16;
    for chunk in buf.chunks(5552) {
        for &x in chunk {
            a += x as u32;
            b += a;
        }
        a %= BASE;
        b %= BASE;
    }
    (b << 16) | a
}

fn crc32(crc: u32, buf: &[u8]) -> u32 {
    let mut h = crc32fast::Hasher::new_with_initial(crc);
    h.update(buf);
    h.finalize()
}

/// The `z_stream` fields one `deflate()` call works with: the caller's
/// input and output buffers and how far into each it has got.
struct Io<'a> {
    input: &'a [u8],
    in_pos: usize,
    out: &'a mut [u8],
    out_pos: usize,
}

impl Io<'_> {
    fn avail_in(&self) -> u32 {
        (self.input.len() - self.in_pos) as u32
    }
    fn avail_out(&self) -> u32 {
        (self.out.len() - self.out_pos) as u32
    }
}

/// One compressor: `deflate_state` plus the `z_stream` counters.
pub(crate) struct Deflate {
    status: i32,
    pending_buf: Vec<u8>,
    pending_out: usize,
    pending_buf_size: u64,
    wrap: i32,
    last_flush: i32,

    w_size: u32,
    w_bits: u32,
    w_mask: u32,
    window: Vec<u8>,
    window_size: u64,
    prev: Vec<u16>,
    head: Vec<u16>,
    ins_h: u32,
    hash_mask: u32,
    hash_shift: u32,
    block_start: i64,
    match_length: u32,
    prev_match: u32,
    match_available: bool,
    strstart: u32,
    match_start: u32,
    lookahead: u32,
    prev_length: u32,
    max_chain_length: u32,
    max_lazy_match: u32,
    level: i32,
    strategy: i32,
    good_match: u32,
    nice_match: u32,

    dyn_ltree: Vec<Ct>,
    dyn_dtree: Vec<Ct>,
    bl_tree: Vec<Ct>,
    l_max_code: i32,
    d_max_code: i32,
    bl_count: [u16; MAX_BITS + 1],
    heap: [i32; 2 * L_CODES + 1],
    heap_len: i32,
    heap_max: i32,
    depth: [u8; 2 * L_CODES + 1],
    sym_buf: Vec<u8>,
    sym_next: u32,
    sym_end: u32,
    opt_len: u64,
    static_len: u64,
    matches: u32,
    insert: u32,
    bi_buf: u16,
    bi_valid: i32,
    high_water: u64,

    /// `strm->adler`: the running Adler-32 (zlib) or CRC-32 (gzip).
    adler: u32,
    total_in: u64,
    total_out: u64,
}

impl Deflate {
    /// `deflateInit2()`: `None` where zlib answers `Z_STREAM_ERROR`. A
    /// negative `window_bits` is raw deflate, 16 and up adds a gzip
    /// wrapper; `level` -1 is zlib's default (6).
    pub(crate) fn new(level: i32, window_bits: i32, mem_level: i32, strategy: i32) -> Option<Deflate> {
        let mut level = level;
        let mut window_bits = window_bits;
        if level == -1 {
            level = 6;
        }
        let wrap;
        if window_bits < 0 {
            wrap = 0;
            if window_bits < -15 {
                return None;
            }
            window_bits = -window_bits;
        } else if window_bits > 15 {
            wrap = 2;
            window_bits -= 16;
        } else {
            wrap = 1;
        }
        if !(1..=9).contains(&mem_level)
            || !(8..=15).contains(&window_bits)
            || !(0..=9).contains(&level)
            || !(0..=Z_FIXED).contains(&strategy)
            || (window_bits == 8 && wrap != 1)
        {
            return None;
        }
        if window_bits == 8 {
            // zlib: "until 256-byte window bug fixed".
            window_bits = 9;
        }
        let w_bits = window_bits as u32;
        let w_size = 1u32 << w_bits;
        let hash_bits = mem_level as u32 + 7;
        let hash_size = 1u32 << hash_bits;
        let lit_bufsize = 1u32 << (mem_level as u32 + 6);
        let mut s = Deflate {
            status: INIT_STATE,
            pending_buf: Vec::with_capacity(lit_bufsize as usize * 4),
            pending_out: 0,
            pending_buf_size: lit_bufsize as u64 * 4,
            wrap,
            last_flush: -2,
            w_size,
            w_bits,
            w_mask: w_size - 1,
            window: vec![0; 2 * w_size as usize],
            window_size: 0,
            prev: vec![0; w_size as usize],
            head: vec![0; hash_size as usize],
            ins_h: 0,
            hash_mask: hash_size - 1,
            hash_shift: hash_bits.div_ceil(MIN_MATCH),
            block_start: 0,
            match_length: 0,
            prev_match: 0,
            match_available: false,
            strstart: 0,
            match_start: 0,
            lookahead: 0,
            prev_length: 0,
            max_chain_length: 0,
            max_lazy_match: 0,
            level,
            strategy,
            good_match: 0,
            nice_match: 0,
            dyn_ltree: vec![Ct::default(); HEAP_SIZE],
            dyn_dtree: vec![Ct::default(); 2 * D_CODES + 1],
            bl_tree: vec![Ct::default(); 2 * BL_CODES + 1],
            l_max_code: 0,
            d_max_code: 0,
            bl_count: [0; MAX_BITS + 1],
            heap: [0; 2 * L_CODES + 1],
            heap_len: 0,
            heap_max: 0,
            depth: [0; 2 * L_CODES + 1],
            sym_buf: vec![0; lit_bufsize as usize * 3],
            sym_next: 0,
            sym_end: (lit_bufsize - 1) * 3,
            opt_len: 0,
            static_len: 0,
            matches: 0,
            insert: 0,
            bi_buf: 0,
            bi_valid: 0,
            high_water: 0,
            adler: 0,
            total_in: 0,
            total_out: 0,
        };
        s.reset();
        Some(s)
    }

    /// `deflateReset()`.
    pub(crate) fn reset(&mut self) {
        self.total_in = 0;
        self.total_out = 0;
        self.pending_buf.clear();
        self.pending_out = 0;
        if self.wrap < 0 {
            self.wrap = -self.wrap;
        }
        self.status = if self.wrap == 2 { GZIP_STATE } else { INIT_STATE };
        self.adler = if self.wrap == 2 { 0 } else { 1 };
        self.last_flush = -2;
        self.tr_init();
        self.lm_init();
    }

    fn lm_init(&mut self) {
        self.window_size = 2 * self.w_size as u64;
        self.clear_hash();
        let (good, lazy, nice, chain, _) = CONFIG[self.level as usize];
        self.max_lazy_match = lazy;
        self.good_match = good;
        self.nice_match = nice;
        self.max_chain_length = chain;
        self.strstart = 0;
        self.block_start = 0;
        self.lookahead = 0;
        self.insert = 0;
        self.match_length = MIN_MATCH - 1;
        self.prev_length = MIN_MATCH - 1;
        self.match_available = false;
        self.ins_h = 0;
    }

    fn clear_hash(&mut self) {
        self.head.iter_mut().for_each(|h| *h = 0);
    }

    fn update_hash(&self, h: u32, c: u8) -> u32 {
        ((h << self.hash_shift) ^ c as u32) & self.hash_mask
    }

    /// `INSERT_STRING`: returns the previous head of the chain.
    fn insert_string(&mut self, str: u32) -> u32 {
        self.ins_h = self.update_hash(self.ins_h, self.window[(str + MIN_MATCH - 1) as usize]);
        let head = self.head[self.ins_h as usize];
        self.prev[(str & self.w_mask) as usize] = head;
        self.head[self.ins_h as usize] = str as u16;
        head as u32
    }

    /// `deflateSetDictionary()`: `Z_OK` or `Z_STREAM_ERROR`.
    pub(crate) fn set_dictionary(&mut self, dictionary: &[u8]) -> i32 {
        let wrap = self.wrap;
        if wrap == 2 || (wrap == 1 && self.status != INIT_STATE) || self.lookahead != 0 {
            return Z_STREAM_ERROR;
        }
        if wrap == 1 {
            self.adler = adler32(self.adler, dictionary);
        }
        self.wrap = 0;
        let mut dict = dictionary;
        if dict.len() >= self.w_size as usize {
            if wrap == 0 {
                self.clear_hash();
                self.strstart = 0;
                self.block_start = 0;
                self.insert = 0;
            }
            dict = &dict[dict.len() - self.w_size as usize..];
        }
        let mut dummy = [0u8; 0];
        let mut io = Io { input: dict, in_pos: 0, out: &mut dummy, out_pos: 0 };
        self.fill_window(&mut io);
        while self.lookahead >= MIN_MATCH {
            let mut str = self.strstart;
            let mut n = self.lookahead - (MIN_MATCH - 1);
            loop {
                self.ins_h = self.update_hash(self.ins_h, self.window[(str + MIN_MATCH - 1) as usize]);
                self.prev[(str & self.w_mask) as usize] = self.head[self.ins_h as usize];
                self.head[self.ins_h as usize] = str as u16;
                str += 1;
                n -= 1;
                if n == 0 {
                    break;
                }
            }
            self.strstart = str;
            self.lookahead = MIN_MATCH - 1;
            self.fill_window(&mut io);
        }
        // read_buf() counted the dictionary into total_in, as zlib's does:
        // deflateSetDictionary() restores next_in/avail_in, not the count.
        self.strstart += self.lookahead;
        self.block_start = self.strstart as i64;
        self.insert = self.lookahead;
        self.lookahead = 0;
        self.match_length = MIN_MATCH - 1;
        self.prev_length = MIN_MATCH - 1;
        self.match_available = false;
        self.wrap = wrap;
        Z_OK
    }

    // ---- output ----------------------------------------------------------

    fn pending(&self) -> usize {
        self.pending_buf.len() - self.pending_out
    }

    fn put_byte(&mut self, c: u8) {
        self.pending_buf.push(c);
    }

    fn put_short(&mut self, w: u16) {
        self.put_byte((w & 0xff) as u8);
        self.put_byte((w >> 8) as u8);
    }

    fn put_short_msb(&mut self, b: u32) {
        self.put_byte((b >> 8) as u8);
        self.put_byte((b & 0xff) as u8);
    }

    fn send_bits(&mut self, value: u32, length: i32) {
        const BUF_SIZE: i32 = 16;
        if self.bi_valid > BUF_SIZE - length {
            self.bi_buf |= (value << self.bi_valid) as u16;
            let b = self.bi_buf;
            self.put_short(b);
            self.bi_buf = ((value as u16 as u32) >> (BUF_SIZE - self.bi_valid)) as u16;
            self.bi_valid += length - BUF_SIZE;
        } else {
            self.bi_buf |= (value << self.bi_valid) as u16;
            self.bi_valid += length;
        }
    }

    fn bi_flush(&mut self) {
        if self.bi_valid == 16 {
            let b = self.bi_buf;
            self.put_short(b);
            self.bi_buf = 0;
            self.bi_valid = 0;
        } else if self.bi_valid >= 8 {
            let b = self.bi_buf;
            self.put_byte(b as u8);
            self.bi_buf >>= 8;
            self.bi_valid -= 8;
        }
    }

    fn bi_windup(&mut self) {
        if self.bi_valid > 8 {
            let b = self.bi_buf;
            self.put_short(b);
        } else if self.bi_valid > 0 {
            let b = self.bi_buf;
            self.put_byte(b as u8);
        }
        self.bi_buf = 0;
        self.bi_valid = 0;
    }

    fn flush_pending(&mut self, io: &mut Io) {
        self.bi_flush();
        let mut len = self.pending();
        if len > io.avail_out() as usize {
            len = io.avail_out() as usize;
        }
        if len == 0 {
            return;
        }
        io.out[io.out_pos..io.out_pos + len]
            .copy_from_slice(&self.pending_buf[self.pending_out..self.pending_out + len]);
        io.out_pos += len;
        self.pending_out += len;
        self.total_out += len as u64;
        if self.pending() == 0 {
            self.pending_buf.clear();
            self.pending_out = 0;
        }
    }

    // ---- input -----------------------------------------------------------

    /// `read_buf()` into the window at `at`.
    fn read_buf_window(&mut self, io: &mut Io, at: usize, size: u32) -> u32 {
        let mut len = io.avail_in();
        if len > size {
            len = size;
        }
        if len == 0 {
            return 0;
        }
        let src = &io.input[io.in_pos..io.in_pos + len as usize];
        self.window[at..at + len as usize].copy_from_slice(src);
        self.checksum(src);
        io.in_pos += len as usize;
        self.total_in += len as u64;
        len
    }

    /// `read_buf()` straight into the caller's output (`deflate_stored()`).
    fn read_buf_out(&mut self, io: &mut Io, len: u32) {
        if len == 0 {
            return;
        }
        let len = len as usize;
        let (i, o) = (io.in_pos, io.out_pos);
        io.out[o..o + len].copy_from_slice(&io.input[i..i + len]);
        let src = &io.input[i..i + len];
        self.checksum(src);
        io.in_pos += len;
        self.total_in += len as u64;
    }

    fn checksum(&mut self, buf: &[u8]) {
        if self.wrap == 1 {
            self.adler = adler32(self.adler, buf);
        } else if self.wrap == 2 {
            self.adler = crc32(self.adler, buf);
        }
    }

    fn max_dist(&self) -> u32 {
        self.w_size - MIN_LOOKAHEAD
    }

    fn slide_hash(&mut self) {
        let wsize = self.w_size;
        for h in self.head.iter_mut() {
            let m = *h as u32;
            *h = if m >= wsize { (m - wsize) as u16 } else { 0 };
        }
        for p in self.prev.iter_mut() {
            let m = *p as u32;
            *p = if m >= wsize { (m - wsize) as u16 } else { 0 };
        }
    }

    fn fill_window(&mut self, io: &mut Io) {
        let wsize = self.w_size;
        loop {
            let mut more = (self.window_size - self.lookahead as u64 - self.strstart as u64) as u32;
            if self.strstart >= wsize + self.max_dist() {
                let n = (wsize - more) as usize;
                self.window.copy_within(wsize as usize..wsize as usize + n, 0);
                self.match_start = self.match_start.wrapping_sub(wsize);
                self.strstart -= wsize;
                self.block_start -= wsize as i64;
                if self.insert > self.strstart {
                    self.insert = self.strstart;
                }
                self.slide_hash();
                more += wsize;
            }
            if io.avail_in() == 0 {
                break;
            }
            let at = (self.strstart + self.lookahead) as usize;
            let n = self.read_buf_window(io, at, more);
            self.lookahead += n;

            if self.lookahead + self.insert >= MIN_MATCH {
                let mut str = self.strstart - self.insert;
                self.ins_h = self.window[str as usize] as u32;
                self.ins_h = self.update_hash(self.ins_h, self.window[str as usize + 1]);
                while self.insert != 0 {
                    self.ins_h = self.update_hash(self.ins_h, self.window[(str + MIN_MATCH - 1) as usize]);
                    self.prev[(str & self.w_mask) as usize] = self.head[self.ins_h as usize];
                    self.head[self.ins_h as usize] = str as u16;
                    str += 1;
                    self.insert -= 1;
                    if self.lookahead + self.insert < MIN_MATCH {
                        break;
                    }
                }
            }
            if !(self.lookahead < MIN_LOOKAHEAD && io.avail_in() != 0) {
                break;
            }
        }
        // The high-water zeroing: bytes past the data that longest_match()
        // may look at must be defined. The window starts zeroed here, so
        // only the mark itself needs keeping.
        if self.high_water < self.window_size {
            let curr = self.strstart as u64 + self.lookahead as u64;
            if self.high_water < curr {
                let mut init = self.window_size - curr;
                if init > WIN_INIT {
                    init = WIN_INIT;
                }
                self.window[curr as usize..(curr + init) as usize].iter_mut().for_each(|b| *b = 0);
                self.high_water = curr + init;
            } else if self.high_water < curr + WIN_INIT {
                let mut init = curr + WIN_INIT - self.high_water;
                if init > self.window_size - self.high_water {
                    init = self.window_size - self.high_water;
                }
                let hw = self.high_water as usize;
                self.window[hw..hw + init as usize].iter_mut().for_each(|b| *b = 0);
                self.high_water += init;
            }
        }
    }

    fn longest_match(&mut self, mut cur_match: u32) -> u32 {
        let mut chain_length = self.max_chain_length;
        let scan = self.strstart as usize;
        let mut best_len = self.prev_length as i32;
        let mut nice_match = self.nice_match as i32;
        let limit = if self.strstart > self.max_dist() { self.strstart - self.max_dist() } else { NIL };
        let wmask = self.w_mask;
        let strend = scan + MAX_MATCH as usize;
        let w = &self.window;
        let mut scan_end1 = w[scan + best_len as usize - 1];
        let mut scan_end = w[scan + best_len as usize];

        if self.prev_length >= self.good_match {
            chain_length >>= 2;
        }
        if nice_match as u32 > self.lookahead {
            nice_match = self.lookahead as i32;
        }
        loop {
            let m = cur_match as usize;
            let skip = w[m + best_len as usize] != scan_end
                || w[m + best_len as usize - 1] != scan_end1
                || w[m] != w[scan]
                || w[m + 1] != w[scan + 1];
            if !skip {
                // scan[2] and match[2] are equal whenever the hash keys
                // are, so the comparison starts at the fourth byte.
                let mut s = scan + 2;
                let mut mm = m + 2;
                'cmp: loop {
                    for _ in 0..8 {
                        s += 1;
                        mm += 1;
                        if w[s] != w[mm] {
                            break 'cmp;
                        }
                    }
                    if s >= strend {
                        break;
                    }
                }
                let len = MAX_MATCH as i32 - (strend - s) as i32;
                if len > best_len {
                    self.match_start = cur_match;
                    best_len = len;
                    if len >= nice_match {
                        break;
                    }
                    scan_end1 = w[scan + best_len as usize - 1];
                    scan_end = w[scan + best_len as usize];
                }
            }
            cur_match = self.prev[(cur_match & wmask) as usize] as u32;
            if cur_match <= limit {
                break;
            }
            chain_length -= 1;
            if chain_length == 0 {
                break;
            }
        }
        if best_len as u32 <= self.lookahead {
            best_len as u32
        } else {
            self.lookahead
        }
    }

    // ---- trees.c ---------------------------------------------------------

    fn tr_init(&mut self) {
        self.bi_buf = 0;
        self.bi_valid = 0;
        self.init_block();
    }

    fn init_block(&mut self) {
        self.dyn_ltree.iter_mut().take(L_CODES).for_each(|c| c.fc = 0);
        self.dyn_dtree.iter_mut().take(D_CODES).for_each(|c| c.fc = 0);
        self.bl_tree.iter_mut().take(BL_CODES).for_each(|c| c.fc = 0);
        self.dyn_ltree[END_BLOCK].fc = 1;
        self.opt_len = 0;
        self.static_len = 0;
        self.sym_next = 0;
        self.matches = 0;
    }

    /// `_tr_tally()`: record a literal (`dist` 0) or a match; `true` when
    /// the block is full.
    fn tally(&mut self, dist: u32, lc: u32) -> bool {
        let i = self.sym_next as usize;
        self.sym_buf[i] = dist as u8;
        self.sym_buf[i + 1] = (dist >> 8) as u8;
        self.sym_buf[i + 2] = lc as u8;
        self.sym_next += 3;
        if dist == 0 {
            self.dyn_ltree[lc as usize].fc += 1;
        } else {
            self.matches += 1;
            let dist = dist - 1;
            let t = tables();
            self.dyn_ltree[t.length_code[lc as usize] as usize + LITERALS + 1].fc += 1;
            self.dyn_dtree[d_code(t, dist) as usize].fc += 1;
        }
        self.sym_next == self.sym_end
    }

    fn smaller(tree: &[Ct], depth: &[u8], n: i32, m: i32) -> bool {
        let (n, m) = (n as usize, m as usize);
        tree[n].fc < tree[m].fc || (tree[n].fc == tree[m].fc && depth[n] <= depth[m])
    }

    fn pqdownheap(&mut self, tree: &[Ct], mut k: i32) {
        let v = self.heap[k as usize];
        let mut j = k << 1;
        while j <= self.heap_len {
            if j < self.heap_len
                && Self::smaller(tree, &self.depth, self.heap[j as usize + 1], self.heap[j as usize])
            {
                j += 1;
            }
            if Self::smaller(tree, &self.depth, v, self.heap[j as usize]) {
                break;
            }
            self.heap[k as usize] = self.heap[j as usize];
            k = j;
            j <<= 1;
        }
        self.heap[k as usize] = v;
    }

    fn gen_bitlen(&mut self, tree: &mut [Ct], kind: Kind, max_code: i32) {
        let t = tables();
        let (stree, extra, base, max_length): (Option<&[Ct]>, &[u8], i32, i32) = match kind {
            Kind::L => (Some(&t.static_ltree[..]), &EXTRA_LBITS[..], LITERALS as i32 + 1, MAX_BITS as i32),
            Kind::D => (Some(&t.static_dtree[..]), &EXTRA_DBITS[..], 0, MAX_BITS as i32),
            Kind::Bl => (None, &EXTRA_BLBITS[..], 0, MAX_BL_BITS),
        };
        let mut overflow = 0;
        self.bl_count = [0; MAX_BITS + 1];
        tree[self.heap[self.heap_max as usize] as usize].dl = 0;
        let mut h = self.heap_max + 1;
        while h < HEAP_SIZE as i32 {
            let n = self.heap[h as usize];
            let mut bits = tree[tree[n as usize].dl as usize].dl as i32 + 1;
            if bits > max_length {
                bits = max_length;
                overflow += 1;
            }
            tree[n as usize].dl = bits as u16;
            h += 1;
            if n > max_code {
                continue;
            }
            self.bl_count[bits as usize] += 1;
            let mut xbits = 0;
            if n >= base {
                xbits = extra[(n - base) as usize] as i32;
            }
            let f = tree[n as usize].fc as u64;
            self.opt_len += f * (bits + xbits) as u64;
            if let Some(st) = stree {
                self.static_len += f * (st[n as usize].dl as i32 + xbits) as u64;
            }
        }
        if overflow == 0 {
            return;
        }
        loop {
            let mut bits = max_length - 1;
            while self.bl_count[bits as usize] == 0 {
                bits -= 1;
            }
            self.bl_count[bits as usize] -= 1;
            self.bl_count[bits as usize + 1] += 2;
            self.bl_count[max_length as usize] -= 1;
            overflow -= 2;
            if overflow <= 0 {
                break;
            }
        }
        let mut h = HEAP_SIZE as i32;
        let mut bits = max_length;
        while bits != 0 {
            let mut n = self.bl_count[bits as usize] as i32;
            while n != 0 {
                h -= 1;
                let m = self.heap[h as usize];
                if m > max_code {
                    continue;
                }
                if tree[m as usize].dl as i32 != bits {
                    self.opt_len = self
                        .opt_len
                        .wrapping_add(((bits as i64 - tree[m as usize].dl as i64) * tree[m as usize].fc as i64) as u64);
                    tree[m as usize].dl = bits as u16;
                }
                n -= 1;
            }
            bits -= 1;
        }
    }

    /// `build_tree()`; returns the tree's `max_code`.
    fn build_tree(&mut self, tree: &mut [Ct], kind: Kind) -> i32 {
        let t = tables();
        let (stree, elems): (Option<&[Ct]>, i32) = match kind {
            Kind::L => (Some(&t.static_ltree[..]), L_CODES as i32),
            Kind::D => (Some(&t.static_dtree[..]), D_CODES as i32),
            Kind::Bl => (None, BL_CODES as i32),
        };
        let mut max_code = -1;
        self.heap_len = 0;
        self.heap_max = HEAP_SIZE as i32;
        for n in 0..elems {
            if tree[n as usize].fc != 0 {
                self.heap_len += 1;
                self.heap[self.heap_len as usize] = n;
                max_code = n;
                self.depth[n as usize] = 0;
            } else {
                tree[n as usize].dl = 0;
            }
        }
        while self.heap_len < 2 {
            let node = if max_code < 2 {
                max_code += 1;
                max_code
            } else {
                0
            };
            self.heap_len += 1;
            self.heap[self.heap_len as usize] = node;
            tree[node as usize].fc = 1;
            self.depth[node as usize] = 0;
            self.opt_len = self.opt_len.wrapping_sub(1);
            if let Some(st) = stree {
                self.static_len = self.static_len.wrapping_sub(st[node as usize].dl as u64);
            }
        }
        let mut n = self.heap_len / 2;
        while n >= 1 {
            self.pqdownheap(tree, n);
            n -= 1;
        }
        let mut node = elems;
        loop {
            // pqremove
            let n = self.heap[1];
            self.heap[1] = self.heap[self.heap_len as usize];
            self.heap_len -= 1;
            self.pqdownheap(tree, 1);
            let m = self.heap[1];
            self.heap_max -= 1;
            self.heap[self.heap_max as usize] = n;
            self.heap_max -= 1;
            self.heap[self.heap_max as usize] = m;
            tree[node as usize].fc = tree[n as usize].fc.wrapping_add(tree[m as usize].fc);
            let dn = self.depth[n as usize];
            let dm = self.depth[m as usize];
            self.depth[node as usize] = (if dn >= dm { dn } else { dm }) + 1;
            tree[n as usize].dl = node as u16;
            tree[m as usize].dl = node as u16;
            self.heap[1] = node;
            node += 1;
            self.pqdownheap(tree, 1);
            if self.heap_len < 2 {
                break;
            }
        }
        self.heap_max -= 1;
        self.heap[self.heap_max as usize] = self.heap[1];
        self.gen_bitlen(tree, kind, max_code);
        let bl_count = self.bl_count;
        gen_codes(tree, max_code, &bl_count);
        max_code
    }

    fn scan_tree(&mut self, tree: &mut [Ct], max_code: i32) {
        let mut prevlen: i32 = -1;
        let mut nextlen = tree[0].dl as i32;
        let mut count = 0;
        let mut max_count = 7;
        let mut min_count = 4;
        if nextlen == 0 {
            max_count = 138;
            min_count = 3;
        }
        tree[(max_code + 1) as usize].dl = 0xffff;
        for n in 0..=max_code {
            let curlen = nextlen;
            nextlen = tree[(n + 1) as usize].dl as i32;
            count += 1;
            if count < max_count && curlen == nextlen {
                continue;
            } else if count < min_count {
                self.bl_tree[curlen as usize].fc += count as u16;
            } else if curlen != 0 {
                if curlen != prevlen {
                    self.bl_tree[curlen as usize].fc += 1;
                }
                self.bl_tree[REP_3_6].fc += 1;
            } else if count <= 10 {
                self.bl_tree[REPZ_3_10].fc += 1;
            } else {
                self.bl_tree[REPZ_11_138].fc += 1;
            }
            count = 0;
            prevlen = curlen;
            if nextlen == 0 {
                max_count = 138;
                min_count = 3;
            } else if curlen == nextlen {
                max_count = 6;
                min_count = 3;
            } else {
                max_count = 7;
                min_count = 4;
            }
        }
    }

    fn send_code(&mut self, c: usize, tree: &[Ct]) {
        self.send_bits(tree[c].fc as u32, tree[c].dl as i32);
    }

    fn send_tree(&mut self, tree: &[Ct], max_code: i32) {
        let bl = std::mem::take(&mut self.bl_tree);
        let mut prevlen: i32 = -1;
        let mut nextlen = tree[0].dl as i32;
        let mut count = 0;
        let mut max_count = 7;
        let mut min_count = 4;
        if nextlen == 0 {
            max_count = 138;
            min_count = 3;
        }
        for n in 0..=max_code {
            let curlen = nextlen;
            nextlen = tree[(n + 1) as usize].dl as i32;
            count += 1;
            if count < max_count && curlen == nextlen {
                continue;
            } else if count < min_count {
                loop {
                    self.send_code(curlen as usize, &bl);
                    count -= 1;
                    if count == 0 {
                        break;
                    }
                }
            } else if curlen != 0 {
                if curlen != prevlen {
                    self.send_code(curlen as usize, &bl);
                    count -= 1;
                }
                self.send_code(REP_3_6, &bl);
                self.send_bits((count - 3) as u32, 2);
            } else if count <= 10 {
                self.send_code(REPZ_3_10, &bl);
                self.send_bits((count - 3) as u32, 3);
            } else {
                self.send_code(REPZ_11_138, &bl);
                self.send_bits((count - 11) as u32, 7);
            }
            count = 0;
            prevlen = curlen;
            if nextlen == 0 {
                max_count = 138;
                min_count = 3;
            } else if curlen == nextlen {
                max_count = 6;
                min_count = 3;
            } else {
                max_count = 7;
                min_count = 4;
            }
        }
        self.bl_tree = bl;
    }

    fn build_bl_tree(&mut self) -> i32 {
        let mut l = std::mem::take(&mut self.dyn_ltree);
        let lmax = self.l_max_code;
        self.scan_tree(&mut l, lmax);
        self.dyn_ltree = l;
        let mut d = std::mem::take(&mut self.dyn_dtree);
        let dmax = self.d_max_code;
        self.scan_tree(&mut d, dmax);
        self.dyn_dtree = d;
        let mut bl = std::mem::take(&mut self.bl_tree);
        self.build_tree(&mut bl, Kind::Bl);
        self.bl_tree = bl;
        let mut max_blindex = BL_CODES as i32 - 1;
        while max_blindex >= 3 {
            if self.bl_tree[BL_ORDER[max_blindex as usize] as usize].dl != 0 {
                break;
            }
            max_blindex -= 1;
        }
        self.opt_len += 3 * (max_blindex as u64 + 1) + 5 + 5 + 4;
        max_blindex
    }

    fn send_all_trees(&mut self, lcodes: i32, dcodes: i32, blcodes: i32) {
        self.send_bits((lcodes - 257) as u32, 5);
        self.send_bits((dcodes - 1) as u32, 5);
        self.send_bits((blcodes - 4) as u32, 4);
        for rank in 0..blcodes as usize {
            let len = self.bl_tree[BL_ORDER[rank] as usize].dl as u32;
            self.send_bits(len, 3);
        }
        let l = std::mem::take(&mut self.dyn_ltree);
        self.send_tree(&l, lcodes - 1);
        self.dyn_ltree = l;
        let d = std::mem::take(&mut self.dyn_dtree);
        self.send_tree(&d, dcodes - 1);
        self.dyn_dtree = d;
    }

    fn compress_block(&mut self, ltree: &[Ct], dtree: &[Ct]) {
        let t = tables();
        let mut sx = 0usize;
        if self.sym_next != 0 {
            loop {
                let mut dist = self.sym_buf[sx] as u32;
                dist += (self.sym_buf[sx + 1] as u32) << 8;
                let mut lc = self.sym_buf[sx + 2] as u32;
                sx += 3;
                if dist == 0 {
                    self.send_code(lc as usize, ltree);
                } else {
                    let code = t.length_code[lc as usize] as usize;
                    self.send_code(code + LITERALS + 1, ltree);
                    let extra = EXTRA_LBITS[code] as i32;
                    if extra != 0 {
                        lc -= t.base_length[code];
                        self.send_bits(lc, extra);
                    }
                    dist -= 1;
                    let code = d_code(t, dist) as usize;
                    self.send_code(code, dtree);
                    let extra = EXTRA_DBITS[code] as i32;
                    if extra != 0 {
                        dist -= t.base_dist[code];
                        self.send_bits(dist, extra);
                    }
                }
                if sx >= self.sym_next as usize {
                    break;
                }
            }
        }
        self.send_code(END_BLOCK, ltree);
    }

    /// `_tr_stored_block()`: `buf` is a range of the window, or `None`
    /// for the empty marker block.
    fn tr_stored_block(&mut self, buf: Option<(usize, usize)>, stored_len: u32, last: bool) {
        self.send_bits((STORED_BLOCK << 1) + last as u32, 3);
        self.bi_windup();
        self.put_short(stored_len as u16);
        self.put_short(!(stored_len as u16));
        if let Some((start, _)) = buf {
            let len = stored_len as usize;
            self.pending_buf.extend_from_slice(&self.window[start..start + len]);
        }
    }

    fn tr_align(&mut self) {
        self.send_bits(STATIC_TREES << 1, 3);
        let t = tables();
        self.send_code(END_BLOCK, &t.static_ltree);
        self.bi_flush();
    }

    fn tr_flush_block(&mut self, buf: Option<usize>, stored_len: u32, last: bool) {
        let mut max_blindex = 0;
        let mut opt_lenb;
        let static_lenb;
        if self.level > 0 {
            let mut l = std::mem::take(&mut self.dyn_ltree);
            self.l_max_code = self.build_tree(&mut l, Kind::L);
            self.dyn_ltree = l;
            let mut d = std::mem::take(&mut self.dyn_dtree);
            self.d_max_code = self.build_tree(&mut d, Kind::D);
            self.dyn_dtree = d;
            max_blindex = self.build_bl_tree();
            opt_lenb = (self.opt_len.wrapping_add(3 + 7)) >> 3;
            static_lenb = (self.static_len.wrapping_add(3 + 7)) >> 3;
            if static_lenb <= opt_lenb {
                opt_lenb = static_lenb;
            }
        } else {
            opt_lenb = stored_len as u64 + 5;
            static_lenb = opt_lenb;
        }
        let t = tables();
        if stored_len as u64 + 4 <= opt_lenb && buf.is_some() {
            self.tr_stored_block(buf.map(|b| (b, stored_len as usize)), stored_len, last);
        } else if self.strategy == Z_FIXED || static_lenb == opt_lenb {
            self.send_bits((STATIC_TREES << 1) + last as u32, 3);
            self.compress_block(&t.static_ltree, &t.static_dtree);
        } else {
            self.send_bits((DYN_TREES << 1) + last as u32, 3);
            let (lm, dm) = (self.l_max_code, self.d_max_code);
            self.send_all_trees(lm + 1, dm + 1, max_blindex + 1);
            let l = std::mem::take(&mut self.dyn_ltree);
            let d = std::mem::take(&mut self.dyn_dtree);
            self.compress_block(&l, &d);
            self.dyn_ltree = l;
            self.dyn_dtree = d;
        }
        self.init_block();
        if last {
            self.bi_windup();
        }
    }

    /// `FLUSH_BLOCK_ONLY`.
    fn flush_block_only(&mut self, io: &mut Io, last: bool) {
        let buf = if self.block_start >= 0 { Some(self.block_start as usize) } else { None };
        let len = (self.strstart as i64 - self.block_start) as u32;
        self.tr_flush_block(buf, len, last);
        self.block_start = self.strstart as i64;
        self.flush_pending(io);
    }

    // ---- the block functions --------------------------------------------

    fn deflate_stored(&mut self, io: &mut Io, flush: i32) -> BlockState {
        let mut min_block = (self.pending_buf_size - 5).min(self.w_size as u64) as u32;
        let mut len;
        let mut left;
        let mut have;
        let mut last = false;
        let mut used = io.avail_in();
        loop {
            len = MAX_STORED;
            have = ((self.bi_valid + 42) >> 3) as u32;
            if io.avail_out() < have {
                break;
            }
            have = io.avail_out() - have;
            left = (self.strstart as i64 - self.block_start) as u32;
            if len as u64 > left as u64 + io.avail_in() as u64 {
                len = left + io.avail_in();
            }
            if len > have {
                len = have;
            }
            if len < min_block
                && ((len == 0 && flush != Z_FINISH) || flush == Z_NO_FLUSH || len != left + io.avail_in())
            {
                break;
            }
            last = flush == Z_FINISH && len == left + io.avail_in();
            self.tr_stored_block(None, 0, last);
            let p = self.pending_buf.len();
            self.pending_buf[p - 4] = len as u8;
            self.pending_buf[p - 3] = (len >> 8) as u8;
            self.pending_buf[p - 2] = !len as u8;
            self.pending_buf[p - 1] = (!len >> 8) as u8;
            self.flush_pending(io);
            if left != 0 {
                if left > len {
                    left = len;
                }
                let bs = self.block_start as usize;
                let o = io.out_pos;
                io.out[o..o + left as usize].copy_from_slice(&self.window[bs..bs + left as usize]);
                io.out_pos += left as usize;
                self.total_out += left as u64;
                self.block_start += left as i64;
                len -= left;
            }
            if len != 0 {
                self.read_buf_out(io, len);
                io.out_pos += len as usize;
                self.total_out += len as u64;
            }
            if last {
                break;
            }
        }

        used -= io.avail_in();
        if used != 0 {
            if used >= self.w_size {
                self.matches = 2;
                let ws = self.w_size as usize;
                let src = &io.input[io.in_pos - ws..io.in_pos];
                self.window[..ws].copy_from_slice(src);
                self.strstart = self.w_size;
                self.insert = self.strstart;
            } else {
                if self.window_size - self.strstart as u64 <= used as u64 {
                    self.strstart -= self.w_size;
                    let ws = self.w_size as usize;
                    let n = self.strstart as usize;
                    self.window.copy_within(ws..ws + n, 0);
                    if self.matches < 2 {
                        self.matches += 1;
                    }
                    if self.insert > self.strstart {
                        self.insert = self.strstart;
                    }
                }
                let at = self.strstart as usize;
                let src = &io.input[io.in_pos - used as usize..io.in_pos];
                self.window[at..at + used as usize].copy_from_slice(src);
                self.strstart += used;
                self.insert += used.min(self.w_size - self.insert);
            }
            self.block_start = self.strstart as i64;
        }
        if self.high_water < self.strstart as u64 {
            self.high_water = self.strstart as u64;
        }

        if last {
            return BlockState::FinishDone;
        }
        if flush != Z_NO_FLUSH
            && flush != Z_FINISH
            && io.avail_in() == 0
            && self.strstart as i64 == self.block_start
        {
            return BlockState::BlockDone;
        }

        have = (self.window_size - self.strstart as u64) as u32;
        if io.avail_in() > have && self.block_start >= self.w_size as i64 {
            self.block_start -= self.w_size as i64;
            self.strstart -= self.w_size;
            let ws = self.w_size as usize;
            let n = self.strstart as usize;
            self.window.copy_within(ws..ws + n, 0);
            if self.matches < 2 {
                self.matches += 1;
            }
            have += self.w_size;
            if self.insert > self.strstart {
                self.insert = self.strstart;
            }
        }
        if have > io.avail_in() {
            have = io.avail_in();
        }
        if have != 0 {
            let at = self.strstart as usize;
            self.read_buf_window(io, at, have);
            self.strstart += have;
            self.insert += have.min(self.w_size - self.insert);
        }
        if self.high_water < self.strstart as u64 {
            self.high_water = self.strstart as u64;
        }

        have = ((self.bi_valid + 42) >> 3) as u32;
        have = (self.pending_buf_size - have as u64).min(MAX_STORED as u64) as u32;
        min_block = have.min(self.w_size);
        left = (self.strstart as i64 - self.block_start) as u32;
        if left >= min_block
            || ((left != 0 || flush == Z_FINISH) && flush != Z_NO_FLUSH && io.avail_in() == 0 && left <= have)
        {
            len = left.min(have);
            last = flush == Z_FINISH && io.avail_in() == 0 && len == left;
            let bs = self.block_start as usize;
            self.tr_stored_block(Some((bs, len as usize)), len, last);
            self.block_start += len as i64;
            self.flush_pending(io);
        }
        if last {
            BlockState::FinishStarted
        } else {
            BlockState::NeedMore
        }
    }

    fn deflate_fast(&mut self, io: &mut Io, flush: i32) -> BlockState {
        loop {
            if self.lookahead < MIN_LOOKAHEAD {
                self.fill_window(io);
                if self.lookahead < MIN_LOOKAHEAD && flush == Z_NO_FLUSH {
                    return BlockState::NeedMore;
                }
                if self.lookahead == 0 {
                    break;
                }
            }
            let mut hash_head = NIL;
            if self.lookahead >= MIN_MATCH {
                hash_head = self.insert_string(self.strstart);
            }
            if hash_head != NIL && self.strstart - hash_head <= self.max_dist() {
                self.match_length = self.longest_match(hash_head);
            }
            let bflush;
            if self.match_length >= MIN_MATCH {
                bflush = self.tally(self.strstart - self.match_start, self.match_length - MIN_MATCH);
                self.lookahead -= self.match_length;
                if self.match_length <= self.max_lazy_match && self.lookahead >= MIN_MATCH {
                    self.match_length -= 1;
                    loop {
                        self.strstart += 1;
                        self.insert_string(self.strstart);
                        self.match_length -= 1;
                        if self.match_length == 0 {
                            break;
                        }
                    }
                    self.strstart += 1;
                } else {
                    self.strstart += self.match_length;
                    self.match_length = 0;
                    self.ins_h = self.window[self.strstart as usize] as u32;
                    self.ins_h = self.update_hash(self.ins_h, self.window[self.strstart as usize + 1]);
                }
            } else {
                bflush = self.tally(0, self.window[self.strstart as usize] as u32);
                self.lookahead -= 1;
                self.strstart += 1;
            }
            if bflush {
                self.flush_block_only(io, false);
                if io.avail_out() == 0 {
                    return BlockState::NeedMore;
                }
            }
        }
        self.insert = if self.strstart < MIN_MATCH - 1 { self.strstart } else { MIN_MATCH - 1 };
        self.finish_block(io, flush)
    }

    fn deflate_slow(&mut self, io: &mut Io, flush: i32) -> BlockState {
        loop {
            if self.lookahead < MIN_LOOKAHEAD {
                self.fill_window(io);
                if self.lookahead < MIN_LOOKAHEAD && flush == Z_NO_FLUSH {
                    return BlockState::NeedMore;
                }
                if self.lookahead == 0 {
                    break;
                }
            }
            let mut hash_head = NIL;
            if self.lookahead >= MIN_MATCH {
                hash_head = self.insert_string(self.strstart);
            }
            self.prev_length = self.match_length;
            self.prev_match = self.match_start;
            self.match_length = MIN_MATCH - 1;

            if hash_head != NIL
                && self.prev_length < self.max_lazy_match
                && self.strstart - hash_head <= self.max_dist()
            {
                self.match_length = self.longest_match(hash_head);
                if self.match_length <= 5
                    && (self.strategy == Z_FILTERED
                        || (self.match_length == MIN_MATCH && self.strstart - self.match_start > TOO_FAR))
                {
                    self.match_length = MIN_MATCH - 1;
                }
            }
            if self.prev_length >= MIN_MATCH && self.match_length <= self.prev_length {
                let max_insert = self.strstart + self.lookahead - MIN_MATCH;
                let bflush = self.tally(self.strstart - 1 - self.prev_match, self.prev_length - MIN_MATCH);
                self.lookahead -= self.prev_length - 1;
                self.prev_length -= 2;
                loop {
                    self.strstart += 1;
                    if self.strstart <= max_insert {
                        self.insert_string(self.strstart);
                    }
                    self.prev_length -= 1;
                    if self.prev_length == 0 {
                        break;
                    }
                }
                self.match_available = false;
                self.match_length = MIN_MATCH - 1;
                self.strstart += 1;
                if bflush {
                    self.flush_block_only(io, false);
                    if io.avail_out() == 0 {
                        return BlockState::NeedMore;
                    }
                }
            } else if self.match_available {
                let bflush = self.tally(0, self.window[self.strstart as usize - 1] as u32);
                if bflush {
                    self.flush_block_only(io, false);
                }
                self.strstart += 1;
                self.lookahead -= 1;
                if io.avail_out() == 0 {
                    return BlockState::NeedMore;
                }
            } else {
                self.match_available = true;
                self.strstart += 1;
                self.lookahead -= 1;
            }
        }
        if self.match_available {
            self.tally(0, self.window[self.strstart as usize - 1] as u32);
            self.match_available = false;
        }
        self.insert = if self.strstart < MIN_MATCH - 1 { self.strstart } else { MIN_MATCH - 1 };
        self.finish_block(io, flush)
    }

    fn deflate_rle(&mut self, io: &mut Io, flush: i32) -> BlockState {
        loop {
            if self.lookahead <= MAX_MATCH {
                self.fill_window(io);
                if self.lookahead <= MAX_MATCH && flush == Z_NO_FLUSH {
                    return BlockState::NeedMore;
                }
                if self.lookahead == 0 {
                    break;
                }
            }
            self.match_length = 0;
            if self.lookahead >= MIN_MATCH && self.strstart > 0 {
                let w = &self.window;
                let mut scan = self.strstart as usize - 1;
                let prev = w[scan];
                if prev == w[scan + 1] && prev == w[scan + 2] && prev == w[scan + 3] {
                    scan += 3;
                    let strend = self.strstart as usize + MAX_MATCH as usize;
                    'cmp: loop {
                        for _ in 0..8 {
                            scan += 1;
                            if prev != w[scan] {
                                break 'cmp;
                            }
                        }
                        if scan >= strend {
                            break;
                        }
                    }
                    self.match_length = MAX_MATCH - (strend - scan) as u32;
                    if self.match_length > self.lookahead {
                        self.match_length = self.lookahead;
                    }
                }
            }
            let bflush;
            if self.match_length >= MIN_MATCH {
                bflush = self.tally(1, self.match_length - MIN_MATCH);
                self.lookahead -= self.match_length;
                self.strstart += self.match_length;
                self.match_length = 0;
            } else {
                bflush = self.tally(0, self.window[self.strstart as usize] as u32);
                self.lookahead -= 1;
                self.strstart += 1;
            }
            if bflush {
                self.flush_block_only(io, false);
                if io.avail_out() == 0 {
                    return BlockState::NeedMore;
                }
            }
        }
        self.insert = 0;
        self.finish_block(io, flush)
    }

    fn deflate_huff(&mut self, io: &mut Io, flush: i32) -> BlockState {
        loop {
            if self.lookahead == 0 {
                self.fill_window(io);
                if self.lookahead == 0 {
                    if flush == Z_NO_FLUSH {
                        return BlockState::NeedMore;
                    }
                    break;
                }
            }
            self.match_length = 0;
            let bflush = self.tally(0, self.window[self.strstart as usize] as u32);
            self.lookahead -= 1;
            self.strstart += 1;
            if bflush {
                self.flush_block_only(io, false);
                if io.avail_out() == 0 {
                    return BlockState::NeedMore;
                }
            }
        }
        self.insert = 0;
        self.finish_block(io, flush)
    }

    /// The common tail of the compressing block functions.
    fn finish_block(&mut self, io: &mut Io, flush: i32) -> BlockState {
        if flush == Z_FINISH {
            self.flush_block_only(io, true);
            if io.avail_out() == 0 {
                return BlockState::FinishStarted;
            }
            return BlockState::FinishDone;
        }
        if self.sym_next != 0 {
            self.flush_block_only(io, false);
            if io.avail_out() == 0 {
                return BlockState::NeedMore;
            }
        }
        BlockState::BlockDone
    }

    // ---- deflate() -------------------------------------------------------

    /// One `deflate()` call: compress from `input` into `out` with the
    /// given flush mode. Answers zlib's status and how many bytes were
    /// consumed and produced.
    pub(crate) fn deflate(&mut self, input: &[u8], out: &mut [u8], flush: i32) -> (i32, usize, usize) {
        let mut io = Io { input, in_pos: 0, out, out_pos: 0 };
        let ret = self.deflate_io(&mut io, flush);
        (ret, io.in_pos, io.out_pos)
    }

    fn deflate_io(&mut self, io: &mut Io, flush: i32) -> i32 {
        if !(0..=Z_BLOCK).contains(&flush) {
            return Z_STREAM_ERROR;
        }
        if self.status == FINISH_STATE && flush != Z_FINISH {
            return Z_STREAM_ERROR;
        }
        if io.avail_out() == 0 {
            return Z_BUF_ERROR;
        }
        let old_flush = self.last_flush;
        self.last_flush = flush;

        if self.pending() != 0 {
            self.flush_pending(io);
            if io.avail_out() == 0 {
                self.last_flush = -1;
                return Z_OK;
            }
        } else if io.avail_in() == 0 && rank(flush) <= rank(old_flush) && flush != Z_FINISH {
            return Z_BUF_ERROR;
        }

        if self.status == FINISH_STATE && io.avail_in() != 0 {
            return Z_BUF_ERROR;
        }

        if self.status == INIT_STATE && self.wrap == 0 {
            self.status = BUSY_STATE;
        }
        if self.status == INIT_STATE {
            let mut header = (8 + ((self.w_bits - 8) << 4)) << 8;
            let level_flags = if self.strategy >= Z_HUFFMAN_ONLY || self.level < 2 {
                0
            } else if self.level < 6 {
                1
            } else if self.level == 6 {
                2
            } else {
                3
            };
            header |= level_flags << 6;
            if self.strstart != 0 {
                header |= PRESET_DICT;
            }
            header += 31 - (header % 31);
            self.put_short_msb(header);
            if self.strstart != 0 {
                let a = self.adler;
                self.put_short_msb(a >> 16);
                self.put_short_msb(a & 0xffff);
            }
            self.adler = 1;
            self.status = BUSY_STATE;
            self.flush_pending(io);
            if self.pending() != 0 {
                self.last_flush = -1;
                return Z_OK;
            }
        }
        if self.status == GZIP_STATE {
            self.adler = 0;
            self.put_byte(31);
            self.put_byte(139);
            self.put_byte(8);
            for _ in 0..5 {
                self.put_byte(0);
            }
            let xfl = if self.level == 9 {
                2
            } else if self.strategy >= Z_HUFFMAN_ONLY || self.level < 2 {
                4
            } else {
                0
            };
            self.put_byte(xfl);
            self.put_byte(OS_CODE);
            self.status = BUSY_STATE;
            self.flush_pending(io);
            if self.pending() != 0 {
                self.last_flush = -1;
                return Z_OK;
            }
        }

        if io.avail_in() != 0 || self.lookahead != 0 || (flush != Z_NO_FLUSH && self.status != FINISH_STATE) {
            let bstate = if self.level == 0 {
                self.deflate_stored(io, flush)
            } else if self.strategy == Z_HUFFMAN_ONLY {
                self.deflate_huff(io, flush)
            } else if self.strategy == Z_RLE {
                self.deflate_rle(io, flush)
            } else if CONFIG[self.level as usize].4 == Func::Fast {
                self.deflate_fast(io, flush)
            } else {
                self.deflate_slow(io, flush)
            };
            if bstate == BlockState::FinishStarted || bstate == BlockState::FinishDone {
                self.status = FINISH_STATE;
            }
            if bstate == BlockState::NeedMore || bstate == BlockState::FinishStarted {
                if io.avail_out() == 0 {
                    self.last_flush = -1;
                }
                return Z_OK;
            }
            if bstate == BlockState::BlockDone {
                if flush == Z_PARTIAL_FLUSH {
                    self.tr_align();
                } else if flush != Z_BLOCK {
                    self.tr_stored_block(None, 0, false);
                    if flush == Z_FULL_FLUSH {
                        self.clear_hash();
                        if self.lookahead == 0 {
                            self.strstart = 0;
                            self.block_start = 0;
                            self.insert = 0;
                        }
                    }
                }
                self.flush_pending(io);
                if io.avail_out() == 0 {
                    self.last_flush = -1;
                    return Z_OK;
                }
            }
        }

        if flush != Z_FINISH {
            return Z_OK;
        }
        if self.wrap <= 0 {
            return Z_STREAM_END;
        }
        let a = self.adler;
        if self.wrap == 2 {
            let t = self.total_in;
            for b in [a as u8, (a >> 8) as u8, (a >> 16) as u8, (a >> 24) as u8] {
                self.put_byte(b);
            }
            for b in [t as u8, (t >> 8) as u8, (t >> 16) as u8, (t >> 24) as u8] {
                self.put_byte(b);
            }
        } else {
            self.put_short_msb(a >> 16);
            self.put_short_msb(a & 0xffff);
        }
        self.flush_pending(io);
        if self.wrap > 0 {
            self.wrap = -self.wrap;
        }
        if self.pending() != 0 {
            Z_OK
        } else {
            Z_STREAM_END
        }
    }
}

fn rank(f: i32) -> i32 {
    (f * 2) - if f > 4 { 9 } else { 0 }
}

fn d_code(t: &Tables, dist: u32) -> u8 {
    if dist < 256 {
        t.dist_code[dist as usize]
    } else {
        t.dist_code[256 + (dist >> 7) as usize]
    }
}

/// zlib's `zError()` texts, which php prints in its warnings.
pub(crate) fn z_error(code: i32) -> &'static str {
    match code {
        2 => "need dictionary",
        1 => "stream end",
        0 => "",
        -1 => "file error",
        -2 => "stream error",
        -3 => "data error",
        -4 => "insufficient memory",
        -5 => "buffer error",
        -6 => "incompatible version",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{Compress, Compression, FlushCompress};

    /// A deterministic byte source with a tunable amount of repetition, so
    /// the tests reach literals, short and long matches and block splits.
    fn sample(len: usize, seed: u64, alphabet: u8) -> Vec<u8> {
        let mut x = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let mut out = Vec::with_capacity(len);
        while out.len() < len {
            x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let r = (x >> 33) as u32;
            if r.is_multiple_of(7) && out.len() > 40 {
                // copy a run from earlier: a match
                let back = 1 + (r as usize >> 3) % out.len().min(40000);
                let n = 3 + (r as usize >> 5) % 60;
                let start = out.len() - back;
                for i in 0..n {
                    let b = out[start + i % back];
                    out.push(b);
                }
            } else {
                out.push(b'a' + (r % alphabet as u32) as u8);
            }
        }
        out.truncate(len);
        out
    }

    fn flate_flush(f: i32) -> FlushCompress {
        match f {
            Z_NO_FLUSH => FlushCompress::None,
            Z_PARTIAL_FLUSH => FlushCompress::Partial,
            Z_SYNC_FLUSH => FlushCompress::Sync,
            Z_FULL_FLUSH => FlushCompress::Full,
            _ => FlushCompress::Finish,
        }
    }

    /// Drive both compressors through the same calls — the same input
    /// chunks, flush modes and output-buffer sizes — and compare.
    fn compare(data: &[u8], level: u32, wrap: i32, chunks: &[(usize, i32)], outsz: usize) {
        let wb = 15;
        let mut ours = Deflate::new(level as i32, match wrap { 0 => -wb, 1 => wb, _ => wb + 16 }, 8, 0).unwrap();
        let mut theirs = match wrap {
            0 => Compress::new(Compression::new(level), false),
            1 => Compress::new(Compression::new(level), true),
            _ => Compress::new_gzip(Compression::new(level), wb as u8),
        };
        let mut a = Vec::new();
        let mut b = Vec::new();
        let mut pos = 0;
        for &(n, flush) in chunks {
            let end = (pos + n).min(data.len());
            // ours
            let mut inp = &data[pos..end];
            loop {
                let mut buf = vec![0u8; outsz];
                let (st, used, made) = ours.deflate(inp, &mut buf, flush);
                a.extend_from_slice(&buf[..made]);
                inp = &inp[used..];
                if st != Z_OK || (made < outsz && inp.is_empty()) {
                    break;
                }
            }
            // theirs
            let mut inp = &data[pos..end];
            loop {
                let mut buf = vec![0u8; outsz];
                let bi = theirs.total_in();
                let bo = theirs.total_out();
                let st = theirs.compress(inp, &mut buf, flate_flush(flush));
                let used = (theirs.total_in() - bi) as usize;
                let made = (theirs.total_out() - bo) as usize;
                b.extend_from_slice(&buf[..made]);
                inp = &inp[used..];
                let done = !matches!(st, Ok(flate2::Status::Ok));
                if done || (made < outsz && inp.is_empty()) {
                    break;
                }
            }
            pos = end;
        }
        assert!(a == b, "level {level} wrap {wrap} outsz {outsz}: {} vs {} bytes", a.len(), b.len());
    }

    #[test]
    fn matches_system_zlib_one_shot() {
        for (len, alpha) in [(0usize, 4u8), (1, 4), (100, 3), (5000, 20), (70000, 4), (200000, 26), (300000, 2)] {
            let data = sample(len, len as u64 + 7, alpha);
            for level in 0..=9 {
                for wrap in 0..3 {
                    compare(&data, level, wrap, &[(len, Z_FINISH)], len + len / 64 + 64);
                }
            }
        }
    }

    #[test]
    fn matches_system_zlib_with_small_buffers_and_flushes() {
        let data = sample(150000, 99, 5);
        let pattern: Vec<(usize, i32)> = vec![
            (1000, Z_NO_FLUSH),
            (7, Z_SYNC_FLUSH),
            (30000, Z_NO_FLUSH),
            (0, Z_PARTIAL_FLUSH),
            (40000, Z_FULL_FLUSH),
            (1, Z_NO_FLUSH),
            (0, Z_SYNC_FLUSH),
            (0, Z_SYNC_FLUSH),
            (70000, Z_NO_FLUSH),
            (10000, Z_FINISH),
        ];
        for level in 0..=9 {
            // An output buffer too small for a flush marker never lets a
            // flush finish (zlib re-emits the marker), so flushing
            // patterns start at 7 bytes; one byte at a time is tried on a
            // pattern without flushes.
            for outsz in [7usize, 64, 8192, 100000] {
                compare(&data, level, 2, &pattern, outsz);
            }
            compare(&data, level, 1, &[(90000, Z_NO_FLUSH), (60000, Z_FINISH)], 1);
        }
    }

    #[test]
    fn matches_system_zlib_dictionary() {
        let data = sample(20000, 5, 6);
        let dict = sample(3000, 5, 6);
        for level in [1u32, 6, 9] {
            for wrap in [0, 1] {
                let mut ours = Deflate::new(level as i32, if wrap == 0 { -15 } else { 15 }, 8, 0).unwrap();
                assert_eq!(ours.set_dictionary(&dict), Z_OK);
                let mut theirs = Compress::new(Compression::new(level), wrap == 1);
                theirs.set_dictionary(&dict).unwrap();
                let mut a = vec![0u8; 40000];
                let (_, _, n) = ours.deflate(&data, &mut a, Z_FINISH);
                a.truncate(n);
                let mut b = Vec::with_capacity(40000);
                theirs.compress_vec(&data, &mut b, FlushCompress::Finish).unwrap();
                assert!(a == b, "dictionary level {level} wrap {wrap}");
            }
        }
    }

    #[test]
    fn matches_system_zlib_window_bits() {
        let data = sample(100000, 11, 8);
        for wb in 9..=15u8 {
            for level in [0u32, 1, 4, 6, 9] {
                let mut ours = Deflate::new(level as i32, wb as i32, 8, 0).unwrap();
                let mut theirs = Compress::new_with_window_bits(Compression::new(level), true, wb);
                let mut a = vec![0u8; 120000];
                let (_, _, n) = ours.deflate(&data, &mut a, Z_FINISH);
                a.truncate(n);
                let mut b = Vec::with_capacity(120000);
                theirs.compress_vec(&data, &mut b, FlushCompress::Finish).unwrap();
                assert!(a == b, "window {wb} level {level}");
            }
        }
    }
}
