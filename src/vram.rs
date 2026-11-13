//! Virtual PS1 VRAM (1024x512 halfwords). TIM uploads are placed by their
//! embedded coordinates; texture pages are then decoded with a CLUT exactly as
//! the GPU would sample them.

use crate::tim::{Image, color15};

pub struct Vram {
    pub data: Vec<u16>,
}

impl Vram {
    pub fn new() -> Self {
        Vram { data: vec![0; 1024 * 512] }
    }

    fn put(&mut self, x: usize, y: usize, w: usize, h: usize, words: &[u8]) {
        if w == 0 || x >= 1024 || y >= 512 {
            return;
        }
        // Only rows backed by data and inside VRAM (headers can claim 65535x65535).
        let rows = h.min(512 - y).min((words.len() / 2).div_ceil(w));
        for row in 0..rows {
            for col in 0..w.min(1024 - x) {
                let i = (row * w + col) * 2;
                if i + 1 >= words.len() {
                    return;
                }
                self.data[(y + row) * 1024 + x + col] = u16::from_le_bytes([words[i], words[i + 1]]);
            }
        }
    }

    /// Uploads a TIM (the game uses IDs 0x10..=0x13 for the first byte).
    /// Returns false if the data isn't a TIM.
    pub fn load_tim(&mut self, d: &[u8]) -> bool {
        self.load_tim_at(d, None)
    }

    /// Uploads a TIM, optionally overriding the (x, y) of its pixel block.
    pub fn load_tim_at(&mut self, d: &[u8], pos: Option<(usize, usize)>) -> bool {
        if d.len() < 20 || !(0x10..=0x13).contains(&d[0]) || d[1..4] != [0, 0, 0] {
            return false;
        }
        let rd16 = |o: usize| u16::from_le_bytes([d[o], d[o + 1]]) as usize;
        let rd32 = |o: usize| u32::from_le_bytes(d[o..o + 4].try_into().unwrap()) as usize;
        let flags = rd32(4);
        let mut o = 8;
        let mut blocks = if flags & 8 != 0 { 2 } else { 1 };
        while blocks > 0 && o + 12 <= d.len() {
            let len = rd32(o);
            let (mut x, mut y, w, h) = (rd16(o + 4), rd16(o + 6), rd16(o + 8), rd16(o + 10));
            if let (Some((px, py)), 1) = (pos, blocks) {
                (x, y) = (px, py);
            }
            if len < 12 {
                break;
            }
            let end = o.saturating_add(len).min(d.len());
            self.put(x, y, w, h, &d[o + 12..end]);
            o = o.saturating_add(len);
            blocks -= 1;
        }
        true
    }

    /// Decodes a 256x256 texture page. `tsb`/`cba` are the TMD words.
    /// Colour 0x0000 decodes as transparent.
    pub fn page(&self, tsb: u16, cba: u16) -> Image {
        let px = (tsb & 15) as usize * 64;
        let py = ((tsb >> 4) & 1) as usize * 256;
        let mode = (tsb >> 7) & 3;
        let cx = (cba & 63) as usize * 16;
        let cy = ((cba >> 6) & 511) as usize;
        let clut = |i: usize| self.data[cy * 1024 + (cx + i).min(1023)];
        let mut rgba = Vec::with_capacity(256 * 256 * 4);
        for v in 0..256usize {
            let row = ((py + v) & 511) * 1024;
            for u in 0..256usize {
                let c = match mode {
                    0 => {
                        let w = self.data[row + ((px + u / 4) & 1023)];
                        clut(((w >> ((u & 3) * 4)) & 15) as usize)
                    }
                    1 => {
                        let w = self.data[row + ((px + u / 2) & 1023)];
                        clut(((w >> ((u & 1) * 8)) & 255) as usize)
                    }
                    _ => self.data[row + ((px + u) & 1023)],
                };
                rgba.extend(color15(c));
            }
        }
        Image { width: 256, height: 256, rgba }
    }
}

impl Default for Vram {
    fn default() -> Self {
        Self::new()
    }
}

/// Stage containers (XX00, BIG00, SMA00) keep their 4 bpp palettes in entries
/// 19..=22, which the game places at CLUT slots (256 + 16·n, 480). Three
/// stages (FOX, MIT, VAK) store entry 20 with entry 19's coordinates, which
/// would leave slot 272 empty, so the slot is taken from the entry index.
pub fn stage_clut_slot(index: usize, d: &[u8]) -> Option<(usize, usize)> {
    (d.first() == Some(&0x11) && (19..=22).contains(&index)).then(|| (256 + 16 * (index - 19), 480))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tim(x: u16, y: u16, w: u16, h: u16, words: &[u16]) -> Vec<u8> {
        let mut d = vec![0x12, 0, 0, 0, 2, 0, 0, 0];
        d.extend((12 + words.len() as u32 * 2).to_le_bytes());
        d.extend([x, y, w, h].iter().flat_map(|v| v.to_le_bytes()));
        d.extend(words.iter().flat_map(|v| v.to_le_bytes()));
        d
    }

    #[test]
    fn uploads_and_overrides_position() {
        let mut v = Vram::new();
        assert!(v.load_tim(&tim(10, 20, 2, 1, &[0x1234, 0x5678])));
        assert_eq!(v.data[20 * 1024 + 11], 0x5678);
        assert!(v.load_tim_at(&tim(0, 0, 1, 1, &[7]), Some((5, 6))));
        assert_eq!(v.data[6 * 1024 + 5], 7);
        assert!(!v.load_tim(&[0x41; 32]));
    }

    #[test]
    fn decodes_4bpp_page_through_clut() {
        let mut v = Vram::new();
        v.load_tim(&tim(0, 480, 2, 1, &[0, 0x7fff])); // CLUT: transparent, white
        v.load_tim(&tim(64, 0, 1, 1, &[0x0010])); // texels 0,1,0,0 at page 1
        let img = v.page(1, 480 << 6);
        assert_eq!(&img.rgba[0..4], &[0, 0, 0, 0]);
        assert_eq!(&img.rgba[4..8], &[255, 255, 255, 255]);
    }

    #[test]
    fn stage_clut_slots() {
        assert_eq!(stage_clut_slot(20, &[0x11]), Some((272, 480)));
        assert_eq!(stage_clut_slot(18, &[0x11]), None);
        assert_eq!(stage_clut_slot(20, &[0x12]), None);
    }
}
