//! Stage background panorama (the far sky/horizon/floor band behind each arena).
//!
//! Built from three entries of an XX00.DAT stage container:
//! - ID 0x22: header `22 03 00 c0 00 01 10 10`, then 8-byte tile definitions
//!   `{u16 uv (u = low byte, v = high byte), u16 cba, u16 0, u16 tsb}`;
//!   the page/CLUT words are templates the game relocates, so only uv is used.
//! - ID 0x23: header `23 00 00 c0 <map width> 04 10 10`, then u16 tile indices
//!   for a map of 16x16 tiles, 8 rows high (64 or 80 columns = 360°).
//! - the stage's 8 bpp TIM (tile pixels) and its 256-colour CLUT bank (row 0).
//!
//! The map's top half (rows 0-3) is the horizon panorama; the bottom half
//! (rows 4-7) is the repeating ground pattern of the floor out to the horizon.

use crate::gltf::{Blend, Gltf, Primitive, png_bytes};
use crate::tim::{Image, Tim, decode_pixels};
use serde_json::json;

const TILE: usize = 16;

pub fn panorama(entries: &[(usize, Vec<u8>)]) -> Option<Image> {
    let find = |id: u8| entries.iter().map(|(_, d)| d).find(|d| d.len() > 8 && d[0] == id);
    let defs = find(0x22)?;
    let map = find(0x23)?;
    let page = entries
        .iter()
        .filter_map(|(_, d)| (d[0] == 0x12).then(|| Tim::parse(d).ok()).flatten())
        .find(|t| t.bpp == 8)?;
    let clut_tim = entries
        .iter()
        .filter_map(|(_, d)| (d[0] == 0x13).then(|| Tim::parse(d).ok()).flatten())
        .find(|t| t.width == 256)?;
    // The CLUT bank is stored as a 16 bpp image: row 0 holds the 256 colours.
    let clut: Vec<u16> =
        clut_tim.pixels.as_chunks::<2>().0.iter().take(256).map(|b| u16::from_le_bytes([b[0], b[1]])).collect();
    let tiles_img = decode_pixels(8, page.width, page.height, &page.pixels, &clut).ok()?;

    let mw = map[4] as usize;
    let idx: Vec<u16> = map[8..].as_chunks::<2>().0.iter().map(|b| u16::from_le_bytes([b[0], b[1]])).collect();
    if mw == 0 || idx.len() < mw {
        return None;
    }
    let mh = idx.len() / mw;
    let (w, h) = (mw * TILE, mh * TILE);
    let mut rgba = vec![0u8; w * h * 4];
    for ty in 0..mh {
        for tx in 0..mw {
            let t = idx[ty * mw + tx] as usize;
            let Some(def) = defs.get(8 + t * 8..10 + t * 8) else { continue };
            let (u, v) = (def[0] as usize, def[1] as usize);
            for yy in 0..TILE {
                for xx in 0..TILE {
                    let (sx, sy) = (u + xx, v + yy);
                    if sx >= tiles_img.width as usize || sy >= tiles_img.height as usize {
                        continue;
                    }
                    let s = (sy * tiles_img.width as usize + sx) * 4;
                    let d = ((ty * TILE + yy) * w + tx * TILE + xx) * 4;
                    rgba[d..d + 4].copy_from_slice(&tiles_img.rgba[s..s + 4]);
                    rgba[d + 3] = 255; // background is opaque
                }
            }
        }
    }
    Some(Image { width: w as u32, height: h as u32, rgba })
}

fn row_average(img: &Image, y: u32) -> [f32; 4] {
    let mut acc = [0f32; 3];
    for x in 0..img.width {
        let o = ((y * img.width + x) * 4) as usize;
        for (c, a) in acc.iter_mut().enumerate() {
            *a += img.rgba[o + c] as f32;
        }
    }
    let n = img.width as f32 * 255.0;
    // vertex colours are linear in glTF; the texture is sRGB
    let lin = |v: f32| v.powf(2.2);
    [lin(acc[0] / n), lin(acc[1] / n), lin(acc[2] / n), 1.0]
}

fn crop(img: &Image, y0: u32, y1: u32) -> Image {
    let row = img.width as usize * 4;
    Image { width: img.width, height: y1 - y0, rgba: img.rgba[y0 as usize * row..y1 as usize * row].to_vec() }
}

