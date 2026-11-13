//! Particle effects (hit sparks, blood, smoke, debris, flares): port of the
//! game's particle system, verified against the original code in an emulator.
//!
//! Tables in SCUS_941.99: particle types 0x801a523c (23 x 20 bytes: frame UV
//! list, per-frame hold ticks, frame count, base size, primitive code, blend),
//! CLUT positions 0x801a4164, texture pages 0x801a4140/0x801a4158, hit-spark
//! presets 0x801a3db8 (24 x 12 bytes). Spawn 0x8013f170, per-tick update and
//! draw 0x8013be10, death children 0x8013b7e4, hit-spark emitter 0x80139f24.
//! The camera is a stand-in (identity rotation, eye at z = -dist).

use crate::bail;
use crate::error::Result;
use crate::tim::Image;
use crate::vram::Vram;
use serde_json::{Value, json};

const BASE: u32 = 0x8012_0000;

struct Exe<'a>(&'a [u8]);

impl Exe<'_> {
    fn off(&self, a: u32) -> usize {
        (a - BASE + 0x800) as usize
    }
    fn u8(&self, a: u32) -> i64 {
        self.0.get(self.off(a)).copied().unwrap_or(0) as i64
    }
    fn u16(&self, a: u32) -> i64 {
        let o = self.off(a);
        self.0.get(o..o + 2).map_or(0, |b| u16::from_le_bytes([b[0], b[1]]) as i64)
    }
    fn u32(&self, a: u32) -> u32 {
        let o = self.off(a);
        self.0.get(o..o + 4).map_or(0, |b| u32::from_le_bytes(b.try_into().unwrap()))
    }
}

fn s16(v: i64) -> i64 {
    v as i16 as i64
}
fn s32(v: i64) -> i64 {
    v as i32 as i64
}
fn s8(v: i64) -> i64 {
    v as i8 as i64
}
/// C-style division (truncates toward zero).
fn cdiv(a: i64, b: i64) -> i64 {
    if b == 0 { 0 } else { a / b }
}
fn cmod(a: i64, b: i64) -> i64 {
    if b == 0 { 0 } else { a % b }
}

#[derive(Clone)]
pub struct ParticleType {
    pub frames: Vec<[(u8, u8); 4]>,
    pub durations: Vec<u8>,
    dur_addr: u32,
    pub size: i64,
    pub tpage_x: usize,
    pub tpage_y: usize,
    pub abr: u8,
    pub semi: bool,
    pub raw: bool,
}

#[derive(Clone, Copy)]
pub struct SparkPreset {
    ang_y: i64,
    ang_x: i64,
    dang_y: i64,
    speed: i64,
    dspeed: i64,
    total: i64,
    frames: i64,
    blood_size: i64,
    nograv: bool,
    bright: i64,
    raw: [u8; 12],
}

pub struct Tables {
    exe: Vec<u8>,
    pub cluts: Vec<(usize, usize)>,
    pub types: Vec<ParticleType>,
    pub sparks: Vec<SparkPreset>,
    /// (particle type, CLUT index) for hit sparks without / with blood.
    spark_part: [(usize, usize); 2],
}

