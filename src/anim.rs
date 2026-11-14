//! Fighter skeletons and keyframe animation from XX.ANM.
//!
//! Derived from the game code (SCUS_941.99): 0x8017ceb4 header parse,
//! 0x8017c874 bind pose, 0x80182280 key decode/interpolation,
//! 0x801819b4/0x80181c2c bone matrices. PSX space (Y down), 4096 = 1.0 = 360°.
//! Matrices act on column vectors: p_parent = M·p_local + t.
//!
//! Keyframe (120 bytes): i16 root yaw, 3×i16 root motion, 3×i16 bone-1
//! translation (/16, added to bind), 3×i16 bone-4 translation (/16), then 16
//! groups for bones 1..16 of {u32 10:10:10 direction (local +Y is rotated onto
//! it), i16 twist about Y}, then i16 Y angles for bones 17 and 18.
//! Bone matrix: M = A · Rdir(n) · Ry(-twist), A = base rotation Rz·Ry·Rx.

use crate::bail;
use crate::error::Result;

pub type M3 = [[f64; 3]; 3];
pub const IDENT: M3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

pub fn mul(a: &M3, b: &M3) -> M3 {
    let mut r = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            r[i][j] = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    r
}

pub fn apply(a: &M3, v: [f64; 3]) -> [f64; 3] {
    [0, 1, 2].map(|i| a[i][0] * v[0] + a[i][1] * v[1] + a[i][2] * v[2])
}

fn cs(a: f64) -> (f64, f64) {
    let t = a * std::f64::consts::TAU / 4096.0;
    (t.cos(), t.sin())
}
fn rot_x(a: f64) -> M3 {
    let (c, s) = cs(a);
    [[1.0, 0.0, 0.0], [0.0, c, -s], [0.0, s, c]]
}
pub fn rot_y(a: f64) -> M3 {
    let (c, s) = cs(a);
    [[c, 0.0, s], [0.0, 1.0, 0.0], [-s, 0.0, c]]
}
fn rot_z(a: f64) -> M3 {
    let (c, s) = cs(a);
    [[c, -s, 0.0], [s, c, 0.0], [0.0, 0.0, 1.0]]
}

/// Minimal rotation taking local +Y onto direction n.
fn rot_dir(n: [f64; 3]) -> M3 {
    let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
    if l == 0.0 {
        return IDENT;
    }
    let (nx, ny, nz) = (n[0] / l, n[1] / l, n[2] / l);
    let h = nx.hypot(nz);
    let (hx, hz) = if h == 0.0 { (0.0, 0.0) } else { (nx / h, nz / h) };
    let k = 1.0 - ny;
    [[ny + k * hz * hz, nx, -k * hx * hz], [-nx, ny, -nz], [-k * hx * hz, nz, ny + k * hx * hx]]
}

fn unpack10(w: u32) -> [f64; 3] {
    let s = |v: u32| (((v & 0x3ff) << 22) as i32 >> 22) as f64;
    [s(w), s(w >> 10), s(w >> 20)]
}

pub struct Form {
    pub parents: Vec<i16>,
    /// TMD object index -> bone (-1 = not attached).
    pub objmap: Vec<i16>,
    pub offsets: Vec<[f64; 3]>,
    pub objofs: Vec<[f64; 3]>,
    pub base: Vec<M3>,
}

#[derive(Clone, Copy, Default)]
struct Key {
    t1: [f64; 3],
    t4: [f64; 3],
    groups: [([f64; 3], f64); 16],
    single: [f64; 2],
}

pub struct Anim {
    pub index: usize,
    /// (key, duration in frames); the last key has duration 0.
    keys: Vec<(Key, u16)>,
    /// Scale of the bone-1 translation (shared CMN.ACT clips: fighter u16 @0x3c / 128).
    pub t1_scale: f64,
}

