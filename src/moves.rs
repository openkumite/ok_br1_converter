//! Fighter move/state data: per-clip names, forms, loops, hand-shape swaps,
//! sound cues and root motion, from the state scripts in XX.ANM (== XX31.DAT,
//! the battle default) and the shared clip bank CMN/CMN.ACT.
//!
//! Game code: header parse 0x8017ceb4; script interpreter 0x801810f4 (handlers
//! 0x801a7388); state start 0x8017d508 (resets hands: objects 11/14 shown,
//! 15..18 hidden); op 0x05 starts a clip (0x8018262c), op 0x1e toggles object
//! visibility, ops 0x26/0x27 request sounds, op 0x1c commits root motion
//! (0x80182438). Sound id: bank = id >> 6 (CMN00, GAME00, stage XX10, fighter
//! XX00), record = id & 63 -> program = record / 16, tone = record % 16.

use std::collections::{BTreeMap, HashMap, HashSet};

pub const OPLEN: [usize; 0x2a] = [
    2, 2, 2, 2, 4, 4, 2, 6, 4, 6, 6, 2, 4, 4, 4, 4, // 0x00..0x0f
    4, 2, 2, 2, 4, 6, 2, 4, 6, 4, 4, 2, 2, 6, 4, 4, // 0x10..0x1f
    4, 6, 4, 4, 2, 6, 4, 6, 2, 4, // 0x20..0x29
];
/// Ops whose flag byte bits 4-5 restrict them to the human (1) or beast (2) form.
const GATED: [u8; 14] = [5, 7, 8, 9, 0x0f, 0x10, 0x14, 0x15, 0x1d, 0x1f, 0x24, 0x25, 0x26, 0x27];

/// Shared state numbering (same for every fighter).
fn generic_state(st: usize) -> Option<&'static str> {
    Some(match st {
        1 => "stance (idle)",
        2 => "walk forward",
        3 => "walk backward",
        4 => "dash forward (start)",
        5 => "run forward (loop)",
        6 => "run/dash stop",
        7 => "back dash",
        8 => "side step (one side)",
        9 => "side step (other side)",
        10 => "crouch (going down)",
        11 => "crouch (hold)",
        12 | 13 => "crouch walk forward",
        14 => "crouch walk backward",
        15 => "crouch turn-around",
        41 => "intro/round start?",
        42 | 43 => "win pose",
        44 => "win pose (form-specific)",
        45 => "time-over/lose?",
        46 => "transform / special sequence?",
        64 => "hit reaction standing A (light/med/heavy clips)",
        65 => "hit reaction standing B",
        66 => "hit reaction standing C",
        67 => "hit reaction crouching A",
        68 => "hit reaction crouching B",
        69 => "hit reaction pushed forward (from behind)",
        107 => "lying on ground (A)",
        108 => "lying on ground (B)",
        113 => "get up",
        115 => "ground roll forward",
        116 => "ground roll backward",
        117 | 118 => "ground roll sideways",
        119 => "get up (B)",
        121 => "ground roll forward (B)",
        122 => "ground roll backward (B)",
        123 | 124 => "ground roll sideways (B)",
        _ => return None,
    })
}

fn rd16(d: &[u8], o: usize) -> Option<u16> {
    d.get(o..o + 2).map(|b| u16::from_le_bytes([b[0], b[1]]))
}
fn rd32(d: &[u8], o: usize) -> Option<u32> {
    d.get(o..o + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()))
}

/// (total frames, key durations) of a clip at `a` (absolute offset).
fn clip_keys(d: &[u8], a: usize) -> Option<(u16, Vec<u16>)> {
    let total = rd16(d, a)?;
    let mut durs = Vec::new();
    let mut p = a + 4;
    loop {
        let dur = rd16(d, p + 2)?;
        durs.push(dur);
        p += 4;
        if dur == 0 || durs.len() > 512 {
            break;
        }
    }
    Some((total, durs))
}

/// Clip offsets of an animation block (table at block + 8, -1 terminated, 0 = empty).
pub fn clip_table(d: &[u8], block: usize, limit: usize) -> Vec<usize> {
    let mut out = Vec::new();
    while out.len() < limit {
        let Some(v) = rd32(d, block + 8 + 4 * out.len()) else { break };
        let v = v as i32;
        if v == -1 {
            break;
        }
        out.push(if v > 0 { block + v as usize } else { 0 });
    }
    out
}

