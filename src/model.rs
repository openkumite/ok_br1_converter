//! TMD objects + VRAM -> glTF primitives (one primitive per material).

use crate::gltf::{Blend, Gltf, Primitive, png_bytes};
use crate::tmd::Object;
use crate::vram::Vram;
use std::collections::HashMap;

/// PS1 units -> metres: fighters are ~440 units tall, so 1/256 gives ~1.7 m.
pub const SCALE: f32 = 1.0 / 256.0;

/// PS1 is Y-down, Z-forward; glTF is Y-up. Rotating 180° about X keeps handedness.
pub fn conv(v: [f32; 3]) -> [f32; 3] {
    [v[0] * SCALE, -v[1] * SCALE, -v[2] * SCALE]
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct MatKey {
    /// (TSB page+colour-mode bits, CBA) when textured.
    tex: Option<(u16, u16)>,
    blend: Blend,
    lit: bool,
    double_sided: bool,
}

/// Shared texture/material cache for one glTF file.
#[derive(Default)]
pub struct Materials {
    textures: HashMap<(u16, u16), usize>,
    mats: HashMap<MatKey, usize>,
    pub pages: Vec<(String, Vec<u8>)>,
}

impl Materials {
    fn get(&mut self, gl: &mut Gltf, vram: &Vram, key: MatKey) -> usize {
        self.get_named(gl, vram, key, "")
    }

    /// Material for the same key as `base`'s material `base_mi`, textured from
    /// another VRAM (a costume variant). Untextured materials are reused as is.
    pub fn variant(&mut self, base: &Materials, base_mi: usize, gl: &mut Gltf, vram: &Vram, suffix: &str) -> usize {
        let Some((&key, _)) = base.mats.iter().find(|(_, m)| **m == base_mi) else { return base_mi };
        if key.tex.is_none() {
            return base_mi;
        }
        self.get_named(gl, vram, key, suffix)
    }

    fn get_named(&mut self, gl: &mut Gltf, vram: &Vram, key: MatKey, suffix: &str) -> usize {
        if let Some(&m) = self.mats.get(&key) {
            return m;
        }
        let tex = key.tex.map(|(tsb, cba)| {
            *self.textures.entry((tsb, cba)).or_insert_with(|| {
                let name = format!(
                    "page{:x}_{}bpp_clut{:x}{suffix}",
                    tsb & 0x1f,
                    [4, 8, 16, 16][(tsb >> 7) as usize & 3],
                    cba
                );
                let png = png_bytes(&vram.page(tsb, cba));
                self.pages.push((name.clone(), png.clone()));
                gl.texture_png(&png, &name)
            })
        });
        let name = format!(
            "{}{}{}{suffix}",
            key.tex.map_or("color".to_string(), |(t, c)| format!("tex{:x}_{:x}", t & 0x1f, c)),
            match key.blend {
                Blend::Semi(m) => format!("_semi{m}"),
                _ => String::new(),
            },
            if key.lit { "_lit" } else { "" }
        );
        let m = gl.material(&name, tex, key.blend, key.double_sided, key.lit);
        self.mats.insert(key, m);
        m
    }
}

/// Per-object placement: optional joint index and a transform applied to vertices.
pub struct Placement<'a> {
    pub joint: &'a dyn Fn(usize) -> Option<u16>,
    pub transform: &'a dyn Fn(usize, [f32; 3]) -> [f32; 3],
}

