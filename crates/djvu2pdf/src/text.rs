//! Invisible text (render mode 3) from the DjVu text layer, in glyphless simple fonts
//! with ToUnicode maps, so PDF viewers can select, search and copy it.

use std::collections::{BTreeSet, HashMap};
use std::io::Write;

use djvu_rs::text::{TextLayer, TextZone, TextZoneKind};

/// A tiny TrueType font: every code maps to one empty glyph 500 units wide.
pub const GLYPHLESS_TTF: &[u8] = include_bytes!("glyphless.ttf");

/// Words of one text line, in DjVu pixels with a bottom-left origin.
pub struct Line {
    pub bbox: [i32; 4],
    pub words: Vec<([i32; 4], String)>,
}

fn bottom_left(z: &TextZone, page_height: u32) -> [i32; 4] {
    let r = &z.rect;
    let h = page_height as i32;
    [
        r.x as i32,
        h - (r.y + r.height) as i32,
        (r.x + r.width) as i32,
        h - r.y as i32,
    ]
}

fn words_in(z: &TextZone, page_height: u32, out: &mut Vec<([i32; 4], String)>) {
    if matches!(z.kind, TextZoneKind::Word | TextZoneKind::Character) || z.children.is_empty() {
        // control characters (OCR debris such as U+001F) carry no text
        let t: String = z.text.chars().filter(|c| !c.is_control()).collect();
        let t = t.trim();
        if !t.is_empty() {
            out.push((bottom_left(z, page_height), t.to_string()));
        }
        return;
    }
    for k in &z.children {
        words_in(k, page_height, out);
    }
}

fn walk(z: &TextZone, page_height: u32, lines: &mut Vec<Line>) {
    if matches!(
        z.kind,
        TextZoneKind::Line | TextZoneKind::Word | TextZoneKind::Character
    ) || z.children.is_empty()
    {
        let mut words = Vec::new();
        words_in(z, page_height, &mut words);
        if !words.is_empty() {
            lines.push(Line {
                bbox: bottom_left(z, page_height),
                words,
            });
        }
        return;
    }
    for k in &z.children {
        walk(k, page_height, lines);
    }
}

/// Lines of a page's text layer. Zones below line level are flattened into words;
/// text outside any line becomes a one-word line.
pub fn lines(layer: &TextLayer, page_height: u32) -> Vec<Line> {
    let mut out = Vec::new();
    for z in &layer.zones {
        walk(z, page_height, &mut out);
    }
    out
}

/// Assignment of characters to single-byte codes in a few simple fonts. Printable ASCII
/// keeps its own code in font 0 (readable content streams).
pub struct TextFonts {
    map: HashMap<char, (usize, u8)>,
    /// Per font: code → character.
    pub fonts: Vec<Vec<(u8, char)>>,
}

const RESERVED: [u8; 5] = [0x0A, 0x0D, b'(', b')', b'\\'];

impl TextFonts {
    pub fn new(chars: &BTreeSet<char>) -> Self {
        let mut fonts: Vec<Vec<(u8, char)>> = vec![Vec::new()];
        let mut rest = Vec::new();
        for &ch in chars {
            if (' '..='~').contains(&ch) {
                fonts[0].push((ch as u8, ch));
            } else {
                rest.push(ch);
            }
        }
        let taken: BTreeSet<u8> = fonts[0].iter().map(|&(c, _)| c).collect();
        let mut free: Vec<u8> = (1..=255)
            .filter(|c| !taken.contains(c) && !RESERVED.contains(c))
            .collect();
        free.reverse();
        for ch in rest {
            if free.is_empty() {
                fonts.push(Vec::new());
                free = (1..=255).filter(|c| !RESERVED.contains(c)).rev().collect();
            }
            let code = free.pop().unwrap();
            fonts.last_mut().unwrap().push((code, ch));
        }
        let mut map = HashMap::new();
        for (fi, f) in fonts.iter_mut().enumerate() {
            f.sort_unstable();
            for &(code, ch) in f.iter() {
                map.insert(ch, (fi, code));
            }
        }
        Self { map, fonts }
    }

    /// Split text into (font index, codes) runs.
    fn runs(&self, text: &str) -> Vec<(usize, Vec<u8>)> {
        let mut res: Vec<(usize, Vec<u8>)> = Vec::new();
        for ch in text.chars() {
            let (fi, code) = self.map[&ch];
            match res.last_mut() {
                Some((f, codes)) if *f == fi => codes.push(code),
                _ => res.push((fi, vec![code])),
            }
        }
        res
    }

