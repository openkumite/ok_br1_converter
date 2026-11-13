//! Fighter export: TMD meshes (XX20.DAT) + textures (XX10..12.DAT) -> glTF.

use crate::anim::{Anm, Chains, M3, Pose, apply, rot_y, world};
use crate::chainsim::{ChainSim, Tables};
use crate::disc::Disc;
use crate::error::{Context, Result};
#[cfg(feature = "cli")]
use crate::gallery;
use crate::gltf::{Channel, Gltf};
use crate::model::{self, Materials, Placement};
use crate::pack;
use crate::tmd;
use crate::vram::Vram;
use serde_json::{Value, json};
use std::path::Path;

/// Fighter folders on the disc.
pub const FIGHTERS: &[&str] = &["EVE", "FOX", "GAD", "GRE", "MIT", "RON", "URI", "VAK", "ZIN"];

/// Display name for a fighter folder: the in-game name-plate order (EXE table
/// 0x801A7178, verified by rendering the HUD) follows the EXE character IDs.
pub fn fighter_name(id: &str) -> &'static str {
    match id {
        "ZIN" => "Yugo",
        "GRE" => "Greg",
        "EVE" => "Alice",
        "VAK" => "Bakuryu",
        "MIT" => "Mitsuko",
        "GAD" => "Gado",
        "FOX" => "Fox",
        "RON" => "Long",
        "URI" => "Uriko",
        _ => "?",
    }
}

pub fn unpack_file(disc: &mut Disc, path: &str) -> Result<Vec<(usize, Vec<u8>)>> {
    let f = disc.find(path).with_context(|| format!("{path} not on disc"))?.clone();
    pack::unpack(&disc.read_file(&f)?)
}

pub fn vram_from(files: &[&[(usize, Vec<u8>)]]) -> Vram {
    let mut v = Vram::new();
    for entries in files {
        for (_, d) in entries.iter() {
            v.load_tim(d);
        }
    }
    v
}

/// Writes a .glb plus its gallery wrapper and returns the manifest entry.
#[cfg(feature = "cli")]
pub fn save_glb(out: &Path, gl: &Gltf, name: &str, group: &str, note: &str) -> Result<Value> {
    std::fs::create_dir_all(out.join("models"))?;
    let glb = gl.to_glb();
    let rel = format!("models/{name}.glb");
    std::fs::write(out.join(&rel), &glb)?;
    gallery::write_glb_js(&out.join(format!("{rel}.js")), name, &glb)?;
    Ok(json!({"name": name, "group": group, "path": rel, "js": format!("{rel}.js"), "note": note}))
}

/// A stage scene: the glTF, its triangle count and the background panorama (when the file has its tile map).
pub struct Scene {
    pub gl: Gltf,
    pub tris: usize,
    pub panorama: Option<crate::tim::Image>,
}

/// The stage file's entry whose TMD is the stage's object list, the ring walls first (`scene_gltf`).
const WALL_ENTRY: usize = 23;

/// The stage file's entry whose TMD the stage's code places (0x801761D8 loads it into every prop slot: G.10[k],
/// G.30[k] its object count; 0x8016D2C8 draws object j with the slot's matrix j), in object space.
const PROP_ENTRY: usize = 25;