#[derive(Clone, Debug)]
struct Command {
    from_player: bool,
    state: usize,
    notation: String,
}

fn notation(d: &[u8], mut o: usize) -> String {
    let mut parts: Vec<String> = Vec::new();
    while let (Some(t), Some(mode), Some(val)) = (rd16(d, o), rd16(d, o + 4), rd16(d, o + 8)) {
        if t == 0 || parts.len() > 12 {
            break;
        }
        match mode {
            1 | 3 | 6 => {
                let dir = match val & 0xf000 {
                    0x0000 => "5",
                    0x1000 => "8",
                    0x2000 => "6",
                    0x4000 => "2",
                    0x8000 => "4",
                    0x3000 => "9",
                    0x6000 => "3",
                    0xc000 => "1",
                    0x9000 => "7",
                    _ => "?",
                };
                parts.push(match mode {
                    3 => format!("[{dir}]"),
                    6 => format!("{{{dir}}}"),
                    _ => dir.to_string(),
                });
            }
            2 => {
                let b: Vec<&str> = [(0x80, "P"), (0x40, "K"), (0x20, "B")]
                    .iter()
                    .filter(|(m, _)| val & m != 0)
                    .map(|(_, n)| *n)
                    .collect();
                let b = if b.is_empty() { "?".to_string() } else { b.join("+") };
                match parts.last_mut() {
                    Some(l) => *l = format!("{l}+{b}"),
                    None => parts.push(b),
                }
            }
            _ => parts.push(format!("?m{mode}")),
        }
        o += 10;
    }
    parts.join(" ")
}

#[derive(Clone, Debug)]
enum Ev {
    Anim { form: u8, idx: i32, victim: bool, cur: (usize, usize) },
    Goto,
    Loop { cur: (usize, usize) },
    Hand { cur: Option<(usize, usize)>, t: f64, obj: u8, hidden: bool },
    Sfx { cur: Option<(usize, usize)>, t: f64, sub: u8, id: i16, delay: i16, delayed: bool },
}

/// Start time, clip index and key timing of each clip start (shared across states).
type CurInfo = HashMap<(usize, usize), (f64, i32, Option<(u16, Vec<u16>)>)>;

struct Script<'a> {
    d: &'a [u8],
    states: Vec<usize>,
    anims: Vec<usize>,
}