    /// The ToUnicode CMap of font `fi`.
    pub fn to_unicode(&self, fi: usize) -> Vec<u8> {
        let mut s = String::from(
            "/CIDInit /ProcSet findresource begin 12 dict begin begincmap\n\
             /CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n\
             /CMapName /Adobe-Identity-UCS def /CMapType 2 def\n\
             1 begincodespacerange <00> <FF> endcodespacerange\n",
        );
        for part in self.fonts[fi].chunks(100) {
            s += &format!("{} beginbfchar\n", part.len());
            for &(code, ch) in part {
                let mut buf = [0u16; 2];
                let hex: String = ch
                    .encode_utf16(&mut buf)
                    .iter()
                    .map(|u| format!("{u:04X}"))
                    .collect();
                s += &format!("<{code:02X}> <{hex}>\n");
            }
            s += "endbfchar\n";
        }
        s += "endcmap CMapName currentdict /CMap defineresource pop end end";
        s.into_bytes()
    }
}

/// Characters used by `lines` (including the word separator).
pub fn chars(lines: &[Line], out: &mut BTreeSet<char>) {
    for l in lines {
        for (_, w) in &l.words {
            out.extend(w.chars());
        }
    }
    if !lines.is_empty() {
        out.insert(' ');
    }
}

fn escape(codes: &[u8], out: &mut Vec<u8>) {
    for &c in codes {
        match c {
            b'\\' | b'(' | b')' => out.extend_from_slice(&[b'\\', c]),
            b'\r' => out.extend_from_slice(b"\\r"),
            _ => out.push(c),
        }
    }
}

/// Content operators for invisible text, in DjVu pixel units (the caller's CTM maps
/// pixels to points). Font `i` is resource `/T<i>`; used fonts are added to `used`.
///
/// One Tf/Tm per line (font size = line height, baseline at 20 % of it so the font box
/// −0.2..0.8 em covers the line box); each word then costs a relative Td, a Tz (only
/// when it changes) and its string with a trailing space so extraction separates words.
/// Words are scaled horizontally so their glyph boxes match the DjVu word boxes.
pub fn text_ops(lines: &[Line], fonts: &TextFonts, used: &mut BTreeSet<usize>) -> Vec<u8> {
    let mut out = b"BT 3 Tr".to_vec();
    let mut cur: Option<(usize, i32)> = None;
    let mut tz_cur = 100;
    for line in lines {
        let [_, ly0, _, ly1] = line.bbox;
        let h = (ly1 - ly0).max(1);
        let base = ly0 + (0.2 * h as f64).round() as i32;
        let mut prev_x = None;
        for ([x0, _, x1, _], word) in &line.words {
            let t = format!("{word} ");
            let n = t.chars().count() - 1;
            let tz = if n > 0 {
                ((100.0 * (x1 - x0) as f64 / (0.5 * h as f64 * n as f64)).round() as i32).max(1)
            } else {
                100
            };
            out.push(b'\n');
            match prev_x {
                None => write!(out, "1 0 0 1 {x0} {base} Tm").unwrap(),
                Some(px) => write!(out, "{} 0 Td", x0 - px).unwrap(),
            }
            prev_x = Some(*x0);
            if tz != tz_cur {
                write!(out, " {tz} Tz").unwrap();
                tz_cur = tz;
            }
            let mut need_space = true;
            for (fi, codes) in fonts.runs(&t) {
                if cur != Some((fi, h)) {
                    write!(out, " /T{fi} {h} Tf").unwrap();
                    cur = Some((fi, h));
                    used.insert(fi);
                    need_space = true;
                }
                if need_space {
                    out.push(b' ');
                }
                out.push(b'(');
                escape(&codes, &mut out);
                out.extend_from_slice(b")Tj");
                need_space = false;
            }
        }
    }
    out.extend_from_slice(b"\nET");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fonts_keep_ascii_and_spill_over() {
        let mut chars: BTreeSet<char> = "Hello (x)".chars().collect();
        chars.extend((0..300).map(|i| char::from_u32(0x400 + i).unwrap()));
        let f = TextFonts::new(&chars);
        assert_eq!(f.map[&'H'], (0, b'H'));
        assert_eq!(f.map[&'('], (0, b'('));
        assert_eq!(f.fonts.len(), 2);
        assert!(
            f.fonts
                .iter()
                .flatten()
                .all(|&(c, ch)| !RESERVED.contains(&c) || ch.is_ascii())
        );
        let cmap = String::from_utf8(f.to_unicode(1)).unwrap();
        assert!(cmap.contains("beginbfchar"));
    }

    #[test]
    fn ops_scale_words_to_boxes() {
        let chars: BTreeSet<char> = "ab c(".chars().collect();
        let fonts = TextFonts::new(&chars);
        let lines = [Line {
            bbox: [10, 100, 200, 120],
            words: vec![
                ([10, 100, 30, 120], "ab".into()),
                ([50, 100, 60, 120], "c(".into()),
            ],
        }];
        let mut used = BTreeSet::new();
        let ops = String::from_utf8(text_ops(&lines, &fonts, &mut used)).unwrap();
        assert_eq!(
            ops,
            "BT 3 Tr\n1 0 0 1 10 104 Tm /T0 20 Tf (ab )Tj\n40 0 Td 50 Tz (c\\( )Tj\nET"
        );
        assert_eq!(used.into_iter().collect::<Vec<_>>(), [0]);
    }
}