/// Static scene: every TMD in `file` becomes one node (stages are in world space), the ring walls of a stage's object
/// list one node each (`tmd_023_wall0..3`) and its other objects too (`tmd_023_obj<k>`), the objects of the props' TMD one
/// node each (`tmd_025_prop0..`, in their own space).
pub fn scene_gltf(disc: &mut Disc, name: &str, file: &str, tex: &[&str]) -> Result<Scene> {
    let tex_entries: Vec<_> = tex.iter().map(|t| unpack_file(disc, t)).collect::<Result<_>>()?;
    let refs: Vec<&[(usize, Vec<u8>)]> = tex_entries.iter().map(|v| v.as_slice()).collect();
    let mut vram = vram_from(&refs);
    let entries = unpack_file(disc, file)?;
    for (i, d) in &entries {
        vram.load_tim_at(d, crate::vram::stage_clut_slot(*i, d));
    }
    let mut gl = Gltf::default();
    let mut mats = Materials::default();
    let root = gl.node(json!({"name": name}));
    gl.scene_roots.push(root);
    let mut tris = 0;
    let panorama = crate::stagebg::panorama(&entries);
    if let Some(img) = &panorama {
        // Floor at the stage's lowest geometry (PSX +Y is down), 360° around the
        // arena and far enough out to enclose the scenery (~105 m).
        let lowest = entries
            .iter()
            .filter_map(|(_, d)| tmd::parse(d).ok())
            .flat_map(|objs| objs.into_iter().flat_map(|o| o.verts))
            .map(|v| v[1])
            .max()
            .unwrap_or(0);
        let floor_y = model::conv([0.0, lowest as f32, 0.0])[1] - 0.05;
        let bg = crate::stagebg::add_to_scene(&mut gl, img, name, 150.0, floor_y);
        gl.add_child(root, bg);
    }
    for (i, d) in &entries {
        let Ok(objs) = tmd::parse(d) else { continue };
        let place = Placement { joint: &|_| None, transform: &|_, v| v };
        // The stage's object list (entry 23, 0x801761D8): objects 0..3 are the ring walls (+X, −X, −Z, +Z; the
        // walls' model ids, 0x80138180), each its own node `tmd_023_wall<k>` so that a broken wall can be hidden.
        let walls = if *i == WALL_ENTRY && objs.len() > 4 { 4 } else { 0 };
        let mut add = |gl: &mut Gltf, mats: &mut Materials, objs: &[tmd::Object], node: String| {
            let prims = model::primitives(gl, mats, &vram, objs, &place);
            if prims.is_empty() {
                return; // an empty object (Uriko's stage's walls 0..3): no node
            }
            tris += prims.iter().map(|p| p.indices.len() / 3).sum::<usize>();
            let mesh = gl.mesh(&format!("{name}_{}", &node[4..]), &prims);
            let n = gl.node(json!({"name": node, "mesh": mesh}));
            gl.add_child(root, n);
        };
        for k in 0..walls {
            add(&mut gl, &mut mats, &objs[k..k + 1], format!("tmd_{i:03}_wall{k}"));
        }
        if *i == PROP_ENTRY {
            for k in 0..objs.len() {
                add(&mut gl, &mut mats, &objs[k..k + 1], format!("tmd_{i:03}_prop{k}"));
            }
            continue;
        }
        if walls > 0 {
            // the rest of the object list one node each (`tmd_023_obj<k>`): Uriko's stage's code draws its walls from
            // objects 5..8, then 9..12 (0x801371A4: G.00 + 0x26C / 0x45C + the wall's model)
            for k in walls..objs.len() {
                add(&mut gl, &mut mats, &objs[k..k + 1], format!("tmd_{i:03}_obj{k}"));
            }
            continue;
        }
        add(&mut gl, &mut mats, &objs[walls..], format!("tmd_{i:03}"));
    }
    Ok(Scene { gl, tris, panorama })
}

/// `scene_gltf` written as `models/<name>.glb` (+ the panorama PNG); the manifest entry.
#[cfg(feature = "cli")]
pub fn export_scene(disc: &mut Disc, out: &Path, name: &str, group: &str, file: &str, tex: &[&str]) -> Result<Value> {
    let scene = scene_gltf(disc, name, file, tex)?;
    let mut note_bg = String::new();
    let mut panorama_rel = None;
    if let Some(img) = &scene.panorama {
        let rel = format!("textures/Backgrounds/{name}_panorama.png");
        std::fs::create_dir_all(out.join("textures/Backgrounds"))?;
        crate::tim::write_png(&out.join(&rel), img)?;
        note_bg = format!(", {}x{} panorama", img.width, img.height);
        panorama_rel = Some((rel, img.width, img.height));
    }
    let mut v = save_glb(out, &scene.gl, name, group, &format!("{file}, {} tris{note_bg}", scene.tris))?;
    if let Some((rel, w, h)) = panorama_rel {
        v["panorama"] = json!({"name": format!("{name}_panorama"), "group": "Backgrounds", "path": rel,
            "width": w, "height": h, "bpp": 8, "source": format!("{file} (tile map 0x22/0x23)")});
    }
    Ok(v)
}

/// Static export of one fighter TMD (until skinning is wired in).
#[cfg(feature = "cli")]
pub fn export_tmd_entry(
    disc: &mut Disc,
    out: &Path,
    id: &str,
    file: &str,
    index: usize,
    form: &str,
    tex: &[&str],
) -> Result<Value> {
    let tex_entries: Vec<_> = tex.iter().map(|t| unpack_file(disc, t)).collect::<Result<_>>()?;
    let refs: Vec<&[(usize, Vec<u8>)]> = tex_entries.iter().map(|v| v.as_slice()).collect();
    let vram = vram_from(&refs);
    let entries = unpack_file(disc, file)?;
    let (_, d) = entries.iter().find(|(i, _)| *i == index).context("no such entry")?;
    let objs = tmd::parse(d)?;
    let mut gl = Gltf::default();
    let mut mats = Materials::default();
    let place = Placement { joint: &|_| None, transform: &|_, v| v };
    let prims = model::primitives(&mut gl, &mut mats, &vram, &objs, &place);
    let name = format!("{id}_{form}");
    let mesh = gl.mesh(&name, &prims);
    let n = gl.node(json!({"name": name, "mesh": mesh}));
    gl.scene_roots.push(n);
    save_glb(
        out,
        &gl,
        &name,
        &format!("Fighter {id} · {}", fighter_name(id)),
        &format!("{} parts, unposed", objs.len()),
    )
}

