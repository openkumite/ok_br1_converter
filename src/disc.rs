//! Disc access: the sectors of the data track (`Sectors`: a raw MODE2/2352 BIN through a CUE sheet in the CLI, any
//! reader the caller brings in the library: CHD, ISO, a browser Blob) and the CD-DA tracks beside it, with a minimal
//! ISO9660 directory walker.

use crate::bail;
#[cfg(feature = "cli")]
use crate::error::Context;
use crate::error::{Error, Result};
#[cfg(feature = "cli")]
use std::path::{Path, PathBuf};

pub const RAW_SECTOR: usize = 2352;
pub const FORM1_DATA: usize = 2048;

/// The game this converter is for: the ISO 9660 volume id and the boot file of SYSTEM.CNF.
pub const VOLUME_ID: &str = "SCUS_94199";
pub const BOOT_EXE: &str = "SCUS_941.99";

/// `Send` where threads exist (a conversion can run on a background thread), nothing on wasm32 (a reader there may
/// hold JavaScript objects).
#[cfg(not(target_arch = "wasm32"))]
pub trait MaybeSend: Send {}
#[cfg(not(target_arch = "wasm32"))]
impl<T: Send> MaybeSend for T {}
#[cfg(target_arch = "wasm32")]
pub trait MaybeSend {}
#[cfg(target_arch = "wasm32")]
impl<T> MaybeSend for T {}

/// A CD-DA track of the disc: its number and length in 2352-byte sectors (from its INDEX 01).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioTrack {
    pub number: u32,
    pub sectors: u32,
}

/// Where the sectors come from.
pub trait Sectors: MaybeSend {
    /// True when `read_sector` gives whole 2352-byte sectors (BIN, CHD); false for an ISO (2048 user bytes).
    fn raw(&self) -> bool;
    /// Number of sectors of the data track.
    fn sectors(&self) -> u32;
    /// Raw images: the whole 2352-byte sector `lba` into `out[..2352]`. ISO: the 2048 user bytes into `out[..2048]`.
    fn read_sector(&mut self, lba: u32, out: &mut [u8]) -> Result<()>;
    /// What the image is: "bin" (raw sectors), "iso" or "chd".
    fn source(&self) -> &'static str {
        if self.raw() { "bin" } else { "iso" }
    }
    /// The disc's CD-DA tracks (none: an ISO, or a CUE whose audio BIN is not there).
    fn audio_tracks(&self) -> Vec<AudioTrack> {
        Vec::new()
    }
    /// Sector `sector` (from INDEX 01) of CD-DA track `track`: 588 stereo frames of 16-bit little-endian PCM into
    /// `out[..2352]`.
    fn read_audio(&mut self, track: u32, sector: u32, out: &mut [u8]) -> Result<()> {
        let _ = (sector, out);
        Err(Error::Format(format!("no CD-DA track {track}")))
    }
}

#[derive(Debug, Clone)]
pub struct Track {
    pub number: u32,
    pub mode: String,
    #[cfg(feature = "cli")]
    pub path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct DiscFile {
    /// Path inside the disc, e.g. "FOX/FOX00.DAT" (version suffix stripped).
    pub path: String,
    pub lba: u32,
    pub size: u32,
}

pub struct Disc {
    /// The CUE sheet's tracks (CLI); empty for a disc opened from `Sectors`.
    pub tracks: Vec<Track>,
    src: Box<dyn Sectors>,
    data_sectors: u32,
    pub files: Vec<DiscFile>,
    buf: Vec<u8>,
}

/// Parses a .cue file, returning the tracks in order.
#[cfg(feature = "cli")]
fn parse_cue(cue: &Path) -> Result<Vec<Track>> {
    let text = std::fs::read_to_string(cue).with_context(|| format!("reading {}", cue.display()))?;
    let dir = cue.parent().unwrap_or(Path::new("."));
    let mut tracks = Vec::new();
    let mut current_file = None;
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("FILE ") {
            let name = rest.split('"').nth(1).context("FILE without quoted name")?;
            current_file = Some(dir.join(name));
        } else if let Some(rest) = line.strip_prefix("TRACK ") {
            let mut it = rest.split_whitespace();
            let number = it.next().context("TRACK number")?.parse()?;
            let mode = it.next().context("TRACK mode")?.to_string();
            let path = current_file.clone().context("TRACK before FILE")?;
            tracks.push(Track { number, mode, path });
        }
    }
    Ok(tracks)
}