impl Script<'_> {
    fn op(&self, pc: usize) -> Option<(u8, u8, Vec<i16>, usize)> {
        let (&o, &f) = (self.d.get(pc)?, self.d.get(pc + 1)?);
        let len = *OPLEN.get(o as usize)?;
        let args = (0..(len - 2) / 2).map(|i| rd16(self.d, pc + 2 + 2 * i).map(|v| v as i16)).collect::<Option<_>>()?;
        Some((o, f, args, len))
    }

    fn keys(&self, idx: i32) -> Option<(u16, Vec<u16>)> {
        let a = *self.anims.get(usize::try_from(idx).ok()?)?;
        if a == 0 { None } else { clip_keys(self.d, a) }
    }

    /// Abstract execution of one state script over both forms (bitmask 1 = human, 2 = beast).
    fn walk(&self, st: usize, info: &mut CurInfo) -> Vec<Ev> {
        let start = self.states[st];
        if start == 0 {
            return vec![];
        }
        let mut ev = Vec::new();
        let mut seen = HashSet::new();
        type Item = (usize, u8, f64, Option<(usize, usize)>, Option<usize>);
        let mut work: Vec<Item> = vec![(start, 3, 0.0, None, None)];
        while let Some((mut pc, mut form, mut t, mut cur, mut label)) = work.pop() {
            if seen.len() >= 5000 {
                break;
            }
            for _ in 0..400 {
                if !seen.insert((pc, form, cur)) {
                    break;
                }
                let Some((o, f, a, len)) = self.op(pc) else { break };
                let npc = pc + len;
                let gate = if GATED.contains(&o) { (f >> 4) & 3 } else { 0 };
                let gform = form & [3, 1, 2, 3][gate as usize];
                match o {
                    4 => {
                        let tgt = a[0] as u16 as usize;
                        let (sel, cond) = (f & 0xc0, f & 0x3f);
                        let jf = match sel {
                            0x40 => form & 1,
                            0x80 => form & 2,
                            _ => form,
                        };
                        let split = |mask: u8, form: &mut u8, work: &mut Vec<Item>| {
                            if mask != 0 {
                                work.push((tgt, mask, t, cur, label));
                            }
                            *form &= !mask;
                        };
                        match cond {
                            0 => split(jf, &mut form, &mut work),
                            0x0a if sel != 0xc0 => split(jf & 1, &mut form, &mut work),
                            0x0b if sel != 0xc0 => split(jf & 2, &mut form, &mut work),
                            _ => {
                                if jf != 0 {
                                    work.push((tgt, jf, t, cur, label));
                                }
                            }
                        }
                        if form == 0 {
                            break;
                        }
                    }
                    0x29 => work.push((a[0] as u16 as usize, form, t, cur, label)),
                    1 => match f {
                        1 => label = Some(npc),
                        0 => {
                            if let (Some(_), Some(c)) = (label, cur) {
                                ev.push(Ev::Loop { cur: c });
                            }
                            break;
                        }
                        _ => {}
                    },
                    0 if f == 0 || f == 2 => break,
                    3 => t += f as f64,
                    2 => {
                        let n = f as i8;
                        if let Some((t0, _, Some((total, durs)))) = cur.and_then(|c| info.get(&c)) {
                            if n == -1 {
                                t = t.max(t0 + *total as f64);
                            } else if n >= 0 {
                                let upto = (n as usize).saturating_sub(1).min(durs.len());
                                t = t.max(t0 + durs[..upto].iter().map(|&d| d as f64).sum::<f64>());
                            }
                        }
                    }
                    5 if gform != 0 => match f & 0xf {
                        0 => {
                            let idx = a[0] as i32;
                            let c = (st, pc);
                            cur = Some(c);
                            let keys = if idx >= 0 { self.keys(idx) } else { None };
                            info.entry(c).or_insert((t, idx, keys));
                            ev.push(Ev::Anim { form: gform, idx, victim: f & 0x40 != 0, cur: c });
                        }
                        1 => {
                            ev.push(Ev::Goto);
                            if gform == form {
                                break;
                            }
                            form &= !gform;
                            if form == 0 {
                                break;
                            }
                        }
                        _ => {}
                    },
                    0x1e => ev.push(Ev::Hand { cur, t, obj: f, hidden: a[0] != 0 }),
                    0x26 if gform != 0 => ev.push(Ev::Sfx { cur, t, sub: f & 0xf, id: a[0], delay: 0, delayed: false }),
                    0x27 if gform != 0 && f & 0xf == 0 => {
                        ev.push(Ev::Sfx { cur, t, sub: 0, id: a[0], delay: a[1], delayed: true })
                    }
                    _ => {}
                }
                pc = npc;
            }
        }
        ev
    }
}

/// Where a sound id lives: (bank file stem, program, tone).
pub fn sfx_source(id: i16, fighter: &str) -> (String, usize, usize) {
    let id = id as u16 as usize;
    let k = id & 63;
    let bank = match id >> 6 {
        0 => "CMN00".to_string(),
        1 => "GAME00".to_string(),
        2 => format!("{fighter}10"), // stage SE bank (the fighter's own stage)
        _ => format!("{fighter}00"),
    };
    (bank, k / 16, k % 16)
}

#[derive(Clone, Debug, Default)]
pub struct Sfx {
    pub frame: f64,
    /// Fixed sound: (bank stem, program, tone); None for runtime-chosen sounds.
    pub sound: Option<(String, usize, usize)>,
    pub what: String,
}

