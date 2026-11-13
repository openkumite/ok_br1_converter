//! The runtime data of the Bloody Roar disc (feature `runtime`): a list of steps run one at a
//! time (`Converter::step`, for a caller that draws progress between them, e.g. a browser worker) or all at once
//! (`Converter::run`). Input is any `disc::Sectors` (the caller opens the image), output any `Sink`; no file system,
//! no external programs, the same bytes on every target for the files a game's simulation reads (`engine/**`,
//! copied verbatim; the glTF models are display data computed with f64).
//!
//! Layout (paths under the output folder), all listed in `manifest.json` `files` with size and CRC-32:
//!
//! | path | from |
//! |---|---|
//! | `engine/<DISC PATH>` | every Form 1 file of the disc but the executable (its tables are `engine/tables.okt`), the pictures, sound banks and streams, verbatim (`XX/XX.ANM`, `XX/XX00.DAT` …) |
//! | `pictures/ART0/*.TIM`, `pictures/ART1/*.TIM` | the 640×480 pictures, verbatim |
//! | `audio/sfx/<DISC PATH>.SND` | the VAB sound banks, verbatim |
//! | `models/<XX>_human.glb`, `<XX>_beast.glb` | `FIGHTERS`: skinned mesh, bind pose, every costume as a `KHR_materials_variants` variant, no animations |
//! | `stages/<XX>00.glb`, `BIG00.glb`, `SMA00.glb` | every stage scene with its sky band and floor |
//! | `audio/music/BGM0n_chK.xa` | each `MOVIE/BGM0n.XA` channel with sound (silent and repeated channels dropped), `.xa` v1 |
//! | `audio/cdda/track02.pcm` | the CD-DA track(s), `.pcm` v1 |
//! | `movies/<NAME>.bsf`, `movies/<NAME>.xa` | each `MOVIE/<NAME>.STR`: its MDEC frames and its CD-XA sound |
//!
//! `.xa` (version 1): `b"BR2XA\0"`, u8 version 1, u8 the XA subheader coding byte, u32 LE
//! sector count N, u32 0, then N × 2304 bytes (each audio sector's 18 sound groups) in disc order up to and including
//! the sector with the end-of-file bit.
//!
//! `.bsf`: `b"BRBS"`, u16 width, u16 height, u32 frames, then per frame u32 sector (in
//! the STR), u32 length, the MDEC bitstream.
//!
//! `.pcm` (version 1, CD-DA): `b"BR2PCM\0"`, u8 version 1, u32 LE rate (44100), u16 LE channels (2), u16 0, u32 LE
//! frames N, u32 0 (a 24-byte header), then N × channels 16-bit little-endian samples, interleaved (the track's bytes
//! from its INDEX 01, unchanged).

use crate::audio::XaDecoder;
use crate::chars;
use crate::disc::{AudioTrack, BOOT_EXE, Disc, DiscFile, RAW_SECTOR};
use crate::error::{Error, Result};
use crate::okt;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashSet};

/// The data format written (`manifest.json` `format`), for a reader to check. 2: `engine/tables.okt` and
/// `content_id`; 3: the executable's clip ranges (`anim.common_clips.ranges`) and no executable in `engine/`; 4: the
/// game's text (`text.strings`) and the default buttons (`input.default_buttons`).
pub const FORMAT_VERSION: u32 = 4;
/// `manifest.json` `game`.
pub const GAME: &str = "Bloody Roar (USA) SCUS-94199";
/// The fighters whose models are converted.
pub const FIGHTERS: [&str; 4] = ["MIT", "FOX", "GRE", "URI"];

pub const XA_MAGIC: &[u8; 6] = b"BR2XA\0";
pub const XA_HEADER: usize = 16;
/// The sound groups of a Mode 2 Form 2 audio sector (its user data without the trailing 20 bytes).
pub const XA_PAYLOAD: usize = 18 * 128;
pub const PCM_MAGIC: &[u8; 7] = b"BR2PCM\0";
pub const PCM_HEADER: usize = 24;
/// The longest CD-DA track believed (sectors of a 100-minute CD): a corrupt image's longer one is an error, not an
/// allocation of its size.
pub const MAX_CDDA_SECTORS: u32 = 100 * 60 * 75;

/// Where the converted files go. `put` replaces an existing file of the same path.
pub trait Sink {
    fn put(&mut self, path: &str, data: &[u8]) -> std::result::Result<(), String>;
}

/// A sink that keeps everything in memory (tests).
#[derive(Default)]
pub struct MemorySink {
    pub files: BTreeMap<String, Vec<u8>>,
}

impl Sink for MemorySink {
    fn put(&mut self, path: &str, data: &[u8]) -> std::result::Result<(), String> {
        self.files.insert(path.to_string(), data.to_vec());
        Ok(())
    }
}

/// Progress: the step being done and how many are done of how many.
#[derive(Clone, Debug)]
pub struct Progress {
    pub step: String,
    pub done: usize,
    pub total: usize,
}

/// What a finished conversion wrote.
#[derive(Clone, Debug)]
pub struct Summary {
    /// path → (size, CRC-32), manifest.json not included
    pub files: BTreeMap<String, (u64, u32)>,
    /// The CD-XA streams were read (music, movie sound): false for an ISO image.
    pub xa: bool,
    /// A CD-DA track was converted.
    pub cdda: bool,
    /// "bin", "chd" or "iso"
    pub source: &'static str,
}