/// Finds the .cue file in a directory (or accepts the .cue path directly).
#[cfg(feature = "cli")]
pub fn find_cue(input: &Path) -> Result<PathBuf> {
    if input.is_file() {
        return Ok(input.to_path_buf());
    }
    for entry in std::fs::read_dir(input)? {
        let p = entry?.path();
        if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("cue")) {
            return Ok(p);
        }
    }
    bail!("no .cue file found in {}", input.display())
}

/// The data track's BIN file (CLI).
#[cfg(feature = "cli")]
struct FileSectors {
    f: std::fs::File,
    count: u32,
}

#[cfg(feature = "cli")]
impl Sectors for FileSectors {
    fn raw(&self) -> bool {
        true
    }
    fn sectors(&self) -> u32 {
        self.count
    }
    fn read_sector(&mut self, lba: u32, out: &mut [u8]) -> Result<()> {
        use std::io::{Read, Seek, SeekFrom};
        self.f.seek(SeekFrom::Start(lba as u64 * RAW_SECTOR as u64))?;
        self.f.read_exact(&mut out[..RAW_SECTOR])?;
        Ok(())
    }
}

impl Disc {
    /// The disc of a CUE sheet (or the folder holding one) on the file system.
    #[cfg(feature = "cli")]
    pub fn open(input: &Path) -> Result<Disc> {
        let tracks = parse_cue(&find_cue(input)?)?;
        let first = tracks.first().context("cue has no tracks")?;
        if first.mode != "MODE2/2352" {
            bail!("unsupported data track mode {}", first.mode);
        }
        let f = std::fs::File::open(&first.path)?;
        let count = (f.metadata()?.len() / RAW_SECTOR as u64) as u32;
        let mut disc = Disc::new(Box::new(FileSectors { f, count }))?;
        disc.tracks = tracks;
        Ok(disc)
    }

    /// The disc on `src`: its ISO9660 file tree (sorted by path).
    pub fn new(src: Box<dyn Sectors>) -> Result<Disc> {
        let data_sectors = src.sectors();
        let mut disc = Disc { tracks: Vec::new(), src, data_sectors, files: Vec::new(), buf: vec![0; RAW_SECTOR] };
        disc.files = disc.walk_iso()?;
        disc.files.sort_by_key(|f| f.path.clone());
        Ok(disc)
    }

    /// `new`, then the check that it is Bloody Roar (USA): a PlayStation volume `SCUS_94199` whose SYSTEM.CNF boots
    /// `SCUS_941.99` (`Error::WrongDisc` naming what it is otherwise).
    pub fn open_checked(src: Box<dyn Sectors>) -> Result<Disc> {
        let mut disc = Disc::new(src)?;
        let pvd = disc.read_form1(16, 1)?;
        let system = String::from_utf8_lossy(&pvd[8..40]).trim().to_string();
        let volume = String::from_utf8_lossy(&pvd[40..72]).trim().to_string();
        if system != "PLAYSTATION" {
            return Err(Error::WrongDisc {
                found: format!("system id \"{system}\" (a PlayStation disc says PLAYSTATION)"),
            });
        }
        if volume != VOLUME_ID {
            return Err(Error::WrongDisc { found: format!("volume id \"{volume}\", expected {VOLUME_ID}") });
        }
        let cnf = disc.find("SYSTEM.CNF").cloned().ok_or_else(|| Error::WrongDisc { found: "no SYSTEM.CNF".into() })?;
        let cnf = String::from_utf8_lossy(&disc.read_file(&cnf)?).to_string();
        if !cnf.contains(BOOT_EXE) || disc.find(BOOT_EXE).is_none() {
            return Err(Error::WrongDisc { found: format!("SYSTEM.CNF does not boot {BOOT_EXE}") });
        }
        Ok(disc)
    }

    /// True for BIN/CUE and CHD (the XA / STR streams can be read), false for an ISO.
    pub fn raw(&self) -> bool {
        self.src.raw()
    }