impl Tables {
    pub fn from_exe(exe: &[u8]) -> Result<Tables> {
        if exe.len() < 0x90000 || &exe[0..8] != b"PS-X EXE" {
            bail!("not the game executable");
        }
        let e = Exe(exe);
        let cluts =
            (0..17).map(|i| (e.u16(0x801a_4164 + 4 * i) as usize, e.u16(0x801a_4166 + 4 * i) as usize)).collect();
        let tpage_val: Vec<i64> = (0..5).map(|i| e.u16(0x801a_4158 + 2 * i)).collect();
        let mut types = Vec::new();
        for t in 0..23u32 {
            let a = 0x801a_523c + 20 * t;
            let (uvp, dp, n) = (e.u32(a), e.u32(a + 4), e.u8(a + 8) as u32);
            let tp = tpage_val.get(e.u8(0x801a_4140 + t) as usize).copied().unwrap_or(0) + (e.u8(a + 0xc) << 5);
            let mut code = e.u8(a + 0xb);
            if matches!(t, 4 | 14 | 16 | 21) {
                code &= 0xfe; // colour-modulated, not raw
            }
            types.push(ParticleType {
                frames: (0..n)
                    .map(|k| {
                        std::array::from_fn(|j| {
                            (e.u8(uvp + 8 * k + 2 * j as u32) as u8, e.u8(uvp + 8 * k + 2 * j as u32 + 1) as u8)
                        })
                    })
                    .collect(),
                durations: (0..n).map(|k| e.u8(dp + k) as u8).collect(),
                dur_addr: dp,
                size: e.u8(a + 0xa),
                tpage_x: (tp & 0xf) as usize * 64,
                tpage_y: ((tp >> 4) & 1) as usize * 256,
                abr: ((tp >> 5) & 3) as u8,
                semi: code & 2 != 0,
                raw: code & 1 != 0,
            });
        }
        let sparks = (0..24u32)
            .map(|i| {
                let o = e.off(0x801a_3db8) + 12 * i as usize;
                let r: [u8; 12] = exe[o..o + 12].try_into().unwrap();
                SparkPreset {
                    ang_y: (r[0] as i64) << 4,
                    ang_x: (r[1] as i64) << 4,
                    dang_y: s8(r[2] as i64),
                    speed: r[4] as i64,
                    dspeed: s8(r[5] as i64),
                    total: r[6] as i64,
                    frames: r[7] as i64,
                    blood_size: r[9] as i64,
                    nograv: r[10] != 0,
                    bright: r[11] as i64,
                    raw: r,
                }
            })
            .collect();
        let sp = |a: u32| (e.u8(a) as usize, e.u8(a + 1) as usize);
        Ok(Tables { cluts, types, sparks, spark_part: [sp(0x8012_18a0), sp(0x8012_18a2)], exe: exe.to_vec() })
    }

    fn e(&self) -> Exe<'_> {
        Exe(&self.exe)
    }
    /// 0x8018c9f8
    fn rsin(&self, a: i64) -> i64 {
        let mut v = (a & 0x3ff) << 1;
        if a & 0x400 != 0 {
            v = 0x800 - v;
        }
        let r = s16(self.e().u16(0x8012_6b78 + v as u32));
        if a & 0x800 != 0 { -r } else { r }
    }
    fn rcos(&self, a: i64) -> i64 {
        self.rsin(a + 0x400)
    }
}

/// libc rand (0x80190604) plus the game's byte table rand (0x8012cd80).
struct Rand {
    seed: u32,
    tidx: u32,
}
impl Rand {
    fn rand(&mut self) -> i64 {
        self.seed = self.seed.wrapping_mul(0x41c6_4e6d).wrapping_add(0x3039);
        ((self.seed >> 16) & 0x7fff) as i64
    }
    fn trand(&mut self, t: &Tables) -> i64 {
        let v = t.e().u8(0x8012_0448 + self.tidx);
        self.tidx = (self.tidx + 1) & 0xff;
        v
    }
    fn sym(&mut self, n: i64) -> i64 {
        n - cmod(self.rand(), 2 * n)
    }
}

/// Stand-in for the GTE camera: projection distance `h`, eye at z = -dist.
#[derive(Clone, Copy)]
pub struct Camera {
    pub h: i64,
    pub dist: i64,
    pub cx: i64,
    pub cy: i64,
    pub pitch: i64,
}
impl Camera {
    fn rtp(&self, x: i64, y: i64, z: i64) -> (i64, i64, i64) {
        let sz = (z + self.dist).max(1);
        (self.cx + cdiv(x * self.h, sz), self.cy + cdiv(y * self.h, sz), sz >> 2)
    }
}

#[derive(Clone)]
struct Particle {
    ty: usize,
    mode: u8,
    p: [i64; 3],
    pos: [i64; 3],
    vel: [i64; 3],
    frame: usize,
    nleft: i64,
    dcount: i64,
    size: i64,
    b3a: i64,
    b3c: i64,
    b3e: i64,
    b40: i64,
    damp: i64,
    grav: i64,
    clut: usize,
    rgb: [i64; 3],
    dead: bool,
}

impl Particle {
    fn new(t: &Tables, ty: usize, mode: u8, pos: [i64; 3], vel: [i64; 3], clut: usize) -> Particle {
        let tt = &t.types[ty];
        Particle {
            ty,
            mode,
            p: pos,
            pos: pos.map(|v| v << 12),
            vel: vel.map(|v| v << 8),
            frame: 0,
            nleft: tt.frames.len() as i64,
            dcount: tt.durations.first().copied().unwrap_or(1) as i64,
            size: tt.size,
            b3a: 0,
            b3c: if ty == 16 { 0x2d } else { 0 },
            b3e: 0,
            b40: 0,
            damp: if !(12..14).contains(&ty) { 0xe66 } else { 0xee1 },
            grav: match ty {
                0..=7 => 0x880,
                8 | 9 => 0,
                10 | 12 => 0x1980,
                _ => 0x880,
            },
            clut,
            rgb: [0xff; 3],
            dead: false,
        }
    }
}