/// CRC-32 (IEEE, the zlib one) of `data`, continuing `crc` (0 to start).
pub fn crc32_update(crc: u32, data: &[u8]) -> u32 {
    const TABLE: [u32; 256] = {
        let mut t = [0u32; 256];
        let mut i = 0;
        while i < 256 {
            let mut c = i as u32;
            let mut k = 0;
            while k < 8 {
                c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
                k += 1;
            }
            t[i] = c;
            i += 1;
        }
        t
    };
    let mut c = !crc;
    for &b in data {
        c = TABLE[((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8);
    }
    !c
}

pub fn crc32(data: &[u8]) -> u32 {
    crc32_update(0, data)
}

/// One stream's audio sectors as a `.xa` file.
pub struct XaWriter {
    pub sectors: u32,
    pub data: Vec<u8>,
    pub done: bool,
}

impl XaWriter {
    pub fn new(coding: u8) -> XaWriter {
        let mut data = Vec::new();
        data.extend_from_slice(XA_MAGIC);
        data.push(1);
        data.push(coding);
        data.extend_from_slice(&[0; 8]);
        XaWriter { sectors: 0, data, done: false }
    }
    /// Adds one raw 2352-byte audio sector; ignored after the end-of-file sector.
    pub fn push(&mut self, raw: &[u8]) {
        if self.done {
            return;
        }
        self.data.extend_from_slice(&raw[24..24 + XA_PAYLOAD]);
        self.sectors += 1;
        self.done = raw[18] & 0x80 != 0;
    }
    pub fn finish(mut self) -> Vec<u8> {
        let n = self.sectors.to_le_bytes();
        self.data[8..12].copy_from_slice(&n);
        self.data
    }
}

/// STR video sectors (Mode 2 Form 1, 32-byte header `u16 0x0160, u16 0x8001, u16 chunk, u16 chunks, u32 frame,
/// u32 frame bytes, u16 width, u16 height`, 2016 bytes of the frame) → `.bsf`.
#[derive(Default)]
pub struct Bsf {
    frames: Vec<BsfFrame>,
}

struct BsfFrame {
    no: u32,
    sector: usize,
    size: usize,
    w: u16,
    h: u16,
    chunks: BTreeMap<u16, Vec<u8>>,
}

impl Bsf {
    /// One raw 2352-byte sector, `s` = its index in the file.
    pub fn push(&mut self, s: usize, sec: &[u8]) {
        if sec[18] & 0x04 == 0 {
            self.push_user(s, &sec[24..24 + 2048]);
        }
    }

    /// A sector's 2048 user bytes (an ISO image: no subheader, so only the STR header tells a video sector).
    pub fn push_user(&mut self, s: usize, d: &[u8]) {
        let u16_at = |o: usize| u16::from_le_bytes([d[o], d[o + 1]]);
        let u32_at = |o: usize| u32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]]);
        if (u16_at(0), u16_at(2)) != (0x0160, 0x8001) {
            return;
        }
        let (chunk, fno, size, w, h) = (u16_at(4), u32_at(8), u32_at(12) as usize, u16_at(16), u16_at(18));
        if self.frames.last().is_none_or(|f| f.no != fno) {
            self.frames.push(BsfFrame { no: fno, sector: s, size, w, h, chunks: BTreeMap::new() });
        }
        if let Some(f) = self.frames.last_mut() {
            f.chunks.insert(chunk, d[32..32 + 2016].to_vec());
        }
    }

    pub fn frames(&self) -> usize {
        self.frames.len()
    }

    /// The MDEC bitstream version of the first frame (its header's fourth u16), 0 without frames.
    pub fn bs_version(&self) -> u16 {
        let Some(c) = self.frames.first().and_then(|f| f.chunks.values().next()) else { return 0 };
        u16::from_le_bytes([c[6], c[7]])
    }

    pub fn finish(self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"BRBS");
        let (w, h) = self.frames.first().map(|f| (f.w, f.h)).unwrap_or((0, 0));
        out.extend_from_slice(&w.to_le_bytes());
        out.extend_from_slice(&h.to_le_bytes());
        out.extend_from_slice(&(self.frames.len() as u32).to_le_bytes());
        for f in &self.frames {
            let mut data: Vec<u8> = f.chunks.values().flatten().copied().collect();
            data.truncate(f.size);
            out.extend_from_slice(&(f.sector as u32).to_le_bytes());
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(&data);
        }
        out
    }
}

/// A `.pcm` file's header for `frames` stereo 44.1 kHz frames.
pub fn pcm_header(frames: u32) -> [u8; PCM_HEADER] {
    let mut h = [0u8; PCM_HEADER];
    h[..7].copy_from_slice(PCM_MAGIC);
    h[7] = 1;
    h[8..12].copy_from_slice(&44100u32.to_le_bytes());
    h[12..14].copy_from_slice(&2u16.to_le_bytes());
    h[16..20].copy_from_slice(&frames.to_le_bytes());
    h
}

/// RAM address of the executable's file offset 0 (its text loads at 0x80120000 after the 0x800-byte header).
const EXE_BASE: u32 = crate::clip_bank::EXE_BASE;

/// The runtime's static tables (`engine/tables.okt`, `okt`): name, element type, count and the RAM address of the
/// first byte (the table's `origin`), each the executable's bytes there verbatim; ordered by address and never
/// overlapping (`tables_are_ordered`). The linked ones (`fx.*`, `fighter.parts`) hold addresses of their own
/// records, which keep pointing into their origins. Every byte of the executable a runtime was recorded reading
/// (`tests/fixtures/runtime_reads.txt`) is in one. `tables_okt` adds `anim.common_clips` (the bytes
/// the clips of the executable read, `clip_bank`; its origin is a file offset), `image_len` (a scalar, origin 0:
/// the executable's length) and `text.strings` (the game's text, origin 0: `text_table`).
pub const TABLES: &[(&str, okt::Elem, u32, u32)] = &[
    ("math.random_bytes", okt::Elem::U8, 0x100, 0x8012_0448),
    // per stage (G.94 0..10): the fighters' light, the walls, the colours
    ("stage.light", okt::Elem::Rec(0x44), 11, 0x8012_09A4),
    ("stage.walls", okt::Elem::Rec(18), 11, 0x8012_1044),
    ("stage.colours", okt::Elem::Rec(44), 11, 0x8012_125C),
    // effects' data (whirl rings, ramps …), linked
    ("fx.data_a", okt::Elem::Blob, 0x2BE, 0x8012_15E4),
    ("math.atan", okt::Elem::I16, 0x401, 0x8012_5C14),
    ("math.sin", okt::Elem::I16, 0x401, 0x8012_6B78),
    // per stage: the ring's edge, the hips test's ring, the shadow's floor (8-byte records)
    ("stage.ring_edge", okt::Elem::Rec(8), 15, 0x801A_3A70),
    ("stage.ring_hips", okt::Elem::Rec(8), 15, 0x801A_3AE8),
    ("stage.shadow_floor", okt::Elem::Rec(8), 15, 0x801A_3B60),
    // the effects' data: node map, records, palette, indices, ramps, sizes, chains, scripts, types, pages (linked)
    ("fx.data_b", okt::Elem::Blob, 0x1E68, 0x801A_3DA4),
    // the fighters' part records by form and character (a pointer list, then the records)
    ("fighter.parts", okt::Elem::Blob, 0x300, 0x801A_5C0C),
    // the sound definitions of the banks (6-byte records, the volume at +4)
    ("sound.volumes", okt::Elem::Blob, 0x7CC, 0x801A_5F0C),
    // the default button configuration
    ("input.default_buttons", okt::Elem::U16, 8, 0x801A_66F8),
    // a stage prop's leg angles
    ("stage.flap", okt::Elem::I16, 4, 0x801A_6B30),
    // the CPU's tables: wait, reaction, guard, recovery
    ("ai.wait", okt::Elem::U8, 0x40, 0x801A_7734),
    ("ai.react", okt::Elem::U8, 0x10, 0x801A_7774),
    ("ai.guard", okt::Elem::U8, 0x10, 0x801A_7784),
    ("ai.recover", okt::Elem::U8, 0x10, 0x801A_7794),
    ("math.sqrt", okt::Elem::I16, 192, 0x801A_A028),
    ("math.rsqrt", okt::Elem::I16, 192, 0x801A_A1BC),
    // the second arctangent table (the fighters' drawing)
    ("math.atan_b", okt::Elem::I16, 0x401, 0x801A_E348),
];

