use ok_br1_converter::{chars, disc, gallery, media, pack};

use anyhow::Result;
use clap::{Parser, Subcommand};
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(name = "okconv-br1", about = "Bloody Roar (USA, PS1) resource converter")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// List files on the disc
    List { disc: PathBuf },
    /// Dump every disc file verbatim into <out>/raw
    Extract {
        disc: PathBuf,
        #[arg(short, long, default_value = "out")]
        out: PathBuf,
    },
    /// Decompress every .DAT container entry into <out>/unpacked
    Unpack {
        disc: PathBuf,
        #[arg(short, long, default_value = "out")]
        out: PathBuf,
    },
    /// Export one fighter with an arbitrary costume file (for checking texture/mesh pairing)
    Fighter {
        disc: PathBuf,
        /// Fighter ID whose mesh and skeleton are used, e.g. ZIN
        id: String,
        /// Costume containers uploaded in order, e.g. URI/URI10.DAT,URI/URI11.DAT
        #[arg(value_delimiter = ',')]
        costume: Vec<String>,
        #[arg(short, long, default_value = "out")]
        out: PathBuf,
    },
    /// Convert resources to modern formats and build the HTML gallery
    Convert {
        disc: PathBuf,
        #[arg(short, long, default_value = "out")]
        out: PathBuf,
        /// Only run these steps (images, music, sfx, video). Default: all.
        /// The gallery is always rebuilt.
        #[arg(long, value_delimiter = ',')]
        only: Vec<String>,
    },
}

