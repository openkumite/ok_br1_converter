//! End-to-end checks against the real game disc. Skipped (with a note) when the
//! disc isn't available; point BR1_DISC at the folder containing the .cue (or the .cue).

use ok_br1_converter::anim::{Anm, Chains, world};
use ok_br1_converter::chainsim::{ChainSim, Tables};
use ok_br1_converter::disc::Disc;
use ok_br1_converter::{audio, pack, tim, tmd};
use std::path::PathBuf;

fn disc() -> Option<Disc> {
    let path = std::env::var_os("BR1_DISC").map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(std::env::var("HOME").unwrap_or_default()).join("Downloads/Games/PS1/Bloody Roar (USA)")
    });
    match Disc::open(&path) {
        Ok(d) => Some(d),
        Err(e) => {
            eprintln!("skipping disc tests ({}): {e}", path.display());
            None
        }
    }
}

#[test]
fn all_containers_models_and_banks_decode() {
    let Some(mut d) = disc() else { return };
    assert_eq!(d.files.len(), 175, "ISO9660 file count");
    let (mut entries, mut tmds, mut tims, mut banks) = (0, 0, 0, 0);
    for f in d.files.clone() {
        if !d.in_data_track(&f) {
            continue;
        }
        let data =
            if f.path.ends_with(".STR") || f.path.ends_with(".XA") { continue } else { d.read_file(&f).unwrap() };
        if f.path.ends_with(".SND") {
            assert!(!audio::parse_vab(&data).unwrap().is_empty(), "{}", f.path);
            banks += 1;
        } else if f.path.ends_with(".DAT") {
            let Ok(list) = pack::unpack(&data) else { continue }; // XX30/XX31 are not containers
            for (_, e) in list {
                entries += 1;
                match e[0] {
                    0x41 => {
                        tmd::parse(&e).unwrap_or_else(|err| panic!("{} TMD: {err}", f.path));
                        tmds += 1;
                    }
                    0x10..=0x13 => {
                        tim::Tim::parse(&e).unwrap();
                        tims += 1;
                    }
                    _ => {}
                }
            }
        }
    }
    assert_eq!(entries, 410, "every compressed container entry");
    assert_eq!(banks, 28);
    assert!(tmds > 50 && tims > 300, "tmds {tmds} tims {tims}");
}

#[test]
fn every_fighter_skeleton_and_animation_parses() {
    let Some(mut d) = disc() else { return };
    for id in ok_br1_converter::chars::FIGHTERS {
        let f = d.find(&format!("{id}/{id}.ANM")).unwrap().clone();
        let anm = Anm::parse(&d.read_file(&f).unwrap()).unwrap();
        assert_eq!(anm.forms.len(), 2, "{id}");
        assert!(anm.anims.len() > 150, "{id}: {} anims", anm.anims.len());
        for form in 0..2 {
            let p = anm.pose(form, &anm.anims[0], 0.0);
            let (_, wt) = world(&anm.forms[form].parents, &p);
            // Head above feet (PSX +Y is down), roughly human height.
            assert!(wt[2][1] < wt[7][1] - 200.0, "{id} form {form}: head {:?} foot {:?}", wt[2], wt[7]);
        }
    }
}

/// Rest elevation of EVE's beast-form ears equals what the original game code
/// produced in the emulator (+31.6° / +33.1°).
#[test]
fn rabbit_ears_match_emulated_game() {
    let Some(mut d) = disc() else { return };
    let exe_f = d.find("SCUS_941.99").unwrap().clone();
    let exe = d.read_file(&exe_f).unwrap();
    let anm_f = d.find("EVE/EVE.ANM").unwrap().clone();
    let anm = Anm::parse(&d.read_file(&anm_f).unwrap()).unwrap();
    let form = &anm.forms[1];
    let (chains, tables) = (Chains::from_exe(&exe, "EVE", 1), Tables::from_exe(&exe).unwrap());
    let mut sim = ChainSim::new(&tables, &chains, &form.parents, &form.offsets);
    let mut pose = anm.bind(1);
    for _ in 0..120 {
        let (wm, wt) = world(&form.parents, &pose);
        for (b, m) in sim.step(&wm, &wt) {
            pose.rot[b] = m;
        }
    }
    let (_, wt) = world(&form.parents, &pose);
    let elev = |a: usize, b: usize| {
        let v = [0, 1, 2].map(|k| wt[b][k] - wt[a][k]);
        (-v[1]).atan2(v[0].hypot(v[2])).to_degrees()
    };
    assert!((elev(19, 20) - 31.6).abs() < 0.2, "left ear {}", elev(19, 20));
    assert!((elev(22, 23) - 33.1).abs() < 0.2, "right ear {}", elev(22, 23));
}

