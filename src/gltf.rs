//! Minimal glTF 2.0 binary (.glb) writer: meshes, skins, unlit textured
//! materials (nearest filtering, PS1 blend modes) and TRS animations.

use serde_json::{Value, json};

#[derive(Default)]
pub struct Gltf {
    bin: Vec<u8>,
    buffer_views: Vec<Value>,
    accessors: Vec<Value>,
    pub nodes: Vec<Value>,
    meshes: Vec<Value>,
    materials: Vec<Value>,
    textures: Vec<Value>,
    images: Vec<Value>,
    skins: Vec<Value>,
    animations: Vec<Value>,
    pub scene_roots: Vec<usize>,
    pub extras: Value,
    /// KHR_materials_variants variant names (e.g. costumes).
    pub variant_names: Vec<String>,
}

/// PS1 semi-transparency modes, stored in material extras and mapped to glTF alpha.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Blend {
    Opaque,
    /// Texels with colour 0x0000 are transparent (alpha test).
    Cutout,
    /// GPU mode 0..3 (0: B/2+F/2, 1: B+F, 2: B-F, 3: B+F/4).
    Semi(u8),
}

/// One animation channel: node, "rotation" | "translation" | "scale", key times
/// (seconds), flattened values; `step` = STEP interpolation instead of LINEAR.
pub struct Channel {
    pub node: usize,
    pub path: &'static str,
    pub times: Vec<f32>,
    pub values: Vec<f32>,
    pub step: bool,
}

/// Rotation keys may deviate by 0.1° and translation keys by 0.2 mm after reduction.
pub const KEY_TOLERANCE: (f64, f64) = (0.1, 0.0002);

/// Drops keys that interpolation between their neighbours reproduces within
/// tolerance (greedy: extend each segment while every skipped key fits).
/// Rotations are compared by angle after spherical interpolation.
pub fn reduce_keys(c: &Channel) -> Channel {
    let comps = if c.path == "rotation" { 4 } else { 3 };
    let n = c.times.len();
    if c.step || n <= 2 || c.values.len() != n * comps {
        return Channel { node: c.node, path: c.path, times: c.times.clone(), values: c.values.clone(), step: c.step };
    }
    let key = |i: usize| &c.values[i * comps..(i + 1) * comps];
    let fits = |a: usize, b: usize, k: usize| -> bool {
        let u = ((c.times[k] - c.times[a]) / (c.times[b] - c.times[a])) as f64;
        let (ka, kb, kk) = (key(a), key(b), key(k));
        if comps == 4 {
            let mut dot: f64 = (0..4).map(|i| ka[i] as f64 * kb[i] as f64).sum();
            let sign = if dot < 0.0 { -1.0 } else { 1.0 };
            dot = dot.abs().min(1.0);
            let th = dot.acos();
            let (wa, wb) =
                if th < 1e-6 { (1.0 - u, u) } else { (((1.0 - u) * th).sin() / th.sin(), (u * th).sin() / th.sin()) };
            let q: Vec<f64> = (0..4).map(|i| wa * ka[i] as f64 + wb * sign * kb[i] as f64).collect();
            let qn = q.iter().map(|v| v * v).sum::<f64>().sqrt().max(1e-12);
            let d: f64 = (0..4).map(|i| q[i] / qn * kk[i] as f64).sum::<f64>().abs().min(1.0);
            (2.0 * d.acos()).to_degrees() <= KEY_TOLERANCE.0
        } else {
            (0..3).all(|i| ((ka[i] as f64) * (1.0 - u) + kb[i] as f64 * u - kk[i] as f64).abs() <= KEY_TOLERANCE.1)
        }
    };
    let mut keep = vec![0];
    let mut a = 0;
    for b in 2..n {
        if !((a + 1)..b).all(|k| fits(a, b, k)) {
            keep.push(b - 1);
            a = b - 1;
        }
    }
    keep.push(n - 1);
    Channel {
        node: c.node,
        path: c.path,
        times: keep.iter().map(|&i| c.times[i]).collect(),
        values: keep.iter().flat_map(|&i| key(i).iter().copied()).collect(),
        step: false,
    }
}