/// Exports textures from every container on the disc.
#[cfg(feature = "cli")]
pub fn export_textures(disc: &mut Disc, out: &Path) -> Result<Vec<Value>> {
    let mut list = Vec::new();
    let dats: Vec<String> = disc.files.iter().map(|f| f.path.clone()).filter(|p| p.ends_with(".DAT")).collect();
    for path in dats {
        let Ok(entries) = unpack_file(disc, &path) else { continue };
        let stem = Path::new(&path).file_stem().unwrap().to_string_lossy().to_string();
        // Costume textures are used by the fighter meshes in XX20.DAT.
        let mut extra = Vec::new();
        let mut vram = Vram::new();
        if stem.len() == 5 && stem.as_bytes()[3] == b'1' {
            let id = &stem[..3];
            if let Ok(models) = unpack_file(disc, &format!("{id}/{id}20.DAT")) {
                extra = models.into_iter().map(|(_, d)| d).collect();
            }
        }
        let is_stage = stem.ends_with("00");
        for (i, d) in &entries {
            vram.load_tim_at(d, if is_stage { crate::vram::stage_clut_slot(*i, d) } else { None });
        }
        let extra_refs: Vec<&[u8]> = extra.iter().map(|d| d.as_slice()).collect();
        list.extend(crate::textures::export(out, &stem, &entries, &extra_refs, &vram)?);
    }
    println!("textures: {}", list.len());
    Ok(list)
}

/// PSX (Y-down) -> glTF basis change C = diag(1,-1,-1); C·M·C keeps rotations proper.
fn conv_m(m: &M3) -> M3 {
    let s = [1.0, -1.0, -1.0];
    let mut r = *m;
    for i in 0..3 {
        for j in 0..3 {
            r[i][j] = m[i][j] * s[i] * s[j];
        }
    }
    r
}

fn conv_t(t: [f64; 3]) -> [f32; 3] {
    model::conv([t[0] as f32, t[1] as f32, t[2] as f32])
}

fn quat(m: &M3) -> [f32; 4] {
    let tr = m[0][0] + m[1][1] + m[2][2];
    let (x, y, z, w) = if tr > 0.0 {
        let s = (tr + 1.0).sqrt() * 2.0;
        ((m[2][1] - m[1][2]) / s, (m[0][2] - m[2][0]) / s, (m[1][0] - m[0][1]) / s, 0.25 * s)
    } else if m[0][0] > m[1][1] && m[0][0] > m[2][2] {
        let s = (1.0 + m[0][0] - m[1][1] - m[2][2]).sqrt() * 2.0;
        (0.25 * s, (m[0][1] + m[1][0]) / s, (m[0][2] + m[2][0]) / s, (m[2][1] - m[1][2]) / s)
    } else if m[1][1] > m[2][2] {
        let s = (1.0 + m[1][1] - m[0][0] - m[2][2]).sqrt() * 2.0;
        ((m[0][1] + m[1][0]) / s, 0.25 * s, (m[1][2] + m[2][1]) / s, (m[0][2] - m[2][0]) / s)
    } else {
        let s = (1.0 + m[2][2] - m[0][0] - m[1][1]).sqrt() * 2.0;
        ((m[0][2] + m[2][0]) / s, (m[1][2] + m[2][1]) / s, 0.25 * s, (m[1][0] - m[0][1]) / s)
    };
    let l = (x * x + y * y + z * z + w * w).sqrt();
    [(x / l) as f32, (y / l) as f32, (z / l) as f32, (w / l) as f32]
}

/// Costume containers on the disc that don't belong to the fighter's mesh and
/// are skipped for model export (their textures are still exported).
/// URI11/URI12: page 000 is a byte copy of ZIN10's, pages 001/002 are shared
/// art that maps onto neither URI20 nor ZIN20 cleanly, and URI12's palette
/// TIMs are malformed (256x1). URI is the boss and has only costume 10 in game.
pub const UNUSED_COSTUMES: &[&str] = &["URI/URI11.DAT", "URI/URI12.DAT"];

/// Parts the state scripts can show or hide (op 0x1e): default hands 11/14
/// (shown), alternate hands 15/16 and extras 17/18 (hidden) — e.g. URI's robe.
const SWAPPABLE: [(usize, bool); 6] = [(11, true), (14, true), (15, false), (16, false), (17, false), (18, false)];