/// Metres per 16-pixel floor tile.
const FLOOR_TILE_M: f32 = 1.0;

/// Adds the background to a stage scene: the sky half as an inward-facing
/// cylinder standing on the horizon at `floor_y`, a sky-coloured cap above it,
/// and the floor half as a repeating textured disk at `floor_y` (all metres).
pub fn add_to_scene(gl: &mut Gltf, img: &Image, name: &str, radius: f32, floor_y: f32) -> usize {
    const SEG: usize = 96;
    let half = img.height / 2;
    let (sky_img, floor_img) = (crop(img, 0, half), crop(img, half, img.height));
    let circumference = std::f32::consts::TAU * radius;
    let (y0, y1) = (floor_y, floor_y + circumference * half as f32 / img.width as f32);
    let ring = |i: usize| {
        let a = i as f32 / SEG as f32 * std::f32::consts::TAU;
        (a.sin() * radius, -a.cos() * radius)
    };

    // Sky band
    let sky_tex = gl.texture_png(&png_bytes(&sky_img), &format!("{name}_sky"));
    let sky_mat = gl.material(&format!("{name}_sky"), Some(sky_tex), Blend::Opaque, true, false);
    let (mut pos, mut uv, mut idx) = (Vec::new(), Vec::new(), Vec::new());
    for i in 0..=SEG {
        let (x, z) = ring(i);
        let u = i as f32 / SEG as f32;
        pos.extend([[x, y1, z], [x, y0, z]]);
        uv.extend([[u, 0.0], [u, 1.0]]);
    }
    for i in 0..SEG as u32 {
        let (a, b, c, d) = (2 * i, 2 * i + 1, 2 * i + 2, 2 * i + 3);
        idx.extend([a, c, b, b, c, d]);
    }
    let band = Primitive {
        positions: pos,
        normals: None,
        uvs: Some(uv),
        colors: None,
        joints: None,
        weights: None,
        indices: idx,
        material: Some(sky_mat),
        variants: vec![],
    };

    // Sky cap, coloured like the top row of the panorama
    let cap_mat = gl.material(&format!("{name}_skycap"), None, Blend::Opaque, true, false);
    let sky = row_average(img, 0);
    let mut cpos = vec![[0.0, y1 + (y1 - y0), 0.0]];
    let mut cidx = Vec::new();
    for i in 0..=SEG {
        let (x, z) = ring(i);
        cpos.push([x, y1, z]);
    }
    for i in 0..SEG as u32 {
        cidx.extend([0, i + 1, i + 2]);
    }
    let n = cpos.len();
    let cap = Primitive {
        positions: cpos,
        normals: None,
        uvs: None,
        colors: Some(vec![sky; n]),
        joints: None,
        weights: None,
        indices: cidx,
        material: Some(cap_mat),
        variants: vec![],
    };

    // Floor disk, tiled with the bottom half (u along the strip, v across it)
    let floor_tex = gl.texture_png_wrap(&png_bytes(&floor_img), &format!("{name}_floor"), true);
    let floor_mat = gl.material(&format!("{name}_floor"), Some(floor_tex), Blend::Opaque, true, false);
    let (su, sv) = (floor_img.width as f32 / 16.0 * FLOOR_TILE_M, floor_img.height as f32 / 16.0 * FLOOR_TILE_M);
    let mut fpos = vec![[0.0, y0, 0.0]];
    let mut fuv = vec![[0.0, 0.0]];
    let mut fidx = Vec::new();
    for i in 0..=SEG {
        let (x, z) = ring(i);
        fpos.push([x, y0, z]);
        fuv.push([x / su, z / sv]);
    }
    for i in 0..SEG as u32 {
        fidx.extend([0, i + 2, i + 1]);
    }
    let floor = Primitive {
        positions: fpos,
        normals: None,
        uvs: Some(fuv),
        colors: None,
        joints: None,
        weights: None,
        indices: fidx,
        material: Some(floor_mat),
        variants: vec![],
    };

    let mesh = gl.mesh(&format!("{name}_background"), &[band, cap, floor]);
    gl.node(json!({"name": format!("{name}_background"), "mesh": mesh}))
}