pub struct Primitive {
    pub positions: Vec<[f32; 3]>,
    pub normals: Option<Vec<[f32; 3]>>,
    pub uvs: Option<Vec<[f32; 2]>>,
    pub colors: Option<Vec<[f32; 4]>>,
    pub joints: Option<Vec<[u16; 4]>>,
    pub weights: Option<Vec<[f32; 4]>>,
    pub indices: Vec<u32>,
    pub material: Option<usize>,
    /// KHR_materials_variants mappings: (material, variant index).
    pub variants: Vec<(usize, usize)>,
}

impl Gltf {
    fn view(&mut self, data: &[u8], target: Option<u32>) -> usize {
        while !self.bin.len().is_multiple_of(4) {
            self.bin.push(0);
        }
        let mut v = json!({"buffer": 0, "byteOffset": self.bin.len(), "byteLength": data.len()});
        if let Some(t) = target {
            v["target"] = t.into();
        }
        self.bin.extend_from_slice(data);
        self.buffer_views.push(v);
        self.buffer_views.len() - 1
    }

    fn accessor(
        &mut self,
        data: &[u8],
        target: Option<u32>,
        ctype: u32,
        count: usize,
        ty: &str,
        minmax: Option<(Vec<f32>, Vec<f32>)>,
    ) -> usize {
        let view = self.view(data, target);
        let mut a = json!({"bufferView": view, "componentType": ctype, "count": count, "type": ty});
        if let Some((mn, mx)) = minmax {
            a["min"] = mn.into();
            a["max"] = mx.into();
        }
        self.accessors.push(a);
        self.accessors.len() - 1
    }

    pub fn floats<const N: usize>(&mut self, v: &[[f32; N]], with_bounds: bool, target: Option<u32>) -> usize {
        let ty = match N {
            1 => "SCALAR",
            2 => "VEC2",
            3 => "VEC3",
            4 => "VEC4",
            16 => "MAT4",
            _ => unreachable!(),
        };
        let bytes: Vec<u8> = v.iter().flatten().flat_map(|f| f.to_le_bytes()).collect();
        let mm = with_bounds.then(|| {
            let mut mn = vec![f32::MAX; N];
            let mut mx = vec![f32::MIN; N];
            for e in v {
                for i in 0..N {
                    mn[i] = mn[i].min(e[i]);
                    mx[i] = mx[i].max(e[i]);
                }
            }
            (mn, mx)
        });
        self.accessor(&bytes, target, 5126, v.len(), ty, mm)
    }

    /// Adds a PNG image + texture (nearest filter, clamp) and returns the texture index.
    pub fn texture_png(&mut self, png: &[u8], name: &str) -> usize {
        self.texture_png_wrap(png, name, false)
    }

    /// `repeat`: REPEAT wrapping (sampler 1) instead of CLAMP_TO_EDGE (sampler 0).
    pub fn texture_png_wrap(&mut self, png: &[u8], name: &str, repeat: bool) -> usize {
        let view = self.view(png, None);
        self.images.push(json!({"bufferView": view, "mimeType": "image/png", "name": name}));
        // Samplers (NEAREST; 0 = CLAMP_TO_EDGE, 1 = REPEAT) are written in to_glb.
        self.textures.push(json!({"source": self.images.len() - 1, "sampler": repeat as u32}));
        self.textures.len() - 1
    }

    /// `lit` polygons used the PS1 light calculation; the rest are unlit (KHR_materials_unlit).
    pub fn material(
        &mut self,
        name: &str,
        texture: Option<usize>,
        blend: Blend,
        double_sided: bool,
        lit: bool,
    ) -> usize {
        let mut pbr = json!({"metallicFactor": 0.0, "roughnessFactor": 1.0});
        if let Some(t) = texture {
            pbr["baseColorTexture"] = json!({"index": t});
        }
        let mut m = json!({"name": name, "pbrMetallicRoughness": pbr, "doubleSided": double_sided});
        if !lit {
            m["extensions"] = json!({"KHR_materials_unlit": {}});
        }
        match blend {
            Blend::Opaque => {}
            Blend::Cutout => {
                m["alphaMode"] = "MASK".into();
                m["alphaCutoff"] = 0.5.into();
            }
            Blend::Semi(mode) => {
                m["alphaMode"] = "BLEND".into();
                // Approximation of the GPU blend equation; exact mode kept in extras.
                let alpha = [0.5, 1.0, 1.0, 0.25][mode as usize & 3];
                m["pbrMetallicRoughness"]["baseColorFactor"] = json!([1.0, 1.0, 1.0, alpha]);
            }
        }
        m["extras"] = json!({"ps1Blend": format!("{blend:?}")});
        self.materials.push(m);
        self.materials.len() - 1
    }