/// Clips of an animation block: table of offsets at block + 8 (relative to the
/// block, 0 = empty, -1 = end), each {u16 frames, u16 0, (u16 key, u16 hold)*}.
pub fn parse_anims(d: &[u8], block: usize, limit: usize, t1_scale: f64) -> Result<Vec<Anim>> {
    let u32_at = |o: usize| -> Result<u32> {
        Ok(u32::from_le_bytes(
            d.get(o..o + 4).ok_or_else(|| crate::format_err!("clip table truncated at {o:#x}"))?.try_into()?,
        ))
    };
    let mut anims = Vec::new();
    for i in 0..limit {
        let v = u32_at(block + 8 + 4 * i)? as i32;
        if v == -1 {
            break;
        }
        if v <= 0 {
            continue;
        }
        let a = block + v as usize;
        let mut keys = Vec::new();
        let mut p = a + 4;
        loop {
            let e = d.get(p..p + 4).ok_or_else(|| crate::format_err!("key table out of range"))?;
            let ko = u16::from_le_bytes([e[0], e[1]]) as usize;
            let dur = u16::from_le_bytes([e[2], e[3]]);
            p += 4;
            keys.push((
                decode_key(d.get(a + ko..a + ko + 0x78).ok_or_else(|| crate::format_err!("key out of range"))?),
                dur,
            ));
            if dur == 0 || keys.len() > 512 {
                break;
            }
        }
        anims.push(Anim { index: i, keys, t1_scale });
    }
    Ok(anims)
}

pub struct Anm {
    pub forms: Vec<Form>,
    pub anims: Vec<Anim>,
    /// Bone-1 translation scale for shared CMN.ACT clips (u16 @0x3c / 128).
    pub cmn_t1_scale: f64,
}

pub struct Pose {
    pub rot: Vec<M3>,
    pub trans: Vec<[f64; 3]>,
}

impl Anm {
    pub fn parse(d: &[u8]) -> Result<Anm> {
        let u32_at = |o: usize| -> Result<u32> {
            Ok(u32::from_le_bytes(
                d.get(o..o + 4).ok_or_else(|| crate::format_err!("ANM truncated at {o:#x}"))?.try_into()?,
            ))
        };
        let i16_at = |o: usize| -> Result<i16> {
            Ok(i16::from_le_bytes(
                d.get(o..o + 2).ok_or_else(|| crate::format_err!("ANM truncated at {o:#x}"))?.try_into()?,
            ))
        };
        let block = u32_at(0)? as usize;
        let form_tbl = u32_at(0x20)? as usize;
        let mut forms = Vec::new();
        for f in 0..2 {
            let n = *d.get(0x2c + 2 * f).ok_or_else(|| crate::format_err!("ANM header truncated"))? as usize;
            if n == 0 || n > 64 {
                bail!("bad bone count");
            }
            let o: Vec<usize> =
                (0..5).map(|i| u32_at(form_tbl + 0x14 * f + 4 * i).map(|v| v as usize)).collect::<Result<_>>()?;
            let parents = (0..n).map(|b| i16_at(o[0] + 2 * b)).collect::<Result<Vec<_>>>()?;
            // Object map runs up to the next table (real files: 17..24 entries).
            let nmap = if o[2] > o[1] { ((o[2] - o[1]) / 2).min(64) } else { 32 };
            let objmap = (0..nmap).map(|i| i16_at(o[1] + 2 * i)).collect::<Result<Vec<_>>>()?;
            let mut offsets = vec![[0.0; 3]];
            let mut base = vec![IDENT];
            for b in 1..n {
                offsets.push(unpack10(u32_at(o[2] + 4 * (b - 1))?));
                let r = unpack10(u32_at(o[4] + 4 * (b - 1))?);
                base.push(mul(&mul(&rot_z(4.0 * r[2]), &rot_y(4.0 * r[1])), &rot_x(4.0 * r[0])));
            }
            let objofs = (0..nmap).map(|i| u32_at(o[3] + 4 * i).map(unpack10)).collect::<Result<Vec<_>>>()?;
            forms.push(Form { parents, objmap, offsets, objofs, base });
        }
        let anims = parse_anims(d, block, usize::MAX, 1.0)?;
        let cmn_t1_scale =
            u16::from_le_bytes([*d.get(0x3c).unwrap_or(&128), *d.get(0x3d).unwrap_or(&0)]) as f64 / 128.0;
        Ok(Anm { forms, anims, cmn_t1_scale })
    }

    pub fn bind(&self, form: usize) -> Pose {
        let f = &self.forms[form];
        Pose { rot: f.base.clone(), trans: f.offsets.clone() }
    }