/// One textured quad to draw (POLY_FT4 semantics).
#[derive(Clone)]
pub struct DrawCmd {
    pub uv: [(u8, u8); 4],
    /// Screen corners in primitive order TL, TR, BL, BR.
    pub quad: [(i64, i64); 4],
    pub otz: i64,
    pub clut: (usize, usize),
    pub tpage: (usize, usize),
    pub abr: u8,
    pub semi: bool,
    pub raw: bool,
    pub rgb: [i64; 3],
}

fn sq(cx: i64, cy: i64, w: i64, h: i64) -> [(i64, i64); 4] {
    let (x0, y0) = (cx - cdiv(w, 2), cy - cdiv(h, 2));
    [(x0, y0), (x0 + w, y0), (x0, y0 + h), (x0 + w, y0 + h)]
}

/// One game tick for one particle (0x8013be10).
fn step(t: &Tables, pt: &mut Particle, cam: &Camera, rnd: &mut Rand) -> Option<DrawCmd> {
    let tt = &t.types[pt.ty];
    let n = tt.frames.len();
    let uv = tt.frames[pt.frame.min(n.saturating_sub(1))];
    pt.dcount = (pt.dcount - 1) & 0xff;
    if pt.dcount == 0 {
        pt.nleft -= 1;
        pt.frame += 1;
        pt.dcount = tt.durations.get(pt.frame).map_or_else(|| t.e().u8(tt.dur_addr + pt.frame as u32), |&d| d as i64);
    }
    match pt.mode {
        1 | 2 | 3 | 6 | 7 | 9 => {
            pt.vel[0] = s32(pt.vel[0] * pt.damp) >> 12;
            pt.vel[2] = s32(pt.vel[2] * pt.damp) >> 12;
            pt.vel[1] = (s32(pt.vel[1] * pt.damp) >> 12) + pt.grav;
            for k in 0..3 {
                pt.pos[k] = s32(pt.pos[k] + pt.vel[k]);
            }
        }
        4 | 5 => {
            pt.b3c = s16(pt.b3c - 3).max(0);
            pt.rgb = [pt.b3c & 0xff; 3];
        }
        8 => {
            pt.pos[0] = s32(pt.vel[0] + pt.p[1] * t.rcos(pt.p[0]));
            pt.pos[2] = s32(pt.vel[2] + pt.p[1] * t.rsin(pt.p[0]));
            pt.p[1] = s16(pt.p[1] + 6);
            pt.p[0] = s16(pt.p[0] + pt.p[2]);
            pt.p[2] = s16((pt.p[2] * pt.damp) >> 12);
        }
        _ => {}
    }
    match pt.ty {
        5 => pt.size = s16(pt.size + 2),
        12 => {
            pt.size = s16(pt.size - 1).max(0);
            if pt.pos[1] > 0 {
                pt.vel[1] = -pt.vel[1];
            }
        }
        13 => {
            if pt.pos[1] > 0 {
                pt.vel[1] = -pt.vel[1];
            }
        }
        14 => {
            let n = pt.nleft;
            pt.size = (if 14 - n >= 0 { n + 2 } else { 30 - n }) * 8 + 0x40;
        }
        15 | 17 => {
            if pt.ty == 17 {
                pt.size = s16(pt.size - 1);
            }
            if pt.pos[1] > 0 {
                pt.vel[1] = -pt.vel[1];
            }
            pt.clut = 2 + 2 * (rnd.trand(t) & 1) as usize;
        }
        16 => {
            pt.b3c = s16(pt.b3c + 2);
            pt.size = s16(pt.size + 60);
        }
        21 => {
            pt.vel[0] = (pt.vel[0] - 2).max(0);
            pt.rgb = [pt.vel[0] & 0xff; 3];
        }
        _ => {}
    }
    let pos = pt.pos.map(|v| s16(v >> 12));
    if pt.nleft <= 0 {
        pt.dead = true;
        return None;
    }
    let size = pt.size;
    let project = |zoff: i64, p: [i64; 3]| {
        let (sx, sy, otz) = cam.rtp(p[0], p[1], p[2]);
        let s = if otz != 0 { cdiv(size * cam.h, otz * 4) } else { 0 };
        (sx, sy, s, otz - zoff)
    };
    let visible = |z: i64| (0x33..0x800).contains(&z);
    let (quad, otz);
    match pt.mode {
        0 | 1 | 7 | 8 | 9 => {
            if pt.mode == 7 && pos[1] > 0 {
                pt.dead = true;
                return None;
            }
            let (sx, sy, s, mut z) = project(30, pos);
            if !visible(z) {
                return None;
            }
            let mut w = s;
            if pt.mode == 9 {
                pt.b3c = s16(pt.b3c + 1);
                if pt.b3c & 1 != 0 {
                    w = -s;
                }
            }
            if pt.mode != 9 && pt.ty == 0 {
                z = 10;
            }
            quad = sq(sx, sy, w, s);
            otz = z;
        }
        2 => {
            let d = ((0..3).map(|k| (pt.p[k] - pos[k]).pow(2)).sum::<i64>() as f64).sqrt() as i64;
            if pt.b3a < d || pt.nleft == 0 {
                pt.dead = true;
                return None;
            }
            pt.b3a = d;
            let (sx, sy, s, z) = project(50, pos);
            if !visible(z) {
                return None;
            }
            quad = sq(sx, sy, s, s);
            otz = z;
        }
        3 | 6 if pos[1] > 0 => {
            pt.dead = true;
            return None;
        }
        3 => {
            let (sx, sy, s, z) = project(0, pos);
            if !visible(z) {
                return None;
            }
            quad = sq(sx, sy, s, s);
            otz = z;
        }
        4 => {
            let (sx, sy, mut s, mut z) = project(0, pos);
            if !visible(z) {
                return None;
            }
            if pt.ty == 16 {
                z = 0x7fc;
                s = cdiv(pt.size * cam.h, z * 4);
            }
            let h = (s * t.rsin(cam.pitch + 0xa0)) >> 12;
            quad = sq(sx, sy, s, h);
            otz = z;
        }
        5 => {
            let (sx, sy, z) = cam.rtp(pos[0], pos[1], pos[2]);
            let (sx2, sy2, _) = cam.rtp(pt.p[0], pt.p[1], pt.p[2]);
            if !visible(z) || sx2 - sx <= 0 {
                return None;
            }
            quad = sq(sx, sy, sx2 - sx, sy2 - sy);
            otz = z;
        }
        6 => {
            let (sx, sy, o) = cam.rtp(pos[0], pos[1], pos[2]);
            let z = o - 5;
            if !visible(z) {
                return None;
            }
            pt.grav += 0xc0;
            pt.size = s16(pt.size + 1);
            let s = cdiv(pt.size * cam.h, z * 4);
            quad = sq(sx, sy, s, s);
            otz = z;
        }
        13 => {
            let (sx, sy, s, z) = project(30, pos);
            if !visible(z) {
                return None;
            }
            pt.b3c = s16(pt.b3c + pt.b3e);
            pt.b3e = s16((pt.b3e * 475) >> 9);
            pt.b40 = s16(pt.b40 + 0x80);
            pt.size = s16(pt.b40) >> 8;
            pt.pos[1] -= 0x1800;
            let q = sq(sx, sy, s, s);
            let (a, b) = ((s * t.rcos(pt.b3c)) >> 14, (s * t.rsin(pt.b3c)) >> 14);
            quad = [
                (q[0].0 - a, q[0].1 + b),
                (q[1].0 + a, q[1].1 + b),
                (q[2].0 - a, q[2].1 - b),
                (q[3].0 + a, q[3].1 - b),
            ];
            otz = z;
        }
        14 => {
            pt.b3a = s16(pt.b3a - pt.b3c);
            pt.b3c = s16((pt.b3c * 1825) >> 11);
            if pt.b3a < 0 {
                pt.dead = true;
                return None;
            }
            let (ax, ay, az) = (s16(pt.vel[0]), s16(pt.vel[0] >> 16), s16(pt.vel[1]));
            let p2 = [
                pt.p[0] + ((pt.b3a * t.rcos(ax)) >> 12),
                pt.p[1] + ((pt.b3a * t.rsin(ay)) >> 12),
                pt.p[2] + ((pt.b3a * t.rcos(az)) >> 12),
            ];
            let (sx, sy, s, z) = project(50, p2);
            if !visible(z) {
                return None;
            }
            quad = sq(sx, sy, s, s);
            otz = z;
        }
        _ => return None, // modes 10..12 follow fighter bones (not modelled)
    }
    Some(DrawCmd {
        uv,
        quad,
        otz,
        clut: t.cluts[pt.clut.min(t.cluts.len() - 1)],
        tpage: (tt.tpage_x, tt.tpage_y),
        abr: tt.abr,
        semi: tt.semi,
        raw: tt.raw,
        rgb: pt.rgb,
    })
}

