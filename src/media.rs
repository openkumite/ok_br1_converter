//! Phase 1 conversions: TIM art, XA music, CD audio, VAB sound effects, STR movies.

use crate::audio;
use crate::disc::{Disc, RAW_SECTOR};
use crate::error::Result;
use crate::tim;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;

fn stem(path: &str) -> &str {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.split('.').next().unwrap_or(name)
}

pub fn images(disc: &mut Disc, out: &Path) -> Result<Vec<Value>> {
    let mut list = Vec::new();
    for f in disc.files.clone().iter().filter(|f| f.path.ends_with(".TIM")) {
        let t = tim::Tim::parse(&disc.read_file(f)?)?;
        let rel = format!("images/{}.png", f.path.trim_end_matches(".TIM"));
        std::fs::create_dir_all(out.join(&rel).parent().unwrap())?;
        tim::write_png(&out.join(&rel), &t.image(0)?)?;
        let group = f.path.split('/').next().unwrap_or_default();
        list.push(json!({"name": stem(&f.path), "group": group, "path": rel,
            "width": t.width, "height": t.height, "bpp": t.bpp, "source": f.path}));
    }
    println!("images: {}", list.len());
    Ok(list)
}

/// Each BGM XA file interleaves up to 8 stereo channels, one song per channel.
pub fn xa_music(disc: &mut Disc, out: &Path) -> Result<Vec<Value>> {
    std::fs::create_dir_all(out.join("music"))?;
    let mut list = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for f in disc.files.clone().iter().filter(|f| f.path.ends_with(".XA")) {
        list.extend(xa_file(disc, out, f, &mut seen)?);
    }
    // CD-DA tracks (index 01 starts after the 2-second pregap).
    for t in disc.tracks.iter().filter(|t| t.mode == "AUDIO") {
        let bytes = std::fs::read(&t.path)?;
        let pcm: Vec<i16> = bytes[(150 * RAW_SECTOR).min(bytes.len())..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| i16::from_le_bytes([b[0], b[1]]))
            .collect();
        let name = format!("cdda_track{:02}", t.number);
        let rel = format!("music/{name}.flac");
        audio::write_flac(&out.join(&rel), 44100, 2, &pcm)?;
        let secs = pcm.len() as f64 / 2.0 / 44100.0;
        list.push(json!({"name": name, "path": rel, "source": format!("Track {}", t.number),
            "rate": 44100, "channels": 2, "seconds": (secs * 10.0).round() / 10.0}));
    }
    println!("music: {}", list.len());
    Ok(list)
}

/// Decodes one interleaved XA file: every non-silent, non-duplicate channel becomes a FLAC.
/// `seen` holds hashes of already exported channels (some files repeat songs).
pub fn xa_file(
    disc: &mut Disc,
    out: &Path,
    f: &crate::disc::DiscFile,
    seen: &mut std::collections::HashSet<u64>,
) -> Result<Vec<Value>> {
    std::fs::create_dir_all(out.join("music"))?;
    let raw = disc.read_raw_file(f)?;
    let mut list = Vec::new();
    let mut chans: BTreeMap<u8, (audio::XaDecoder, Vec<i16>, bool, u32, bool)> = BTreeMap::new();
    for s in raw.as_chunks::<RAW_SECTOR>().0 {
        let (submode, coding) = (s[18], s[19]);
        if submode & 0x04 == 0 {
            continue; // not an audio sector
        }
        let stereo = coding & 1 != 0;
        let rate = if coding & 4 != 0 { 18900 } else { 37800 };
        let e = chans.entry(s[17]).or_insert((audio::XaDecoder::new(), Vec::new(), stereo, rate, false));
        if e.4 {
            continue; // past this channel's end-of-file marker
        }
        e.0.sector(s, stereo, &mut e.1);
        e.4 = submode & 0x80 != 0;
    }
    for (ch, (_, pcm, stereo, rate, _)) in chans {
        if pcm.iter().all(|&v| v.unsigned_abs() < 8) {
            continue;
        }
        // Some channels are byte-identical copies (e.g. BGM04 ch2..7).
        let mut h = std::collections::hash_map::DefaultHasher::new();
        std::hash::Hash::hash(&pcm, &mut h);
        if !seen.insert(std::hash::Hasher::finish(&h)) {
            continue;
        }
        let nch = if stereo { 2 } else { 1 };
        let name = format!("{}_ch{ch}", stem(&f.path));
        let rel = format!("music/{name}.flac");
        audio::write_flac(&out.join(&rel), rate, nch, &pcm)?;
        let secs = pcm.len() as f64 / nch as f64 / rate as f64;
        list.push(json!({"name": name, "path": rel, "source": f.path, "channel": ch,
            "rate": rate, "channels": nch, "seconds": (secs * 10.0).round() / 10.0}));
    }
    Ok(list)
}