    /// Local bone transforms at `frame` (60 Hz frames).
    pub fn pose(&self, form: usize, anim: &Anim, frame: f64) -> Pose {
        let k = anim.sample(frame);
        let f = &self.forms[form];
        let mut p = self.bind(form);
        for b in 1..f.parents.len() {
            if b <= 16 {
                let (dir, tw) = k.groups[b - 1];
                p.rot[b] = mul(&mul(&f.base[b], &rot_dir(dir)), &rot_y(-tw));
            } else if b <= 18 {
                p.rot[b] = rot_y(-k.single[b - 17]); // base rotation not applied
            }
        }
        // bone 1 carries the root translation; a form of a single bone has none
        if f.offsets.len() > 1 {
            p.trans[1] = [0, 1, 2].map(|j| f.offsets[1][j] + k.t1[j] * anim.t1_scale / 16.0);
        }
        if f.parents.len() > 4 {
            p.trans[4] = k.t4.map(|v| v / 16.0);
        }
        p
    }
}

impl Anim {
    pub fn frames(&self) -> u32 {
        self.keys.iter().map(|(_, d)| *d as u32).sum::<u32>().max(1)
    }

    fn sample(&self, frame: f64) -> Key {
        let mut t = frame.max(0.0);
        let mut i = 0;
        while i + 1 < self.keys.len() && t >= self.keys[i].1 as f64 {
            t -= self.keys[i].1 as f64;
            i += 1;
        }
        if i + 1 >= self.keys.len() {
            return self.keys[self.keys.len() - 1].0;
        }
        let (a, b, f) = (&self.keys[i].0, &self.keys[i + 1].0, t / self.keys[i].1 as f64);
        let l = |u: f64, v: f64| u + (v - u) * f;
        let l3 = |u: [f64; 3], v: [f64; 3]| [0, 1, 2].map(|j| l(u[j], v[j]));
        let mut out = Key {
            t1: l3(a.t1, b.t1),
            t4: l3(a.t4, b.t4),
            single: [l(a.single[0], b.single[0]), l(a.single[1], b.single[1])],
            ..Key::default()
        };
        for g in 0..16 {
            out.groups[g] = (l3(a.groups[g].0, b.groups[g].0), l(a.groups[g].1, b.groups[g].1));
        }
        out
    }
}

fn decode_key(kb: &[u8]) -> Key {
    let h = |o: usize| i16::from_le_bytes([kb[o], kb[o + 1]]) as f64;
    let mut k =
        Key { t1: [h(8), h(10), h(12)], t4: [h(14), h(16), h(18)], single: [h(0x74), h(0x76)], ..Key::default() };
    for (g, group) in k.groups.iter_mut().enumerate() {
        let o = 0x14 + 6 * g;
        let w = u32::from_le_bytes(kb[o..o + 4].try_into().unwrap());
        *group = (unpack10(w).map(|v| v * 32.0), h(o + 4));
    }
    k
}

/// World transforms from local ones.
pub fn world(parents: &[i16], p: &Pose) -> (Vec<M3>, Vec<[f64; 3]>) {
    let n = parents.len();
    let mut wm: Vec<Option<M3>> = vec![None; n];
    let mut wt = vec![[0.0; 3]; n];
    fn solve(b: usize, parents: &[i16], p: &Pose, wm: &mut Vec<Option<M3>>, wt: &mut Vec<[f64; 3]>, depth: usize) {
        if wm[b].is_some() || depth > 64 {
            return;
        }
        let par = parents[b];
        if par < 0 || par as usize >= parents.len() {
            wm[b] = Some(p.rot[b]);
            wt[b] = p.trans[b];
        } else {
            let pi = par as usize;
            solve(pi, parents, p, wm, wt, depth + 1);
            let pm = wm[pi].unwrap_or(IDENT);
            wm[b] = Some(mul(&pm, &p.rot[b]));
            let v = apply(&pm, p.trans[b]);
            wt[b] = [v[0] + wt[pi][0], v[1] + wt[pi][1], v[2] + wt[pi][2]];
        }
    }
    for b in 0..n {
        solve(b, parents, p, &mut wm, &mut wt, 0);
    }
    (wm.into_iter().map(|m| m.unwrap_or(IDENT)).collect(), wt)
}

/// Physics chain bones (19+: hair, ears, tails): per-character parameter
/// records from the EXE, simulated by `chainsim`.
pub struct Chains {
    /// One record per chain bone (19, 20, ...): flags/mode, inertia, rest pull,
    /// rest angles X/Z, X limits, Z limits, wag speed.
    pub(crate) recs: Vec<[i16; 10]>,
    /// Mode-1 behaviour bits set at init (0x80158398), per bone.
    pub(crate) bits: Vec<(usize, u8)>,
}

/// Character order of the EXE's per-character tables (its file-name table).
const EXE_CHAR_ORDER: [&str; 9] = ["ZIN", "GRE", "EVE", "VAK", "MIT", "GAD", "FOX", "RON", "URI"];