    pub fn mesh(&mut self, name: &str, prims: &[Primitive]) -> usize {
        let mut out = Vec::new();
        for p in prims {
            let mut attrs = serde_json::Map::new();
            attrs.insert("POSITION".into(), self.floats(&p.positions, true, Some(34962)).into());
            if let Some(n) = &p.normals {
                attrs.insert("NORMAL".into(), self.floats(n, false, Some(34962)).into());
            }
            if let Some(uv) = &p.uvs {
                attrs.insert("TEXCOORD_0".into(), self.floats(uv, false, Some(34962)).into());
            }
            if let Some(c) = &p.colors {
                attrs.insert("COLOR_0".into(), self.floats(c, false, Some(34962)).into());
            }
            if let Some(j) = &p.joints {
                let bytes: Vec<u8> = j.iter().flatten().flat_map(|v| v.to_le_bytes()).collect();
                attrs.insert("JOINTS_0".into(), self.accessor(&bytes, Some(34962), 5123, j.len(), "VEC4", None).into());
            }
            if let Some(w) = &p.weights {
                attrs.insert("WEIGHTS_0".into(), self.floats(w, false, Some(34962)).into());
            }
            let bytes: Vec<u8> = p.indices.iter().flat_map(|v| v.to_le_bytes()).collect();
            let idx = self.accessor(&bytes, Some(34963), 5125, p.indices.len(), "SCALAR", None);
            let mut prim = json!({"attributes": attrs, "indices": idx, "mode": 4});
            if let Some(m) = p.material {
                prim["material"] = m.into();
            }
            if !p.variants.is_empty() {
                let mappings: Vec<Value> =
                    p.variants.iter().map(|&(m, v)| json!({"material": m, "variants": [v]})).collect();
                prim["extensions"] = json!({"KHR_materials_variants": {"mappings": mappings}});
            }
            out.push(prim);
        }
        self.meshes.push(json!({"name": name, "primitives": out}));
        self.meshes.len() - 1
    }

    pub fn node(&mut self, v: Value) -> usize {
        self.nodes.push(v);
        self.nodes.len() - 1
    }

    pub fn add_child(&mut self, parent: usize, child: usize) {
        let n = &mut self.nodes[parent];
        if n.get("children").is_none() {
            n["children"] = json!([]);
        }
        n["children"].as_array_mut().unwrap().push(child.into());
    }

    /// `joints` are node indices; inverse bind matrices are column-major.
    pub fn skin(&mut self, name: &str, joints: &[usize], inverse_bind: &[[f32; 16]], skeleton: usize) -> usize {
        let ibm = self.floats(inverse_bind, false, None);
        self.skins.push(json!({"name": name, "joints": joints, "inverseBindMatrices": ibm, "skeleton": skeleton}));
        self.skins.len() - 1
    }