/// The text the game draws from the game's own lists, by the game's pointer tables: key prefix, the table's address,
/// its entries (`<prefix>.<n>` for entry n). `names`: the fighters' names only (no beasts), in
/// the executable's list order, by file code YUG GAD GRE MIT EVE FOX RON VAK URI (not the HUD's order).
/// `stages`: the stages' names by the VS select's index k, which is G.94 k for k < 8, G.94 9 for 8 and G.94 10 for 9
/// (G.94 8, Uriko's stage, is `final_stage` in `TEXT`).
pub const TEXT_LISTS: &[(&str, u32, u32)] = &[("names", 0x801A_59C4, 9), ("stages", 0x801A_5A54, 10)];

/// The other text the game draws, by key: the C string at the address (`text_table`).
pub const TEXT: &[(&str, u32)] = &[("final_stage", 0x8012_2180)];

/// The C string at `addr` of `exe` (at most 256 bytes), its trailing spaces and line ends cut; Err when it is not
/// printable ASCII or not inside the executable.
fn c_string(exe: &[u8], addr: u32) -> Result<String> {
    let bad = |what: &str| Error::Format(format!("{BOOT_EXE}: the text at {addr:08X}: {what}"));
    let o = addr.checked_sub(EXE_BASE).map(|o| o as usize).filter(|&o| o < exe.len()).ok_or_else(|| bad("outside"))?;
    let b = &exe[o..exe.len().min(o + 256)];
    let n = b.iter().position(|&c| c == 0).ok_or_else(|| bad("no end"))?;
    let s = std::str::from_utf8(&b[..n]).ok().filter(|s| s.bytes().all(|c| (0x20..0x7F).contains(&c) || c == b'\n'));
    s.map(|s| s.trim_end_matches([' ', '\n']).to_string()).ok_or_else(|| bad("not text"))
}

/// The table `text.strings` (`TEXT_LISTS`, then `TEXT`): each key, NUL, its text, NUL.
pub fn text_table(exe: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut put = |key: &str, text: String| {
        out.extend(key.as_bytes());
        out.push(0);
        out.extend(text.as_bytes());
        out.push(0);
    };
    for &(prefix, table, n) in TEXT_LISTS {
        for k in 0..n {
            let o = (table + 4 * k - EXE_BASE) as usize;
            let p = exe
                .get(o..o + 4)
                .map(|w| u32::from_le_bytes(w.try_into().unwrap()))
                .ok_or_else(|| Error::Format(format!("{BOOT_EXE} is too short for the text list {prefix}")))?;
            put(&format!("{prefix}.{k}"), c_string(exe, p)?);
        }
    }
    for &(key, addr) in TEXT {
        put(key, c_string(exe, addr)?);
    }
    Ok(out)
}

/// `engine/tables.okt` from the executable `exe` (`TABLES`; then `anim.common_clips` when there are such clips:
/// from the first of the file offset ranges `clips` (in order, apart) to the last one's end, the executable's bytes in
/// them and zeros between them, bytes no clip reads; then `anim.common_clips.ranges`, those ranges as
/// (start, end) u32 pairs; then `image_len`; then the text, `text.strings`).
pub fn tables_okt(exe: &[u8], clips: &[(usize, usize)]) -> Result<Vec<u8>> {
    let mut tables = Vec::new();
    for &(name, elem, count, origin) in TABLES {
        let o = origin.wrapping_sub(EXE_BASE) as usize;
        let n = count as usize * elem.size();
        let bytes = exe
            .get(o..o + n)
            .ok_or_else(|| Error::Format(format!("{BOOT_EXE} is too short for the table {name}")))?
            .to_vec();
        tables.push(okt::Table { name: name.into(), elem, count, origin, bytes });
    }
    if let (Some(&(s, _)), Some(&(_, e))) = (clips.first(), clips.last()) {
        let mut bytes = vec![0; e.saturating_sub(s)];
        for &(a, b) in clips {
            let src = exe
                .get(a..b)
                .filter(|_| a >= s && b <= e)
                .ok_or_else(|| Error::Format(format!("{BOOT_EXE} is too short for its clips")))?;
            bytes[a - s..b - s].copy_from_slice(src);
        }
        tables.push(okt::Table {
            name: "anim.common_clips".into(),
            elem: okt::Elem::Blob,
            count: bytes.len() as u32,
            origin: s as u32,
            bytes,
        });
        // the ranges themselves (start, end) as file offsets: a reader can check its clip reads against them
        let ranges: Vec<u8> =
            clips.iter().flat_map(|&(a, b)| [(a as u32).to_le_bytes(), (b as u32).to_le_bytes()]).flatten().collect();
        tables.push(okt::Table {
            name: "anim.common_clips.ranges".into(),
            elem: okt::Elem::U32,
            count: 2 * clips.len() as u32,
            origin: 0,
            bytes: ranges,
        });
    }
    tables.push(okt::Table {
        name: "image_len".into(),
        elem: okt::Elem::U32,
        count: 1,
        origin: 0,
        bytes: (exe.len() as u32).to_le_bytes().to_vec(),
    });
    let text = text_table(exe)?;
    tables.push(okt::Table {
        name: "text.strings".into(),
        elem: okt::Elem::Blob,
        count: text.len() as u32,
        origin: 0,
        bytes: text,
    });
    okt::write(&tables).map_err(Error::Format)
}