impl Chains {
    /// Reads the chain records for fighter `id` / `form` from SCUS_941.99.
    pub fn from_exe(exe: &[u8], id: &str, form: usize) -> Chains {
        let Some(ch) = EXE_CHAR_ORDER.iter().position(|c| *c == id) else {
            return Chains { recs: vec![], bits: vec![] };
        };
        let file = |a: u32| (a.wrapping_sub(0x8012_0000) + 0x800) as usize;
        let u32_at = |a: u32| exe.get(file(a)..file(a) + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
        let mut recs = Vec::new();
        if let Some(list) = u32_at(0x801a_5c0c + 4 * (form * 9 + ch) as u32).filter(|&p| p != 0) {
            while let Some(r) = u32_at(list + 4 * recs.len() as u32).filter(|&p| p != 0) {
                let Some(b) = exe.get(file(r)..file(r) + 20) else { break };
                recs.push(std::array::from_fn(|i| i16::from_le_bytes([b[2 * i], b[2 * i + 1]])));
                if recs.len() > 16 {
                    break;
                }
            }
        }
        let bits: &[(usize, u8)] = match (ch, form) {
            (0, 1) => &[(19, 2), (20, 1)],
            (4..=7, 1) => &[(19, 1)],
            (8, 1) => &[(19, 2), (20, 4), (21, 4), (22, 8), (23, 8)],
            _ => &[],
        };
        Chains { recs, bits: bits.to_vec() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: &M3, b: &M3) -> bool {
        a.iter().flatten().zip(b.iter().flatten()).all(|(x, y)| (x - y).abs() < 1e-9)
    }

    #[test]
    fn unpack10_sign_extends() {
        assert_eq!(unpack10(0x3ff | 0x200 << 10 | 1 << 20), [-1.0, -512.0, 1.0]);
    }

    #[test]
    fn rot_dir_maps_y_onto_direction() {
        assert!(close(&rot_dir([0.0, 5.0, 0.0]), &IDENT));
        let m = rot_dir([1.0, 0.0, 0.0]);
        let v = apply(&m, [0.0, 1.0, 0.0]);
        assert!((v[0] - 1.0).abs() < 1e-9 && v[1].abs() < 1e-9 && v[2].abs() < 1e-9);
        // orthonormal for an arbitrary direction
        let m = rot_dir([0.3, -0.8, 0.5]);
        let mt = [0, 1, 2].map(|i| [m[0][i], m[1][i], m[2][i]]);
        assert!(close(&mul(&m, &mt), &IDENT));
    }

    #[test]
    fn quarter_turns() {
        let v = apply(&rot_z(1024.0), [1.0, 0.0, 0.0]);
        assert!(v[0].abs() < 1e-9 && (v[1] - 1.0).abs() < 1e-9);
    }

    /// A fuzzer input (made-up bytes) whose first form has a single bone: posing it must not index the root bone.
    #[test]
    fn pose_of_a_single_bone_form() {
        let hex = concat!(
            "00000000000100000a00000000000000020000000a0000000100000046000000000000003f000000ffffffff02e401111111",
            "11111111040100000111110001ff00010000000000007c46000000000023003f000000000000000000000000000000000200",
            "00000000000000ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
            "6969696969aeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeae32ffff0000fe11115c000000000000001111",
            "11111111111111151515cececececececececececececececececece15151515151515151515151515151515151515151515",
            "1515151515151515151515151515151515151515151515151515151515151515151515151515151515151515151515151515",
            "1515151515151515111111110000696969691111",
        );
        let data: Vec<u8> =
            (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap()).collect();
        let anm = Anm::parse(&data).expect("the input parses");
        assert!(anm.forms.iter().any(|f| f.parents.len() == 1));
        for form in 0..anm.forms.len() {
            for a in anm.anims.iter().take(8) {
                let p = anm.pose(form, a, 0.0);
                let _ = world(&anm.forms[form].parents, &p);
            }
        }
    }

    #[test]
    fn world_composes_parent_first() {
        let pose = Pose { rot: vec![rot_z(1024.0), IDENT], trans: vec![[0.0; 3], [10.0, 0.0, 0.0]] };
        let (_, wt) = world(&[-1, 0], &pose);
        assert!(wt[1][0].abs() < 1e-9 && (wt[1][1] - 10.0).abs() < 1e-9);
    }
}
