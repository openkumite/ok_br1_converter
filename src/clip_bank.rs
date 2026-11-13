//! The executable's bytes the game plays as clips: the `anim.common_clips` table of `engine/tables.okt`.
//!
//! A script's op 0x05 sub 0 names a clip by index `k`: the fighter's own table when it has the clip, else (an empty
//! own slot below its count and below 0x100) CMN.ACT's table, read past its end when `k` is at or past CMN.ACT's
//! count. There the file's raw words are clip addresses; one into the executable's image (through the RAM mirrors)
//! is played as a clip there: the header (+0 the total, +6 key 0's duration), the key list at +4 (4 bytes a key: the
//! target's offset from the clip, its duration) and each target pose (the root and two translations at +0..+20, nodes
//! 1..16 at +20 + 6·slot, the hips' twists at +0x54 + 2·slot, the slot by the bone maps). A reader keeps such a
//! clip by its file offset, so the table's origin is the file offset of its first byte, not a RAM address as the
//! other tables' (the two ranges cannot meet: file offsets stay below the image's length, RAM addresses start at
//! 0x80000000).
//!
//! A clip runs its keys in order from key 1 until a key's duration gives no next one (0, or 0 once × 16 in 16 bits);
//! a script's op 0x0C sub 1 jumps to key `a + 1` after reading key `a`'s duration, sub 3 adds to the pending duration
//! (which could carry a run past a stop). `extents` walks every executable clip the nine fighters' scripts can name
//! from key 1 and from every key their op 0x0C sub 1 jumps to, stopping at the stops: `Scripts::of` follows the
//! scripts' control flow and checks that no op 0x0C sub 1 / 3 runs while a clip of the executable may be playing.

use crate::moves::OPLEN;
use std::collections::BTreeSet;

/// RAM address of the executable's file offset 0.
pub const EXE_BASE: u32 = 0x8011_F800;
/// Entries of an ANM's state table.
const STATES: usize = 466;
/// The most points of the scripts' runs `Scripts::of` follows (a fighter of the USA disc: under 7000).
const MAX_POINTS: usize = 1 << 21;

fn rd16(b: &[u8], o: usize) -> i16 {
    o.checked_add(2).and_then(|e| b.get(o..e)).map_or(0, |s| i16::from_le_bytes([s[0], s[1]]))
}