/// Planted-foot test: total horizontal slide of the feet (bones 7 and 10) while
/// they touch the ground (within 3 units of their lowest point, runs of 3+
/// frames), with vs. without the root offset. True when root motion doesn't
/// make the planted feet slide more (or nothing is planted, e.g. jumps).
#[allow(dead_code)]
fn root_motion_keeps_feet(parents: &[i16], poses: &[Pose], root: &[[f64; 4]]) -> bool {
    if parents.len() <= 10 || poses.is_empty() {
        return true;
    }
    let feet: Vec<[[f64; 3]; 2]> = poses
        .iter()
        .map(|p| {
            let (_, wt) = world(parents, p);
            [wt[7], wt[10]]
        })
        .collect();
    let drift = |with_root: bool| -> (f64, usize) {
        let (mut total, mut segments) = (0.0, 0);
        for k in 0..2 {
            let ymax = feet.iter().map(|f| f[k][1]).fold(f64::MIN, f64::max); // PSX +Y is down
            let mut run: Vec<[f64; 2]> = Vec::new();
            for (i, f) in feet.iter().enumerate().chain(std::iter::once((usize::MAX, &feet[0]))) {
                let r = root[i.min(root.len() - 1)];
                let planted = i != usize::MAX && f[k][1] > ymax - 3.0;
                if planted {
                    let (dx, dz) = if with_root { (r[0], r[2]) } else { (0.0, 0.0) };
                    run.push([f[k][0] + dx, f[k][2] + dz]);
                } else {
                    if run.len() >= 3 {
                        let r0 = run[0];
                        total += run.iter().map(|p| (p[0] - r0[0]).hypot(p[1] - r0[1])).fold(0.0, f64::max);
                        segments += 1;
                    }
                    run.clear();
                }
            }
        }
        (total, segments)
    };
    let ((with, n), (without, _)) = (drift(true), drift(false));
    n == 0 || with <= without
}

/// The game runs one animation frame per 60 Hz tick.
const FPS: f64 = 60.0;

/// Model file for a costume: costumes XX1n pair with XX2(n/3) when present
/// (EVE ships EVE21 for its costumes 13..15), otherwise XX20.
pub fn model_for_costume(disc: &Disc, id: &str, costume: &str) -> String {
    let n = costume.as_bytes()[costume.len() - 5] - b'0';
    let alt = format!("{id}/{id}2{}.DAT", n / 3);
    if disc.find(&alt).is_some() { alt } else { format!("{id}/{id}20.DAT") }
}

/// Skinned + animated fighter: TMD `index` of the costume's model file posed by skeleton `form` of XX.ANM.
#[cfg(feature = "cli")]
pub fn export_fighter(
    disc: &mut Disc,
    out: &Path,
    id: &str,
    form: usize,
    index: usize,
    costume: &str,
) -> Result<Value> {
    export_fighter_layered(disc, out, id, form, index, &[costume], &[])
}

/// One file per fighter form and mesh: the first costume is the default
/// material set, every costume is a KHR_materials_variants variant.
#[cfg(feature = "cli")]
pub fn export_fighter_costumes(
    disc: &mut Disc,
    out: &Path,
    id: &str,
    form: usize,
    index: usize,
    costumes: &[&str],
) -> Result<Value> {
    let (first, rest) = costumes.split_first().context("no costumes")?;
    export_fighter_layered(disc, out, id, form, index, &[first], rest)
}

/// Like `export_fighter`, uploading several costume containers to VRAM in order
/// (later ones overwrite earlier ones); the last one names the export.
#[cfg(feature = "cli")]
pub fn export_fighter_layered(
    disc: &mut Disc,
    out: &Path,
    id: &str,
    form: usize,
    index: usize,
    layers: &[&str],
    variants: &[&str],
) -> Result<Value> {
    let f = fighter_gltf(disc, id, form, index, layers, variants, true)?;
    save_glb(out, &f.gl, &f.name, &format!("Fighter {id} · {}", fighter_name(id)), &f.note)
}

/// A skinned fighter model: the glTF, its name (`XX_human`, `XX_beast_c13-15`, …) and the gallery note.
pub struct Fighter {
    pub gl: Gltf,
    pub name: String,
    pub note: String,
}