#[derive(Clone, Debug, Default)]
pub struct ClipInfo {
    pub index: usize,
    /// From CMN/CMN.ACT (the fighter's own slot is empty).
    pub common: bool,
    pub total_frames: u16,
    pub human: bool,
    pub beast: bool,
    /// "script" when the state scripts restrict the clip to one form.
    pub form_source: &'static str,
    pub looped: bool,
    pub label: String,
    pub states: Vec<String>,
    /// (frame, object, visible) changes; objects 11/14 start shown, 15..18 hidden.
    pub hands: Vec<(f64, usize, bool)>,
    pub sfx: Vec<Sfx>,
    /// Per frame (dx, dy, dz, yaw): cumulative root motion in model units,
    /// facing frame at clip start; yaw 4096 = 360°.
    pub root: Vec<[f64; 4]>,
}

/// Root motion per frame: segment 0 ramps from zero (the previous clip's motion
/// was committed), later segments interpolate key i -> key i+1 linearly.
fn root_track(d: &[u8], a: usize) -> Vec<[f64; 4]> {
    let Some(total) = rd16(d, a) else { return vec![] };
    let mut ents = Vec::new();
    let mut p = a + 4;
    while let (Some(ko), Some(dur)) = (rd16(d, p), rd16(d, p + 2)) {
        p += 4;
        let k = a + ko as usize;
        let h = |o: usize| rd16(d, k + o).unwrap_or(0) as i16 as f64;
        ents.push((h(0), [h(2), h(4), h(6)], dur));
        if dur == 0 || ents.len() > 512 {
            break;
        }
    }
    (0..=total)
        .map(|fr| {
            let (mut t, mut i) = (fr as f64, 0);
            while i + 1 < ents.len() && t >= ents[i].2 as f64 {
                t -= ents[i].2 as f64;
                i += 1;
            }
            let (y, r) = if i + 1 >= ents.len() {
                let e = ents.last().unwrap();
                (e.0, e.1)
            } else {
                let (y0, r0) = if i == 0 { (0.0, [0.0; 3]) } else { (ents[i].0, ents[i].1) };
                let (y1, r1) = (ents[i + 1].0, ents[i + 1].1);
                let u = t / ents[i].2 as f64;
                (y0 + (y1 - y0) * u, [0, 1, 2].map(|j| r0[j] + (r1[j] - r0[j]) * u))
            };
            [r[0] / 16.0, r[1] / 16.0, r[2] / 16.0, y]
        })
        .collect()
}

/// Motion-based description when no state name is known.
fn describe(d: &[u8], a: usize) -> (String, f64) {
    let mut keys = Vec::new();
    let mut p = a + 4;
    while let (Some(ko), Some(dur)) = (rd16(d, p), rd16(d, p + 2)) {
        p += 4;
        let k = a + ko as usize;
        let h = |o: usize| rd16(d, k + o).unwrap_or(0) as i16 as f64;
        keys.push((h(0), [h(2), h(4), h(6)], h(10)));
        if dur == 0 || keys.len() > 512 {
            break;
        }
    }
    let Some(&(yaw, rm, _)) = keys.last() else { return ("in place".into(), 0.0) };
    let (fwd, side) = (-rm[2] / 16.0, rm[0] / 16.0);
    let t1y: Vec<f64> = keys.iter().map(|k| k.2).collect();
    let (lo, hi) = (t1y.iter().cloned().fold(f64::MAX, f64::min), t1y.iter().cloned().fold(f64::MIN, f64::max));
    let mut parts = Vec::new();
    if fwd.abs() > 20.0 {
        parts.push(format!("moves {} {:.0}u", if fwd > 0.0 { "forward" } else { "back" }, fwd.abs()));
    }
    if side.abs() > 20.0 {
        parts.push(format!("moves sideways {side:+.0}u"));
    }
    if yaw.abs() >= 512.0 {
        parts.push(format!("turns {} deg", (yaw * 360.0 / 4096.0).round()));
    }
    if lo > 1100.0 {
        parts.push("low posture (crouch/down)".into());
    } else if hi - lo > 900.0 {
        parts.push("large height change".into());
    }
    (if parts.is_empty() { "in place".into() } else { parts.join(", ") }, yaw)
}