fn rd32(b: &[u8], o: usize) -> u32 {
    o.checked_add(4).and_then(|e| b.get(o..e)).map_or(0, |s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

/// A clip table (`base`: its block; u32 size, u16 count, entries at +8, 0 or negative = empty): per entry, whether
/// it holds a clip.
fn clip_table(b: &[u8], base: usize) -> Vec<bool> {
    let at = |o: usize| base.saturating_add(o);
    let count = rd16(b, at(4)) as u16 as usize;
    (0..count).map(|i| (rd32(b, at(8 + 4 * i)) as i32) > 0).collect()
}

/// The file offset of the executable clip CMN.ACT's table names at index `k` (past its count): its word there as an
/// address into the image of `exe_len` bytes (kuseg / kseg0 / kseg1 over the first 8 MB, the 2 MB of RAM mirrored;
/// at or above the image's load address 0x80120000). A reader resolves it the same way.
pub fn clip_address(cmn: &[u8], k: usize, exe_len: usize) -> Option<usize> {
    let w = cmn.get(8 + 4 * k..12 + 4 * k)?;
    let phys = u32::from_le_bytes(w.try_into().unwrap()) & 0x1FFF_FFFF;
    let ram = 0x8000_0000 | (phys & 0x1F_FFFF);
    (phys < 0x80_0000 && ram >= 0x8012_0000).then(|| (ram - EXE_BASE) as usize).filter(|&o| o < exe_len)
}

/// What a fighter's scripts (its ANM, `XX31.DAT`) do with the executable's clips: the instructions reachable from the
/// states' starts, followed through the jumps, calls and returns, the loop marks and the gates' skips (the game's
/// op handlers 0x8017D680..0x801810EC), each with whether a clip of the executable may be playing.
#[derive(Debug)]
pub struct Scripts {
    /// the executable clips its op 0x05 sub 0 can start (file offsets)
    pub clips: BTreeSet<usize>,
    /// the keys its op 0x0C sub 1 jumps to (the argument)
    pub keys: BTreeSet<i16>,
    /// the highest slot of its bone maps
    pub max_slot: u8,
    /// the states whose start a call also enters (a return to an unknown pc there ends the path)
    pub called_states: BTreeSet<usize>,
    /// the returns to an unknown pc: (the loop mark they run under, their pc)
    pub unknown_returns: BTreeSet<(usize, usize)>,
}

/// A point of a script's run: the pc, whether a clip of the executable may be playing, the return pc (None: not
/// known, the one a previous state's call left) and the loop mark.
type Point = (usize, bool, Option<usize>, usize);

impl Scripts {
    /// Err when the ANM does not decode (a reachable opcode past the table, a bone map past the end), or when an op
    /// 0x0C sub 1 / 3 can run while a clip of the executable may play.
    ///
    /// The flag holds along every path: at a state's start it is set (the previous state's clip plays on; a state
    /// also starts from outside the scripts), an op 0x05 sub 0 that surely runs sets it to whether its clip is the
    /// executable's, one behind a form gate may only add to it, one whose index is past the own table does nothing.
    /// A point is kept per return pc and loop mark, so a call returns to its own caller with the callee's flag (a
    /// callee that starts no clip leaves the caller's). A return to an unknown pc (a state's script returning without a
    /// call: the return pc is the one a call of an earlier state left) goes, with the flag set, to every call's return
    /// pc; not under a loop mark that is a call's target (`called_states`: a state's start, a subroutine the state
    /// table lists; keyed on the loop mark, so a loop mark another script sets there counts too). That holds by what
    /// starts those states on the USA disc (states 10, 15, 16, 18 and 63 of each fighter, `tests/runtime.rs` checks it):
    /// 10, 15, 16 and 63 are never started as a state in play (not by the game's decision, landing or throws, nor by
    /// the scripts' op 0x05 sub 1; the game's debug move cheats, ForceMove / NextMove / PrevMove, can start any
    /// state, and are not covered); 18 only by the decision, on a crouch's release with F.50 bit 0x20 set, and its one
    /// return to an unknown pc is its first `00 03` (return when the result is 0), right after `0A 14 20000000` sets
    /// the result to F.50 & 0x20: it does not fire when 18 starts as a state. The disc is accepted by its volume ID,
    /// so another pressing goes through the same rule unchecked unless the BR1_DISC tests run on it.
    ///
    /// Not modelled either: the return pc a match starts with (0, no init sets it), so a return before any call of the
    /// match would run the ANM's header as script; the paths that do so are the same unknown returns.
    ///
    /// Err also when the points to follow pass `MAX_POINTS` (data no disc holds).
    pub fn of(anm: &[u8], cmn: &[u8], exe_len: usize) -> Result<Self, String> {
        let hdr = |k: usize| rd32(anm, 4 * k) as usize;
        let starts: BTreeSet<usize> = (0..STATES)
            .filter_map(|i| hdr(1).checked_add(2 * i))
            .map(|o| rd16(anm, o) as u16)
            .filter(|&s| s != 0 && s != 0xFFFF)
            .map(usize::from)
            .collect();
        if starts.is_empty() {
            return Err("no state script".into());
        }
        let own = clip_table(anm, hdr(0));
        let cmn_count = clip_table(cmn, 0).len();
        // index k (below the own table's count) of op 0x05 sub 0 → the executable clip it plays, if it does
        let exe_clip =
            |k: usize| (!own[k] && k < 0x100 && k >= cmn_count).then(|| clip_address(cmn, k, exe_len)).flatten();
        let (mut clips, mut keys) = (BTreeSet::new(), BTreeSet::new());
        let mut seen: BTreeSet<Point> = BTreeSet::new();
        let mut work: Vec<Point> = starts.iter().map(|&s| (s, true, None, s)).collect();
        // the calls' return pcs and targets, and the loop marks a return to an unknown pc ran under
        let (mut rets, mut called, mut unknown) = (BTreeSet::new(), BTreeSet::new(), BTreeSet::new());
        loop {
            while let Some(pt) = work.pop() {
                if !seen.insert(pt) {
                    continue;
                }
                if seen.len() > MAX_POINTS {
                    return Err("the scripts' control flow is too large".into());
                }
                let (p, mut exe, ret, lp) = pt;
                // the game stops at a pc outside the file
                if p.checked_add(2).is_none_or(|e| e > anm.len()) {
                    continue;
                }
                let (op, sub) = (anm[p], anm[p + 1]);
                let len = *OPLEN.get(op as usize).ok_or_else(|| format!("op {op:#x} at {p:#x}"))?;
                let (next, skip, a) = (p + len, p + 4, rd16(anm, p + 2));
                // ops 05 / 24's form gate: 1 human only, 2 beast only (either may be closed), 0 / 3 always open
                let may_gate = matches!(((sub as i8) >> 4) & 3, 1 | 2);
                let mut to = |pc: usize, exe: bool, ret: Option<usize>, lp: usize| work.push((pc, exe, ret, lp));
                match op {
                    0x00 => {
                        // (falls through, returns) by the sub: 0 stops, 3..6 by the result, 1 / 7 / 8 go on
                        let (on, back) = match sub as i8 {
                            0 => (false, false),
                            1 | 7 | 8 => (true, false),
                            3..=6 => (true, true),
                            _ => (false, true),
                        };
                        if on {
                            to(next, exe, ret, lp);
                        }
                        match ret {
                            Some(r) if back => to(r, exe, ret, lp),
                            None if back => {
                                unknown.insert((lp, p));
                            }
                            _ => {}
                        }
                    }
                    0x01 => match sub as i8 {
                        0 => to(lp, exe, ret, lp),
                        1 => to(next, exe, ret, next),
                        _ => to(next, exe, ret, lp),
                    },
                    0x04 => {
                        let target = a as u16 as usize;
                        // (always jumps, calls) by the condition; a gate on the form may skip it
                        let (always, call) = match sub & 0x3F {
                            5 => (true, true),
                            6..=9 => (false, true),
                            1..=4 | 10..=14 | 16..=24 => (false, false),
                            _ => (true, false),
                        };
                        if call {
                            rets.insert(next);
                            called.insert(target);
                            to(target, exe, Some(next), lp);
                        } else {
                            to(target, exe, ret, lp);
                        }
                        if !always || matches!(sub & 0xC0, 0x40 | 0x80) {
                            to(next, exe, ret, lp);
                        }
                    }
                    0x05 => {
                        if sub & 0xF == 0 {
                            // a = −1: the clip cycle over the own table's indices (none: nothing starts); another
                            // negative index: the null clip; one past the own table: nothing starts
                            let played: Option<Vec<usize>> = match a {
                                -1 if own.is_empty() => None,
                                -1 => Some((0..own.len()).filter_map(exe_clip).collect()),
                                ..-1 => Some(Vec::new()),
                                _ if a as usize >= own.len() => None,
                                _ => Some(exe_clip(a as usize).into_iter().collect()),
                            };
                            if let Some(played) = played {
                                clips.extend(&played);
                                exe = if may_gate { exe || !played.is_empty() } else { !played.is_empty() };
                            }
                        }
                        // sub 1 starts a state (a start: set already) unless the gate skips it
                        if sub & 0xF != 1 || may_gate {
                            to(next, exe, ret, lp);
                        }
                    }
                    0x0C => {
                        if sub == 1 || sub == 3 {
                            if exe {
                                return Err(format!("op 0x0C sub {sub} at {p:#x} may act on a clip of the executable"));
                            }
                            if sub == 1 {
                                keys.insert(a);
                            }
                        }
                        to(next, exe, ret, lp);
                    }
                    0x24 if may_gate => {
                        to(next, exe, ret, lp);
                        to(skip, exe, ret, lp);
                    }
                    0x29 => {
                        to(skip, exe, ret, lp);
                        to(a as u16 as usize, exe, ret, lp);
                    }
                    _ => to(next, exe, ret, lp),
                }
            }
            // a return to an unknown pc: to every call's return pc (its own return pc then; not to pc 0, the return
            // pc before a match's first call), unless it runs under a loop mark the scripts call: a state's start the
            // state table lists as a subroutine too (see above)
            let marks: BTreeSet<usize> = unknown.iter().map(|&(lp, _)| lp).filter(|lp| !called.contains(lp)).collect();
            // the fan-out counts against the bound before it is pushed (marks × returns could be ~10^8 points)
            if marks.len().saturating_mul(rets.len()).saturating_add(seen.len()).saturating_add(work.len()) > MAX_POINTS
            {
                return Err("the scripts' control flow is too large".into());
            }
            for &lp in &marks {
                for &r in &rets {
                    if !seen.contains(&(r, true, Some(r), lp)) {
                        work.push((r, true, Some(r), lp));
                    }
                }
            }
            if work.is_empty() {
                break;
            }
        }
        // the bone maps: header word 9 → four offsets of 20 slots
        let mut max_slot = 0;
        for k in 0..4 {
            let o = hdr(9).checked_add(4 * k).map_or(0, |o| rd32(anm, o) as usize);
            let m = o.checked_add(20).and_then(|e| anm.get(o..e)).ok_or("a bone map past the end")?;
            max_slot = max_slot.max(*m.iter().max().unwrap());
        }
        let called_states = (0..STATES)
            .filter(|&i| hdr(1).checked_add(2 * i).is_some_and(|o| called.contains(&(rd16(anm, o) as u16 as usize))))
            .collect();
        Ok(Self { clips, keys, max_slot, called_states, unknown_returns: unknown })
    }
}

/// The [start, end) ranges of the bytes of `exe` the executable clips of `scripts` (every fighter's) read: each
/// clip's span, overlapping or touching ones merged, in order; empty without a clip. Err when a run of keys does not
/// end inside the image.
pub fn extents(exe: &[u8], scripts: &[Scripts]) -> Result<Vec<(usize, usize)>, String> {
    let max_slot = scripts.iter().map(|s| s.max_slot as usize).max().unwrap_or(0);
    let jumps: BTreeSet<i16> = scripts.iter().flat_map(|s| s.keys.iter().copied()).collect();
    let clips: BTreeSet<usize> = scripts.iter().flat_map(|s| s.clips.iter().copied()).collect();
    let pose = (20 + 6 * max_slot + 6).max(0x54 + 2 * max_slot + 2) as isize;
    // the time of the next key from a duration (1: one frame step, never 0)
    let next = |d: i16| if d == 1 { 1 } else { d.wrapping_shl(4) };
    let mut spans = Vec::new();
    for &c in &clips {
        let c = c as isize;
        let mut span = (c, c + 8);
        let mut mark = |a: isize, n: isize| span = (span.0.min(a), span.1.max(a + n));
        // where a run starts: key 1 after the clip's start (key 0's duration pending), key a + 1 after op 0x0C sub 1
        // (key a's duration: read at the clip + 6 + 4a)
        let mut starts = vec![(1i16, next(rd16(exe, c as usize + 6)))];
        for &a in &jumps {
            let at = c + 6 + 4 * a as isize;
            mark(at, 2);
            let d = if at < 0 { 0 } else { rd16(exe, at as usize) as i32 };
            starts.push((a.wrapping_add(1), if d < 2 { 1 } else { (d << 4) as i16 }));
        }
        for (mut key, mut pending) in starts {
            let mut steps = 0;
            loop {
                let e = c + 4 + 4 * key as u16 as isize;
                if e as usize + 4 > exe.len() || steps > 0xFFFF {
                    return Err(format!("the keys of the clip at {c:#x} do not end inside the image"));
                }
                // the entry is read even when nothing is pending (the run stops there)
                mark(e, 4);
                if pending == 0 {
                    break;
                }
                let (off, dur) = (rd16(exe, e as usize), rd16(exe, e as usize + 2));
                mark(c + off as isize, pose);
                pending = next(dur);
                key = key.wrapping_add(1);
                steps += 1;
            }
        }
        spans.push((span.0.max(0) as usize, (span.1.max(0) as usize).min(exe.len())));
    }
    spans.sort();
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (s, e) in spans {
        match merged.last_mut() {
            Some(last) if s <= last.1 => last.1 = last.1.max(e),
            _ => merged.push((s, e)),
        }
    }
    Ok(merged)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put16(b: &mut [u8], o: usize, v: u16) {
        b[o..o + 2].copy_from_slice(&v.to_le_bytes());
    }
    fn put32(b: &mut [u8], o: usize, v: u32) {
        b[o..o + 4].copy_from_slice(&v.to_le_bytes());
    }

    /// An executable with a clip at file offset 0x1000 (key 0's duration 2; key 1 → +0x100, duration 3; key 2 →
    /// +0x200, duration 0: the stop), a CMN.ACT of one clip whose word 1 is that clip's address, and an ANM whose own
    /// slot 1 is empty, with one state running `script` and a bone map reaching slot 18.
    fn data(script: &[u8]) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        let mut exe = vec![0u8; 0x3000];
        put16(&mut exe, 0x1006, 2);
        put16(&mut exe, 0x1008, 0x100);
        put16(&mut exe, 0x100A, 3);
        put16(&mut exe, 0x100C, 0x200);
        let mut cmn = vec![0u8; 16];
        put16(&mut cmn, 4, 1);
        put32(&mut cmn, 8, 8);
        put32(&mut cmn, 12, EXE_BASE + 0x1000);
        let mut anm = vec![0u8; 0x600 + script.len()];
        // header: 0 the clip block, 1 the state table, 2 the scripts' end, 9 the bone maps
        put32(&mut anm, 0, 0x40);
        put32(&mut anm, 4, 0x60);
        let end = anm.len() as u32;
        put32(&mut anm, 8, end);
        put32(&mut anm, 36, 0x500);
        put16(&mut anm, 0x44, 2);
        put32(&mut anm, 0x48, 8);
        put16(&mut anm, 0x60, 0x600);
        for k in 0..4 {
            put32(&mut anm, 0x500 + 4 * k, 0x520);
        }
        anm[0x520 + 5] = 18;
        anm[0x600..].copy_from_slice(script);
        (exe, cmn, anm)
    }

    #[test]
    fn clip_addresses_through_the_mirrors() {
        let mut cmn = vec![0u8; 32];
        for (k, w) in [(2, 0x0012_1000u32), (3, 0xA012_1000), (4, 0x8011_FF00), (5, 0x0092_1000)] {
            put32(&mut cmn, 8 + 4 * k, w);
        }
        assert_eq!(clip_address(&cmn, 2, 0x3000), Some(0x1800));
        assert_eq!(clip_address(&cmn, 3, 0x3000), Some(0x1800), "kseg1");
        assert_eq!(clip_address(&cmn, 3, 0x1800), None, "past the image");
        assert_eq!(clip_address(&cmn, 4, 0x3000), None, "below the load address");
        assert_eq!(clip_address(&cmn, 5, 0x3000), None, "past the first 8 MB");
        assert_eq!(clip_address(&cmn, 9, 0x3000), None, "past the file");
    }

    #[test]
    fn extent_of_the_runs() {
        let pose = 20 + 6 * 18 + 6;
        // the clip started (the own slot 1 is empty: CMN.ACT's word 1), then an own clip, then a jump to key 2
        let (exe, cmn, anm) = data(&[5, 0, 1, 0, 5, 0, 0, 0, 0x0C, 1, 2, 0]);
        let s = Scripts::of(&anm, &cmn, exe.len()).unwrap();
        assert_eq!(
            (s.clips.iter().copied().collect::<Vec<_>>(), s.keys.iter().copied().collect::<Vec<_>>(), s.max_slot),
            (vec![0x1000], vec![2], 18)
        );
        // key 1 → +0x100, key 2 → +0x200 (then the stop's entry 3 is read); the jump: key 2's duration at +0xE, keys 3
        // (→ +0) and 4
        assert_eq!(extents(&exe, &[s]).unwrap(), [(0x1000, 0x1200 + pose)]);
        // without the jump the same span: the key list's reads lie inside it
        let s = Scripts::of(&data(&[5, 0, 1, 0]).2, &cmn, exe.len()).unwrap();
        assert_eq!(extents(&exe, &[s]).unwrap(), [(0x1000, 0x1200 + pose)]);
        // no clip of the executable: an own clip only, or a gated start of an index CMN.ACT has
        let s = Scripts::of(&data(&[5, 0, 0, 0, 5, 0x10, 0, 0]).2, &cmn, exe.len()).unwrap();
        assert_eq!(extents(&exe, &[s]).unwrap(), []);
        // two clips: apart, two ranges (a clip with nothing pending reads its header and key 1's entry); overlapping, one
        let two = |b: usize| Scripts {
            clips: [0x1000, b].into(),
            keys: BTreeSet::new(),
            max_slot: 18,
            called_states: BTreeSet::new(),
            unknown_returns: BTreeSet::new(),
        };
        assert_eq!(extents(&exe, &[two(0x2000)]).unwrap(), [(0x1000, 0x1200 + pose), (0x2000, 0x200C)]);
        assert_eq!(extents(&exe, &[two(0x1100)]).unwrap(), [(0x1000, 0x1200 + pose)]);
        // a jump to key 3, past key 2's stop: key 3's duration (0: one step) runs key 4 (→ +0x400, the stop)
        let mut far = exe.clone();
        put16(&mut far, 0x1014, 0x400);
        let s = Scripts::of(&data(&[5, 0, 1, 0, 5, 0, 0, 0, 0x0C, 1, 3, 0]).2, &cmn, exe.len()).unwrap();
        assert_eq!(extents(&far, &[s]).unwrap(), [(0x1000, 0x1400 + pose)]);
        // keys that run off the image
        let mut long = exe.clone();
        for k in 1..0x7FE {
            put16(&mut long, 0x1004 + 4 * k + 2, 5);
        }
        let s = Scripts::of(&anm, &cmn, exe.len()).unwrap();
        assert!(extents(&long, &[s]).unwrap_err().contains("do not end"));
    }

    #[test]
    fn duration_ops_on_a_clip_of_the_executable_are_refused() {
        let (exe, cmn, _) = data(&[]);
        // after the executable's clip, sub 3 and sub 1
        for script in [[5, 0, 1, 0, 0x0C, 3, 0, 0], [5, 0, 1, 0, 0x0C, 1, 2, 0]] {
            assert!(Scripts::of(&data(&script).2, &cmn, exe.len()).unwrap_err().contains("0x0C"));
        }
        // at a state's start, before its first clip; a gated start of an own clip does not replace it
        assert!(Scripts::of(&data(&[0x0C, 3, 0, 0]).2, &cmn, exe.len()).is_err());
        assert!(Scripts::of(&data(&[5, 0, 1, 0, 5, 0x10, 0, 0, 0x0C, 3, 0, 0]).2, &cmn, exe.len()).is_err());
        assert!(Scripts::of(&data(&[5, 0, 0, 0, 0x0C, 3, 0, 0]).2, &cmn, exe.len()).is_ok());
        // the game's gate: bits 4..5 of 3 are always open (that start replaces the clip), bits 6..7 do not gate
        assert!(Scripts::of(&data(&[5, 0, 1, 0, 5, 0x30, 0, 0, 0x0C, 3, 0, 0]).2, &cmn, exe.len()).is_ok());
        assert!(Scripts::of(&data(&[5, 0, 1, 0, 5, 0x40, 0, 0, 0x0C, 3, 0, 0]).2, &cmn, exe.len()).is_ok());
        assert!(Scripts::of(&data(&[5, 0, 1, 0, 5, 0x20, 0, 0, 0x0C, 3, 0, 0]).2, &cmn, exe.len()).is_err());
        // an index past the own table's count starts nothing: the executable's clip plays on
        assert!(Scripts::of(&data(&[5, 0, 1, 0, 5, 0, 9, 0, 0x0C, 3, 0, 0]).2, &cmn, exe.len()).is_err());
        // the clip cycle over the own table (slot 1 empty: CMN.ACT's word 1, the executable's clip)
        let s = Scripts::of(&data(&[5, 0, 0xFF, 0xFF]).2, &cmn, exe.len()).unwrap();
        assert_eq!(s.clips.iter().copied().collect::<Vec<_>>(), [0x1000]);
        assert!(Scripts::of(&data(&[5, 0, 0xFF, 0xFF, 0x0C, 3, 0, 0]).2, &cmn, exe.len()).is_err());
        // an opcode past the table
        assert!(Scripts::of(&data(&[0x2A, 0]).2, &cmn, exe.len()).unwrap_err().contains("op 0x2a"));
    }

    /// A script whose returns to an unknown pc would fan out past `MAX_POINTS` (1500 loop marks with such a return ×
    /// 1500 calls' return pcs) is refused before the points are made.
    #[test]
    fn the_unknown_return_fan_out_is_bounded() {
        let (exe, cmn, _) = data(&[]);
        let n = 1500;
        let mut script = Vec::new();
        // a loop mark, then a return by the result (falls through too) with no call before it: an unknown return
        for _ in 0..n {
            script.extend([0x01, 1, 0x00, 3]);
        }
        // conditional calls (each a return pc) to a stop after them
        let stop = 0x600 + script.len() + 4 * n;
        for _ in 0..n {
            script.extend([0x04, 6, stop as u8, (stop >> 8) as u8]);
        }
        script.extend([0, 0]);
        let e = Scripts::of(&data(&script).2, &cmn, exe.len()).unwrap_err();
        assert!(e.contains("too large"), "{e}");
    }

    /// The flag follows the jumps, the calls' returns and the loop marks; code no path reaches is not looked at.
    #[test]
    fn duration_ops_by_the_control_flow() {
        let (exe, cmn, _) = data(&[]);
        let of = |script: &[u8]| Scripts::of(&data(script).2, &cmn, exe.len());
        // (the script starts at 0x600) at the state's start a jump over the duration op; a conditional jump to it
        // after the executable's clip; a jump past the executable's clip to a start of an own clip
        assert!(of(&[0x04, 0, 0x08, 0x06, 0x0C, 3, 0, 0, 0, 0]).is_ok());
        assert!(of(&[5, 0, 1, 0, 0x04, 3, 0x10, 0x06, 5, 0, 0, 0, 0, 0, 0, 0, 0x0C, 3, 0, 0, 0, 0]).is_err());
        assert!(of(&[5, 0, 1, 0, 0x04, 0, 0x10, 0x06, 5, 0, 0, 0, 0, 0, 0, 0, 5, 0, 0, 0, 0x0C, 3, 0, 0]).is_ok());
        // a call into a subroutine that starts no clip keeps the caller's own clip; one that starts the executable's
        // returns with it
        let call = |clip: u8| [5, 0, 0, 0, 0x04, 5, 0x10, 0x06, 0x0C, 3, 0, 0, 0, 0, 0, 0, 5, 0, clip, 0, 0, 2];
        assert!(of(&call(0)).is_ok());
        assert!(of(&call(1)).is_err());
        // a second state returning without a call: to the call's return pc with the flag set; not when its start is
        // the call's target (a subroutine the state table lists)
        let script = [5, 0, 0, 0, 0x04, 5, 0x10, 0x06, 0x0C, 3, 0, 0, 0, 0, 0, 0, 0, 2, 0, 2];
        for (start, ok) in [(0x612u16, false), (0x610, true)] {
            let mut anm = data(&script).2;
            put16(&mut anm, 0x62, start);
            let s = Scripts::of(&anm, &cmn, exe.len());
            assert_eq!(s.as_ref().is_ok(), ok);
            if let Ok(s) = s {
                assert_eq!(s.called_states.into_iter().collect::<Vec<_>>(), [1]);
            }
        }
        // the loop mark: back to after the executable's clip
        assert!(of(&[5, 0, 0, 0, 0x01, 1, 0x0C, 3, 0, 0, 5, 0, 1, 0, 0x01, 0]).is_err());
        assert!(of(&[5, 0, 1, 0, 5, 0, 0, 0, 0x01, 1, 0x0C, 3, 0, 0, 0x01, 0]).is_ok());
    }
}
