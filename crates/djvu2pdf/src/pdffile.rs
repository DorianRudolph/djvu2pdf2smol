//! Streaming PDF file writer: objects are serialized with `pdf-writer`; dictionaries are
//! packed into compressed object streams, streams are written directly, and the file
//! ends with a cross-reference stream (PDF 1.5+). Output is written as it is produced,
//! so memory use does not grow with the document.

use std::io::{self, Write};

use miniz_oxide::deflate::compress_to_vec_zlib;
use pdf_writer::{Chunk, Filter, Name, Obj, Ref, Settings};

/// Objects per object stream.
const OBJECTS_PER_STREAM: usize = 200;

/// zlib level for content streams and object streams.
pub const DEFLATE_LEVEL: u8 = 9;

pub fn deflate(data: &[u8]) -> Vec<u8> {
    compress_to_vec_zlib(data, DEFLATE_LEVEL)
}

/// A chunk for objects written to a [`PdfFile`] (compact serialization).
pub fn chunk() -> Chunk {
    Chunk::with_settings(Settings { pretty: false })
}

#[derive(Clone, Copy)]
enum Entry {
    Free,
    Offset(u64),
    /// (object stream number, index in it)
    InStream(i32, u32),
}

pub struct PdfFile<W: Write> {
    out: W,
    pos: u64,
    entries: Vec<Entry>,
    next_id: i32,
    /// Serialized dictionaries waiting for the next object stream.
    pending: Vec<(Ref, Vec<u8>)>,
}

impl<W: Write> PdfFile<W> {
    pub fn new(out: W) -> io::Result<Self> {
        let mut f = Self {
            out,
            pos: 0,
            entries: vec![Entry::Free],
            next_id: 1,
            pending: Vec::new(),
        };
        f.raw(b"%PDF-1.7\n%\x80\x80\x80\x80\n")?;
        Ok(f)
    }

    fn raw(&mut self, data: &[u8]) -> io::Result<()> {
        self.out.write_all(data)?;
        self.pos += data.len() as u64;
        Ok(())
    }

    /// Allocate an object number.
    pub fn alloc(&mut self) -> Ref {
        let r = Ref::new(self.next_id);
        self.next_id += 1;
        self.entries.push(Entry::Free);
        r
    }

    fn set(&mut self, id: Ref, entry: Entry) {
        let slot = &mut self.entries[id.get() as usize];
        assert!(
            matches!(slot, Entry::Free),
            "object {} written twice",
            id.get()
        );
        *slot = entry;
    }

    /// Write a chunk holding exactly the one indirect object `id` (typically a stream)
    /// at the current position.
    pub fn write_chunk(&mut self, id: Ref, chunk: &Chunk) -> io::Result<()> {
        debug_assert!(chunk.refs().len() == 1 && chunk.refs().next() == Some(id));
        self.set(id, Entry::Offset(self.pos));
        self.raw(chunk.as_bytes())
    }

    /// Queue a non-stream object for the next object stream. `write` fills in the object.
    pub fn object(&mut self, id: Ref, write: impl FnOnce(Obj<'_>)) -> io::Result<()> {
        let mut c = chunk();
        write(c.indirect(id));
        self.pending.push((id, object_body(c.as_bytes()).to_vec()));
        if self.pending.len() >= OBJECTS_PER_STREAM {
            self.flush_objects()?;
        }
        Ok(())
    }

    fn flush_objects(&mut self) -> io::Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let pending = std::mem::take(&mut self.pending);
        let stream_id = self.alloc();
        let mut header = Vec::new();
        let mut body = Vec::new();
        for (k, (id, data)) in pending.iter().enumerate() {
            write!(header, "{} {} ", id.get(), body.len())?;
            body.extend_from_slice(data);
            body.push(b'\n');
            self.set(*id, Entry::InStream(stream_id.get(), k as u32));
        }
        let first = header.len();
        header.extend_from_slice(&body);
        let data = deflate(&header);
        let mut c = chunk();
        let mut s = c.stream(stream_id, &data);
        s.filter(Filter::FlateDecode);
        s.pair(Name(b"Type"), Name(b"ObjStm"));
        s.pair(Name(b"N"), pending.len() as i32);
        s.pair(Name(b"First"), first as i32);
        drop(s);
        self.write_chunk(stream_id, &c)
    }