/// Children spawned when a particle dies (0x8013b7e4, inside-arena branches).
fn kill_children(t: &Tables, pt: &Particle, rnd: &mut Rand) -> Vec<Particle> {
    let (x, z) = (s16(pt.pos[0] >> 12), s16(pt.pos[2] >> 12));
    let mut out = Vec::new();
    if pt.ty == 5 && rnd.rand() & 1 == 0 {
        let mut c = Particle::new(t, 4, 4, [x, 0, z], [0; 3], 16);
        c.grav = 0;
        c.size = s16(pt.b3c + rnd.sym(0x20));
        c.b3c = 0xff;
        out.push(c);
    } else if pt.ty == 19 || (pt.ty == 10 && pt.mode != 6 && rnd.rand() & 1 == 0) {
        let mut c = Particle::new(t, 11, 4, [x, 0, z], [0; 3], if pt.ty == 19 { 6 } else { 5 });
        c.grav = 0;
        c.size = 0x50;
        out.push(c);
    }
    out
}

pub fn default_clut(ty: usize) -> usize {
    match ty {
        4 => 16,
        16 => 13,
        5 => 1,
        6 => 2,
        11 => 5,
        12 => 11,
        13 => 12,
        14 => 13,
        20 => 14,
        _ => 0,
    }
}