    /// "bin", "chd" or "iso".
    pub fn source(&self) -> &'static str {
        self.src.source()
    }

    pub fn audio_tracks(&self) -> Vec<AudioTrack> {
        self.src.audio_tracks()
    }

    /// One sector of a CD-DA track (`Sectors::read_audio`).
    pub fn read_audio(&mut self, track: u32, sector: u32) -> Result<&[u8]> {
        self.src.read_audio(track, sector, &mut self.buf)?;
        Ok(&self.buf[..RAW_SECTOR])
    }

    pub fn raw_sector(&mut self, lba: u32) -> Result<[u8; RAW_SECTOR]> {
        if lba >= self.data_sectors {
            bail!("sector {lba} past end of data track");
        }
        if !self.src.raw() {
            return Err(Error::NotRaw2352("an ISO image has no raw sectors".into()));
        }
        let mut buf = [0u8; RAW_SECTOR];
        self.src.read_sector(lba, &mut buf)?;
        Ok(buf)
    }

    /// Reads `count` Form 1 user-data sectors (2048 bytes each).
    pub fn read_form1(&mut self, lba: u32, count: u32) -> Result<Vec<u8>> {
        let mut out = Vec::with_capacity(count as usize * FORM1_DATA);
        for i in 0..count {
            if lba + i >= self.data_sectors {
                bail!("sector {} past end of data track", lba + i);
            }
            self.src.read_sector(lba + i, &mut self.buf)?;
            let at = if self.src.raw() { 24 } else { 0 };
            out.extend_from_slice(&self.buf[at..at + FORM1_DATA]);
        }
        Ok(out)
    }

    /// Reads a regular (Form 1) file's contents, truncated to its size.
    pub fn read_file(&mut self, f: &DiscFile) -> Result<Vec<u8>> {
        let mut data = self.read_form1(f.lba, f.size.div_ceil(FORM1_DATA as u32))?;
        data.truncate(f.size as usize);
        Ok(data)
    }

    /// Reads the raw 2352-byte sectors of an interleaved (STR/XA) file.
    /// ISO sizes of such files count 2048 bytes per sector.
    pub fn read_raw_file(&mut self, f: &DiscFile) -> Result<Vec<u8>> {
        let count = f.size.div_ceil(FORM1_DATA as u32);
        let mut out = Vec::with_capacity(count as usize * RAW_SECTOR);
        for i in 0..count {
            out.extend_from_slice(&self.raw_sector(f.lba + i)?);
        }
        Ok(out)
    }

    pub fn in_data_track(&self, f: &DiscFile) -> bool {
        f.lba + f.size.div_ceil(FORM1_DATA as u32) <= self.data_sectors
    }

    pub fn find(&self, path: &str) -> Option<&DiscFile> {
        self.files.iter().find(|f| f.path.eq_ignore_ascii_case(path))
    }

    fn walk_iso(&mut self) -> Result<Vec<DiscFile>> {
        let pvd = self.read_form1(16, 1)?;
        if &pvd[1..6] != b"CD001" {
            bail!("no ISO9660 primary volume descriptor");
        }
        let root = &pvd[156..156 + 34];
        let lba = u32::from_le_bytes(root[2..6].try_into()?);
        let size = u32::from_le_bytes(root[10..14].try_into()?);
        let mut files = Vec::new();
        self.walk_dir(lba, size, "", &mut files, 0)?;
        Ok(files)
    }

    fn walk_dir(&mut self, lba: u32, size: u32, prefix: &str, out: &mut Vec<DiscFile>, depth: u32) -> Result<()> {
        if depth > 8 {
            bail!("ISO directory nesting too deep");
        }
        let data = self.read_form1(lba, size.div_ceil(FORM1_DATA as u32))?;
        let mut o = 0;
        while o < data.len() {
            let len = data[o] as usize;
            if len == 0 {
                // Records never straddle sectors; skip to the next one.
                o = (o / FORM1_DATA + 1) * FORM1_DATA;
                continue;
            }
            let rec = &data[o..(o + len).min(data.len())];
            o += len;
            if rec.len() < 34 {
                continue;
            }
            let elba = u32::from_le_bytes(rec[2..6].try_into()?);
            let esz = u32::from_le_bytes(rec[10..14].try_into()?);
            let flags = rec[25];
            let nlen = rec[32] as usize;
            let name = &rec[33..(33 + nlen).min(rec.len())];
            if name == [0] || name == [1] {
                continue;
            }
            let name = String::from_utf8_lossy(name);
            let name = name.split(';').next().unwrap_or_default();
            let path = format!("{prefix}{name}");
            if flags & 2 != 0 {
                self.walk_dir(elba, esz, &format!("{path}/"), out, depth + 1)?;
            } else {
                out.push(DiscFile { path, lba: elba, size: esz });
            }
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[cfg(feature = "cli")]
    #[test]
    fn parses_cue_tracks() {
        let dir = std::env::temp_dir().join(format!("okconv-br1-cue-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let cue = dir.join("game.cue");
        std::fs::write(
            &cue,
            "FILE \"a (Track 1).bin\" BINARY\r\n  TRACK 01 MODE2/2352\r\n    INDEX 01 00:00:00\r\n\
             FILE \"a (Track 2).bin\" BINARY\r\n  TRACK 02 AUDIO\r\n",
        )
        .unwrap();
        let t = parse_cue(&cue).unwrap();
        assert_eq!(find_cue(&dir).unwrap(), cue);
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(t.len(), 2);
        assert_eq!((t[0].number, t[0].mode.as_str()), (1, "MODE2/2352"));
        assert_eq!(t[1].mode, "AUDIO");
        assert!(t[1].path.ends_with("a (Track 2).bin"));
    }

    /// Sectors held in memory: raw (2352) or ISO (2048), with optional CD-DA tracks.
    pub struct MemSectors {
        pub data: Vec<u8>,
        pub raw: bool,
        pub audio: Vec<(u32, Vec<u8>)>,
    }

    impl Sectors for MemSectors {
        fn raw(&self) -> bool {
            self.raw
        }
        fn sectors(&self) -> u32 {
            (self.data.len() / if self.raw { RAW_SECTOR } else { FORM1_DATA }) as u32
        }
        fn read_sector(&mut self, lba: u32, out: &mut [u8]) -> Result<()> {
            let n = if self.raw { RAW_SECTOR } else { FORM1_DATA };
            let s = self.data.get(lba as usize * n..(lba as usize + 1) * n).ok_or(Error::Io("past the end".into()))?;
            out[..n].copy_from_slice(s);
            Ok(())
        }
        fn audio_tracks(&self) -> Vec<AudioTrack> {
            self.audio.iter().map(|(n, d)| AudioTrack { number: *n, sectors: (d.len() / RAW_SECTOR) as u32 }).collect()
        }
        fn read_audio(&mut self, track: u32, sector: u32, out: &mut [u8]) -> Result<()> {
            let (_, d) = self.audio.iter().find(|a| a.0 == track).ok_or(Error::Format("no such track".into()))?;
            let at = sector as usize * RAW_SECTOR;
            out[..RAW_SECTOR].copy_from_slice(d.get(at..at + RAW_SECTOR).ok_or(Error::Io("past the end".into()))?);
            Ok(())
        }
    }

    /// A tiny ISO 9660 image (`files`: name → bytes in the root, Form 1), PVD volume `volume`, SYSTEM.CNF booting
    /// SCUS_941.99; each file starts on its own sector after the root directory (sector 18).
    pub fn tiny_iso(raw: bool, volume: &str, files: &[(&str, Vec<u8>, bool)]) -> Vec<u8> {
        let mut all: Vec<(String, Vec<u8>, bool)> =
            vec![("SYSTEM.CNF".into(), b"BOOT = cdrom:\\SCUS_941.99;1\n".to_vec(), false)];
        all.extend(files.iter().map(|(n, d, form2)| (n.to_string(), d.clone(), *form2)));
        let mut next = 19u32;
        let mut placed = Vec::new();
        for (n, d, form2) in &all {
            let k = d.len().div_ceil(if *form2 { RAW_SECTOR } else { FORM1_DATA }).max(1) as u32;
            placed.push((n.clone(), next, d.clone(), *form2));
            next += k;
        }
        let total = next as usize;
        let mut user = vec![vec![0u8; FORM1_DATA]; total];
        let mut whole: Vec<Option<Vec<u8>>> = vec![None; total];
        let p = &mut user[16];
        p[0] = 1;
        p[1..6].copy_from_slice(b"CD001");
        let pad = |s: &str| format!("{s:<32}").into_bytes();
        p[8..40].copy_from_slice(&pad("PLAYSTATION"));
        p[40..72].copy_from_slice(&pad(volume));
        fn rec(lba: u32, size: u32, dir: bool, name: &[u8]) -> Vec<u8> {
            let mut r = vec![0u8; 33];
            r[2..6].copy_from_slice(&lba.to_le_bytes());
            r[10..14].copy_from_slice(&size.to_le_bytes());
            r[25] = if dir { 2 } else { 0 };
            r[32] = name.len() as u8;
            r.extend_from_slice(name);
            if r.len() % 2 == 1 {
                r.push(0);
            }
            r[0] = r.len() as u8;
            r
        }
        let root = rec(18, 2048, true, b"\0");
        user[16][156..156 + root.len()].copy_from_slice(&root);
        let mut dir = Vec::new();
        dir.extend(rec(18, 2048, true, b"\0"));
        dir.extend(rec(18, 2048, true, b"\x01"));
        for (n, lba, d, form2) in &placed {
            // an interleaved file's ISO size counts 2048 bytes per sector
            let size = if *form2 { (d.len() / RAW_SECTOR * FORM1_DATA) as u32 } else { d.len() as u32 };
            dir.extend(rec(*lba, size, false, format!("{n};1").as_bytes()));
        }
        user[18][..dir.len()].copy_from_slice(&dir);
        for (_, lba, d, form2) in &placed {
            if *form2 {
                for (k, s) in d.chunks(RAW_SECTOR).enumerate() {
                    whole[*lba as usize + k] = Some(s.to_vec());
                }
            } else {
                for (k, s) in d.chunks(FORM1_DATA).enumerate() {
                    user[*lba as usize + k][..s.len()].copy_from_slice(s);
                }
            }
        }
        let mut out = Vec::new();
        for (i, s) in user.iter().enumerate() {
            if raw {
                if let Some(w) = &whole[i] {
                    out.extend_from_slice(w);
                    continue;
                }
                out.extend_from_slice(&[0, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0]);
                out.extend_from_slice(&[0, 2, i as u8, 2]);
                out.extend_from_slice(&[0, 0, 8, 0, 0, 0, 8, 0]);
                out.extend_from_slice(s);
                out.extend_from_slice(&[0u8; 280]);
            } else {
                out.extend_from_slice(s);
            }
        }
        out
    }

    #[test]
    fn iso_and_raw_sectors_and_the_game_check() {
        let files = [("SCUS_941.99", vec![7u8; 3000], false), ("A.DAT", b"abc".to_vec(), false)];
        for raw in [false, true] {
            let data = tiny_iso(raw, VOLUME_ID, &files);
            let mut d = Disc::open_checked(Box::new(MemSectors { data, raw, audio: Vec::new() })).unwrap();
            let names: Vec<&str> = d.files.iter().map(|f| f.path.as_str()).collect();
            assert_eq!(names, ["A.DAT", "SCUS_941.99", "SYSTEM.CNF"]);
            let a = d.find("a.dat").unwrap().clone();
            assert_eq!(d.read_file(&a).unwrap(), b"abc");
            assert_eq!(d.raw_sector(a.lba).is_ok(), raw);
            assert_eq!(d.source(), if raw { "bin" } else { "iso" });
        }
        let other = tiny_iso(true, "SLUS_00001", &files);
        let e = Disc::open_checked(Box::new(MemSectors { data: other, raw: true, audio: Vec::new() })).err().unwrap();
        assert!(matches!(&e, Error::WrongDisc { found } if found.contains("SLUS_00001")), "{e}");
        let no_exe = tiny_iso(true, VOLUME_ID, &files[1..]);
        assert!(matches!(
            Disc::open_checked(Box::new(MemSectors { data: no_exe, raw: true, audio: Vec::new() })),
            Err(Error::WrongDisc { .. })
        ));
    }
}