/// The ranges of `anim.common_clips` (`clip_bank::extents`) from the executable `exe`, CMN.ACT `cmn` and the nine
/// fighters' `XX31.DAT` `anms`.
pub fn common_clips(exe: &[u8], cmn: &[u8], anms: &[&[u8]]) -> Result<Vec<(usize, usize)>> {
    let scripts = anms
        .iter()
        .map(|a| crate::clip_bank::Scripts::of(a, cmn, exe.len()))
        .collect::<std::result::Result<Vec<_>, _>>();
    let scripts = scripts.map_err(|e| Error::Format(format!("the fighters' scripts: {e}")))?;
    crate::clip_bank::extents(exe, &scripts).map_err(Error::Format)
}

/// The files the content id covers (the ones a game's simulation reads): the EXE,
/// CMN.ACT and every character's `XX31.DAT`, sorted by path.
pub fn content_files() -> Vec<String> {
    let mut v: Vec<String> = [format!("engine/{BOOT_EXE}"), "engine/CMN/CMN.ACT".to_string()].into();
    v.extend(chars::FIGHTERS.iter().map(|c| format!("engine/{c}/{c}31.DAT")));
    v.sort();
    v
}

/// The install's content id (`manifest.json` `content_id`): CRC-32 of the "path:crc32\n" lines of `content_files`
/// (CRC in 8 lower-case hex digits), at least 1, from `sources` (each one's size and CRC as the disc has it: the
/// executable is read but not written); None when one of them is not on the disc. A replay or a netplay peer
/// carries it, so data of another disc is refused.
pub fn content_id(sources: &BTreeMap<String, (u64, u32)>) -> Option<u32> {
    let mut text = String::new();
    for p in content_files() {
        let (_, crc) = sources.get(&p)?;
        text.push_str(&format!("{p}:{crc:08x}\n"));
    }
    Some(crc32(text.as_bytes()).max(1))
}

#[derive(Clone, Debug)]
enum Task {
    /// the Form 1 files copied verbatim
    Raw,
    /// fighter, form (0 human, 1 beast)
    Fighter(&'static str, usize),
    /// output name, disc file
    Stage(String, String),
    Music(DiscFile),
    Cdda(AudioTrack),
    Movie(DiscFile),
    Finish,
}

fn stem(path: &str) -> &str {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.split('.').next().unwrap_or(name)
}

impl Task {
    fn name(&self) -> String {
        match self {
            Task::Raw => "game files".into(),
            Task::Fighter(id, f) => format!("fighter {} {}", chars::fighter_name(id), ["human", "beast"][*f]),
            Task::Stage(n, _) => format!("stage {n}"),
            Task::Music(f) => format!("music {}", stem(&f.path)),
            Task::Cdda(t) => format!("CD audio track {}", t.number),
            Task::Movie(f) => format!("movie {}", stem(&f.path)),
            Task::Finish => "manifest".into(),
        }
    }
}

/// Where a Form 1 file goes (None: not copied).
fn raw_dest(path: &str) -> Option<String> {
    let up = path.to_ascii_uppercase();
    if up.ends_with(".STR") || up.ends_with(".XA") || up.ends_with(".WAV") {
        return None; // streams; NEG_M01.WAV aliases the CD-DA track
    }
    Some(if up.ends_with(".TIM") {
        format!("pictures/{path}")
    } else if up.ends_with(".SND") {
        format!("audio/sfx/{path}")
    } else {
        format!("engine/{path}")
    })
}

/// The conversion of one Bloody Roar disc.
pub struct Converter {
    disc: Disc,
    tasks: Vec<Task>,
    next: usize,
    written: BTreeMap<String, (u64, u32)>,
    /// the size and CRC of each file of `content_files` read from the disc (written or not)
    sources: BTreeMap<String, (u64, u32)>,
    /// manifest pieces: fighters, stages, music, cd audio, movies
    fighters: Vec<Value>,
    stages: Vec<Value>,
    music: Vec<Value>,
    cd_audio: Vec<Value>,
    movies: Vec<Value>,
    /// hashes of the music channels written (some files repeat a song)
    seen: HashSet<u64>,
}

impl Converter {
    /// The steps for `disc` (opened with `Disc::open_checked`).
    pub fn new(disc: Disc) -> Converter {
        let mut tasks = vec![Task::Raw];
        for id in FIGHTERS {
            tasks.push(Task::Fighter(id, 0));
            tasks.push(Task::Fighter(id, 1));
        }
        for id in chars::FIGHTERS {
            tasks.push(Task::Stage(format!("{id}00"), format!("{id}/{id}00.DAT")));
        }
        for n in ["BIG00", "SMA00"] {
            tasks.push(Task::Stage(n.into(), format!("CMN/{n}.DAT")));
        }
        let streams = |ext: &str| -> Vec<DiscFile> {
            disc.files
                .iter()
                .filter(|f| f.path.to_ascii_uppercase().ends_with(ext) && disc.in_data_track(f))
                .cloned()
                .collect()
        };
        if disc.raw() {
            tasks.extend(streams(".XA").into_iter().map(Task::Music));
        }
        tasks.extend(disc.audio_tracks().into_iter().filter(|t| t.sectors > 0).map(Task::Cdda));
        // the movies' frames are Form 1 sectors: an ISO has them too (without the sound)
        tasks.extend(streams(".STR").into_iter().map(Task::Movie));
        tasks.push(Task::Finish);
        Converter {
            disc,
            tasks,
            next: 0,
            written: BTreeMap::new(),
            sources: BTreeMap::new(),
            fighters: Vec::new(),
            stages: Vec::new(),
            music: Vec::new(),
            cd_audio: Vec::new(),
            movies: Vec::new(),
            seen: HashSet::new(),
        }
    }

    pub fn total(&self) -> usize {
        self.tasks.len()
    }