/// Initial-state overrides for a single particle (fields the spawner sets).
#[derive(Default, Clone, Copy)]
pub struct Extra {
    pub b3c: Option<i64>,
    pub b3e: Option<i64>,
    pub b40: Option<i64>,
    pub grav: Option<i64>,
    pub size: Option<i64>,
}

/// Per-tick draw commands of one particle until it dies.
pub fn particle_frames(
    t: &Tables,
    ty: usize,
    mode: u8,
    pos: [i64; 3],
    vel: [i64; 3],
    extra: Extra,
    cam: &Camera,
) -> Vec<Vec<DrawCmd>> {
    let mut pt = Particle::new(t, ty, mode, pos, vel, default_clut(ty));
    if let Some(v) = extra.b3c {
        pt.b3c = v;
    }
    if let Some(v) = extra.b3e {
        pt.b3e = v;
    }
    if let Some(v) = extra.b40 {
        pt.b40 = v;
    }
    if let Some(v) = extra.grav {
        pt.grav = v;
    }
    if let Some(v) = extra.size {
        pt.size = v;
    }
    let mut rnd = Rand { seed: 1, tidx: 0 };
    let mut out = Vec::new();
    while !pt.dead && out.len() < 400 {
        out.push(step(t, &mut pt, cam, &mut rnd).into_iter().collect());
    }
    out
}

