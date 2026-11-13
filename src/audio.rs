//! SPU (VAG) and CD-XA ADPCM decoders, VAB sound banks, WAV output, ffmpeg helpers.

use crate::bail;
#[cfg(feature = "cli")]
use crate::error::Context;
use crate::error::Result;
#[cfg(feature = "cli")]
use std::path::Path;
#[cfg(feature = "cli")]
use std::process::Command;

const K0: [i32; 5] = [0, 60, 115, 98, 122];
const K1: [i32; 5] = [0, 0, -52, -55, -60];

/// Decodes SPU ADPCM (VAG body). Stops at the first block with the end flag.
pub fn decode_vag(data: &[u8]) -> Vec<i16> {
    let (mut s1, mut s2) = (0i32, 0i32);
    let mut out = Vec::with_capacity(data.len() / 16 * 28);
    for block in data.as_chunks::<16>().0 {
        let shift = (block[0] & 15) as i32;
        let filter = ((block[0] >> 4) as usize).min(4);
        for i in 0..28 {
            let nib = (block[2 + i / 2] >> ((i & 1) * 4)) & 15;
            let mut s = ((nib as i32) << 28 >> 16) >> shift.min(12);
            s += (s1 * K0[filter] + s2 * K1[filter] + 32) >> 6;
            let s = s.clamp(-32768, 32767);
            out.push(s as i16);
            s2 = s1;
            s1 = s;
        }
        if block[1] & 1 != 0 {
            break;
        }
    }
    out
}

/// Streaming CD-XA ADPCM decoder (4-bit only, which is all this game uses).
pub struct XaDecoder {
    hist: [[i32; 2]; 2],
}

impl XaDecoder {
    pub fn new() -> Self {
        XaDecoder { hist: [[0; 2]; 2] }
    }

    /// Decodes one 2352-byte raw sector. Returns interleaved samples.
    pub fn sector(&mut self, s: &[u8], stereo: bool, out: &mut Vec<i16>) {
        let data = &s[24..24 + 18 * 128];
        let mut l = Vec::with_capacity(28 * 4);
        let mut r = Vec::with_capacity(28 * 4);
        for g in data.as_chunks::<128>().0 {
            for unit in 0..8 {
                let param = g[4 + unit];
                let shift = 12 - (param & 15).min(12) as i32;
                let filter = ((param >> 4) & 3) as usize;
                let ch = if stereo { unit & 1 } else { 0 };
                let h = &mut self.hist[ch];
                let dst = if stereo && unit & 1 == 1 { &mut r } else { &mut l };
                for j in 0..28 {
                    let nib = (g[16 + j * 4 + unit / 2] >> ((unit & 1) * 4)) & 15;
                    let mut v = ((nib as i32) << 28 >> 16) >> (12 - shift);
                    v += (h[0] * K0[filter] + h[1] * K1[filter] + 32) >> 6;
                    let v = v.clamp(-32768, 32767);
                    h[1] = h[0];
                    h[0] = v;
                    dst.push(v as i16);
                }
            }
        }
        if stereo {
            for (a, b) in l.iter().zip(&r) {
                out.push(*a);
                out.push(*b);
            }
        } else {
            out.extend(l);
        }
    }
}

impl Default for XaDecoder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "cli")]
pub fn write_wav(path: &Path, rate: u32, channels: u16, samples: &[i16]) -> Result<()> {
    let bytes = samples.len() as u32 * 2;
    let mut w = Vec::with_capacity(44 + bytes as usize);
    w.extend(b"RIFF");
    w.extend((36 + bytes).to_le_bytes());
    w.extend(b"WAVEfmt ");
    w.extend(16u32.to_le_bytes());
    w.extend(1u16.to_le_bytes());
    w.extend(channels.to_le_bytes());
    w.extend(rate.to_le_bytes());
    w.extend((rate * channels as u32 * 2).to_le_bytes());
    w.extend((channels * 2).to_le_bytes());
    w.extend(16u16.to_le_bytes());
    w.extend(b"data");
    w.extend(bytes.to_le_bytes());
    for s in samples {
        w.extend(s.to_le_bytes());
    }
    std::fs::write(path, w)?;
    Ok(())
}

/// Runs ffmpeg with the given arguments (quiet, overwrite).
#[cfg(feature = "cli")]
pub fn ffmpeg(args: &[&str]) -> Result<()> {
    let st = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(args)
        .status()
        .context("running ffmpeg (is it installed and on PATH?)")?;
    if !st.success() {
        bail!("ffmpeg failed: {args:?}");
    }
    Ok(())
}

/// Writes PCM to a temporary WAV and encodes it to FLAC.
#[cfg(feature = "cli")]
pub fn write_flac(path: &Path, rate: u32, channels: u16, samples: &[i16]) -> Result<()> {
    let tmp = path.with_extension("tmp.wav");
    write_wav(&tmp, rate, channels, samples)?;
    let r = ffmpeg(&["-i", tmp.to_str().unwrap(), "-compression_level", "8", path.to_str().unwrap()]);
    std::fs::remove_file(&tmp).ok();
    r
}

pub struct VabSample {
    pub index: usize,
    pub rate: u32,
    pub pcm: Vec<i16>,
    pub looped: bool,
}