    /// channels: (node, "rotation"|"translation"|"scale", keyframe times, values (vec3/vec4 as Vec<f32>)).
    /// Adds an animation; `extras` (if not null) is stored on the animation.
    /// LINEAR channels are reduced to the keys needed to stay within
    /// `KEY_TOLERANCE` of the full-rate data (see `reduce_keys`).
    pub fn animation(&mut self, name: &str, channels: &[Channel], extras: Value) {
        let reduced: Vec<Channel> = channels.iter().map(reduce_keys).collect();
        let channels = &reduced[..];
        let mut samplers = Vec::new();
        let mut chans = Vec::new();
        // Channels sharing the same key times share one input accessor.
        let mut inputs: Vec<(&Vec<f32>, usize)> = Vec::new();
        for c in channels {
            let input = match inputs.iter().find(|(t, _)| *t == &c.times) {
                Some(&(_, i)) => i,
                None => {
                    let t: Vec<[f32; 1]> = c.times.iter().map(|&x| [x]).collect();
                    let i = self.floats(&t, true, None);
                    inputs.push((&c.times, i));
                    i
                }
            };
            let output = if c.path == "rotation" {
                let v: Vec<[f32; 4]> = c.values.as_chunks::<4>().0.iter().map(|c| [c[0], c[1], c[2], c[3]]).collect();
                self.floats(&v, false, None)
            } else {
                let v: Vec<[f32; 3]> = c.values.as_chunks::<3>().0.iter().map(|c| [c[0], c[1], c[2]]).collect();
                self.floats(&v, false, None)
            };
            let interp = if c.step { "STEP" } else { "LINEAR" };
            samplers.push(json!({"input": input, "output": output, "interpolation": interp}));
            chans.push(json!({"sampler": samplers.len() - 1, "target": {"node": c.node, "path": c.path}}));
        }
        let mut a = json!({"name": name, "samplers": samplers, "channels": chans});
        if !extras.is_null() {
            a["extras"] = extras;
        }
        self.animations.push(a);
    }

    pub fn to_glb(&self) -> Vec<u8> {
        let mut doc = json!({
            "asset": {"version": "2.0", "generator": "okconv-br1"},
            "scene": 0,
            "scenes": [{"nodes": self.scene_roots}],
            "nodes": self.nodes,
            "buffers": [{"byteLength": self.bin.len()}],
            "bufferViews": self.buffer_views,
            "accessors": self.accessors,
            "extensionsUsed": ["KHR_materials_unlit"],
        });
        if !self.variant_names.is_empty() {
            doc["extensionsUsed"].as_array_mut().unwrap().push("KHR_materials_variants".into());
            let v: Vec<Value> = self.variant_names.iter().map(|n| json!({"name": n})).collect();
            doc["extensions"] = json!({"KHR_materials_variants": {"variants": v}});
        }
        let mut put = |k: &str, v: &Vec<Value>| {
            if !v.is_empty() {
                doc[k] = v.clone().into();
            }
        };
        put("meshes", &self.meshes);
        put("materials", &self.materials);
        put("textures", &self.textures);
        put("images", &self.images);
        put("skins", &self.skins);
        put("animations", &self.animations);
        if !self.textures.is_empty() {
            let mut samplers = vec![json!({"magFilter": 9728, "minFilter": 9728, "wrapS": 33071, "wrapT": 33071})];
            if self.textures.iter().any(|t| t["sampler"] == 1) {
                samplers.push(json!({"magFilter": 9728, "minFilter": 9728, "wrapS": 10497, "wrapT": 10497}));
            }
            doc["samplers"] = samplers.into();
        }
        if !self.extras.is_null() {
            doc["asset"]["extras"] = self.extras.clone();
        }
        let mut js = serde_json::to_vec(&doc).unwrap();
        while !js.len().is_multiple_of(4) {
            js.push(b' ');
        }
        let mut bin = self.bin.clone();
        while !bin.len().is_multiple_of(4) {
            bin.push(0);
        }
        let total = 12 + 8 + js.len() + 8 + bin.len();
        let mut out = Vec::with_capacity(total);
        out.extend(b"glTF");
        out.extend(2u32.to_le_bytes());
        out.extend((total as u32).to_le_bytes());
        out.extend((js.len() as u32).to_le_bytes());
        out.extend(b"JSON");
        out.extend(js);
        out.extend((bin.len() as u32).to_le_bytes());
        out.extend(b"BIN\0");
        out.extend(bin);
        out
    }
}