/// Hit-spark burst (0x8014d370 -> emitter kind 3, preset table 0x801a3db8).
pub fn spark_frames(t: &Tables, preset: usize, blood: bool, cam: &Camera, origin: [i64; 3]) -> Vec<Vec<DrawCmd>> {
    let s = t.sparks[preset];
    let (ty, clut) = t.spark_part[blood as usize];
    let mut rnd = Rand { seed: 1, tidx: 0 };
    let (mut angy, angx, mut speed) = (s.ang_y + 0x400, s.ang_x, s.speed);
    let per = if s.frames > 0 { (s.total / s.frames).max(1) } else { 0 };
    let mut parts: Vec<Particle> = Vec::new();
    let mut out = Vec::new();
    for tick in 0..200 {
        if tick < s.frames {
            if tick & 3 == 0 {
                speed += s.dspeed;
            }
            speed = speed.max(2);
            angy = s16(angy + s.dang_y);
            let (s4, s0, s5) = (speed * 20, speed * 8 + 0x60, speed * 4 + 0x30);
            for _ in 0..per {
                let j = s5 - cmod(rnd.rand(), s0);
                let vx = cdiv(s4 * t.rcos(angy + j), 4096);
                let vz = cdiv(s4 * t.rsin(angy + j), 4096);
                let j = s5 - cmod(rnd.rand(), s0);
                let vy = cdiv(s4 * t.rcos(angx + j), 4096);
                let f = 0x400 + (rnd.rand() & 0xbff);
                let mut p = Particle::new(t, ty, 3, origin, [(vx * f) >> 11, (vy * f) >> 11, (vz * f) >> 11], clut);
                if blood {
                    p.size = s.blood_size;
                }
                p.b3c = s.bright;
                if s.nograv {
                    p.grav = 0;
                    p.damp = 0xdac;
                }
                parts.push(p);
            }
        }
        // Children born this tick are stepped this tick too (the game appends
        // them to the live list it is walking).
        let mut cmds = Vec::new();
        let mut i = 0;
        while i < parts.len() {
            if !parts[i].dead {
                let c = step(t, &mut parts[i], cam, &mut rnd);
                if parts[i].dead {
                    let born = kill_children(t, &parts[i], &mut rnd);
                    parts.extend(born);
                } else if let Some(c) = c {
                    cmds.push(c);
                }
            }
            i += 1;
        }
        out.push(cmds);
        if tick >= s.frames && parts.iter().all(|p| p.dead) {
            break;
        }
    }
    out
}

/// Software rasteriser for one POLY_FT4 (affine UVs, 4 bpp CLUT, STP + blend modes)
/// into an RGB framebuffer over black; `cov` marks pixels that were drawn.
fn draw(fb: &mut [u8], cov: &mut [bool], cell: i64, vram: &Vram, c: &DrawCmd, (ox, oy): (i64, i64)) {
    let (w, h) = (cell, cell);
    let q = c.quad.map(|(x, y)| (x + ox, y + oy));
    let (tpx, tpy) = c.tpage;
    let (cx, cy) = c.clut;
    for tri in [[0, 1, 2], [1, 3, 2]] {
        let [(x0, y0), (x1, y1), (x2, y2)] = tri.map(|i| q[i]);
        let den = ((x1 - x0) * (y2 - y0) - (x2 - x0) * (y1 - y0)) as f64;
        if den == 0.0 {
            continue;
        }
        for py in y0.min(y1).min(y2).max(0)..y0.max(y1).max(y2).min(h) {
            for px in x0.min(x1).min(x2).max(0)..x0.max(x1).max(x2).min(w) {
                let a = ((x1 - px) * (y2 - py) - (x2 - px) * (y1 - py)) as f64 / den;
                let b = ((x2 - px) * (y0 - py) - (x0 - px) * (y2 - py)) as f64 / den;
                let cc = 1.0 - a - b;
                if a.min(b).min(cc) < -1e-9 {
                    continue;
                }
                let uvc = |k: usize| -> f64 {
                    let f = |i: usize| if k == 0 { c.uv[tri[i]].0 as f64 } else { c.uv[tri[i]].1 as f64 };
                    a * f(0) + b * f(1) + cc * f(2)
                };
                let (u, v) = ((uvc(0) as i64 & 0xff) as usize, (uvc(1) as i64 & 0xff) as usize);
                let word = vram.data[((tpy + v) & 511) * 1024 + ((tpx + (u >> 2)) & 1023)];
                let idx = ((word >> ((u & 3) * 4)) & 0xf) as usize;
                let texel = vram.data[(cy & 511) * 1024 + ((cx + idx) & 1023)];
                if texel == 0 {
                    continue;
                }
                let e = |s: u16| ((s & 31) << 3) as i64;
                let mut f = [e(texel), e(texel >> 5), e(texel >> 10)];
                if !c.raw {
                    for (fk, m) in f.iter_mut().zip(c.rgb) {
                        *fk = ((*fk * m) >> 7).min(255);
                    }
                }
                let i = ((py * w + px) * 3) as usize;
                for k in 0..3 {
                    let bg = fb[i + k] as i64;
                    fb[i + k] = if c.semi && texel & 0x8000 != 0 {
                        match c.abr {
                            0 => (bg + f[k]) / 2,
                            1 => bg + f[k],
                            2 => bg - f[k],
                            _ => bg + f[k] / 4,
                        }
                    } else {
                        f[k]
                    }
                    .clamp(0, 255) as u8;
                }
                cov[(py * w + px) as usize] = true;
            }
        }
    }
}