/// The fighter of `export_fighter_layered`; `animations` false leaves out every clip (mesh, skin, bind pose,
/// costume variants and the visibility joints only).
pub fn fighter_gltf(
    disc: &mut Disc,
    id: &str,
    form: usize,
    index: usize,
    layers: &[&str],
    variants: &[&str],
    animations: bool,
) -> Result<Fighter> {
    let costume = *layers.last().context("no costume")?;
    let anm_file = disc.find(&format!("{id}/{id}.ANM")).context("no ANM")?.clone();
    let anm_bytes = disc.read_file(&anm_file)?;
    let anm = Anm::parse(&anm_bytes)?;
    let cmn_bytes = disc.find("CMN/CMN.ACT").cloned().map(|c| disc.read_file(&c)).transpose()?;
    let cmn_anims = match &cmn_bytes {
        Some(c) if animations => crate::anim::parse_anims(c, 0, 127, anm.cmn_t1_scale)?,
        _ => Vec::new(),
    };
    let clips =
        if animations { crate::moves::clip_info(id, &anm_bytes, cmn_bytes.as_deref()) } else { Default::default() };
    let f = &anm.forms[form];
    let tex: Vec<_> = layers.iter().map(|c| unpack_file(disc, c)).collect::<Result<_>>()?;
    let refs: Vec<&[(usize, Vec<u8>)]> = tex.iter().map(|v| v.as_slice()).collect();
    let vram = vram_from(&refs);
    let model_file = model_for_costume(disc, id, costume);
    let entries = unpack_file(disc, &model_file)?;
    let (_, d) = entries.iter().find(|(i, _)| *i == index).context("no model entry")?;
    let mut objs = tmd::parse(d)?;
    let nb = f.parents.len();
    let bone_of = |o: usize| f.objmap.get(o).copied().filter(|&b| b >= 0 && (b as usize) < nb).map(|b| b as u16);
    for (o, obj) in objs.iter_mut().enumerate() {
        if bone_of(o).is_none() {
            obj.polys.clear(); // not attached to any bone
        }
    }
    // Swappable parts get their own visibility joint (child of their bone):
    // the state scripts show/hide them per clip (op 0x1e), exported as joint
    // scale 1/0. Every state starts with 11/14 shown and 15..18 hidden.
    let vis: Vec<(usize, usize, bool)> = SWAPPABLE
        .iter()
        .filter(|&&(o, _)| bone_of(o).is_some() && objs.get(o).is_some_and(|x| !x.polys.is_empty()))
        .map(|&(o, shown)| (o, bone_of(o).unwrap() as usize, shown))
        .collect();
    let joint_of =
        |o: usize| -> Option<u16> { vis.iter().position(|v| v.0 == o).map(|k| (nb + k) as u16).or_else(|| bone_of(o)) };
    let exe_file = disc.find("SCUS_941.99").context("no EXE")?.clone();
    let exe = disc.read_file(&exe_file)?;
    let chains = Chains::from_exe(&exe, id, form);
    let tables = Tables::from_exe(&exe).context("EXE tables")?;
    // Runs the game's chain physics (hair/ears/tails) over a sequence of poses:
    // `settle` frames holding the first pose, then one step per pose.
    let simulate = |poses: &mut [Pose], settle: usize| {
        let mut sim = ChainSim::new(&tables, &chains, &f.parents, &f.offsets);
        for i in 0..settle + poses.len() {
            let k = i.saturating_sub(settle);
            let (pw, pt) = world(&f.parents, &poses[k]);
            let rots = sim.step(&pw, &pt);
            if i >= settle {
                for (b, m) in rots {
                    poses[k].rot[b] = m;
                }
            }
        }
    };
    // Rest pose includes the chains settled under gravity in the bind pose.
    let mut rest = [anm.bind(form)];
    simulate(&mut rest, 120);
    let [bind] = rest;
    let (wm, wt) = world(&f.parents, &bind);

    let mut gl = Gltf::default();
    let mut mats = Materials::default();
    let transform = |o: usize, v: [f32; 3]| -> [f32; 3] {
        let b = bone_of(o).unwrap_or(0) as usize;
        let ofs = f.objofs.get(o).copied().unwrap_or_default();
        let p = apply(&wm[b], [v[0] as f64 + ofs[0], v[1] as f64 + ofs[1], v[2] as f64 + ofs[2]]);
        [(p[0] + wt[b][0]) as f32, (p[1] + wt[b][1]) as f32, (p[2] + wt[b][2]) as f32]
    };
    let place = Placement { joint: &joint_of, transform: &transform };
    let mut prims = model::primitives(&mut gl, &mut mats, &vram, &objs, &place);
    let tris: usize = prims.iter().map(|p| p.indices.len() / 3).sum();
    // The shadow model: the model file's TMD `index` + 2 (flat untextured polygons only). The game merges its object k
    // into body object k (the 0x7C objects' shadow lists +0x50 / +0x54), drawn flattened onto the floor by 0x8018DF8C
    // in the stage's shadow colour (0x8018C604); exported as a second mesh on the same skin, `<name>_shadow_proxy`.
    let shadow_prims = entries
        .iter()
        .find(|(i, _)| *i == index + 2)
        .and_then(|(_, d)| tmd::parse(d).ok())
        .filter(|s| s.iter().all(|o| o.polys.iter().all(|p| p.uvs.is_none())))
        .map(|mut s| {
            for (o, obj) in s.iter_mut().enumerate() {
                if bone_of(o).is_none() {
                    obj.polys.clear();
                }
            }
            model::primitives(&mut gl, &mut mats, &vram, &s, &place)
        })
        .filter(|p| !p.is_empty());
    let stem = |c: &str| Path::new(c).file_stem().unwrap().to_string_lossy().to_string();
    let costume_name = stem(costume);
    let form_name = ["human", "beast"][form];
    let name = if variants.is_empty() {
        format!("{id}_{form_name}_c{}", &costume_name[3..])
    } else if costume_name.ends_with("10") {
        format!("{id}_{form_name}")
    } else {
        format!("{id}_{form_name}_c{}-{}", &costume_name[3..], &stem(variants.last().unwrap())[3..])
    };
    // Costume variants: same geometry, materials re-created from each costume's VRAM.
    if !variants.is_empty() {
        gl.variant_names =
            std::iter::once(&costume).chain(variants).map(|c| format!("costume {}", &stem(c)[3..])).collect();
        for p in prims.iter_mut() {
            p.variants.push((p.material.unwrap(), 0));
        }
        for (k, c) in variants.iter().enumerate() {
            let tex = unpack_file(disc, c)?;
            let vram_k = vram_from(&[tex.as_slice()]);
            let mut mats_k = Materials::default();
            let suffix = format!("_c{}", &stem(c)[3..]);
            for p in prims.iter_mut() {
                let m = mats_k.variant(&mats, p.material.unwrap(), &mut gl, &vram_k, &suffix);
                p.variants.push((m, k + 1));
            }
        }
    }

    // Joint nodes with bind-pose local TRS.
    let joints: Vec<usize> = (0..nb)
        .map(|b| gl.node(json!({"name": format!("bone_{b:02}"), "rotation": quat(&conv_m(&bind.rot[b])), "translation": conv_t(bind.trans[b])})))
        .collect();
    let armature = gl.node(json!({"name": format!("{name}_armature")}));
    for b in 0..nb {
        match f.parents[b] {
            p if p >= 0 && (p as usize) < nb => gl.add_child(joints[p as usize], joints[b]),
            _ => gl.add_child(armature, joints[b]),
        }
    }
    let vis_joints: Vec<usize> = vis
        .iter()
        .map(|&(o, b, shown)| {
            let s = if shown { 1.0 } else { 0.0 };
            let n = gl.node(json!({"name": format!("vis_obj{o:02}"), "scale": [s, s, s]}));
            gl.add_child(joints[b], n);
            n
        })
        .collect();
    let skin_joints: Vec<usize> = joints.iter().chain(&vis_joints).copied().collect();
    let ibm: Vec<[f32; 16]> = (0..nb)
        .chain(vis.iter().map(|v| v.1))
        .map(|b| {
            let r = conv_m(&wm[b]);
            let t = conv_t(wt[b]).map(|v| v as f64);
            // inverse of [R t]: [R^T  -R^T t], column-major
            let it = [0, 1, 2].map(|i| -(r[0][i] * t[0] + r[1][i] * t[1] + r[2][i] * t[2]));
            let mut m = [0f32; 16];
            for c in 0..3 {
                for row in 0..3 {
                    m[c * 4 + row] = r[c][row] as f32; // column c of R^T = row c of R
                }
            }
            m[12] = it[0] as f32;
            m[13] = it[1] as f32;
            m[14] = it[2] as f32;
            m[15] = 1.0;
            m
        })
        .collect();
    let skin = gl.skin(&name, &skin_joints, &ibm, joints[0]);
    let mesh = gl.mesh(&name, &prims);
    let mesh_node = gl.node(json!({"name": name, "mesh": mesh, "skin": skin}));
    // Skinned mesh nodes belong at the scene root (their transform is ignored).
    gl.scene_roots.push(armature);
    gl.scene_roots.push(mesh_node);
    if let Some(sp) = &shadow_prims {
        let mesh = gl.mesh(&format!("{name}_shadow_proxy"), sp);
        let node = gl.node(json!({"name": format!("{name}_shadow_proxy"), "mesh": mesh, "skin": skin}));
        gl.scene_roots.push(node);
    }

    // Sound banks referenced by sound cues: (bank stem) -> (program, tone) -> VAG index.
    let mut banks: std::collections::HashMap<String, std::collections::HashMap<(usize, usize), usize>> =
        Default::default();
    let mut exported = 0;
    for ci in clips.iter().filter(|c| if form == 0 { c.human } else { c.beast }) {
        let pool = if ci.common { &cmn_anims } else { &anm.anims };
        let Some(a) = pool.iter().find(|a| a.index == ci.index) else { continue };
        let n = a.frames();
        let times: Vec<f32> = (0..=n).map(|i| (i as f64 / FPS) as f32).collect();
        let mut poses: Vec<_> = (0..=n).map(|i| anm.pose(form, a, i as f64)).collect();
        // Settle on the first frame, then simulate through the clip like the game.
        simulate(&mut poses, 120);
        let mut channels = Vec::new();
        let ch = |node, path, values, step| Channel { node, path, times: times.clone(), values, step };
        for (b, &joint) in joints.iter().enumerate().skip(1) {
            let q: Vec<f32> = poses.iter().flat_map(|p| quat(&conv_m(&p.rot[b]))).collect();
            channels.push(ch(joint, "rotation", q, false));
            if b == 1 || b == 4 {
                let t: Vec<f32> = poses.iter().flat_map(|p| conv_t(p.trans[b])).collect();
                channels.push(ch(joint, "translation", t, false));
            }
        }
        // Root motion on bone 0: cumulative offset and yaw in the facing frame at clip
        // start — only where it keeps planted feet planted (some clips move the body
        // through the hip bone instead, e.g. URI's walk).
        let has_root = ci.root.iter().any(|r| r.iter().any(|v| v.abs() > 1e-6));
        let root_ok = has_root;
        if root_ok {
            let at = |i: usize| ci.root[i.min(ci.root.len() - 1)];
            let t: Vec<f32> = (0..=n as usize).flat_map(|i| conv_t([at(i)[0], at(i)[1], at(i)[2]])).collect();
            let q: Vec<f32> = (0..=n as usize).flat_map(|i| quat(&conv_m(&rot_y(-at(i)[3])))).collect();
            channels.push(ch(joints[0], "translation", t, false));
            channels.push(ch(joints[0], "rotation", q, false));
        }
        // Part visibility: state default at frame 0, then the script's toggles.
        for (k, &(o, _, shown)) in vis.iter().enumerate() {
            let mut keys: Vec<(f32, f32)> = vec![(0.0, if shown { 1.0 } else { 0.0 })];
            for &(fr, _, visible) in ci.hands.iter().filter(|h| h.1 == o) {
                let t = (fr.clamp(0.0, n as f64) / FPS) as f32;
                keys.retain(|k| k.0 < t);
                keys.push((t, if visible { 1.0 } else { 0.0 }));
            }
            channels.push(Channel {
                node: vis_joints[k],
                path: "scale",
                times: keys.iter().map(|k| k.0).collect(),
                values: keys.iter().flat_map(|k| [k.1; 3]).collect(),
                step: true,
            });
        }
        // Sound cues, resolved to the exported WAVs where the sound is fixed.
        let mut sfx = Vec::new();
        for s in &ci.sfx {
            let mut e = json!({"frame": s.frame, "time": s.frame / FPS, "what": s.what});
            if let Some((bank, prog, tone)) = &s.sound {
                if !banks.contains_key(bank) {
                    let dir = if bank.starts_with("CMN") || bank.starts_with("GAME") { "CMN" } else { &bank[..3] };
                    let map = match disc.find(&format!("{dir}/{bank}.SND")).cloned() {
                        Some(f) => crate::audio::vab_tone_map(&disc.read_file(&f)?),
                        None => Default::default(),
                    };
                    banks.insert(bank.clone(), map);
                }
                e["bank"] = json!(bank);
                e["program"] = json!(prog);
                e["tone"] = json!(tone);
                if let Some(vag) = banks[bank].get(&(*prog, *tone)) {
                    e["wav"] = json!(format!("sfx/{bank}/{bank}_{vag:03}.wav"));
                }
            }
            sfx.push(e);
        }
        let forms: Vec<&str> =
            [ci.human.then_some("human"), ci.beast.then_some("beast")].into_iter().flatten().collect();
        let extras = json!({
            "index": ci.index, "label": ci.label, "loop": ci.looped, "frames": n, "fps": FPS,
            "forms": forms,
            "form_source": ci.form_source, "source": if ci.common { "CMN/CMN.ACT" } else { "own" },
            "states": ci.states, "sfx": sfx,
            "root_motion": if root_ok { "applied" } else if has_root { "skipped (feet slide with it)" } else { "none" },
            // Skipped clips keep the raw track: per frame [dx, dy, dz] in metres (glTF axes) and yaw in degrees.
            "root_track": if has_root && !root_ok {
                json!(ci.root.iter().map(|r| { let t = conv_t([r[0], r[1], r[2]]); [t[0], t[1], t[2], (r[3] * 360.0 / 4096.0) as f32] }).collect::<Vec<_>>())
            } else { Value::Null },
        });
        gl.animation(&format!("{:03} {}", ci.index, ci.label), &channels, extras);
        exported += 1;
    }
    let note = format!(
        "{nb} bones, {exported} anims, {tris} tris, costume{} {}, mesh {}",
        if variants.is_empty() { "" } else { "s" },
        std::iter::once(&costume).chain(variants).map(|c| stem(c)[3..].to_string()).collect::<Vec<_>>().join("/"),
        Path::new(&model_file).file_stem().unwrap().to_string_lossy()
    );
    Ok(Fighter { gl, name, note })
}

