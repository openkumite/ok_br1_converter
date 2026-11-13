//! PlayStation TIM images and 15-bit colour helpers.

use crate::bail;
use crate::error::Result;

/// RGBA image.
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Converts a PS1 15-bit colour to RGBA. 0x0000 is the hardware's
/// "transparent" value; everything else is opaque.
pub fn color15(c: u16) -> [u8; 4] {
    if c == 0 {
        return [0, 0, 0, 0];
    }
    let e = |v: u16| ((v & 31) << 3 | (v & 31) >> 2) as u8;
    [e(c), e(c >> 5), e(c >> 10), 255]
}

fn u16_at(d: &[u8], o: usize) -> Result<u16> {
    d.get(o..o + 2)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .ok_or_else(|| crate::format_err!("TIM truncated at {o:#x}"))
}

fn u32_at(d: &[u8], o: usize) -> Result<u32> {
    d.get(o..o + 4)
        .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
        .ok_or_else(|| crate::format_err!("TIM truncated at {o:#x}"))
}

/// Decodes indexed (4/8 bpp) or direct (16/24 bpp) pixel data.
/// `width` is in pixels; `clut` is ignored for 16/24 bpp.
pub fn decode_pixels(bpp: u32, width: u32, height: u32, pix: &[u8], clut: &[u16]) -> Result<Image> {
    // PS1 VRAM is 1024x512 halfwords: no real image is wider than 4096 px (4 bpp) or taller than 512.
    if width > 4096 || height > 1024 {
        bail!("image size {width}x{height} out of range");
    }
    let needed = (width as usize * height as usize * bpp as usize).div_ceil(8);
    if pix.len() < needed {
        bail!("pixel data {} bytes, {width}x{height}x{bpp}bpp needs {needed}", pix.len());
    }
    let n = (width * height) as usize;
    let mut rgba = Vec::with_capacity(n * 4);
    let lookup = |i: usize| color15(clut.get(i).copied().unwrap_or(0));
    match bpp {
        4 => {
            for i in 0..n {
                let b = *pix.get(i / 2).unwrap_or(&0);
                rgba.extend(lookup(((b >> ((i & 1) * 4)) & 15) as usize));
            }
        }
        8 => {
            for i in 0..n {
                rgba.extend(lookup(*pix.get(i).unwrap_or(&0) as usize));
            }
        }
        16 => {
            for i in 0..n {
                rgba.extend(color15(u16_at(pix, i * 2).unwrap_or(0)));
            }
        }
        24 => {
            let stride = (width as usize * 3).div_ceil(2) * 2;
            for y in 0..height as usize {
                for x in 0..width as usize {
                    let o = y * stride + x * 3;
                    let p = pix.get(o..o + 3).unwrap_or(&[0, 0, 0]);
                    rgba.extend([p[0], p[1], p[2], 255]);
                }
            }
        }
        _ => bail!("unsupported bpp {bpp}"),
    }
    Ok(Image { width, height, rgba })
}