/// Background grey for previews: subtractive effects are drawn over light grey.
pub fn preview_bg(seq: &[Vec<DrawCmd>]) -> u8 {
    if seq.iter().flatten().any(|c| c.semi && c.abr == 2) { 176 } else { 0 }
}

/// Centre and cell size that frame every quad of a sequence (cell 64..=256, multiple of 32).
pub fn frame_of(seq: &[Vec<DrawCmd>]) -> ((i64, i64), i64) {
    let pts = || seq.iter().flatten().flat_map(|c| c.quad);
    let (Some(x0), Some(x1)) = (pts().map(|p| p.0).min(), pts().map(|p| p.0).max()) else { return ((0, 0), 64) };
    let (y0, y1) = (pts().map(|p| p.1).min().unwrap(), pts().map(|p| p.1).max().unwrap());
    let size = ((x1 - x0).max(y1 - y0) + 16).clamp(64, 256);
    (((x0 + x1) / 2, (y0 + y1) / 2), (size + 31) / 32 * 32)
}

/// Renders a tick sequence into a sprite sheet (cell x cell frames, `cols` per row).
/// Pixels are composited over a grey level `bg` (0 for additive effects; a light
/// grey makes subtractive ones visible); alpha is 0 where nothing was drawn.
pub fn render_sheet(seq: &[Vec<DrawCmd>], vram: &Vram, cell: i64, center: (i64, i64), cols: usize, bg: u8) -> Image {
    let n = seq.len().max(1);
    let rows = n.div_ceil(cols);
    let (w, h) = (cell as usize * cols.min(n), cell as usize * rows);
    let mut rgba = vec![0u8; w * h * 4];
    for (k, cmds) in seq.iter().enumerate() {
        let mut fb = vec![bg; (cell * cell * 3) as usize];
        let mut cov = vec![false; (cell * cell) as usize];
        let mut sorted: Vec<&DrawCmd> = cmds.iter().collect();
        sorted.sort_by_key(|c| -c.otz); // ordering table: far first
        for c in sorted {
            draw(&mut fb, &mut cov, cell, vram, c, (cell / 2 - center.0, cell / 2 - center.1));
        }
        let (gx, gy) = ((k % cols) * cell as usize, (k / cols) * cell as usize);
        for y in 0..cell as usize {
            for x in 0..cell as usize {
                let s = y * cell as usize + x;
                let d = ((gy + y) * w + gx + x) * 4;
                rgba[d..d + 3].copy_from_slice(&fb[s * 3..s * 3 + 3]);
                rgba[d + 3] = if cov[s] { 255 } else { 0 };
            }
        }
    }
    Image { width: w as u32, height: h as u32, rgba }
}

fn blend_name(semi: bool, abr: u8) -> &'static str {
    if !semi {
        return "opaque";
    }
    ["average", "add", "subtract", "add_quarter"][abr as usize & 3]
}

/// Static definitions for other programs: particle types (UV quads, hold ticks, blend,
/// base size, texture page/CLUT) and hit-spark presets.
pub fn definitions(t: &Tables) -> Value {
    json!({
        "tick_rate": 60,
        "cluts": t.cluts,
        "particle_types": t.types.iter().enumerate().map(|(i, p)| json!({
            "type": i, "frames": p.frames.iter().map(|f| f.iter().map(|&(u, v)| [u, v]).collect::<Vec<_>>()).collect::<Vec<_>>(),
            "hold_ticks": p.durations, "base_size": p.size, "blend": blend_name(p.semi, p.abr),
            "colour_modulated": !p.raw, "texture_page": [p.tpage_x, p.tpage_y], "default_clut": default_clut(i),
        })).collect::<Vec<_>>(),
        "spark_presets": t.sparks.iter().enumerate().map(|(i, s)| json!({
            "id": i, "yaw": s.ang_y, "pitch": s.ang_x, "yaw_per_tick": s.dang_y, "speed": s.speed,
            "speed_change_per_4_ticks": s.dspeed, "particles": s.total, "active_ticks": s.frames,
            "blood_size": s.blood_size, "no_gravity": s.nograv, "brightness": s.bright, "raw": s.raw,
        })).collect::<Vec<_>>(),
        "spark_particle": {"normal": t.spark_part[0], "blood": t.spark_part[1]},
    })
}