/// Loads the existing manifest so partial runs (--only) keep other sections.
fn load_manifest(out: &Path) -> Map<String, Value> {
    std::fs::read_to_string(out.join("manifest.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::List { disc } => {
            let d = disc::Disc::open(&disc)?;
            for f in &d.files {
                println!("{:<24} lba={:<7} size={}", f.path, f.lba, f.size);
            }
        }
        Cmd::Extract { disc, out } => {
            let mut d = disc::Disc::open(&disc)?;
            for f in d.files.clone() {
                if !d.in_data_track(&f) {
                    continue; // e.g. NEG_M01.WAV aliases the CD-audio track
                }
                let dst = out.join("raw").join(&f.path);
                std::fs::create_dir_all(dst.parent().unwrap())?;
                let data = if f.path.ends_with(".STR") || f.path.ends_with(".XA") {
                    d.read_raw_file(&f)?
                } else {
                    d.read_file(&f)?
                };
                std::fs::write(&dst, data)?;
            }
            println!("extracted {} files", d.files.len());
        }
        Cmd::Unpack { disc, out } => {
            let mut d = disc::Disc::open(&disc)?;
            let mut total = 0;
            for f in d.files.clone().iter().filter(|f| f.path.ends_with(".DAT")) {
                let Ok(entries) = pack::unpack(&d.read_file(f)?) else { continue };
                let dir = out.join("unpacked").join(f.path.trim_end_matches(".DAT"));
                std::fs::create_dir_all(&dir)?;
                for (i, data) in entries {
                    std::fs::write(dir.join(format!("{i:03}.bin")), data)?;
                    total += 1;
                }
            }
            println!("unpacked {total} entries");
        }
        Cmd::Fighter { disc, id, costume, out } => {
            let mut d = disc::Disc::open(&disc)?;
            for form in 0..2 {
                let layers: Vec<&str> = costume.iter().map(String::as_str).collect();
                let v = chars::export_fighter_layered(&mut d, &out, &id, form, form, &layers, &[])?;
                println!("{}", v["path"]);
            }
        }
        Cmd::Convert { disc, out, only } => {
            let mut d = disc::Disc::open(&disc)?;
            std::fs::create_dir_all(&out)?;
            let want = |s: &str| only.is_empty() || only.iter().any(|o| o == s);
            let mut m = load_manifest(&out);
            if want("images") {
                m.insert("images".into(), media::images(&mut d, &out)?.into());
            }
            if want("music") {
                m.insert("music".into(), media::xa_music(&mut d, &out)?.into());
            }
            if want("sfx") {
                m.insert("sfx".into(), media::sfx(&mut d, &out)?.into());
            }
            if want("models") {
                let mut list = Vec::new();
                let mut stages = Vec::new();
                for id in chars::FIGHTERS {
                    let tex = format!("{id}/{id}10.DAT");
                    // Every costume on the disc: XX10.DAT, XX11.DAT, XX12.DAT (EVE has up to XX15).
                    let costumes: Vec<String> = (0..10)
                        .map(|c| format!("{id}/{id}1{c}.DAT"))
                        .filter(|p| d.find(p).is_some() && !chars::UNUSED_COSTUMES.contains(&p.as_str()))
                        .collect();
                    // One file per form and mesh file, costumes as material variants.
                    let mut groups: Vec<(String, Vec<&str>)> = Vec::new();
                    for c in &costumes {
                        let m = chars::model_for_costume(&d, id, c);
                        match groups.iter_mut().find(|g| g.0 == m) {
                            Some(g) => g.1.push(c),
                            None => groups.push((m, vec![c])),
                        }
                    }
                    for (_, group) in &groups {
                        for form in 0..2 {
                            match chars::export_fighter_costumes(&mut d, &out, id, form, form, group) {
                                Ok(v) => list.push(v),
                                Err(e) => eprintln!("{id} form {form} {group:?}: {e:#}"),
                            }
                        }
                    }
                    // Unposed 15-part models of each mesh file, textured with its first costume.
                    let models: Vec<String> = [0, 1]
                        .map(|m| format!("{id}/{id}2{m}.DAT"))
                        .into_iter()
                        .filter(|p| d.find(p).is_some())
                        .collect();
                    for model in models {
                        let first = costumes
                            .iter()
                            .find(|c| chars::model_for_costume(&d, id, c) == model)
                            .cloned()
                            .unwrap_or(tex.clone());
                        let tag = &model[model.len() - 6..model.len() - 4];
                        for (i, form) in [(2, "extra_a"), (3, "extra_b")] {
                            let name = if tag == "20" { form.to_string() } else { format!("{form}_{tag}") };
                            if let Ok(v) = chars::export_tmd_entry(&mut d, &out, id, &model, i, &name, &[&first]) {
                                list.push(v);
                            }
                        }
                    }
                    stages.push(chars::export_scene(
                        &mut d,
                        &out,
                        &format!("{id}_stage"),
                        "Stages",
                        &format!("{id}/{id}00.DAT"),
                        &[],
                    )?);
                }
                for (f, name) in [("CMN/BIG00.DAT", "BIG_stage"), ("CMN/SMA00.DAT", "SMA_stage")] {
                    stages.push(chars::export_scene(&mut d, &out, name, "Stages", f, &[])?);
                }
                let backgrounds: Vec<Value> =
                    stages.iter_mut().filter_map(|s| s.as_object_mut()?.remove("panorama")).collect();
                println!("stage backgrounds: {}", backgrounds.len());
                m.insert("backgrounds".into(), backgrounds.into());
                list.extend(stages);
                list.push(chars::export_scene(&mut d, &out, "EFFECT", "VFX", "CMN/EFFECT.DAT", &["CMN/INIT.DAT"])?);
                println!("models: {}", list.len());
                m.insert("models".into(), list.into());
            }
            if want("textures") {
                m.insert("textures".into(), chars::export_textures(&mut d, &out)?.into());
            }
            if want("vfx") {
                m.insert("vfx".into(), chars::export_vfx(&mut d, &out)?.into());
            }
            if want("video") {
                m.insert("videos".into(), media::videos(&mut d, &out)?.into());
            }
            std::fs::write(out.join("manifest.json"), serde_json::to_string_pretty(&m)?)?;
            gallery::write(&out, &m)?;
            println!("gallery: {}", out.join("index.html").display());
        }
    }
    Ok(())
}
