//! Segment headers (T.88 7.2) and the page information segment (7.4.8).

pub(crate) const SYMBOL_DICTIONARY: u8 = 0;
pub(crate) const IMMEDIATE_TEXT_REGION: u8 = 6;
pub(crate) const PAGE_INFORMATION: u8 = 48;

pub(crate) fn put16(v: &mut Vec<u8>, x: u16) {
    v.extend_from_slice(&x.to_be_bytes());
}

pub(crate) fn put32(v: &mut Vec<u8>, x: u32) {
    v.extend_from_slice(&x.to_be_bytes());
}

/// Append one segment. `refs` are the referred-to segment numbers; at most 4, because
/// decoders disagree on the size of the long form's retention field (T.88 says
/// ceil((n+1)/8) bytes, jbig2dec reads floor((n+1)/8), poppler floor((n+9)/8)).
pub(crate) fn segment(
    out: &mut Vec<u8>,
    number: u32,
    kind: u8,
    refs: &[u32],
    page: u8,
    data: &[u8],
) {
    assert!(
        refs.len() <= 4,
        "segments refer to at most 4 others (long form avoided)"
    );
    put32(out, number);
    out.push(kind); // page association field size: 1 byte (bit 6 clear)
    out.push((refs.len() as u8) << 5);
    for &r in refs {
        if number <= 256 {
            out.push(r as u8);
        } else if number <= 65536 {
            put16(out, r as u16);
        } else {
            put32(out, r);
        }
    }
    out.push(page);
    put32(out, data.len() as u32);
    out.extend_from_slice(data);
}

/// Page information segment data: size, resolution, flags (eventually lossless, may
/// contain refinements, default pixel 0, combination operator OR), no striping.
pub(crate) fn page_information(width: u32, height: u32, ppm: u32, refinements: bool) -> Vec<u8> {
    let mut d = Vec::with_capacity(19);
    put32(&mut d, width);
    put32(&mut d, height);
    put32(&mut d, ppm);
    put32(&mut d, ppm);
    d.push(0x01 | if refinements { 0x02 } else { 0 });
    put16(&mut d, 0);
    d
}