pub fn primitives(
    gl: &mut Gltf,
    mats: &mut Materials,
    vram: &Vram,
    objs: &[Object],
    place: &Placement,
) -> Vec<Primitive> {
    #[derive(Default)]
    struct Acc {
        pos: Vec<[f32; 3]>,
        nrm: Vec<[f32; 3]>,
        uv: Vec<[f32; 2]>,
        col: Vec<[f32; 4]>,
        joints: Vec<[u16; 4]>,
    }
    let mut groups: HashMap<usize, Acc> = HashMap::new();
    let mut order = Vec::new();
    let mut textured = std::collections::HashSet::new();
    let skinned = objs.iter().enumerate().any(|(i, _)| (place.joint)(i).is_some());

    for (oi, obj) in objs.iter().enumerate() {
        let vpos = |i: u16| {
            let v = obj.verts.get(i as usize).copied().unwrap_or_default();
            conv((place.transform)(oi, [v[0] as f32, v[1] as f32, v[2] as f32]))
        };
        let vnrm = |i: u16| {
            let n = obj.normals.get(i as usize).copied().unwrap_or_default();
            let n = [n[0] as f32 / 4096.0, -(n[1] as f32) / 4096.0, -(n[2] as f32) / 4096.0];
            let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().max(1e-6);
            [n[0] / l, n[1] / l, n[2] / l]
        };
        // PS1 front faces are clockwise on screen: in TMD space (v1-v0)x(v2-v0)
        // points away from the stored normal. Lit polygons vote to confirm;
        // objects with only unlit polygons use that convention.
        let mut vote = 0i32;
        for p in obj.polys.iter().filter(|p| p.normals.is_some()) {
            let (a, b, c) = (vpos(p.verts[0]), vpos(p.verts[1]), vpos(p.verts[2]));
            let e1 = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
            let e2 = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
            let g = [e1[1] * e2[2] - e1[2] * e2[1], e1[2] * e2[0] - e1[0] * e2[2], e1[0] * e2[1] - e1[1] * e2[0]];
            let n = vnrm(p.normals.as_ref().unwrap()[0]);
            vote += if g[0] * n[0] + g[1] * n[1] + g[2] * n[2] >= 0.0 { 1 } else { -1 };
        }
        let flip = vote <= 0;

        for p in &obj.polys {
            let tex = p.uvs.as_ref().map(|_| (p.tsb & 0x19f, p.cba));
            let blend = if p.semi {
                Blend::Semi(((p.tsb >> 5) & 3) as u8)
            } else if tex.is_some() {
                Blend::Cutout
            } else {
                Blend::Opaque
            };
            let key = MatKey { tex, blend, lit: !p.unlit, double_sided: p.double_sided };
            let mi = mats.get(gl, vram, key);
            if tex.is_some() {
                textured.insert(mi);
            }
            if !groups.contains_key(&mi) {
                order.push(mi);
            }
            let acc = groups.entry(mi).or_default();
            let tris: &[[usize; 3]] = if p.verts.len() == 4 { &[[0, 1, 2], [1, 3, 2]] } else { &[[0, 1, 2]] };
            for t in tris {
                let t = if flip { [t[0], t[2], t[1]] } else { *t };
                // Geometric normal, used where the TMD stores a zero normal.
                let (a, b, c) = (vpos(p.verts[t[0]]), vpos(p.verts[t[1]]), vpos(p.verts[t[2]]));
                let (e1, e2) = ([0, 1, 2].map(|i| b[i] - a[i]), [0, 1, 2].map(|i| c[i] - a[i]));
                let g = [e1[1] * e2[2] - e1[2] * e2[1], e1[2] * e2[0] - e1[0] * e2[2], e1[0] * e2[1] - e1[1] * e2[0]];
                let gl_ = (g[0] * g[0] + g[1] * g[1] + g[2] * g[2]).sqrt();
                let face = if gl_ > 1e-12 { g.map(|v| v / gl_) } else { [0.0, 1.0, 0.0] };
                for &k in &t {
                    acc.pos.push(vpos(p.verts[k]));
                    let n = p
                        .normals
                        .as_ref()
                        .map(|n| vnrm(n[k]))
                        .filter(|n| n[0] * n[0] + n[1] * n[1] + n[2] * n[2] > 0.5);
                    acc.nrm.push(n.unwrap_or(face));
                    let uv = p.uvs.as_ref().map_or([0, 0], |u| u[k]);
                    acc.uv.push([(uv[0] as f32 + 0.5) / 256.0, (uv[1] as f32 + 0.5) / 256.0]);
                    // 0x80 = 1.0 (GPU colour modulation); untextured colours are absolute.
                    let c = p.colors.as_ref().map_or([128, 128, 128], |c| c[k]);
                    let div = if tex.is_some() { 128.0 } else { 255.0 };
                    acc.col.push(
                        [0, 1, 2]
                            .map(|j| (c[j] as f32 / div).min(1.0))
                            .into_iter()
                            .chain([1.0])
                            .collect::<Vec<_>>()
                            .try_into()
                            .unwrap(),
                    );
                    acc.joints.push([(place.joint)(oi).unwrap_or(0), 0, 0, 0]);
                }
            }
        }
    }
    order
        .into_iter()
        .map(|mi| {
            let a = groups.remove(&mi).unwrap();
            let n = a.pos.len();
            Primitive {
                positions: a.pos,
                normals: Some(a.nrm),
                uvs: textured.contains(&mi).then_some(a.uv),
                colors: Some(a.col),
                weights: skinned.then(|| vec![[1.0, 0.0, 0.0, 0.0]; n]),
                joints: skinned.then_some(a.joints),
                indices: (0..n as u32).collect(),
                material: Some(mi),
                variants: vec![],
            }
        })
        .collect()
}
