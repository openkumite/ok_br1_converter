//! Exports every TIM inside the compressed .DAT containers as PNG.
//! Indexed (4/8 bpp) images are decoded with the CLUTs that TMD polygons
//! actually reference on that texture page; otherwise with the container's
//! first palette.

use crate::error::Result;
use crate::tim::{Image, Tim, write_png};
use crate::tmd;
use crate::vram::Vram;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::path::Path;

/// VRAM rectangle of a TIM's pixel block, in halfwords.
fn rect(d: &[u8]) -> Option<(usize, usize, usize, usize)> {
    let flags = u32::from_le_bytes(d.get(4..8)?.try_into().ok()?);
    let mut o = 8;
    if flags & 8 != 0 {
        o += u32::from_le_bytes(d.get(8..12)?.try_into().ok()?) as usize;
    }
    let r = |i: usize| d.get(o + i..o + i + 2).map(|b| u16::from_le_bytes([b[0], b[1]]) as usize);
    Some((r(4)?, r(6)?, r(8)?, r(10)?))
}

fn decode_with_vram_clut(t: &Tim, vram: &Vram, cba: u16) -> Result<Image> {
    let cx = (cba & 63) as usize * 16;
    let cy = ((cba >> 6) & 511) as usize;
    let n = if t.bpp == 4 { 16 } else { 256 };
    let clut: Vec<u16> = (0..n).map(|i| vram.data[cy * 1024 + (cx + i).min(1023)]).collect();
    crate::tim::decode_pixels(t.bpp, t.width, t.height, &t.pixels, &clut)
}

/// `entries`: this container's unpacked entries; `extra_tmds`: models from other
/// containers that texture from these pages (e.g. XX20 for costumes XX10..12).
#[cfg(feature = "cli")]
pub fn export(
    out: &Path,
    group: &str,
    entries: &[(usize, Vec<u8>)],
    extra_tmds: &[&[u8]],
    vram: &Vram,
) -> Result<Vec<Value>> {
    // (tsb page bits, cba) pairs used by polygons.
    let mut used = BTreeSet::new();
    for d in entries.iter().map(|(_, d)| d.as_slice()).chain(extra_tmds.iter().copied()) {
        if let Ok(objs) = tmd::parse(d) {
            for p in objs.iter().flat_map(|o| &o.polys).filter(|p| p.uvs.is_some()) {
                used.insert((p.tsb & 0x19f, p.cba));
            }
        }
    }
    // Default palette: first CLUT bank in the container (IDs 0x11/0x13 at y >= 480).
    let default_cba = entries.iter().find_map(|(_, d)| {
        let (x, y, _, _) = rect(d).filter(|_| d[0] == 0x11 || d[0] == 0x13)?;
        Some((x / 16) as u16 | (y as u16) << 6)
    });
    let dir = out.join("textures").join(group);
    std::fs::create_dir_all(&dir)?;
    let mut list = Vec::new();
    for (i, d) in entries {
        let Ok(t) = Tim::parse(d) else { continue };
        let Some((x, y, w, _)) = rect(d) else { continue };
        if d[0] == 0x11 || d[0] == 0x13 {
            continue; // palette banks
        }
        let mut variants = Vec::new();
        if t.bpp >= 16 {
            variants.push((None, t.image(0)?));
        } else {
            // Polygons whose texture page overlaps this image's VRAM rect.
            let mut cbas: Vec<u16> = used
                .iter()
                .filter(|(tsb, _)| {
                    let (px, py) = ((tsb & 15) as usize * 64, ((tsb >> 4) & 1) as usize * 256);
                    let mode_bpp: usize = [4, 8, 16, 16][(tsb >> 7) as usize & 3];
                    mode_bpp == t.bpp as usize && px < x + w && x < px + 256 * mode_bpp / 16 && py <= y && y < py + 256
                })
                .map(|&(_, c)| c)
                .collect();
            cbas.dedup();
            if cbas.is_empty() {
                cbas.extend(default_cba);
            }
            for c in cbas.into_iter().take(16) {
                variants.push((Some(c), decode_with_vram_clut(&t, vram, c)?));
            }
        }
        for (cba, img) in variants {
            let name = match cba {
                Some(c) => format!("{group}_{i:03}_clut{c:04x}"),
                None => format!("{group}_{i:03}"),
            };
            let rel = format!("textures/{group}/{name}.png");
            write_png(&out.join(&rel), &img)?;
            list.push(json!({"name": name, "group": group, "path": rel, "width": img.width, "height": img.height,
                "bpp": t.bpp, "source": format!("{group}.DAT #{i}"), "vram": [x, y]}));
        }
    }
    Ok(list)
}
