//! MQ arithmetic encoder (T.88 Annex E) and the integer / symbol-ID coders (Annex A).

/// (Qe, NMPS, NLPS, SWITCH) for each of the 47 probability states.
const QE: [(u16, u8, u8, bool); 47] = [
    (0x5601, 1, 1, true), (0x3401, 2, 6, false), (0x1801, 3, 9, false), (0x0AC1, 4, 12, false),
    (0x0521, 5, 29, false), (0x0221, 38, 33, false), (0x5601, 7, 6, true), (0x5401, 8, 14, false),
    (0x4801, 9, 14, false), (0x3801, 10, 14, false), (0x3001, 11, 17, false), (0x2401, 12, 18, false),
    (0x1C01, 13, 20, false), (0x1601, 29, 21, false), (0x5601, 15, 14, true), (0x5401, 16, 14, false),
    (0x5101, 17, 15, false), (0x4801, 18, 16, false), (0x3801, 19, 17, false), (0x3401, 20, 18, false),
    (0x3001, 21, 19, false), (0x2801, 22, 19, false), (0x2401, 23, 20, false), (0x2201, 24, 21, false),
    (0x1C01, 25, 22, false), (0x1801, 26, 23, false), (0x1601, 27, 24, false), (0x1401, 28, 25, false),
    (0x1201, 29, 26, false), (0x1101, 30, 27, false), (0x0AC1, 31, 28, false), (0x09C1, 32, 29, false),
    (0x08A1, 33, 30, false), (0x0521, 34, 31, false), (0x0441, 35, 32, false), (0x02A1, 36, 33, false),
    (0x0221, 37, 34, false), (0x0141, 38, 35, false), (0x0111, 39, 36, false), (0x0085, 40, 37, false),
    (0x0049, 41, 38, false), (0x0025, 42, 39, false), (0x0015, 43, 40, false), (0x0009, 44, 41, false),
    (0x0005, 45, 42, false), (0x0001, 45, 43, false), (0x5601, 46, 46, false),
];

/// One adaptive context: low 7 bits = state index, bit 7 = MPS.
pub type Cx = u8;

/// MQ arithmetic encoder.
pub struct Mq {
    a: u32,
    c: u32,
    ct: u32,
    /// `out[0]` is the dummy byte "before" the stream (removed by [`Mq::finish`]).
    out: Vec<u8>,
}

impl Default for Mq {
    fn default() -> Self {
        Self { a: 0x8000, c: 0, ct: 12, out: vec![0] }
    }
}

impl Mq {
    pub fn new() -> Self {
        Self::default()
    }

    fn byte_out(&mut self) {
        let b = self.out.last_mut().expect("dummy byte");
        if *b == 0xFF {
            self.out.push((self.c >> 20) as u8);
            self.c &= 0xFFFFF;
            self.ct = 7;
        } else if self.c < 0x800_0000 {
            self.out.push((self.c >> 19) as u8);
            self.c &= 0x7FFFF;
            self.ct = 8;
        } else {
            *b = b.wrapping_add(1);
            if *b == 0xFF {
                self.c &= 0x7FF_FFFF;
                self.out.push((self.c >> 20) as u8);
                self.c &= 0xFFFFF;
                self.ct = 7;
            } else {
                self.out.push((self.c >> 19) as u8);
                self.c &= 0x7FFFF;
                self.ct = 8;
            }
        }
    }

    fn renorm(&mut self) {
        loop {
            self.a <<= 1;
            self.c <<= 1;
            self.ct -= 1;
            if self.ct == 0 {
                self.byte_out();
            }
            if self.a & 0x8000 != 0 {
                break;
            }
        }
    }

    /// Encode bit `d` in context `cx`.
    #[inline]
    pub fn encode(&mut self, cx: &mut Cx, d: bool) {
        let (qe, nmps, nlps, switch) = QE[(*cx & 0x7F) as usize];
        let qe = qe as u32;
        let mut mps = *cx >> 7;
        self.a -= qe;
        if d as u8 == mps {
            if self.a & 0x8000 == 0 {
                if self.a < qe {
                    self.a = qe;
                } else {
                    self.c += qe;
                }
                *cx = (*cx & 0x80) | nmps;
                self.renorm();
            } else {
                self.c += qe;
            }
        } else {
            if self.a < qe {
                self.c += qe;
            } else {
                self.a = qe;
            }
            if switch {
                mps ^= 1;
            }
            *cx = (mps << 7) | nlps;
            self.renorm();
        }
    }

