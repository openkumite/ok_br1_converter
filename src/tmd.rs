//! Sony TMD models (ID 0x41) as used by Bloody Roar.

use crate::bail;
use crate::error::Result;

#[derive(Clone, Debug)]
pub struct Poly {
    /// 3 or 4 vertex indices (quads in PS1 "Z" order: 0,1,2 / 1,3,2).
    pub verts: Vec<u16>,
    /// Per-vertex normal indices (lit polygons only).
    pub normals: Option<Vec<u16>>,
    /// Per-vertex RGB (0x80 = 1.0); flat colours are replicated.
    pub colors: Option<Vec<[u8; 3]>>,
    pub uvs: Option<Vec<[u8; 2]>>,
    pub tsb: u16,
    pub cba: u16,
    /// Semi-transparent (ABE): the mode is TSB bits 5-6.
    pub semi: bool,
    /// Light calculation disabled.
    pub unlit: bool,
    pub double_sided: bool,
}

pub struct Object {
    pub verts: Vec<[i16; 3]>,
    pub normals: Vec<[i16; 3]>,
    pub polys: Vec<Poly>,
}

fn svectors(d: &[u8], at: usize, n: usize) -> Result<Vec<[i16; 3]>> {
    let end = at + n * 8;
    if end > d.len() {
        bail!("vector table out of range");
    }
    Ok(d[at..end].as_chunks::<8>().0.iter().map(|c| [0, 2, 4].map(|o| i16::from_le_bytes([c[o], c[o + 1]]))).collect())
}

pub fn parse(d: &[u8]) -> Result<Vec<Object>> {
    if d.len() < 12 || d[0] != 0x41 {
        bail!("not a TMD");
    }
    let rd32 = |o: usize| u32::from_le_bytes(d[o..o + 4].try_into().unwrap()) as usize;
    let fixp = rd32(4) & 1 != 0;
    let nobj = rd32(8);
    let mut objs = Vec::new();
    for i in 0..nobj {
        let t = 12 + i * 28;
        if t + 28 > d.len() {
            bail!("object table out of range");
        }
        let base = if fixp { 0 } else { 12 };
        let verts = svectors(d, base + rd32(t), rd32(t + 4))?;
        let normals = svectors(d, base + rd32(t + 8), rd32(t + 12))?;
        let mut p = base + rd32(t + 16);
        let mut polys = Vec::new();
        for _ in 0..rd32(t + 20) {
            if p + 4 > d.len() {
                bail!("primitive out of range");
            }
            let (ilen, flag, mode) = (d[p + 1] as usize, d[p + 2], d[p + 3]);
            let w = &d[p + 4..(p + 4 + ilen * 4).min(d.len())];
            p += 4 + ilen * 4;
            if mode >> 5 != 1 {
                continue; // lines / sprites: not used by this game's models
            }
            if let Some(poly) = decode_poly(mode, flag, w) {
                polys.push(poly);
            }
        }
        objs.push(Object { verts, normals, polys });
    }
    Ok(objs)
}

fn decode_poly(mode: u8, flag: u8, w: &[u8]) -> Option<Poly> {
    let quad = mode & 8 != 0;
    let tex = mode & 4 != 0;
    let gouraud = mode & 0x10 != 0;
    let unlit = flag & 1 != 0;
    let n = if quad { 4 } else { 3 };
    let h = |o: usize| w.get(o..o + 2).map(|b| u16::from_le_bytes([b[0], b[1]]));
    let mut o = 0;
    let (mut uvs, mut tsb, mut cba) = (None, 0, 0);
    if tex {
        // UV0 CBA | UV1 TSB | UV2 pad | (UV3 pad)
        uvs = Some((0..n).map(|i| w.get(i * 4..i * 4 + 2).map(|b| [b[0], b[1]])).collect::<Option<Vec<_>>>()?);
        cba = h(2)?;
        tsb = h(6)?;
        o = n * 4;
    }
    let rgb = |o: usize| w.get(o..o + 3).map(|b| [b[0], b[1], b[2]]);
    let mut colors = None;
    let mut normals = None;
    let verts: Vec<u16>;
    if unlit {
        // colours: one word (flat) or one per vertex (gradation)
        let ncol = if gouraud { n } else { 1 };
        let c: Vec<[u8; 3]> = (0..ncol).map(|i| rgb(o + i * 4)).collect::<Option<_>>()?;
        colors = Some((0..n).map(|i| c[i.min(ncol - 1)]).collect());
        o += ncol * 4;
        verts = (0..n).map(|i| h(o + i * 2)).collect::<Option<_>>()?;
    } else {
        if !tex {
            colors = Some(vec![rgb(o)?; n]);
            o += 4;
        }
        if gouraud {
            // N0 V0 N1 V1 ...
            normals = Some((0..n).map(|i| h(o + i * 4)).collect::<Option<Vec<_>>>()?);
            verts = (0..n).map(|i| h(o + i * 4 + 2)).collect::<Option<_>>()?;
        } else {
            // N0 V0 V1 V2 (V3)
            let n0 = h(o)?;
            normals = Some(vec![n0; n]);
            verts = (0..n).map(|i| h(o + 2 + i * 2)).collect::<Option<_>>()?;
        }
    }
    Some(Poly { verts, normals, colors, uvs, tsb, cba, semi: mode & 2 != 0, unlit, double_sided: flag & 2 != 0 })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One object, 3 vertices, 1 normal, one flat lit untextured triangle (mode 0x20).
    fn tiny_tmd() -> Vec<u8> {
        let mut d = Vec::new();
        d.extend([0x41u32, 0, 1].iter().flat_map(|v| v.to_le_bytes()));
        let (vt, nt, pt) = (28u32, 28 + 24, 28 + 24 + 8);
        d.extend([vt, 3, nt, 1, pt, 1, 0].iter().flat_map(|v| v.to_le_bytes()));
        for v in [[0i16, 0, 0, 0], [100, 0, 0, 0], [0, 100, 0, 0]] {
            d.extend(v.iter().flat_map(|x| x.to_le_bytes()));
        }
        d.extend([0i16, 0, -4096, 0].iter().flat_map(|x| x.to_le_bytes()));
        // olen ilen flag mode | r g b mode | n0 v0 | v1 v2
        d.extend([4u8, 3, 0, 0x20, 10, 20, 30, 0x20]);
        d.extend([0u16, 0, 1, 2].iter().flat_map(|x| x.to_le_bytes()));
        d
    }

    #[test]
    fn parses_flat_triangle() {
        let objs = parse(&tiny_tmd()).unwrap();
        assert_eq!(objs.len(), 1);
        assert_eq!(objs[0].verts, vec![[0, 0, 0], [100, 0, 0], [0, 100, 0]]);
        let p = &objs[0].polys[0];
        assert_eq!(p.verts, vec![0, 1, 2]);
        assert_eq!(p.normals, Some(vec![0, 0, 0]));
        assert_eq!(p.colors, Some(vec![[10, 20, 30]; 3]));
        assert!(!p.unlit && p.uvs.is_none());
    }

    #[test]
    fn rejects_truncated() {
        let d = tiny_tmd();
        assert!(parse(&d[..20]).is_err());
        assert!(parse(&[0x10, 0, 0, 0]).is_err());
    }
}