pub struct Tim {
    pub bpp: u32,
    /// All CLUT rows (palettes), each `clut_w` colours.
    pub cluts: Vec<Vec<u16>>,
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl Tim {
    pub fn parse(d: &[u8]) -> Result<Tim> {
        // The game stores IDs 0x10..=0x13 in its compressed containers.
        if d.len() < 8 || !(0x10..=0x13).contains(&u32_at(d, 0)?) {
            bail!("not a TIM");
        }
        let flags = u32_at(d, 4)?;
        let bpp = [4, 8, 16, 24][(flags & 3) as usize];
        let mut o = 8;
        let mut cluts = Vec::new();
        if flags & 8 != 0 {
            let len = u32_at(d, o)? as usize;
            let w = u16_at(d, o + 8)? as usize;
            let h = u16_at(d, o + 10)? as usize;
            if o + 12 + w * h * 2 > d.len() {
                bail!("TIM CLUT block truncated");
            }
            for row in 0..h {
                cluts.push((0..w).map(|i| u16_at(d, o + 12 + (row * w + i) * 2)).collect::<Result<_>>()?);
            }
            o = o.checked_add(len).filter(|&v| v <= d.len()).ok_or_else(|| crate::format_err!("bad CLUT length"))?;
        }
        let len = u32_at(d, o)? as usize;
        let w16 = u16_at(d, o + 8)? as u32;
        let height = u16_at(d, o + 10)? as u32;
        let width = match bpp {
            4 => w16 * 4,
            8 => w16 * 2,
            16 => w16,
            _ => w16 * 2 / 3,
        };
        let end = o.saturating_add(len).clamp(o + 12, d.len());
        Ok(Tim { bpp, cluts, width, height, pixels: d[o + 12..end].to_vec() })
    }

    pub fn image(&self, clut: usize) -> Result<Image> {
        let pal = self.cluts.get(clut).map(Vec::as_slice).unwrap_or(&[]);
        decode_pixels(self.bpp, self.width, self.height, &self.pixels, pal)
    }
}

#[cfg(feature = "cli")]
pub fn write_png(path: &std::path::Path, img: &Image) -> Result<()> {
    let file = std::io::BufWriter::new(std::fs::File::create(path)?);
    let mut enc = png::Encoder::new(file, img.width, img.height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(&img.rgba)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tim(bpp_flag: u32, clut: Option<&[u16]>, w16: u16, h: u16, pixels: &[u8]) -> Vec<u8> {
        let mut d = Vec::new();
        d.extend(0x10u32.to_le_bytes());
        d.extend((bpp_flag | if clut.is_some() { 8 } else { 0 }).to_le_bytes());
        if let Some(c) = clut {
            d.extend((12 + c.len() as u32 * 2).to_le_bytes());
            d.extend([0u16, 480, c.len() as u16, 1].iter().flat_map(|v| v.to_le_bytes()));
            d.extend(c.iter().flat_map(|v| v.to_le_bytes()));
        }
        d.extend((12 + pixels.len() as u32).to_le_bytes());
        d.extend([0u16, 0, w16, h].iter().flat_map(|v| v.to_le_bytes()));
        d.extend(pixels);
        d
    }

    #[test]
    fn color15_expands_and_marks_transparent() {
        assert_eq!(color15(0), [0, 0, 0, 0]);
        assert_eq!(color15(0x7fff), [255, 255, 255, 255]);
        assert_eq!(color15(0x001f), [255, 0, 0, 255]);
        assert_eq!(color15(0x8000), [0, 0, 0, 255]); // black with STP bit is opaque
    }

    #[test]
    fn decodes_4bpp_with_clut() {
        let clut = [0u16, 0x001f, 0x03e0, 0x7c00];
        let t = Tim::parse(&tim(0, Some(&clut), 1, 1, &[0x21, 0x03])).unwrap();
        assert_eq!((t.bpp, t.width, t.height), (4, 4, 1));
        let img = t.image(0).unwrap();
        let px: Vec<[u8; 4]> = img.rgba.chunks(4).map(|c| c.try_into().unwrap()).collect();
        assert_eq!(px, vec![color15(0x001f), color15(0x03e0), color15(0x7c00), color15(0)]);
    }

    #[test]
    fn decodes_16bpp_and_game_ids() {
        let mut d = tim(2, None, 2, 1, &[0xff, 0x7f, 0x1f, 0x00]);
        d[0] = 0x12; // container variant ID
        let img = Tim::parse(&d).unwrap().image(0).unwrap();
        assert_eq!(&img.rgba[..4], &[255, 255, 255, 255]);
        assert_eq!(&img.rgba[4..], &[255, 0, 0, 255]);
        assert!(Tim::parse(&[0x41, 0, 0, 0, 0, 0, 0, 0]).is_err());
    }
}