pub fn sfx(disc: &mut Disc, out: &Path) -> Result<Vec<Value>> {
    let mut banks = Vec::new();
    for f in disc.files.clone().iter().filter(|f| f.path.ends_with(".SND")) {
        let samples = match audio::parse_vab(&disc.read_file(f)?) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("skip {}: {e}", f.path);
                continue;
            }
        };
        let bank = stem(&f.path).to_string();
        std::fs::create_dir_all(out.join("sfx").join(&bank))?;
        let mut items = Vec::new();
        for s in samples {
            let rel = format!("sfx/{bank}/{bank}_{:03}.wav", s.index);
            audio::write_wav(&out.join(&rel), s.rate, 1, &s.pcm)?;
            items.push(json!({"name": format!("{bank}_{:03}", s.index), "path": rel, "rate": s.rate,
                "loop": s.looped, "seconds": ((s.pcm.len() as f64 / s.rate as f64) * 100.0).round() / 100.0}));
        }
        banks.push(json!({"bank": bank, "source": f.path, "items": items}));
    }
    println!("sfx banks: {}", banks.len());
    Ok(banks)
}

pub fn videos(disc: &mut Disc, out: &Path) -> Result<Vec<Value>> {
    let mut list = Vec::new();
    for f in disc.files.clone().iter().filter(|f| f.path.ends_with(".STR")) {
        list.push(video(disc, out, f)?);
        println!("  video {}", stem(&f.path));
    }
    Ok(list)
}

/// Converts one STR movie to MP4 (H.264/AAC via ffmpeg) plus a poster JPEG.
pub fn video(disc: &mut Disc, out: &Path, f: &crate::disc::DiscFile) -> Result<Value> {
    let cache = out.join(".cache");
    std::fs::create_dir_all(&cache)?;
    std::fs::create_dir_all(out.join("video"))?;
    let tmp = cache.join(format!("{}.str", stem(&f.path)));
    std::fs::write(&tmp, disc.read_raw_file(f)?)?;
    let rel = format!("video/{}.mp4", stem(&f.path));
    let poster = format!("video/{}.jpg", stem(&f.path));
    audio::ffmpeg(&[
        "-i",
        tmp.to_str().unwrap(),
        "-c:v",
        "libx264",
        "-crf",
        "16",
        "-preset",
        "slow",
        "-pix_fmt",
        "yuv420p",
        "-c:a",
        "aac",
        "-b:a",
        "192k",
        "-movflags",
        "+faststart",
        out.join(&rel).to_str().unwrap(),
    ])?;
    // Poster: most representative frame shortly after 30% (openings fade from black).
    let secs = f.size as f64 / 2048.0 / 150.0; // 2x CD speed
    audio::ffmpeg(&[
        "-ss",
        &format!("{:.1}", secs * 0.3),
        "-i",
        out.join(&rel).to_str().unwrap(),
        "-vf",
        "thumbnail=60",
        "-frames:v",
        "1",
        out.join(&poster).to_str().unwrap(),
    ])?;
    std::fs::remove_file(&tmp).ok();
    Ok(json!({"name": stem(&f.path), "path": rel, "poster": poster, "source": f.path}))
}