    /// Flush (SETBITS + two BYTEOUTs + 0xFF 0xAC marker) and return the coded bytes.
    pub fn finish(mut self) -> Vec<u8> {
        let tempc = self.c + self.a;
        self.c |= 0xFFFF;
        if self.c >= tempc {
            self.c -= 0x8000;
        }
        self.c <<= self.ct;
        self.byte_out();
        self.c <<= self.ct;
        self.byte_out();
        if *self.out.last().expect("byte") != 0xFF {
            self.out.push(0xFF);
        }
        self.out.push(0xAC);
        self.out.remove(0);
        self.out
    }
}

/// Integer arithmetic coder context set (IAx, T.88 A.2): 512 contexts.
pub struct IntCtx([Cx; 512]);

impl Default for IntCtx {
    fn default() -> Self {
        Self([0; 512])
    }
}

impl IntCtx {
    /// Encode `v`, or OOB when `v` is `None`.
    pub fn encode(&mut self, mq: &mut Mq, v: Option<i32>) {
        let (s, mag) = match v {
            None => (true, 0u32),
            Some(v) if v < 0 => (true, v.unsigned_abs()),
            Some(v) => (false, v as u32),
        };
        let (ones, ntail, off) = match (v, mag) {
            (None, _) | (_, 0..=3) => (0, 2, 0),
            (_, 4..=19) => (1, 4, 4),
            (_, 20..=83) => (2, 6, 20),
            (_, 84..=339) => (3, 8, 84),
            (_, 340..=4435) => (4, 12, 340),
            _ => (5, 32, 4436),
        };
        let mut prev: usize = 1;
        let mut bit = |mq: &mut Mq, b: bool, tail: bool, prev: &mut usize| {
            mq.encode(&mut self.0[*prev], b);
            *prev = if tail { ((*prev << 1) & 511) | (*prev & 256) | b as usize } else { (*prev << 1) | b as usize };
        };
        bit(mq, s, false, &mut prev);
        for _ in 0..ones {
            bit(mq, true, false, &mut prev);
        }
        if ones < 5 {
            bit(mq, false, false, &mut prev);
        }
        let t = mag - off;
        for i in (0..ntail).rev() {
            bit(mq, (t >> i) & 1 == 1, true, &mut prev);
        }
    }
}

/// Symbol-ID coder (IAID, T.88 A.3) for a code length of `len` bits.
pub struct IaidCtx {
    len: u32,
    cx: Vec<Cx>,
}

impl IaidCtx {
    pub fn new(len: u32) -> Self {
        Self { len, cx: vec![0; 1usize << (len + 1)] }
    }

    pub fn encode(&mut self, mq: &mut Mq, id: u32) {
        let mut prev = 1usize;
        for i in (0..self.len).rev() {
            let b = (id >> i) & 1 == 1;
            mq.encode(&mut self.cx[prev], b);
            prev = (prev << 1) | b as usize;
        }
    }
}

/// Symbol-ID code length: ceil(log2(n)) (T.88 6.4.10 / 6.5.8.2.3); 0 for n <= 1.
pub fn code_len(n: u64) -> u32 {
    if n <= 1 { 0 } else { 64 - (n - 1).leading_zeros() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_len_matches_spec() {
        assert_eq!([0, 1, 2, 3, 4, 5, 512, 513].map(code_len), [0, 0, 1, 2, 2, 3, 9, 10]);
    }

    #[test]
    fn stream_ends_with_marker() {
        let mut mq = Mq::new();
        let mut cx = 0;
        for i in 0..1000 {
            mq.encode(&mut cx, i % 7 == 0);
        }
        let out = mq.finish();
        assert_eq!(&out[out.len() - 2..], &[0xFF, 0xAC]);
    }
}