/// Runs the export pipeline (everything except the ffmpeg steps) into a temp dir.
#[test]
fn export_pipeline_end_to_end() {
    let Some(mut d) = disc() else { return };
    let out = std::env::temp_dir().join(format!("okconv-br1-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&out).unwrap();
    let glb_ok = |rel: &str| {
        let b = std::fs::read(out.join(rel)).unwrap();
        &b[0..4] == b"glTF" && u32::from_le_bytes(b[8..12].try_into().unwrap()) as usize == b.len()
    };

    let fighter = ok_br1_converter::chars::export_fighter(&mut d, &out, "EVE", 1, 1, "EVE/EVE10.DAT").unwrap();
    assert!(glb_ok(fighter["path"].as_str().unwrap()));
    assert!(fighter["note"].as_str().unwrap().contains("25 bones"));

    let stage =
        ok_br1_converter::chars::export_scene(&mut d, &out, "FOX_stage", "Stages", "FOX/FOX00.DAT", &[]).unwrap();
    assert!(glb_ok(stage["path"].as_str().unwrap()));
    assert_eq!(stage["panorama"]["width"], 1024);

    let extra = ok_br1_converter::chars::export_tmd_entry(
        &mut d,
        &out,
        "GRE",
        "GRE/GRE20.DAT",
        2,
        "extra_a",
        &["GRE/GRE10.DAT"],
    );
    assert!(glb_ok(extra.unwrap()["path"].as_str().unwrap()));

    let textures = ok_br1_converter::chars::export_textures(&mut d, &out).unwrap();
    assert_eq!(textures.len(), 1274);
    let images = ok_br1_converter::media::images(&mut d, &out).unwrap();
    assert_eq!(images.len(), 42);
    let sfx = ok_br1_converter::media::sfx(&mut d, &out).unwrap();
    assert_eq!(sfx.len(), 28);

    let mut m = serde_json::Map::new();
    m.insert("models".into(), vec![fighter, stage].into());
    ok_br1_converter::gallery::write(&out, &m).unwrap();
    assert!(out.join("index.html").exists() && out.join("manifest.js").exists());
    std::fs::remove_dir_all(&out).ok();
}

/// The CLI binary: `list` and `unpack` on the real disc.
#[test]
fn cli_list_and_unpack() {
    let Some(d) = disc() else { return };
    let disc_dir = d.tracks[0].path.parent().unwrap().to_path_buf();
    let bin = env!("CARGO_BIN_EXE_okconv-br1");
    let list = std::process::Command::new(bin).arg("list").arg(&disc_dir).output().unwrap();
    assert!(list.status.success());
    assert_eq!(String::from_utf8_lossy(&list.stdout).lines().count(), 175);

    let out = std::env::temp_dir().join(format!("okconv-br1-cli-{}", std::process::id()));
    let unpack = std::process::Command::new(bin).arg("unpack").arg(&disc_dir).arg("-o").arg(&out).output().unwrap();
    assert!(unpack.status.success());
    assert!(String::from_utf8_lossy(&unpack.stdout).contains("unpacked 410 entries"));
    std::fs::remove_dir_all(&out).ok();

    let bad = std::process::Command::new(bin).arg("list").arg("/nonexistent").output().unwrap();
    assert!(!bad.status.success());
}

fn has_ffmpeg() -> bool {
    std::process::Command::new("ffmpeg").arg("-version").output().is_ok_and(|o| o.status.success())
}

/// ffmpeg-based paths: one XA music file and one STR movie (skipped without ffmpeg).
#[test]
fn music_and_video_via_ffmpeg() {
    let Some(mut d) = disc() else { return };
    if !has_ffmpeg() {
        eprintln!("skipping: ffmpeg not found");
        return;
    }
    let out = std::env::temp_dir().join(format!("okconv-br1-av-{}", std::process::id()));
    let xa = d.find("MOVIE/BGM00.XA").unwrap().clone();
    let mut seen = std::collections::HashSet::new();
    let tracks = ok_br1_converter::media::xa_file(&mut d, &out, &xa, &mut seen).unwrap();
    assert_eq!(tracks.len(), 8, "BGM00 holds 8 songs");
    assert!(tracks.iter().all(|t| out.join(t["path"].as_str().unwrap()).exists() && t["rate"] == 37800));
    // Repeating the file adds nothing: every channel is now a duplicate.
    assert!(ok_br1_converter::media::xa_file(&mut d, &out, &xa, &mut seen).unwrap().is_empty());

    let str_file = d.find("MOVIE/ALICE.STR").unwrap().clone();
    let v = ok_br1_converter::media::video(&mut d, &out, &str_file).unwrap();
    let mp4 = std::fs::read(out.join(v["path"].as_str().unwrap())).unwrap();
    assert_eq!(&mp4[4..8], b"ftyp");
    assert!(out.join(v["poster"].as_str().unwrap()).exists());
    std::fs::remove_dir_all(&out).ok();
}

/// `convert --only images` through the CLI, including manifest and gallery output.
#[test]
fn cli_convert_images_writes_gallery() {
    let Some(d) = disc() else { return };
    let disc_dir = d.tracks[0].path.parent().unwrap().to_path_buf();
    let out = std::env::temp_dir().join(format!("okconv-br1-conv-{}", std::process::id()));
    let r = std::process::Command::new(env!("CARGO_BIN_EXE_okconv-br1"))
        .args([
            "convert".as_ref(),
            disc_dir.as_os_str(),
            "-o".as_ref(),
            out.as_os_str(),
            "--only".as_ref(),
            "images".as_ref(),
        ])
        .output()
        .unwrap();
    assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(out.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest["images"].as_array().unwrap().len(), 42);
    assert!(out.join("index.html").exists() && out.join("shaders/godot/ps1.gdshader").exists());
    std::fs::remove_dir_all(&out).ok();
}

/// The VFX against recorded values: tick count, draw count
/// and summed quad corners for hit sparks; then the full VFX export.
#[test]
fn vfx_matches_reference_and_exports() {
    use ok_br1_converter::vfx::{self, Camera, Extra};
    let Some(mut d) = disc() else { return };
    let exe_f = d.find("SCUS_941.99").unwrap().clone();
    let t = vfx::Tables::from_exe(&d.read_file(&exe_f).unwrap()).unwrap();
    let far = Camera { h: 512, dist: 1000, cx: 160, cy: 120, pitch: 0 };
    for (preset, blood, expect) in
        [(0, false, (53, 690, -91464)), (13, true, (104, 589, 140698)), (22, false, (123, 2300, -847131))]
    {
        let s = vfx::spark_frames(&t, preset, blood, &far, [0, -700, 0]);
        let n: usize = s.iter().map(Vec::len).sum();
        let sum: i64 = s.iter().flatten().map(|c| c.quad[0].0 + c.quad[3].1).sum();
        assert_eq!((s.len(), n, sum), expect, "spark {preset} blood {blood}");
    }
    let near = Camera { h: 512, dist: 2048, cx: 160, cy: 120, pitch: 0 };
    for (ty, expect) in [(0, (22, 21)), (16, (45, 44))] {
        let s = vfx::particle_frames(&t, ty, 0, [0, -400, 0], [0; 3], Extra::default(), &near);
        assert_eq!((s.len(), s.iter().filter(|c| !c.is_empty()).count()), expect, "type {ty}");
    }
    let out = std::env::temp_dir().join(format!("okconv-br1-vfx-{}", std::process::id()));
    let list = ok_br1_converter::chars::export_vfx(&mut d, &out).unwrap();
    assert_eq!(list.len(), 23 + 48 + 4);
    let defs: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(out.join("vfx/vfx.json")).unwrap()).unwrap();
    assert_eq!(defs["particle_types"].as_array().unwrap().len(), 23);
    std::fs::remove_dir_all(&out).ok();
}

/// Move data (state scripts + CMN.ACT): names, forms, loops, hand swaps, sound
/// cues; values checked against the reference decoder.
#[test]
fn move_data_labels_forms_hands_sfx() {
    let Some(mut d) = disc() else { return };
    let read = |d: &mut Disc, p: &str| {
        let f = d.find(p).unwrap().clone();
        d.read_file(&f).unwrap()
    };
    let cmn = read(&mut d, "CMN/CMN.ACT");
    let fox = ok_br1_converter::moves::clip_info("FOX", &read(&mut d, "FOX/FOX.ANM"), Some(&cmn));
    let by = |v: &[ok_br1_converter::moves::ClipInfo], i: usize| v.iter().find(|c| c.index == i).cloned().unwrap();
    let idle = by(&fox, 0);
    assert_eq!((idle.label.as_str(), idle.looped, idle.human, idle.beast), ("stance (idle)", true, true, false));
    assert_eq!(by(&fox, 1).label, "walk forward");
    assert!((by(&fox, 1).root.last().unwrap()[2] - -456.8).abs() < 0.1, "walk root motion");
    assert!(fox.iter().any(|c| c.common), "shared CMN.ACT clips used for empty slots");
    let stop = by(&fox, 4);
    assert_eq!(stop.sfx[0].sound, Some(("GAME00".to_string(), 1, 13)));
    let uri = ok_br1_converter::moves::clip_info("URI", &read(&mut d, "URI/URI.ANM"), Some(&cmn));
    assert!(uri.iter().any(|c| c.label.starts_with("command ")), "special moves labelled by input");
    let eve = ok_br1_converter::moves::clip_info("EVE", &read(&mut d, "EVE/EVE.ANM"), Some(&cmn));
    let win = by(&eve, 132);
    assert!(win.hands.iter().any(|h| h.1 == 15 && h.2), "alternate hand shown during the win pose");
    let total: usize = ["EVE", "FOX", "GAD", "GRE", "MIT", "RON", "URI", "VAK", "ZIN"]
        .iter()
        .map(|id| ok_br1_converter::moves::clip_info(id, &read(&mut d, &format!("{id}/{id}.ANM")), Some(&cmn)).len())
        .sum();
    assert_eq!(total, 1601, "clips across all fighters");
}

/// Exported fighter: named clips, visibility joints, per-clip root-motion decision.
#[test]
fn fighter_export_has_move_metadata() {
    let Some(mut d) = disc() else { return };
    let out = std::env::temp_dir().join(format!("okconv-br1-moves-{}", std::process::id()));
    let doc = |d: &mut Disc, id: &str| -> serde_json::Value {
        let v = ok_br1_converter::chars::export_fighter(d, &out, id, 0, 0, &format!("{id}/{id}10.DAT")).unwrap();
        let b = std::fs::read(out.join(v["path"].as_str().unwrap())).unwrap();
        let n = u32::from_le_bytes(b[12..16].try_into().unwrap()) as usize;
        serde_json::from_slice(&b[20..20 + n]).unwrap()
    };
    let fox = doc(&mut d, "FOX");
    let anims = fox["animations"].as_array().unwrap();
    assert_eq!(anims[0]["name"], "000 stance (idle)");
    let walk = anims.iter().find(|a| a["extras"]["label"] == "walk forward").unwrap();
    assert_eq!(walk["extras"]["root_motion"], "applied");
    let vis: Vec<&str> = fox["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|n| n["name"].as_str())
        .filter(|n| n.starts_with("vis_obj"))
        .collect();
    assert_eq!(vis, ["vis_obj11", "vis_obj14", "vis_obj15", "vis_obj16"]);
    let uri = doc(&mut d, "URI");
    let walk = uri["animations"].as_array().unwrap().iter().find(|a| a["extras"]["label"] == "walk forward").unwrap();
    assert_eq!(walk["extras"]["root_motion"], "applied");
    std::fs::remove_dir_all(&out).ok();
}

/// Costumes as KHR_materials_variants in one file; animation keys reduced.
#[test]
fn fighter_costume_variants() {
    let Some(mut d) = disc() else { return };
    let out = std::env::temp_dir().join(format!("okconv-br1-var-{}", std::process::id()));
    let costumes = ["FOX/FOX10.DAT", "FOX/FOX11.DAT", "FOX/FOX12.DAT"];
    let v = ok_br1_converter::chars::export_fighter_costumes(&mut d, &out, "FOX", 0, 0, &costumes).unwrap();
    assert_eq!(v["name"], "FOX_human");
    let b = std::fs::read(out.join(v["path"].as_str().unwrap())).unwrap();
    let n = u32::from_le_bytes(b[12..16].try_into().unwrap()) as usize;
    let doc: serde_json::Value = serde_json::from_slice(&b[20..20 + n]).unwrap();
    assert_eq!(doc["extensions"]["KHR_materials_variants"]["variants"].as_array().unwrap().len(), 3);
    let prims = doc["meshes"][0]["primitives"].as_array().unwrap();
    assert!(prims.iter().all(|p| p["extensions"]["KHR_materials_variants"]["mappings"].as_array().unwrap().len() == 3));
    // Reduced animation: far fewer keys than the 60 fps sample count.
    let acc = doc["accessors"].as_array().unwrap();
    let anim = &doc["animations"][0];
    let keys: u64 = anim["samplers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| acc[s["input"].as_u64().unwrap() as usize]["count"].as_u64().unwrap())
        .sum();
    let frames = anim["extras"]["frames"].as_u64().unwrap() + 1;
    assert!(keys < frames * anim["samplers"].as_array().unwrap().len() as u64 / 2, "keys {keys}");
    std::fs::remove_dir_all(&out).ok();
}