    pub fn progress(&self) -> Progress {
        Progress {
            step: self.tasks.get(self.next).map(Task::name).unwrap_or_else(|| "done".into()),
            done: self.next,
            total: self.tasks.len(),
        }
    }

    fn put(&mut self, sink: &mut dyn Sink, path: &str, data: &[u8]) -> Result<()> {
        sink.put(path, data).map_err(Error::Sink)?;
        if path != "manifest.json" {
            self.written.insert(path.to_string(), (data.len() as u64, crc32(data)));
        }
        Ok(())
    }

    /// Runs the next step; true when the conversion is complete.
    pub fn step(&mut self, sink: &mut dyn Sink) -> Result<bool> {
        let Some(task) = self.tasks.get(self.next).cloned() else { return Ok(true) };
        self.run_task(&task, sink).map_err(|e| match e {
            Error::Format(m) => Error::Format(format!("{}: {m}", task.name())),
            e => e,
        })?;
        self.next += 1;
        Ok(self.next == self.tasks.len())
    }

    /// Runs every step; `progress` returns false to cancel (`Error::Cancelled`).
    pub fn run(mut self, sink: &mut dyn Sink, progress: &mut dyn FnMut(&Progress) -> bool) -> Result<Summary> {
        loop {
            if !progress(&self.progress()) {
                return Err(Error::Cancelled);
            }
            if self.step(sink)? {
                return Ok(self.summary());
            }
        }
    }

    pub fn summary(&self) -> Summary {
        Summary {
            files: self.written.clone(),
            xa: self.disc.raw(),
            cdda: !self.cd_audio.is_empty(),
            source: self.disc.source(),
        }
    }

    fn run_task(&mut self, task: &Task, sink: &mut dyn Sink) -> Result<()> {
        match task {
            Task::Raw => {
                // the executable, CMN.ACT and the fighters' XX31.DAT (by their written paths) for its tables
                let mut keep: BTreeMap<String, Vec<u8>> = BTreeMap::new();
                let wanted = content_files();
                let exe_path = format!("engine/{BOOT_EXE}");
                for f in self.disc.files.clone() {
                    let Some(dst) = raw_dest(&f.path) else { continue };
                    if !self.disc.in_data_track(&f) {
                        continue;
                    }
                    let data = self.disc.read_file(&f)?;
                    // the executable only feeds the tables and the content id (format 3: not in `engine/`)
                    if dst != exe_path {
                        self.put(sink, &dst, &data)?;
                    }
                    if wanted.contains(&dst) {
                        self.sources.insert(dst.clone(), (data.len() as u64, crc32(&data)));
                        keep.insert(dst, data);
                    }
                }
                // the executable's tables by name; the clips of the executable with CMN.ACT and every
                // fighter's scripts
                if let Some(exe) = keep.get(&exe_path) {
                    let file = |p: String| {
                        keep.get(&p).map(Vec::as_slice).ok_or_else(|| Error::Format(format!("no {p} on the disc")))
                    };
                    let anms = chars::FIGHTERS
                        .iter()
                        .map(|c| file(format!("engine/{c}/{c}31.DAT")))
                        .collect::<Result<Vec<_>>>()?;
                    let clips = common_clips(exe, file("engine/CMN/CMN.ACT".into())?, &anms)?;
                    let okt = tables_okt(exe, &clips)?;
                    self.put(sink, "engine/tables.okt", &okt)?;
                }
            }
            Task::Fighter(id, form) => self.fighter(sink, id, *form)?,
            Task::Stage(name, file) => {
                let scene = chars::scene_gltf(&mut self.disc, name, file, &[])?;
                let path = format!("stages/{name}.glb");
                self.put(sink, &path, &scene.gl.to_glb())?;
                self.stages.push(json!({"code": name, "file": path, "source": file, "tris": scene.tris,
                    "background": scene.panorama.is_some()}));
            }
            Task::Music(f) => self.music_file(sink, f)?,
            Task::Cdda(t) => self.cdda(sink, *t)?,
            Task::Movie(f) => self.movie(sink, f)?,
            Task::Finish => self.finish(sink)?,
        }
        Ok(())
    }

    /// One fighter form: every costume of its mesh as a material variant (`chars::fighter_gltf`, no animations).
    fn fighter(&mut self, sink: &mut dyn Sink, id: &'static str, form: usize) -> Result<()> {
        let costumes: Vec<String> = (0..10)
            .map(|c| format!("{id}/{id}1{c}.DAT"))
            .filter(|p| self.disc.find(p).is_some() && !chars::UNUSED_COSTUMES.contains(&p.as_str()))
            .collect();
        let mesh = format!("{id}/{id}20.DAT");
        if costumes.iter().any(|c| chars::model_for_costume(&self.disc, id, c) != mesh) {
            return Err(Error::Format(format!("{id}: a costume of another mesh than {mesh}")));
        }
        let (first, rest) = costumes.split_first().ok_or_else(|| Error::Format(format!("{id}: no costume")))?;
        let rest: Vec<&str> = rest.iter().map(String::as_str).collect();
        let f = chars::fighter_gltf(&mut self.disc, id, form, form, &[first], &rest, false)?;
        let form_name = ["human", "beast"][form];
        let path = format!("models/{id}_{form_name}.glb");
        self.put(sink, &path, &f.gl.to_glb())?;
        let names: Vec<String> = costumes.iter().map(|c| stem(c)[3..].to_string()).collect();
        if form == 0 {
            self.fighters.push(json!({"code": id, "name": chars::fighter_name(id), "costumes": names,
                "models": {"human": path, "beast": format!("models/{id}_beast.glb")}}));
        }
        Ok(())
    }

