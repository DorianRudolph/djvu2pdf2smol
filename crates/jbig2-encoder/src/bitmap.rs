//! A simple 1-bit image: one byte per pixel (0 = white, 1 = black), top row first.

/// Bilevel bitmap, top row first; one byte per pixel (0 = white, 1 = black).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Default)]
pub struct Bitmap {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

impl Bitmap {
    /// All-white bitmap.
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            pixels: vec![0; width as usize * height as usize],
        }
    }

    /// Build from a predicate `black(x, y)`.
    pub fn from_fn(width: u32, height: u32, mut black: impl FnMut(u32, u32) -> bool) -> Self {
        let mut b = Self::new(width, height);
        for y in 0..height {
            for x in 0..width {
                if black(x, y) {
                    b.set(x, y, true);
                }
            }
        }
        b
    }

    /// Build from MSB-first packed rows (`stride` bytes per row, top row first) — the
    /// layout of PBM data and of most bilevel image libraries.
    pub fn from_packed(width: u32, height: u32, stride: usize, data: &[u8]) -> Self {
        Self::from_fn(width, height, |x, y| {
            data[y as usize * stride + x as usize / 8] & (0x80 >> (x % 8)) != 0
        })
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// Whether the bitmap has no pixels.
    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }

    /// Pixel at (x, y); 0 outside the bitmap (the JBIG2 convention for contexts).
    #[inline]
    pub fn get(&self, x: i32, y: i32) -> u8 {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            0
        } else {
            self.pixels[y as usize * self.width as usize + x as usize]
        }
    }

    pub fn set(&mut self, x: u32, y: u32, black: bool) {
        self.pixels[y as usize * self.width as usize + x as usize] = black as u8;
    }

    /// Row `y` as bytes (0/1).
    pub fn row(&self, y: u32) -> &[u8] {
        let w = self.width as usize;
        &self.pixels[y as usize * w..(y as usize + 1) * w]
    }
}
