//! The runtime-data conversion (`runtime::Converter`) of the real disc, read through a `Sectors` over the CUE's two
//! BIN files (the data track, the CD-DA track after its 2-second pregap). Skipped without the disc (BR1_DISC, as in
//! `tests/disc.rs`; the folder holding the .cue, or the .cue itself). BR1_RUNTIME_OUT=DIR also writes the files there.

use ok_br1_converter::disc::{AudioTrack, Disc, RAW_SECTOR, Sectors};
use ok_br1_converter::okt;
use ok_br1_converter::runtime::{Converter, Sink, TABLES, crc32};
use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;

struct Bins {
    data: std::fs::File,
    count: u32,
    /// track 2: the file, the sector of its INDEX 01, sectors from there
    audio: Option<(std::fs::File, u32, u32)>,
}

impl Sectors for Bins {
    fn raw(&self) -> bool {
        true
    }
    fn sectors(&self) -> u32 {
        self.count
    }
    fn read_sector(&mut self, lba: u32, out: &mut [u8]) -> ok_br1_converter::Result<()> {
        self.data.seek(SeekFrom::Start(lba as u64 * RAW_SECTOR as u64))?;
        self.data.read_exact(&mut out[..RAW_SECTOR])?;
        Ok(())
    }
    fn audio_tracks(&self) -> Vec<AudioTrack> {
        self.audio.iter().map(|a| AudioTrack { number: 2, sectors: a.2 }).collect()
    }
    fn read_audio(&mut self, _track: u32, sector: u32, out: &mut [u8]) -> ok_br1_converter::Result<()> {
        let (f, start, _) = self.audio.as_mut().ok_or(ok_br1_converter::Error::Format("no track 2".into()))?;
        f.seek(SeekFrom::Start((*start + sector) as u64 * RAW_SECTOR as u64))?;
        f.read_exact(&mut out[..RAW_SECTOR])?;
        Ok(())
    }
}

/// Size and CRC-32 of each file; the files themselves under `dir` when given; the bytes of the manifest and the tables.
struct Check {
    dir: Option<PathBuf>,
    files: BTreeMap<String, (u64, u32)>,
    kept: BTreeMap<String, Vec<u8>>,
}

impl Sink for Check {
    fn put(&mut self, path: &str, data: &[u8]) -> Result<(), String> {
        if let Some(d) = &self.dir {
            let p = d.join(path);
            std::fs::create_dir_all(p.parent().unwrap()).map_err(|e| e.to_string())?;
            std::fs::write(p, data).map_err(|e| e.to_string())?;
        }
        self.files.insert(path.to_string(), (data.len() as u64, crc32(data)));
        if path == "manifest.json" || path == "engine/tables.okt" {
            self.kept.insert(path.to_string(), data.to_vec());
        }
        Ok(())
    }
}

fn bins() -> Option<Bins> {
    let dir = std::env::var_os("BR1_DISC").map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(std::env::var("HOME").unwrap_or_default()).join("Downloads/Games/PS1/Bloody Roar (USA)")
    });
    // a .cue names its folder
    let dir =
        if dir.extension().is_some_and(|e| e.eq_ignore_ascii_case("cue")) { dir.parent()?.to_path_buf() } else { dir };
    let t1 = dir.join("Bloody Roar (USA) (Track 1).bin");
    let t2 = dir.join("Bloody Roar (USA) (Track 2).bin");
    let Ok(data) = std::fs::File::open(&t1) else {
        eprintln!("skipping: no {}", t1.display());
        return None;
    };
    let count = (data.metadata().ok()?.len() / RAW_SECTOR as u64) as u32;
    let audio = std::fs::File::open(&t2).ok().map(|f| {
        let n = (f.metadata().unwrap().len() / RAW_SECTOR as u64) as u32;
        (f, 150, n - 150)
    });
    Some(Bins { data, count, audio })
}