    /// One XA file: every channel with sound and not a repeat of an earlier one (`media::xa_file`'s choice) as
    /// `audio/music/<FILE>_ch<n>.xa`.
    fn music_file(&mut self, sink: &mut dyn Sink, f: &DiscFile) -> Result<()> {
        struct Chan {
            dec: XaDecoder,
            pcm: Vec<i16>,
            xa: XaWriter,
            coding: u8,
        }
        let mut chans: BTreeMap<u8, Chan> = BTreeMap::new();
        let n = f.size.div_ceil(2048);
        for i in 0..n {
            let s = self.disc.raw_sector(f.lba + i)?;
            let (submode, coding) = (s[18], s[19]);
            if submode & 0x04 == 0 {
                continue; // not an audio sector
            }
            let e = chans.entry(s[17]).or_insert_with(|| Chan {
                dec: XaDecoder::new(),
                pcm: Vec::new(),
                xa: XaWriter::new(coding),
                coding,
            });
            if e.xa.done {
                continue; // past this channel's end-of-file marker
            }
            e.dec.sector(&s, coding & 1 != 0, &mut e.pcm);
            e.xa.push(&s);
        }
        for (ch, c) in chans {
            if c.pcm.iter().all(|&v| v.unsigned_abs() < 8) {
                continue;
            }
            let mut h = std::collections::hash_map::DefaultHasher::new();
            std::hash::Hash::hash(&c.pcm, &mut h);
            if !self.seen.insert(std::hash::Hasher::finish(&h)) {
                continue;
            }
            let (stereo, rate) = (c.coding & 1 != 0, if c.coding & 4 != 0 { 18900u64 } else { 37800 });
            let ms = c.pcm.len() as u64 / if stereo { 2 } else { 1 } * 1000 / rate;
            let path = format!("audio/music/{}_ch{ch}.xa", stem(&f.path));
            let sectors = c.xa.sectors;
            self.put(sink, &path, &c.xa.finish())?;
            self.music.push(json!({"file": path, "source": f.path, "channel": ch, "coding": c.coding,
                "sectors": sectors, "ms": ms}));
        }
        Ok(())
    }

    /// A CD-DA track as `audio/cdda/trackNN.pcm` (the sectors' bytes as they are: 16-bit LE stereo).
    fn cdda(&mut self, sink: &mut dyn Sink, t: AudioTrack) -> Result<()> {
        if t.sectors > MAX_CDDA_SECTORS {
            return Err(Error::Format(format!("CD-DA track {}: {} sectors, longer than a CD", t.number, t.sectors)));
        }
        let mut data = Vec::with_capacity(PCM_HEADER + t.sectors as usize * RAW_SECTOR);
        data.extend_from_slice(&pcm_header(t.sectors * (RAW_SECTOR as u32 / 4)));
        for s in 0..t.sectors {
            data.extend_from_slice(self.disc.read_audio(t.number, s)?);
        }
        let path = format!("audio/cdda/track{:02}.pcm", t.number);
        self.put(sink, &path, &data)?;
        let frames = t.sectors as u64 * (RAW_SECTOR as u64 / 4);
        self.cd_audio.push(json!({"file": path, "track": t.number, "frames": frames, "ms": frames * 1000 / 44100}));
        Ok(())
    }

    /// One STR movie: `movies/<NAME>.bsf` and (raw images) `movies/<NAME>.xa`.
    fn movie(&mut self, sink: &mut dyn Sink, f: &DiscFile) -> Result<()> {
        let name = stem(&f.path).to_string();
        let n = f.size.div_ceil(2048);
        let mut bsf = Bsf::default();
        let mut xa: Option<XaWriter> = None;
        for i in 0..n {
            if !self.disc.raw() {
                bsf.push_user(i as usize, &self.disc.read_form1(f.lba + i, 1)?);
                continue;
            }
            let s = self.disc.raw_sector(f.lba + i)?;
            if s[18] & 0x04 == 0 {
                bsf.push(i as usize, &s);
            } else {
                xa.get_or_insert_with(|| XaWriter::new(s[19])).push(&s);
            }
        }
        let (frames, version) = (bsf.frames(), bsf.bs_version());
        let path = format!("movies/{name}.bsf");
        self.put(sink, &path, &bsf.finish())?;
        let mut e = json!({"name": name, "bsf": path, "frames": frames, "bs_version": version, "source": f.path});
        if let Some(w) = xa {
            let p = format!("movies/{name}.xa");
            self.put(sink, &p, &w.finish())?;
            e["xa"] = json!(p);
        }
        self.movies.push(e);
        Ok(())
    }

    /// manifest.json: the index and every file written with its size and CRC-32.
    fn finish(&mut self, sink: &mut dyn Sink) -> Result<()> {
        let files: Vec<Value> =
            self.written.iter().map(|(p, (s, c))| json!({"path": p, "size": s, "crc32": c})).collect();
        let m = json!({
            "game": GAME,
            "format": FORMAT_VERSION,
            "source": self.disc.source(),
            "xa": self.disc.raw(),
            "cdda": !self.cd_audio.is_empty(),
            "content_id": content_id(&self.sources),
            "converter": format!("ok_br1_converter {}", env!("CARGO_PKG_VERSION")),
            "fighters": self.fighters,
            "stages": self.stages,
            "music": self.music,
            "cd_audio": self.cd_audio,
            "movies": self.movies,
            "files": files,
        });
        let text = serde_json::to_string_pretty(&m)?;
        self.put(sink, "manifest.json", text.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::disc::tests::{MemSectors, tiny_iso};
    use crate::disc::{FORM1_DATA, VOLUME_ID};

    #[test]
    fn crc32_check_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32_update(crc32(b"1234"), b"56789"), 0xCBF4_3926);
    }

    /// `exe` with text where `text_table` reads it: each list's entry k pointing at 15 letters and a NUL at file offset
    /// 0x800 + 16k, each single text "TEXT  " (to be trimmed).
    fn with_text(mut exe: Vec<u8>) -> Vec<u8> {
        for &(_, table, n) in TEXT_LISTS {
            for k in 0..n {
                let o = (table - EXE_BASE + 4 * k) as usize;
                exe[o..o + 4].copy_from_slice(&(EXE_BASE + 0x800 + 16 * k).to_le_bytes());
                let s = (0x800 + 16 * k) as usize;
                exe[s..s + 16].copy_from_slice(b"ABCDEFGHIJKLMNO\0");
            }
        }
        for &(_, addr) in TEXT {
            let o = (addr - EXE_BASE) as usize;
            exe[o..o + 7].copy_from_slice(b"TEXT  \0");
        }
        exe
    }