/// Tone table of a VAB bank: (program, tone within program) -> VAG index (1-based),
/// i.e. which exported `<bank>_NNN.wav` a (program, tone) sound request plays.
pub fn vab_tone_map(d: &[u8]) -> std::collections::HashMap<(usize, usize), usize> {
    let u16_at = |o: usize| d.get(o..o + 2).map_or(0, |b| u16::from_le_bytes([b[0], b[1]]));
    let mut map = std::collections::HashMap::new();
    if d.len() < 32 || &d[0..4] != b"pBAV" {
        return map;
    }
    let ps = u16_at(0x12) as usize;
    for prog in 0..ps.min(128) {
        for tone in 0..16 {
            let vag = u16_at(32 + 128 * 16 + (prog * 16 + tone) * 32 + 22) as usize;
            if vag > 0 {
                map.insert((prog, tone), vag);
            }
        }
    }
    map
}

/// Parses a VAB ("pBAV") bank and decodes every VAG sample.
pub fn parse_vab(d: &[u8]) -> Result<Vec<VabSample>> {
    if d.len() < 32 || &d[0..4] != b"pBAV" {
        bail!("not a VAB");
    }
    let u16_at = |o: usize| d.get(o..o + 2).map_or(0, |b| u16::from_le_bytes([b[0], b[1]]));
    let ps = u16_at(0x12) as usize;
    let vs = u16_at(0x16) as usize;
    let tones_at = 32 + 128 * 16;
    let table_at = tones_at + ps * 16 * 32;
    let mut body = table_at + 512;
    if body > d.len() {
        bail!("VAB header truncated");
    }
    // Tones are a drum map (one key per tone, min == max); the game keys them
    // on at that note. The first tone referencing each VAG decides its pitch.
    let mut pitch = vec![None; vs + 1];
    for t in 0..ps * 16 {
        let o = tones_at + t * 32;
        let vag = u16_at(o + 22) as usize;
        let Some(t) = d.get(o..o + 8) else { break };
        if vag >= 1 && vag <= vs && pitch[vag].is_none() && t[4] != 0 {
            let key = if t[6] == t[7] { t[6] } else { 60 };
            pitch[vag] = Some(key as f64 - t[4] as f64 - t[5] as f64 / 128.0);
        }
    }
    let mut out = Vec::new();
    for (i, semis) in pitch.iter().enumerate().skip(1) {
        let size = u16_at(table_at + i * 2) as usize * 8;
        let end = (body + size).min(d.len());
        let raw = &d[body..end];
        body = end;
        let semis = semis.unwrap_or(0.0);
        let rate = (44100.0 * 2f64.powf(semis / 12.0)).round() as u32;
        let looped = raw.as_chunks::<16>().0.iter().any(|b| b[1] & 4 != 0);
        out.push(VabSample { index: i, rate: rate.clamp(4000, 96000), pcm: decode_vag(raw), looped });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vag_silence_and_end_flag() {
        let mut d = vec![0u8; 48];
        d[17] = 1; // end flag on the second block
        assert_eq!(decode_vag(&d).len(), 56);
        assert!(decode_vag(&d).iter().all(|&s| s == 0));
    }

    #[test]
    fn vag_nibbles_are_signed() {
        let mut b = [0u8; 16];
        b[0] = 12; // shift 12, filter 0 -> sample = nibble (sign-extended)
        b[2] = 0x8f; // samples: -1 (0xf), -8 (0x8)
        let s = decode_vag(&b);
        assert_eq!(&s[..2], &[-1, -8]);
    }

    #[test]
    fn xa_sector_sizes() {
        let sector = vec![0u8; 2352];
        let mut out = Vec::new();
        XaDecoder::new().sector(&sector, true, &mut out);
        assert_eq!(out.len(), 18 * 8 * 28); // 18 groups x 8 units x 28 samples
        out.clear();
        XaDecoder::new().sector(&sector, false, &mut out);
        assert_eq!(out.len(), 18 * 8 * 28);
    }

    #[test]
    fn vab_rejects_garbage_and_parses_minimal_bank() {
        assert!(parse_vab(b"nope").is_err());
        // 1 program, 1 tone (key 48, centre 72 -> 11025 Hz), 1 VAG of one 16-byte block
        let (ps, vs) = (1usize, 1usize);
        let mut d = vec![0u8; 32 + 128 * 16 + ps * 16 * 32 + 512 + 16];
        d[0..4].copy_from_slice(b"pBAV");
        d[0x12..0x14].copy_from_slice(&(ps as u16).to_le_bytes());
        d[0x16..0x18].copy_from_slice(&(vs as u16).to_le_bytes());
        let tone = 32 + 128 * 16;
        d[tone + 4] = 72;
        d[tone + 6] = 48;
        d[tone + 7] = 48;
        d[tone + 22..tone + 24].copy_from_slice(&1u16.to_le_bytes());
        let table = tone + ps * 16 * 32;
        d[table + 2..table + 4].copy_from_slice(&2u16.to_le_bytes()); // 2*8 = 16 bytes
        let s = parse_vab(&d).unwrap();
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].rate, 11025);
        assert_eq!(s[0].pcm.len(), 28);
    }

    #[cfg(feature = "cli")]
    #[test]
    fn wav_header() {
        let p = std::env::temp_dir().join(format!("okconv-br1-test-{}.wav", std::process::id()));
        write_wav(&p, 22050, 1, &[1, -1]).unwrap();
        let d = std::fs::read(&p).unwrap();
        std::fs::remove_file(&p).ok();
        assert_eq!(&d[0..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(d[24..28].try_into().unwrap()), 22050);
        assert_eq!(d.len(), 44 + 4);
    }
}