#[cfg(feature = "cli")]
/// Particle effects: every particle type as an animated sprite sheet, every
/// hit-spark preset (normal and blood) as a rendered burst, a few moving-particle
/// examples, and `vfx/vfx.json` with the decoded tables.
pub fn export_vfx(disc: &mut Disc, out: &Path) -> Result<Vec<Value>> {
    use crate::vfx::{self, Camera, Extra};
    let exe_f = disc.find("SCUS_941.99").context("no EXE")?.clone();
    let t = vfx::Tables::from_exe(&disc.read_file(&exe_f)?)?;
    // EFFECT.DAT pages/CLUTs plus the stage pages some particle types sample.
    let mut vram = Vram::new();
    for file in ["CMN/BIG00.DAT", "CMN/EFFECT.DAT"] {
        for (i, d) in unpack_file(disc, file)? {
            vram.load_tim_at(&d, crate::vram::stage_clut_slot(i, &d));
        }
    }
    std::fs::create_dir_all(out.join("vfx/particles"))?;
    std::fs::create_dir_all(out.join("vfx/sparks"))?;
    std::fs::write(out.join("vfx/vfx.json"), serde_json::to_string_pretty(&vfx::definitions(&t))?)?;
    let mut list = Vec::new();
    let mut save = |name: String, kind: &str, rel: String, seq: &[Vec<vfx::DrawCmd>], note: String| -> Result<()> {
        // Each sheet is framed on the bounding box of everything it draws.
        let (center, cell) = vfx::frame_of(seq);
        let (cols, bg) = (16, vfx::preview_bg(seq));
        crate::tim::write_png(&out.join(&rel), &vfx::render_sheet(seq, &vram, cell, center, cols, bg))?;
        list.push(json!({"name": name, "kind": kind, "path": rel, "frames": seq.len(), "cell": cell,
            "cols": cols, "fps": 60, "background": bg, "note": note}));
        Ok(())
    };
    let near = Camera { h: 512, dist: 2048, cx: 160, cy: 120, pitch: 0 };
    for ty in 0..t.types.len() {
        let pos = [0, -400, 0];
        let seq = vfx::particle_frames(&t, ty, 0, pos, [0; 3], Extra::default(), &near);
        let p = &t.types[ty];
        let blend = if p.semi { ["average", "add", "subtract", "add 1/4"][p.abr as usize] } else { "opaque" };
        save(
            format!("particle_{ty:02}"),
            "particle",
            format!("vfx/particles/particle_{ty:02}.png"),
            &seq,
            format!("{} frames, blend {blend}", p.frames.len()),
        )?;
    }
    let far = Camera { h: 512, dist: 1000, cx: 160, cy: 120, pitch: 0 };
    for preset in 0..t.sparks.len() {
        for blood in [false, true] {
            let seq = vfx::spark_frames(&t, preset, blood, &far, [0, -700, 0]);
            let name = format!("spark_{preset:02}{}", if blood { "_blood" } else { "" });
            save(
                name.clone(),
                "spark",
                format!("vfx/sparks/{name}.png"),
                &seq,
                format!("hit spark preset {preset}{}", if blood { " (blood on)" } else { "" }),
            )?;
        }
    }
    let examples = [
        ("debris_falling", 10, 6, [0, -600, 0], [40, -60, 10], Extra::default()),
        (
            "blood_stain",
            4,
            4,
            [0, 0, 0],
            [0; 3],
            Extra { b3c: Some(0xff), grav: Some(0), size: Some(80), ..Extra::default() },
        ),
        ("smoke_spin", 8, 13, [0, -500, 0], [0; 3], Extra { b3e: Some(0x100), b40: Some(0x4000), ..Extra::default() }),
        ("flare_rising", 22, 1, [0, -700, 0], [0, -20, 0], Extra::default()),
    ];
    for (name, ty, mode, pos, vel, extra) in examples {
        let seq = vfx::particle_frames(&t, ty, mode, pos, vel, extra, &far);
        save(
            name.to_string(),
            "motion",
            format!("vfx/particles/{name}.png"),
            &seq,
            format!("particle type {ty}, motion mode {mode}"),
        )?;
    }
    println!("vfx: {}", list.len());
    Ok(list)
}