#[test]
fn converts_the_disc_for_a_runtime() {
    let Some(src) = bins() else { return };
    let t = std::time::Instant::now();
    let disc = Disc::open_checked(Box::new(src)).unwrap();
    let mut sink = Check {
        dir: std::env::var_os("BR1_RUNTIME_OUT").map(PathBuf::from),
        files: BTreeMap::new(),
        kept: BTreeMap::new(),
    };
    let mut steps = 0;
    let summary = Converter::new(disc)
        .run(&mut sink, &mut |p| {
            steps = p.total;
            true
        })
        .unwrap();
    eprintln!("{} steps, {} files in {:.1} s", steps, summary.files.len(), t.elapsed().as_secs_f64());
    assert!(summary.xa && summary.cdda);
    assert_eq!(summary.source, "bin");
    // every written file but the manifest is in the summary, with the size and CRC the sink saw
    for (p, v) in &summary.files {
        assert_eq!(sink.files.get(p), Some(v), "{p}");
    }
    assert_eq!(sink.files.len(), summary.files.len() + 1);
    let has = |p: &str| sink.files.contains_key(p);
    for p in [
        "engine/tables.okt",
        "engine/MIT/MIT.ANM",
        "engine/CMN/CMN.ACT",
        "engine/CMN/EFFECT.DAT",
        "audio/sfx/CMN/GAME00.SND",
        "pictures/ART0/ART000.TIM",
        "audio/cdda/track02.pcm",
        "audio/music/BGM00_ch0.xa",
        "movies/BR.bsf",
        "movies/BR.xa",
        "movies/FOX.bsf",
        "movies/GREG.bsf",
        "movies/MITUKO.bsf",
    ] {
        assert!(has(p), "{p}");
    }
    for id in ok_br1_converter::runtime::FIGHTERS {
        assert!(has(&format!("models/{id}_human.glb")) && has(&format!("models/{id}_beast.glb")), "{id}");
    }
    for id in ok_br1_converter::chars::FIGHTERS {
        assert!(has(&format!("stages/{id}00.glb")), "{id}");
    }
    assert!(!sink.files.keys().any(|p| p.ends_with(".WAV") || p.ends_with(".STR") || p.ends_with(".XA")));
    // the executable is not written (since format 3): its tables are
    assert!(!sink.files.contains_key("engine/SCUS_941.99"));
    let exe = {
        let mut d = Disc::open_checked(Box::new(bins().unwrap())).unwrap();
        let f = d.find("SCUS_941.99").unwrap().clone();
        d.read_file(&f).unwrap()
    };
    assert_eq!(exe.len(), 595968);
    // the tables (every one, the clips, their ranges, the EXE's length, the text) and the content id of this
    // disc
    let tables = okt::read(&sink.kept["engine/tables.okt"]).unwrap();
    assert_eq!(tables.len(), TABLES.len() + 4);
    let len = tables.iter().find(|t| t.name == "image_len").unwrap();
    assert_eq!(len.bytes, 595968u32.to_le_bytes());
    // the bytes the executable's clips read (CMN.ACT's clip 143, `clip_bank`): by file offset, inside the image, so
    // below every other table's RAM origin (no address resolves into both)
    let clips = tables.iter().find(|t| t.name == "anim.common_clips").unwrap();
    assert_eq!((clips.origin, clips.origin as usize + clips.bytes.len()), (0x15E6, 0x7_6859));
    for (a, b) in [(0x15E6, 0x75B5), (0x3_899A, 0x4_3536), (0x6_C703, 0x7_6859)] {
        assert_eq!(clips.bytes[a - 0x15E6..b - 0x15E6], exe[a..b]);
    }
    let ranges = tables.iter().find(|t| t.name == "anim.common_clips.ranges").unwrap();
    let pairs: Vec<u32> = ranges.bytes.chunks(4).map(|b| u32::from_le_bytes(b.try_into().unwrap())).collect();
    assert_eq!(pairs, [0x15E6, 0x75B5, 0x3_899A, 0x4_3536, 0x6_C703, 0x7_6859]);
    // zeros between the three clips' ranges (CMN.ACT's 143 of state 47, two the clip cycle reaches)
    for (a, b) in [(0x75B5, 0x3_899A), (0x4_3536, 0x6_C703)] {
        assert!(clips.bytes[a - 0x15E6..b - 0x15E6].iter().all(|&v| v == 0));
    }
    assert!(
        tables
            .iter()
            .filter(|t| !matches!(
                t.name.as_str(),
                "anim.common_clips" | "anim.common_clips.ranges" | "image_len" | "text.strings"
            ))
            .all(|t| t.origin >= 0x8000_0000)
    );
    // the text: every entry's key and length as the fixture lists them, in its order, and the CRC-32 of each group of
    // entries (each text and its NUL) at the group's `crc` line
    let text = tables.iter().find(|t| t.name == "text.strings").unwrap();
    let mut entries = text.bytes.split(|&b| b == 0);
    let mut group = Vec::new();
    for l in include_str!("fixtures/text_strings.txt")
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
    {
        let Some((key, v)) = l.split_once(' ') else { panic!("bad fixture line: {l}") };
        if key == "crc" {
            assert_eq!(format!("{:08x}", crc32(&group)), v, "the group ending before fixture line {l:?}");
            group.clear();
            continue;
        }
        assert_eq!(entries.next(), Some(key.as_bytes()));
        let t = entries.next().unwrap_or_else(|| panic!("{key}: no text"));
        assert_eq!(t.len().to_string(), v, "{key}");
        group.extend(t);
        group.push(0);
    }
    assert!(group.is_empty(), "entries after the fixture's last crc line");
    assert_eq!((entries.next(), entries.next()), (Some(&[][..]), None));
    let m: serde_json::Value = serde_json::from_slice(&sink.kept["manifest.json"]).unwrap();
    assert_eq!((m["format"].as_u64(), m["content_id"].as_u64()), (Some(4), Some(0x85DC_8E52)));
    let total: u64 = sink.files.values().map(|f| f.0).sum();
    eprintln!("{} MB", total >> 20);
}