    /// Write the remaining objects and the cross-reference stream; `info` is the
    /// document information dictionary, if any.
    pub fn finish(mut self, catalog: Ref, info: Option<Ref>) -> io::Result<W> {
        self.flush_objects()?;
        let xref = self.alloc();
        let xref_pos = self.pos;
        self.entries[xref.get() as usize] = Entry::Offset(xref_pos);
        let width = (1..=8).find(|&n| xref_pos < 1u64 << (8 * n)).unwrap();
        let mut table = Vec::with_capacity(self.entries.len() * (3 + width));
        for e in &self.entries {
            let (kind, field2, field3) = match *e {
                Entry::Free => (0u8, 0u64, 0u16),
                Entry::Offset(o) => (1, o, 0),
                Entry::InStream(s, i) => (2, s as u64, i as u16),
            };
            table.push(kind);
            table.extend_from_slice(&field2.to_be_bytes()[8 - width..]);
            table.extend_from_slice(&field3.to_be_bytes());
        }
        assert!(
            self.entries
                .iter()
                .all(|e| !matches!(e, Entry::InStream(_, i) if *i > u16::MAX as u32))
        );
        let data = deflate(&table);
        let mut c = chunk();
        let mut s = c.stream(xref, &data);
        s.filter(Filter::FlateDecode);
        s.pair(Name(b"Type"), Name(b"XRef"));
        s.pair(Name(b"Size"), self.entries.len() as i32);
        s.pair(Name(b"Root"), catalog);
        if let Some(info) = info {
            s.pair(Name(b"Info"), info);
        }
        s.insert(Name(b"W")).array().items([1, width as i32, 2]);
        drop(s);
        self.raw(c.as_bytes())?;
        let tail = format!("startxref\n{xref_pos}\n%%EOF\n");
        self.raw(tail.as_bytes())?;
        self.out.flush()?;
        Ok(self.out)
    }
}

/// The object between `N 0 obj` and `endobj` of a serialized indirect object.
fn object_body(bytes: &[u8]) -> &[u8] {
    let start = bytes
        .windows(3)
        .position(|w| w == b"obj")
        .expect("indirect object")
        + 3;
    let end = bytes
        .windows(6)
        .rposition(|w| w == b"endobj")
        .expect("indirect object");
    bytes[start..end].trim_ascii()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_is_unwrapped() {
        let mut c = chunk();
        c.indirect(Ref::new(12)).dict().pair(Name(b"A"), 1);
        assert_eq!(object_body(c.as_bytes()), b"<</A 1>>");
    }

    #[test]
    fn writes_object_and_xref_streams() {
        let mut f = PdfFile::new(Vec::new()).unwrap();
        let catalog = f.alloc();
        let pages = f.alloc();
        let page = f.alloc();
        let content = f.alloc();
        f.object(catalog, |o| {
            o.dict()
                .pair(Name(b"Type"), Name(b"Catalog"))
                .pair(Name(b"Pages"), pages);
        })
        .unwrap();
        f.object(pages, |o| {
            let mut d = o.dict();
            d.pair(Name(b"Type"), Name(b"Pages"))
                .pair(Name(b"Count"), 1);
            d.insert(Name(b"Kids")).array().item(page);
        })
        .unwrap();
        f.object(page, |o| {
            let mut d = o.dict();
            d.pair(Name(b"Type"), Name(b"Page"))
                .pair(Name(b"Parent"), pages)
                .pair(Name(b"Contents"), content);
            d.insert(Name(b"MediaBox")).array().items([0, 0, 100, 100]);
        })
        .unwrap();
        let mut c = chunk();
        c.stream(content, b"0 0 m 10 10 l S");
        f.write_chunk(content, &c).unwrap();
        let pdf = f.finish(catalog, None).unwrap();
        assert!(pdf.starts_with(b"%PDF-1.7"));
        assert!(pdf.ends_with(b"%%EOF\n"));
        let text = String::from_utf8_lossy(&pdf);
        assert!(
            text.contains("/Type/ObjStm") && text.contains("/Type/XRef"),
            "{text}"
        );
    }
}