/// Encodes RGBA pixels as PNG bytes.
pub fn png_bytes(img: &crate::tim::Image) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, img.width, img.height);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header().unwrap().write_image_data(&img.rgba).unwrap();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The PNG bytes of a fixed 64 × 64 page (a 15-bit colour pattern with transparent texels) do not drift: the deflate
    /// backend is pinned (flate2's zlib-rs, the one a build with other crates that enable it gets too; png's default
    /// features still enable miniz_oxide, which flate2 uses only without a zlib backend), so a disc converts to the
    /// same bytes in every build.
    #[test]
    fn png_bytes_are_pinned() {
        let rgba = (0..64 * 64u32)
            .flat_map(|i| {
                let c = (i.wrapping_mul(2654435761) >> 17) as u16 & if i % 7 == 0 { 0 } else { 0xFFFF };
                crate::tim::color15(c)
            })
            .collect();
        let png = png_bytes(&crate::tim::Image { width: 64, height: 64, rgba });
        let mut crc = flate2::Crc::new();
        crc.update(&png);
        // (miniz_oxide, flate2's default backend, gives 6661 bytes, CRC 0x10CA_8862)
        assert_eq!((png.len(), crc.sum()), (6675, 0xB4AC_3C09));
    }

    #[test]
    fn glb_container_layout() {
        let mut g = Gltf::default();
        let m = g.material("m", None, Blend::Cutout, false, false);
        let prim = Primitive {
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            normals: None,
            uvs: None,
            colors: None,
            joints: None,
            weights: None,
            indices: vec![0, 1, 2],
            material: Some(m),
            variants: vec![],
        };
        let mesh = g.mesh("tri", &[prim]);
        let n = g.node(json!({"mesh": mesh}));
        g.scene_roots.push(n);
        let glb = g.to_glb();
        assert_eq!(&glb[0..4], b"glTF");
        assert_eq!(u32::from_le_bytes(glb[8..12].try_into().unwrap()) as usize, glb.len());
        let jlen = u32::from_le_bytes(glb[12..16].try_into().unwrap()) as usize;
        assert_eq!(jlen % 4, 0);
        let doc: Value = serde_json::from_slice(&glb[20..20 + jlen]).unwrap();
        assert_eq!(doc["accessors"][0]["max"], json!([1.0, 1.0, 0.0]));
        assert_eq!(doc["materials"][0]["alphaMode"], "MASK");
        assert!(doc.get("samplers").is_none());
        assert_eq!(&glb[20 + jlen + 4..20 + jlen + 8], b"BIN\0");
    }

    #[test]
    fn key_reduction_keeps_shape() {
        // Linear ramp collapses to its endpoints; a corner is kept.
        let ramp = Channel {
            node: 0,
            path: "translation",
            times: (0..10).map(|i| i as f32).collect(),
            values: (0..10).flat_map(|i| [i as f32, 0.0, 0.0]).collect(),
            step: false,
        };
        assert_eq!(reduce_keys(&ramp).times, vec![0.0, 9.0]);
        let corner = Channel {
            node: 0,
            path: "translation",
            times: (0..5).map(|i| i as f32).collect(),
            values: [0.0, 1.0, 2.0, 1.0, 0.0].iter().flat_map(|&x| [x, 0.0, 0.0]).collect(),
            step: false,
        };
        assert_eq!(reduce_keys(&corner).times, vec![0.0, 2.0, 4.0]);
        // Constant rotation collapses; STEP channels are untouched.
        let still = Channel {
            node: 0,
            path: "rotation",
            times: vec![0.0, 1.0, 2.0],
            values: [0.0, 0.0, 0.0, 1.0].repeat(3),
            step: false,
        };
        assert_eq!(reduce_keys(&still).times.len(), 2);
        let step = Channel { node: 0, path: "scale", times: vec![0.0, 1.0, 2.0], values: vec![1.0; 9], step: true };
        assert_eq!(reduce_keys(&step).times.len(), 3);
    }

    #[test]
    fn repeat_sampler_only_when_used() {
        let mut g = Gltf::default();
        g.texture_png(&[0; 4], "a");
        let doc = |g: &Gltf| -> Value {
            let glb = g.to_glb();
            let jlen = u32::from_le_bytes(glb[12..16].try_into().unwrap()) as usize;
            serde_json::from_slice(&glb[20..20 + jlen]).unwrap()
        };
        assert_eq!(doc(&g)["samplers"].as_array().unwrap().len(), 1);
        g.texture_png_wrap(&[0; 4], "b", true);
        assert_eq!(doc(&g)["samplers"].as_array().unwrap().len(), 2);
    }
}