/// What `clip_bank::Scripts::of` takes for the states whose start a call also enters: on every fighter they are
/// states 10, 15, 16, 18 and 63, and state 18 (the one the game starts, with F.50 bit 0x20 set) begins with
/// `0A 14 20000000` (the result = F.50 & 0x20) and `00 03` (return when it is 0): that return does not fire, and it is
/// the only return to an unknown pc under state 18's start.
#[test]
fn scripts_called_states() {
    let Some(src) = bins() else { return };
    let mut d = Disc::open_checked(Box::new(src)).unwrap();
    let mut read = |p: &str| {
        let f = d.find(p).unwrap_or_else(|| panic!("{p}")).clone();
        d.read_file(&f).unwrap()
    };
    let (exe, cmn) = (read("SCUS_941.99"), read("CMN/CMN.ACT"));
    for id in ok_br1_converter::chars::FIGHTERS {
        let anm = read(&format!("{id}/{id}31.DAT"));
        let s = ok_br1_converter::clip_bank::Scripts::of(&anm, &cmn, exe.len()).unwrap();
        assert_eq!(s.called_states.iter().copied().collect::<Vec<_>>(), [10, 15, 16, 18, 63], "{id}");
        let table = u32::from_le_bytes(anm[4..8].try_into().unwrap()) as usize;
        let start = u16::from_le_bytes(anm[table + 36..table + 38].try_into().unwrap()) as usize;
        assert_eq!(anm[start..start + 8], [0x0A, 0x14, 0x20, 0, 0, 0, 0, 3], "{id}");
        let under = s.unknown_returns.iter().filter(|&&(lp, _)| lp == start).map(|&(_, p)| p).collect::<Vec<_>>();
        assert_eq!(under, [start + 6], "{id}");
    }
}