    /// `text.strings`: every list entry and single text by key, in order, trimmed; a text that is not one refused.
    #[test]
    fn text_table_entries() {
        let exe = with_text((0..595_968u32).map(|i| (i % 251) as u8).collect());
        let mut want = Vec::new();
        for &(prefix, _, n) in TEXT_LISTS {
            for k in 0..n {
                want.extend(format!("{prefix}.{k}\0ABCDEFGHIJKLMNO\0").bytes());
            }
        }
        for &(key, _) in TEXT {
            want.extend(format!("{key}\0TEXT\0").bytes());
        }
        assert_eq!(text_table(&exe).unwrap(), want);
        let mut bad = exe.clone();
        bad[(TEXT[0].1 - EXE_BASE) as usize] = 1;
        assert!(matches!(text_table(&bad), Err(Error::Format(m)) if m.ends_with("not text")));
        let mut far = exe;
        let o = (TEXT_LISTS[0].1 - EXE_BASE) as usize;
        far[o..o + 4].copy_from_slice(&0x1000u32.to_le_bytes());
        assert!(matches!(text_table(&far), Err(Error::Format(m)) if m.ends_with("outside")));
    }

    /// A raw Mode 2 Form 2 audio sector of channel `ch` (submode `sub`) whose sound groups hold `fill`.
    fn audio_sector(ch: u8, sub: u8, fill: u8) -> Vec<u8> {
        let mut s = vec![0u8; RAW_SECTOR];
        s[1..11].fill(0xFF);
        s[16..20].copy_from_slice(&[1, ch, sub, 0x01]);
        s[20..24].copy_from_slice(&[1, ch, sub, 0x01]);
        s[24..24 + XA_PAYLOAD].fill(fill);
        s
    }

    #[test]
    fn xa_pcm_and_bsf_files() {
        let mut w = XaWriter::new(1);
        w.push(&audio_sector(0, 0x64, 7));
        w.push(&audio_sector(0, 0xE4, 9));
        w.push(&audio_sector(0, 0x64, 5)); // after the end of file: ignored
        let f = w.finish();
        assert_eq!((&f[..6], f[6], f[7]), (&XA_MAGIC[..], 1, 1));
        assert_eq!(u32::from_le_bytes(f[8..12].try_into().unwrap()), 2);
        assert_eq!(f.len(), XA_HEADER + 2 * XA_PAYLOAD);
        assert_eq!((f[XA_HEADER], f[XA_HEADER + XA_PAYLOAD]), (7, 9));
        let h = pcm_header(588);
        assert_eq!((&h[..7], h[7]), (&PCM_MAGIC[..], 1));
        assert_eq!(u32::from_le_bytes(h[8..12].try_into().unwrap()), 44100);
        assert_eq!((u16::from_le_bytes([h[12], h[13]]), u32::from_le_bytes(h[16..20].try_into().unwrap())), (2, 588));
        let mut b = Bsf::default();
        for (chunk, fill) in [(1u16, 2u8), (0, 1)] {
            let mut d = vec![fill; 2048];
            d[..20].fill(0);
            d[0..2].copy_from_slice(&0x0160u16.to_le_bytes());
            d[2..4].copy_from_slice(&0x8001u16.to_le_bytes());
            d[4..6].copy_from_slice(&chunk.to_le_bytes());
            d[8..12].copy_from_slice(&1u32.to_le_bytes());
            d[12..16].copy_from_slice(&3000u32.to_le_bytes());
            d[16..18].copy_from_slice(&320u16.to_le_bytes());
            d[18..20].copy_from_slice(&240u16.to_le_bytes());
            b.push_user(chunk as usize + 4, &d);
        }
        let out = b.finish();
        assert_eq!(&out[..4], b"BRBS");
        assert_eq!(u32::from_le_bytes(out[8..12].try_into().unwrap()), 1);
        assert_eq!(u32::from_le_bytes(out[12..16].try_into().unwrap()), 5, "the sector its first chunk came in");
        assert_eq!(u32::from_le_bytes(out[16..20].try_into().unwrap()), 3000);
        assert_eq!((out[20 + 12], out[20 + 2016 + 12]), (1, 2), "chunks in order");
    }

    /// `anim.common_clips`: from the first range to the last one's end, the executable's bytes in the ranges and zeros
    /// between them, its origin the first range's file offset, and the ranges (`anim.common_clips.ranges`); none without
    /// ranges; a range past the image refused.
    #[test]
    fn common_clips_table() {
        let exe = with_text((0..595_968u32).map(|i| (i % 251) as u8 + 1).collect());
        let t = okt::read(&tables_okt(&exe, &[(0x100, 0x110), (0x200, 0x204)]).unwrap()).unwrap();
        assert_eq!(t.len(), TABLES.len() + 4);
        let c = &t[TABLES.len()];
        assert_eq!((c.name.as_str(), c.origin, c.count, c.bytes.len()), ("anim.common_clips", 0x100, 0x104, 0x104));
        assert_eq!((&c.bytes[..0x10], &c.bytes[0x100..]), (&exe[0x100..0x110], &exe[0x200..0x204]));
        assert!(c.bytes[0x10..0x100].iter().all(|&b| b == 0));
        // the ranges as (start, end) pairs
        let r = &t[TABLES.len() + 1];
        let pairs: Vec<u32> = r.bytes.chunks(4).map(|b| u32::from_le_bytes(b.try_into().unwrap())).collect();
        assert_eq!(
            (r.name.as_str(), r.elem, r.count, &pairs[..]),
            ("anim.common_clips.ranges", okt::Elem::U32, 4, &[0x100, 0x110, 0x200, 0x204][..])
        );
        assert_eq!(okt::read(&tables_okt(&exe, &[]).unwrap()).unwrap().len(), TABLES.len() + 2);
        assert!(tables_okt(&exe, &[(595_960, 595_970)]).is_err());
    }