/// Per-clip records for a fighter. `anm` is XX.ANM (== XX31.DAT), `cmn` is CMN/CMN.ACT.
pub fn clip_info(fighter: &str, anm: &[u8], cmn: Option<&[u8]>) -> Vec<ClipInfo> {
    let hdr: Vec<usize> = (0..5).map(|i| rd32(anm, 4 * i).unwrap_or(0) as usize).collect();
    let nstates = hdr[3].saturating_sub(hdr[1]) / 2;
    let states = (0..nstates.min(1024)).map(|i| rd16(anm, hdr[1] + 2 * i).unwrap_or(0) as usize).collect();
    let anims = clip_table(anm, hdr[0], 4096);
    let cmn_anims = cmn.map(|c| clip_table(c, 0, 127)).unwrap_or_default();
    let script = Script { d: anm, states, anims };

    // Command table (header u32[3]..u32[4]): rows of 16 bytes, player rows have b == 0.
    let mut commands = Vec::new();
    let mut o = hdr[3];
    while o + 16 <= hdr[4] {
        let (Some(b), Some(st), Some(seq)) = (rd16(anm, o + 2), rd16(anm, o + 4), rd32(anm, o + 12)) else { break };
        if st == 0 {
            break;
        }
        if seq != 0 && (seq as usize) < anm.len() {
            commands.push(Command { from_player: b == 0, state: st as usize, notation: notation(anm, seq as usize) });
        }
        o += 16;
    }
    let state_name = |st: usize| -> String {
        let cmds: Vec<&str> =
            commands.iter().filter(|c| c.from_player && c.state + 0x80 == st).map(|c| c.notation.as_str()).collect();
        if !cmds.is_empty() {
            return format!("state {st} [{}]", cmds.join(" | "));
        }
        match generic_state(st) {
            Some(g) => format!("state {st} {g}"),
            None => format!("state {st}"),
        }
    };

    #[derive(Default)]
    struct Rec {
        forms: u8,
        names: Vec<String>,
        primary: Vec<String>,
        state_ids: Vec<usize>,
        looped: bool,
        victim: bool,
        hands: Vec<(f64, u8, bool, usize)>,
        sfx: Vec<Sfx>,
    }
    let mut recs: BTreeMap<usize, Rec> = BTreeMap::new();
    let mut info = CurInfo::new();
    for st in 0..script.states.len() {
        let evs = script.walk(st, &mut info);
        let mut seq2idx: HashMap<(usize, usize), i32> = HashMap::new();
        let mut first = true;
        for e in &evs {
            if let Ev::Anim { form, idx, victim, cur, .. } = e {
                seq2idx.insert(*cur, *idx);
                let Ok(i) = usize::try_from(*idx) else { continue };
                let r = recs.entry(i).or_default();
                r.victim |= *victim;
                r.forms |= form;
                let nm = state_name(st);
                r.state_ids.push(st);
                if first {
                    r.primary.push(nm.clone());
                }
                first = false;
                if !r.names.contains(&nm) {
                    r.names.push(nm);
                }
            }
        }
        // Hand ops before the first clip of a state apply from frame 0 of the next clip.
        let mut pend = Vec::new();
        let mut evs2 = Vec::new();
        for e in evs {
            match &e {
                Ev::Hand { cur: None, .. } => {
                    pend.push(e);
                    continue;
                }
                Ev::Anim { cur, .. } => {
                    let t0 = info[cur].0;
                    let c = *cur;
                    evs2.push(e);
                    for h in pend.drain(..) {
                        if let Ev::Hand { obj, hidden, .. } = h {
                            evs2.push(Ev::Hand { cur: Some(c), t: t0, obj, hidden });
                        }
                    }
                }
                _ => evs2.push(e),
            }
        }
        for e in evs2 {
            let clip_of = |cur: &Option<(usize, usize)>| {
                cur.and_then(|c| seq2idx.get(&c).copied()).and_then(|i| usize::try_from(i).ok()).zip(*cur)
            };
            match e {
                Ev::Loop { cur } => {
                    if let Some((i, _)) = clip_of(&Some(cur)) {
                        recs.entry(i).or_default().looped = true;
                    }
                }
                Ev::Hand { cur, t, obj, hidden } => {
                    if let Some((i, c)) = clip_of(&cur) {
                        let h = (t - info[&c].0, obj, hidden, st);
                        let r = recs.entry(i).or_default();
                        if !r.hands.contains(&h) {
                            r.hands.push(h);
                        }
                    }
                }
                Ev::Sfx { cur, t, sub, id, delay, delayed } => {
                    let Some((i, c)) = clip_of(&cur) else { continue };
                    let frame = t - info[&c].0 + delay as f64;
                    let s = match (delayed, sub) {
                        (true, _) | (false, 0) => Sfx {
                            frame,
                            sound: Some(sfx_source(id, fighter)),
                            what: if delayed { "play (delayed)".into() } else { "play".into() },
                        },
                        (false, 1) => Sfx { frame, sound: None, what: "hit impact (chosen by victim)".into() },
                        (false, 5) => Sfx { frame, sound: None, what: "random swing whoosh (GAME00)".into() },
                        (false, 6) => Sfx { frame, sound: None, what: "character voice".into() },
                        _ => continue,
                    };
                    let r = recs.entry(i).or_default();
                    if !r.sfx.iter().any(|x| x.frame == s.frame && x.sound == s.sound && x.what == s.what) {
                        r.sfx.push(s);
                    }
                }
                _ => {}
            }
        }
    }

    let mut out = Vec::new();
    let max_idx = script.anims.len().max(cmn_anims.len());
    for i in 0..max_idx {
        let own = script.anims.get(i).copied().filter(|&a| a != 0);
        let shared = if own.is_none() && i < 0x100 { cmn_anims.get(i).copied().filter(|&a| a != 0) } else { None };
        let (d, a, common) = match (own, shared, cmn) {
            (Some(a), _, _) => (anm, a, false),
            (None, Some(a), Some(c)) => (c, a, true),
            _ => continue,
        };
        let r = recs.remove(&i).unwrap_or_default();
        if r.victim || i >= 0x100 {
            continue; // throw-victim clips are played on the opponent's skeleton
        }
        let Some((total, _)) = clip_keys(d, a) else { continue };
        let (motion, yaw_end) = describe(d, a);
        // Labels: first state that starts with this clip, command notation, or motion.
        let label = if yaw_end.abs() >= 1024.0 && !r.names.iter().any(|n| n.contains('[')) {
            format!("turn around ({} deg)", (yaw_end * 360.0 / 4096.0).round())
        } else {
            let named = r.primary.iter().chain(&r.names).find_map(|n| {
                if let Some(p) = n.find('[') {
                    return Some(format!("command {}", &n[p + 1..n.len() - 1]));
                }
                let parts: Vec<&str> = n.splitn(3, ' ').collect();
                (parts.len() == 3).then(|| parts[2].to_string())
            });
            named.unwrap_or_else(|| {
                let base = if r.state_ids.iter().any(|&s| s >= 128) {
                    "move"
                } else if common {
                    "common/reaction"
                } else {
                    "action"
                };
                format!("{base} ({motion})")
            })
        };
        let (mut human, mut beast) = (r.forms & 1 != 0, r.forms & 2 != 0);
        let form_source = if human != beast { "script" } else { "heuristic" };
        if form_source == "heuristic" {
            (human, beast) = match i {
                200..=255 => (false, true),
                144..=199 if !common => (true, false),
                _ => (true, true),
            };
        }
        // Hand timeline from the first state that toggles hands in this clip.
        let hands = match r.hands.first() {
            None => vec![],
            Some(&(_, _, _, st0)) => {
                let mut v: Vec<(f64, usize, bool)> = Vec::new();
                let mut hs: Vec<_> = r.hands.iter().filter(|h| h.3 == st0).collect();
                hs.sort_by(|a, b| a.0.total_cmp(&b.0));
                for &(fr, obj, hidden, _) in hs {
                    let objs: Vec<usize> = if obj == 0x80 { (0..26).collect() } else { vec![obj as usize] };
                    v.extend(objs.into_iter().map(|o| (fr, o, !hidden)));
                }
                v
            }
        };
        out.push(ClipInfo {
            index: i,
            common,
            total_frames: total,
            human,
            beast,
            form_source,
            looped: r.looped,
            label,
            states: r.names,
            hands,
            sfx: r.sfx,
            root: root_track(d, a),
        });
    }
    out
}
