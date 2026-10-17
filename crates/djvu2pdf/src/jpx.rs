//! JPEG 2000 (JP2 file format) encoding with the openjp2 port of OpenJPEG, in memory.

use std::ffi::c_void;

use openjp2::openjpeg::*;
use openjp2::opj_image_comptparm;

struct Sink {
    buf: Vec<u8>,
    pos: usize,
}

impl Sink {
    fn seek(&mut self, pos: usize) {
        if pos > self.buf.len() {
            self.buf.resize(pos, 0);
        }
        self.pos = pos;
    }
}

extern "C" fn write_cb(p: *mut c_void, n: OPJ_SIZE_T, user: *mut c_void) -> OPJ_SIZE_T {
    // SAFETY: `user` is the `Sink` registered in `encode`; `p` points to `n` bytes.
    let (s, src) = unsafe {
        (
            &mut *(user as *mut Sink),
            std::slice::from_raw_parts(p as *const u8, n),
        )
    };
    let end = s.pos + n;
    if end > s.buf.len() {
        s.buf.resize(end, 0);
    }
    s.buf[s.pos..end].copy_from_slice(src);
    s.pos = end;
    n
}

extern "C" fn skip_cb(n: OPJ_OFF_T, user: *mut c_void) -> OPJ_OFF_T {
    // SAFETY: as in `write_cb`.
    let s = unsafe { &mut *(user as *mut Sink) };
    s.seek((s.pos as i64 + n).max(0) as usize);
    n
}

extern "C" fn seek_cb(n: OPJ_OFF_T, user: *mut c_void) -> OPJ_BOOL {
    // SAFETY: as in `write_cb`.
    let s = unsafe { &mut *(user as *mut Sink) };
    s.seek(n.max(0) as usize);
    1
}

/// Number of resolution levels for a `w` x `h` image (as the prototype used with
/// `opj_compress -n`): log2 of the short side minus one, clamped to 1..=6.
fn resolutions(w: u32, h: u32) -> i32 {
    let short = w.min(h).max(1);
    ((31 - short.leading_zeros()) as i32 - 1).clamp(1, 6)
}

/// Encode 8-bit interleaved samples (1 = gray or 3 = RGB components) as a JP2 file.
/// `target_bytes`: one quality layer at about that size (irreversible 9/7 wavelet and
/// ICT); `None` = lossless (reversible 5/3). Returns `None` if OpenJPEG fails.
pub fn encode(
    samples: &[u8],
    w: u32,
    h: u32,
    comps: u32,
    target_bytes: Option<usize>,
) -> Option<Vec<u8>> {
    assert!(comps == 1 || comps == 3);
    assert_eq!(samples.len(), (w * h * comps) as usize);
    // SAFETY: plain use of the OpenJPEG C API; every object created here is destroyed
    // before returning, and the sink outlives the stream that writes into it.
    unsafe {
        let mut params: opj_cparameters_t = std::mem::zeroed();
        opj_set_default_encoder_parameters(&mut params);
        params.numresolution = resolutions(w, h);
        params.tcp_numlayers = 1;
        params.cp_disto_alloc = 1;
        match target_bytes {
            Some(t) => {
                let raw = samples.len() as f32;
                params.tcp_rates[0] = (raw / t.max(1) as f32).max(1.0);
                params.irreversible = 1;
            }
            None => params.tcp_rates[0] = 0.0,
        }
        params.tcp_mct = (comps == 3) as _;
        let mut cp: Vec<opj_image_comptparm> = (0..comps)
            .map(|_| {
                let mut c: opj_image_comptparm = std::mem::zeroed();
                c.dx = 1;
                c.dy = 1;
                c.w = w;
                c.h = h;
                c.prec = 8;
                c
            })
            .collect();
        let space = if comps == 3 {
            OPJ_CLRSPC_SRGB
        } else {
            OPJ_CLRSPC_GRAY
        };
        let image = opj_image_create(comps, cp.as_mut_ptr(), space);
        if image.is_null() {
            return None;
        }
        (*image).x1 = w;
        (*image).y1 = h;
        for c in 0..comps as usize {
            let comp = &mut *(*image).comps.add(c);
            let dst = std::slice::from_raw_parts_mut(comp.data, (w * h) as usize);
            for (i, v) in dst.iter_mut().enumerate() {
                *v = samples[i * comps as usize + c] as i32;
            }
        }
        let codec = opj_create_compress(OPJ_CODEC_JP2);
        let mut sink = Box::new(Sink {
            buf: Vec::new(),
            pos: 0,
        });
        let stream = opj_stream_create(1 << 20, 0);
        opj_stream_set_user_data(stream, &mut *sink as *mut Sink as *mut c_void, None);
        opj_stream_set_write_function(stream, Some(write_cb));
        opj_stream_set_skip_function(stream, Some(skip_cb));
        opj_stream_set_seek_function(stream, Some(seek_cb));
        let ok = opj_setup_encoder(codec, &mut params, image) != 0
            && opj_start_compress(codec, image, stream) != 0
            && opj_encode(codec, stream) != 0
            && opj_end_compress(codec, stream) != 0;
        opj_stream_destroy(stream);
        opj_destroy_codec(codec);
        opj_image_destroy(image);
        ok.then(|| {
            let n = sink.pos.max(sink.buf.len());
            sink.buf.truncate(n);
            sink.buf
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolution_levels() {
        assert_eq!(
            [1, 7, 8, 64, 1044, 5000].map(|s| resolutions(s, 10_000)),
            [1, 1, 2, 5, 6, 6]
        );
    }

    #[test]
    fn encodes_jp2_near_budget() {
        let (w, h) = (300, 200);
        let rgb: Vec<u8> = (0..w * h * 3).map(|i| ((i * 7919) % 251) as u8).collect();
        let lossy = encode(&rgb, w, h, 3, Some(20_000)).unwrap();
        assert_eq!(&lossy[4..8], b"jP  ", "JP2 signature box");
        assert!(lossy.len() < 21_000, "{}", lossy.len());
        let lossless = encode(&rgb, w, h, 3, None).unwrap();
        assert!(lossless.len() > lossy.len());
        let gray: Vec<u8> = (0..w * h).map(|i| (i % 256) as u8).collect();
        assert!(encode(&gray, w, h, 1, Some(2_000)).is_some());
    }
}