    /// The tables are in address order, inside the EXE and never overlap; the content id follows its formula.
    #[test]
    fn tables_are_ordered() {
        // RAM origins, apart from the file offsets of `anim.common_clips` and `image_len` (below an image's length)
        assert!(TABLES.iter().all(|t| t.3 >= 0x8000_0000));
        for w in TABLES.windows(2) {
            let end = w[0].3 + w[0].2 * w[0].1.size() as u32;
            assert!(end <= w[1].3, "{} overlaps {}", w[0].0, w[1].0);
        }
        // every byte of the EXE a runtime was recorded reading is in a table
        let span = |&(_, e, n, o): &(&str, okt::Elem, u32, u32)| o..o + n * e.size() as u32;
        let reads = include_str!("../tests/fixtures/runtime_reads.txt");
        for l in reads.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
            let (a, b) = l.split_once(' ').unwrap();
            for x in u32::from_str_radix(a, 16).unwrap()..u32::from_str_radix(b, 16).unwrap() {
                assert!(TABLES.iter().any(|t| span(t).contains(&x)), "{x:08X} is in no table");
            }
        }
        let last = TABLES[TABLES.len() - 1];
        assert!(last.3 + last.2 * last.1.size() as u32 <= EXE_BASE + 595_968);
        let mut w: BTreeMap<String, (u64, u32)> =
            content_files().into_iter().enumerate().map(|(k, p)| (p, (1, k as u32 + 1))).collect();
        let text: String = content_files().iter().enumerate().map(|(k, p)| format!("{p}:{:08x}\n", k + 1)).collect();
        assert_eq!(content_id(&w), Some(crc32(text.as_bytes()).max(1)));
        assert_eq!(content_files().len(), 11);
        w.remove("engine/CMN/CMN.ACT");
        assert_eq!(content_id(&w), None);
    }

    #[test]
    fn raw_files_streams_and_cd_audio_of_a_tiny_disc() {
        // two XA channels (one silent), an EXE, a picture and a CD-DA track of 2 sectors; no fighters or stages on
        // it, so only the steps that need the disc's streams are run
        let mut xa = Vec::new();
        for (ch, sub, fill) in [(0u8, 0x64u8, 0x11u8), (1, 0x64, 0), (0, 0xE4, 0x22), (1, 0xE4, 0)] {
            xa.extend(audio_sector(ch, sub, fill));
        }
        // the EXE as long as the real one (its tables are cut out of it), a byte pattern
        let exe = with_text((0..595_968u32).map(|i| (i % 251) as u8).collect());
        let files = [("SCUS_941.99", exe.clone(), false), ("ART000.TIM", vec![2u8; 10], false), ("BGM00.XA", xa, true)];
        let data = tiny_iso(true, VOLUME_ID, &files);
        let cd: Vec<u8> = (0..2 * RAW_SECTOR).map(|i| i as u8).collect();
        let disc = Disc::open_checked(Box::new(MemSectors { data, raw: true, audio: vec![(2, cd.clone())] })).unwrap();
        let mut c = Converter::new(disc);
        let mut sink = MemorySink::default();
        // no CMN.ACT, no fighters: the raw files but the executable are written, then the tables refused (their clips
        // need the scripts)
        let e = c.run_task(&Task::Raw, &mut sink).unwrap_err();
        assert!(matches!(&e, Error::Format(m) if m == "no engine/EVE/EVE31.DAT on the disc"), "{e:?}");
        let music = c.tasks.iter().find(|t| matches!(t, Task::Music(_))).cloned().unwrap();
        c.run_task(&music, &mut sink).unwrap();
        let cdda = c.tasks.iter().find(|t| matches!(t, Task::Cdda(_))).cloned().unwrap();
        c.run_task(&cdda, &mut sink).unwrap();
        c.run_task(&Task::Finish, &mut sink).unwrap();
        let names: Vec<&str> = sink.files.keys().map(String::as_str).collect();
        assert_eq!(
            names,
            [
                "audio/cdda/track02.pcm",
                "audio/music/BGM00_ch0.xa",
                "engine/SYSTEM.CNF",
                "manifest.json",
                "pictures/ART000.TIM"
            ]
        );
        // its tables (as a disc with no clips of the executable gets them): the EXE's bytes at each origin, then its
        // length, then the text
        let t = okt::read(&tables_okt(&exe, &[]).unwrap()).unwrap();
        assert_eq!(t.len(), TABLES.len() + 2);
        for (tab, &(name, _, _, origin)) in t.iter().zip(TABLES) {
            let o = (origin - EXE_BASE) as usize;
            assert_eq!((tab.name.as_str(), tab.origin, &tab.bytes[..]), (name, origin, &exe[o..o + tab.bytes.len()]));
        }
        let (len, text) = (&t[t.len() - 2], &t[t.len() - 1]);
        assert_eq!((len.name.as_str(), &len.bytes[..]), ("image_len", &595_968u32.to_le_bytes()[..]));
        assert_eq!(
            (text.name.as_str(), text.origin, text.bytes.clone()),
            ("text.strings", 0, text_table(&exe).unwrap())
        );
        let x = &sink.files["audio/music/BGM00_ch0.xa"];
        assert_eq!((x.len(), x[XA_HEADER], x[XA_HEADER + XA_PAYLOAD]), (XA_HEADER + 2 * XA_PAYLOAD, 0x11, 0x22));
        let p = &sink.files["audio/cdda/track02.pcm"];
        assert_eq!((p.len(), &p[PCM_HEADER..]), (PCM_HEADER + cd.len(), &cd[..]));
        let m: Value = serde_json::from_slice(&sink.files["manifest.json"]).unwrap();
        assert_eq!((m["game"].as_str(), m["format"].as_u64(), m["cdda"].as_bool()), (Some(GAME), Some(4), Some(true)));
        // no CMN.ACT, no fighters: no content id
        assert!(m["content_id"].is_null());
        let listed: Vec<&str> = m["files"].as_array().unwrap().iter().map(|f| f["path"].as_str().unwrap()).collect();
        assert_eq!(listed.len(), 4);
        assert!(!listed.contains(&"manifest.json") && !listed.contains(&"engine/SCUS_941.99"));
        // the executable read for the content id all the same (the id itself needs CMN.ACT and the fighters too)
        assert_eq!(c.sources.get("engine/SCUS_941.99"), Some(&(595_968, crc32(&exe))));
        // a track longer than a CD (a corrupt image): an error before anything is read or allocated
        let long = AudioTrack { number: 3, sectors: 4_000_000_000 };
        assert!(matches!(c.cdda(&mut sink, long), Err(Error::Format(_))));
        // an ISO: no music step, no raw sectors
        let iso = tiny_iso(false, VOLUME_ID, &files[..2]);
        let disc = Disc::open_checked(Box::new(MemSectors { data: iso, raw: false, audio: Vec::new() })).unwrap();
        let c = Converter::new(disc);
        assert!(!c.tasks.iter().any(|t| matches!(t, Task::Music(_) | Task::Cdda(_))));
        assert_eq!(FORM1_DATA, 2048);
    }
}
