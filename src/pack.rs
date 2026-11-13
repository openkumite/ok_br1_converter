//! Bloody Roar .DAT containers and their LZSS-style compression
//! (port of SCUS_941.99 @ 0x8016eac8).
//!
//! Container: u32 count N, N u32 offsets relative to 4 + 4N (0xFFFFFFFF, or 0
//! for any slot but the first, = empty). Entry: u32 type (2), u32 packed size,
//! u32 raw size, then the bitstream.
//!
//! Bitstream: flag bits MSB-first, a flag byte fetched from the stream only when
//! a bit is needed. `1` literal byte; `00ab` + byte o: copy 2+2a+b from 0x100-o
//! back; `01` + b0 b1: distance 0x2000 - (b0b1 >> 3), length (b1&7)+2, or if
//! b1&7 == 0 a further byte n (0 = end, else length n+1). Copies are byte-wise
//! from the output, so they may overlap.

use crate::bail;
use crate::error::Result;

pub fn decompress(entry: &[u8]) -> Result<Vec<u8>> {
    if entry.len() < 12 {
        bail!("entry too short");
    }
    let rd = |o: usize| u32::from_le_bytes(entry[o..o + 4].try_into().unwrap()) as usize;
    let (packed, raw) = (rd(4), rd(8));
    let src = entry.get(12..12 + packed).unwrap_or(&entry[12..]);
    // Real entries are at most a few hundred KB; the header size isn't trusted
    // for allocation and output is capped at it while decoding.
    const MAX_RAW: usize = 16 << 20;
    if raw > MAX_RAW {
        bail!("declared size {raw} too large");
    }
    let mut out = Vec::with_capacity(raw);
    let (mut sp, mut bits, mut nbits) = (0usize, 0u8, 0u32);
    let byte = |sp: &mut usize| -> Result<u8> {
        let b = *src.get(*sp).ok_or_else(|| crate::format_err!("compressed stream overrun"))?;
        *sp += 1;
        Ok(b)
    };
    macro_rules! bit {
        () => {{
            if nbits == 0 {
                bits = byte(&mut sp)?;
                nbits = 8;
            }
            nbits -= 1;
            (bits >> nbits) & 1
        }};
    }
    loop {
        if bit!() == 1 {
            if out.len() >= raw {
                bail!("output exceeds declared size {raw}");
            }
            out.push(byte(&mut sp)?);
            continue;
        }
        let (dist, n);
        if bit!() == 0 {
            let a = bit!() as usize;
            let b = bit!() as usize;
            n = 2 + a * 2 + b;
            dist = 0x100 - byte(&mut sp)? as usize;
        } else {
            let b0 = byte(&mut sp)? as usize;
            let b1 = byte(&mut sp)? as usize;
            dist = 0x2000 - ((b0 << 8 | b1) >> 3);
            n = match b1 & 7 {
                0 => match byte(&mut sp)? {
                    0 => break,
                    m => m as usize + 1,
                },
                m => m + 2,
            };
        }
        if dist > out.len() {
            bail!("back-reference before start of output");
        }
        if out.len() + n > raw {
            bail!("output exceeds declared size {raw}");
        }
        // Byte-wise so a copy may overlap its own output.
        for p in (out.len() - dist..).take(n) {
            out.push(out[p]);
        }
    }
    if out.len() != raw {
        bail!("decompressed {} bytes, expected {raw}", out.len());
    }
    Ok(out)
}

/// Returns (slot index, decompressed bytes) for every non-empty entry.
pub fn unpack(data: &[u8]) -> Result<Vec<(usize, Vec<u8>)>> {
    let n = u32::from_le_bytes(data.get(0..4).unwrap_or(&[0; 4]).try_into()?) as usize;
    if n == 0 || n > 1024 || 4 + 4 * n > data.len() {
        bail!("not a container");
    }
    let base = 4 + 4 * n;
    let mut out = Vec::new();
    for i in 0..n {
        let o = u32::from_le_bytes(data[4 + 4 * i..8 + 4 * i].try_into()?);
        if o == 0xFFFF_FFFF || (o == 0 && i > 0) {
            continue;
        }
        let p = base + o as usize;
        let entry = data.get(p..).unwrap_or_default();
        if entry.len() < 12 || u32::from_le_bytes(entry[0..4].try_into()?) != 2 {
            bail!("entry {i}: unexpected type");
        }
        out.push((i, decompress(entry)?));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds an entry: header {2, packed, raw} + stream.
    fn entry(raw: usize, stream: &[u8]) -> Vec<u8> {
        let mut e = Vec::new();
        e.extend(2u32.to_le_bytes());
        e.extend((stream.len() as u32).to_le_bytes());
        e.extend((raw as u32).to_le_bytes());
        e.extend(stream);
        e
    }

    #[test]
    fn literals_then_end() {
        // flags 1,1,0,1 = lit 'A', lit 'B', long ref with b1&7==0 and n==0 -> end
        let s = [0b1101_0000, b'A', b'B', 0x00, 0x00, 0x00];
        assert_eq!(decompress(&entry(2, &s)).unwrap(), b"AB");
    }

    #[test]
    fn short_overlapping_copy() {
        // flag bits 1 | 0 0 1 1 | 0 1: lit 'x', short ref len 2+2+1 = 5 at dist 1
        // (o = 0xff) overlapping its own output, then end (long ref, n = 0)
        let s = [0b1001_1010, b'x', 0xff, 0x00, 0x00, 0x00];
        assert_eq!(decompress(&entry(6, &s)).unwrap(), b"xxxxxx");
    }

    #[test]
    fn long_copy_with_length_byte() {
        // lits 'a','b'; long ref dist 2 (0x2000-2 = 0x1ffe -> b0b1 = 0xfff0), len n+1 = 6; end
        let s = [0b1101_0100, b'a', b'b', 0xff, 0xf0, 0x05, 0x00, 0x00, 0x00];
        assert_eq!(decompress(&entry(8, &s)).unwrap(), b"abababab");
    }

    #[test]
    fn rejects_bad_streams() {
        // back-reference before start
        assert!(decompress(&entry(1, &[0b0000_0000, 0xff])).is_err());
        // truncated stream
        assert!(decompress(&entry(5, &[0b1111_1111, b'a'])).is_err());
        // wrong raw size
        assert!(decompress(&entry(9, &[0b1101_0000, b'A', b'B', 0, 0, 0])).is_err());
        assert!(decompress(&[1, 2, 3]).is_err());
    }

    #[test]
    fn container_skips_empty_slots() {
        let e = entry(2, &[0b1101_0000, b'A', b'B', 0, 0, 0]);
        let mut c = Vec::new();
        c.extend(3u32.to_le_bytes());
        c.extend(0u32.to_le_bytes()); // slot 0 at base
        c.extend(0xFFFF_FFFFu32.to_le_bytes()); // empty
        c.extend(0u32.to_le_bytes()); // 0 for a later slot = empty
        c.extend(&e);
        let out = unpack(&c).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0], (0, b"AB".to_vec()));
        assert!(unpack(&[0, 0, 0, 0]).is_err());
    }
}
