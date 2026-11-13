//! Integer-exact port of the game's secondary-chain simulation (bones 19+:
//! hair, ears, tails). Init 0x801582c8, per frame 0x80158458; verified
//! bit-for-bit against the original code in an emulator (except URI human
//! bone 25, mode 5, which is approximated). Matrices: 3x3 ints, 4096 = 1.0.
//!
//! Mode 0: each bone points at its child's last position plus inertia
//! (r1/256 of the child's movement), gravity 3/frame (PSX +Y down), a
//! constant pull r2 along the rest frame F = parent·Rx(r3)·Rz(r4) and
//! optional wag; the direction is converted to angles Rx(s0)·Rz(a1) clamped
//! to [r5,r6] / [r7,r8]. Mode 1: time-driven wag toward target angles.

use crate::anim::{Chains, M3};

type Mi = [[i64; 3]; 3];
const IDI: Mi = [[4096, 0, 0], [0, 4096, 0], [0, 0, 4096]];

fn s32(v: i64) -> i64 {
    v as i32 as i64
}
/// Arithmetic shift right rounding toward zero.
fn tdiv(a: i64, sh: u32) -> i64 {
    if a < 0 { -((-a) >> sh) } else { a >> sh }
}
fn sat(v: i64) -> i64 {
    v.clamp(-0x8000, 0x7fff)
}

pub struct Tables {
    sin: Vec<i64>,
    atn: Vec<i64>,
}

impl Tables {
    pub fn from_exe(exe: &[u8]) -> Option<Tables> {
        let read = |addr: u32| -> Option<Vec<i64>> {
            let o = (addr - 0x8012_0000 + 0x800) as usize;
            let b = exe.get(o..o + 1025 * 2)?;
            Some(b.as_chunks::<2>().0.iter().map(|c| i16::from_le_bytes([c[0], c[1]]) as i64).collect())
        };
        Some(Tables { sin: read(0x8012_6b78)?, atn: read(0x801a_e348)? })
    }
    fn isin(&self, a: i64) -> i64 {
        let i = (a & 0x3ff) as usize;
        let v = if a & 0x400 != 0 { self.sin[0x400 - i] } else { self.sin[i] };
        if a & 0x800 != 0 { -v } else { v }
    }
    fn icos(&self, a: i64) -> i64 {
        self.isin(a + 0x400)
    }
    /// 0x80198a14
    fn ratan2(&self, y: i64, x: i64) -> i64 {
        let (nx, ny) = (x < 0, y < 0);
        let (x, y) = (x.abs(), y.abs());
        if x == 0 && y == 0 {
            return 0;
        }
        let quot = |a: i64, b: i64| -> usize {
            let q = if a & 0x7fe0_0000 != 0 { s32(a / (b >> 10).max(1)) } else { s32((a << 10) & 0xffff_ffff) / b };
            q.clamp(0, 1024) as usize
        };
        let mut r = if y < x { self.atn[quot(y, x)] } else { 0x400 - self.atn[quot(x, y)] };
        if nx {
            r = 0x800 - r;
        }
        if ny { -r } else { r }
    }
    fn rx(&self, a: i64) -> Mi {
        let (c, n) = (self.icos(a), self.isin(a));
        [[4096, 0, 0], [0, c, -n], [0, n, c]]
    }
    /// 0x8018fd50: Rx(a)·Rz(b)
    fn rx_rz(&self, a: i64, b: i64) -> Mi {
        let (sa, ca, sb, cb) = (self.isin(a), self.icos(a), self.isin(b), self.icos(b));
        [[cb, -sb, 0], [(ca * sb) >> 12, (ca * cb) >> 12, -sa], [(sa * sb) >> 12, (sa * cb) >> 12, ca]]
    }
    /// 0x8018f9e0
    fn ry(&self, a: i64) -> Mi {
        let (c, n) = (self.icos(a), self.isin(a));
        [[c, 0, n], [0, 4096, 0], [-n, 0, c]]
    }
}

/// GTE MulMatrix.
fn mmul(a: &Mi, b: &Mi) -> Mi {
    let mut r = [[0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            r[i][j] = sat((0..3).map(|k| a[i][k] * b[k][j]).sum::<i64>() >> 12);
        }
    }
    r
}
/// GTE MVMVA (sf=1) on a short vector.
fn mvec(a: &Mi, v: [i64; 3]) -> [i64; 3] {
    [0, 1, 2].map(|i| (a[i][0] * v[0] + a[i][1] * v[1] + a[i][2] * v[2]) >> 12)
}
/// 0x80197b84 ApplyMatrixLV (long vector).
fn apply_long(a: &Mi, v: [i64; 3]) -> [i64; 3] {
    let hi = v.map(|x| tdiv(x, 15));
    let lo = [0, 1, 2].map(|k| v[k] - (hi[k] << 15));
    [0, 1, 2].map(|i| {
        let ph = (0..3).map(|k| a[i][k] * hi[k]).sum::<i64>() * 8;
        let pl = (0..3).map(|k| a[i][k] * lo[k]).sum::<i64>() >> 12;
        ph + pl
    })
}
fn transpose(a: &Mi) -> Mi {
    [0, 1, 2].map(|i| [a[0][i], a[1][i], a[2][i]])
}

#[derive(Clone, Copy, Default)]
struct BoneData {
    a: [i64; 4],
    prev: [i64; 3],
    phase: i64,
}

pub struct ChainSim<'a> {
    t: &'a Tables,
    chains: &'a Chains,
    parents: &'a [i16],
    offs: Vec<[i64; 3]>,
    n: usize,
    floor: i64,
    bd: Vec<BoneData>,
    m: Vec<Mi>,
    wm: Vec<Mi>,
    wt: Vec<[i64; 3]>,
    first: bool,
}

impl<'a> ChainSim<'a> {
    /// Fresh state as after the game's init: chain rotations identity, zero velocity.
    pub fn new(t: &'a Tables, chains: &'a Chains, parents: &'a [i16], offsets: &[[f64; 3]]) -> Self {
        let n = parents.len();
        let mut bd = vec![BoneData { a: [0, 0x7fff, 0, 0], ..Default::default() }; n + 1];
        for &(b, bits) in &chains.bits {
            if b < n {
                bd[b].a[3] = bits as i64;
            }
        }
        ChainSim {
            t,
            chains,
            parents,
            offs: offsets.iter().map(|o| o.map(|v| v as i64)).collect(),
            n,
            floor: 0,
            bd,
            m: vec![IDI; n],
            wm: vec![IDI; n],
            wt: vec![[0; 3]; n],
            first: true,
        }
    }

    /// One game frame. `wm`/`wt`: float world transforms of bones (only < 19 are read).
    /// Returns the local rotation of every chain bone (bone's world = parentWorld · local).
    pub fn step(&mut self, wmf: &[M3], wtf: &[[f64; 3]]) -> Vec<(usize, M3)> {
        let t = self.t;
        let n = self.n;
        let mut wm = self.wm.clone();
        let mut wt = self.wt.clone();
        for b in 0..19.min(n) {
            wm[b] = wmf[b].map(|r| r.map(|x| (x * 4096.0).round() as i64));
            wt[b] = wtf[b].map(|x| x.round() as i64);
        }
        if self.first {
            for b in 19..n {
                let p = self.parents[b] as usize;
                wm[b] = wm[p];
                let v = mvec(&wm[p], self.offs[b]);
                wt[b] = [0, 1, 2].map(|k| v[k] + wt[p][k]);
                self.bd[b].prev = wt[b];
            }
            self.first = false;
        }
        let mut out = Vec::new();
        for b in 19..n {
            let r =
                self.chains.recs.get(b - 19).copied().unwrap_or([0x20, 0, 0, 0, 0, 0, 0, 0, 0, 0]).map(|v| v as i64);
            let (fl, mode) = (r[0], r[0] & 0xf);
            let p = self.parents[b] as usize;
            let pbd = self.bd[b - 1];
            let v = mvec(&wm[p], self.offs[b]);
            let newt = [0, 1, 2].map(|k| v[k] + wt[p][k]);

            let local = if mode == 0 {
                self.bd[b].prev = wt[b].map(sat);
                wt[b] = newt;
                if fl & 0x20 != 0 {
                    out.push((b, self.m[b])); // tip: rotation never updated
                    continue;
                }
                // Child's previous-frame world position (not yet updated this frame).
                let ct = if b + 1 < n { wt[b + 1] } else { newt };
                let cprev = self.bd[b + 1].prev;
                let mut d = [0, 1, 2].map(|k| tdiv((ct[k] - cprev[k]) * r[1], 8) + ct[k] - newt[k]);
                d[1] += 3;
                let over = newt[1] + d[1] - self.floor;
                if over > 0 {
                    d[1] -= over;
                }
                let (rr, f) = if r[3] != 0 || r[4] != 0 {
                    let rr = t.rx_rz(r[3], r[4]);
                    (Some(rr), mmul(&wm[p], &rr))
                } else {
                    (None, wm[p])
                };
                let mut dl = apply_long(&transpose(&f), d.map(|x| s32(x << 16)));
                let bd = &mut self.bd[b];
                if fl & 0x10 == 0 {
                    bd.a[3] = pbd.a[3];
                }
                let (mut vx, mut vz) = (0, 0);
                if fl & 0x80 != 0 {
                    if fl & 0x10 != 0 {
                        if bd.a[3] & 1 != 0 {
                            bd.phase = (bd.phase + r[9]) & 0xffff;
                        }
                    } else {
                        bd.phase = (pbd.phase + 0x400) & 0xffff;
                    }
                    bd.a[0] = tdiv(t.isin(bd.phase), 4);
                    vx = bd.a[0];
                } else if fl & 0x100 != 0 {
                    if fl & 0x10 != 0 {
                        if bd.a[3] & 1 != 0 {
                            bd.phase = (bd.phase + r[9]) & 0xffff;
                            bd.a[2] = tdiv(t.isin(bd.phase), 4);
                        }
                    } else {
                        bd.a[2] = pbd.a[2];
                    }
                    vz = bd.a[2];
                }
                let w = mvec(&t.rx_rz(vx, vz), [0, r[2], 0]);
                dl = [0, 1, 2].map(|k| s32(dl[k] + (w[k] << 16)));
                let s0 = t.ratan2(dl[2], dl[1]).clamp(r[5].min(r[6]), r[6].max(r[5]));
                let dp = apply_long(&t.rx(-s0), dl);
                let a1 = t.ratan2(-dp[0], dp[1]).clamp(r[7].min(r[8]), r[8].max(r[7]));
                self.m[b] = t.rx_rz(s0, a1);
                wm[b] = mmul(&f, &self.m[b]);
                match rr {
                    Some(rr) => mmul(&rr, &self.m[b]),
                    None => self.m[b],
                }
            } else {
                wt[b] = newt;
                let bd = &mut self.bd[b];
                if mode == 1 {
                    let bits = bd.a[3];
                    let mid = tdiv(r[5] + r[6], 1);
                    // 0x80135e68: approach target by at most 0x40 (12-bit wraparound)
                    let appr = |cur: i64, tgt: i64| -> i64 {
                        let (c, g) = ((cur << 20) & 0xffff_ffff, (tgt << 20) & 0xffff_ffff);
                        let dd = s32(g - c);
                        let st = 0x40i64 << 20;
                        if dd == 0 {
                            cur
                        } else if dd > st {
                            s32(c + st) >> 20
                        } else if -dd > st {
                            s32(c - st) >> 20
                        } else {
                            s32(g) >> 20
                        }
                    };
                    if bits & 1 != 0 {
                        bd.a[1] = 0x271;
                        bd.a[0] = appr(bd.a[0], bd.a[1]);
                        self.m[b] = t.rx(bd.a[0]);
                    } else if bits & 2 != 0 {
                        bd.phase = (bd.phase + r[9]) & 0xffff;
                        bd.a[1] = tdiv(t.isin(bd.phase) * (r[6] - r[5]), 12) + mid;
                        bd.a[0] = appr(bd.a[0], bd.a[1]);
                        self.m[b] = t.rx(bd.a[0]);
                    } else if bits & 0xc != 0 {
                        bd.phase = (bd.phase + r[9]) & 0xffff;
                        let sn = t.isin(bd.phase);
                        let sn = if bits & 4 != 0 { sn } else { -sn };
                        bd.a[0] = tdiv(sn * (r[6] - r[5]), 12) + mid;
                        bd.a[1] = appr(bd.a[1], bd.a[0]);
                        self.m[b] = t.ry(bd.a[1]);
                    }
                } else if mode == 5 {
                    self.m[b] = t.rx(-0x71); // approximation (URI human bone 25)
                } else {
                    self.m[b] = IDI; // mode 3 idle (GRE) / unknown
                }
                wm[b] = mmul(&wm[p], &self.m[b]);
                self.m[b]
            };
            out.push((b, local));
        }
        self.wm = wm;
        self.wt = wt;
        out.into_iter().map(|(b, m)| (b, m.map(|r| r.map(|x| x as f64 / 4096.0)))).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integer_helpers_match_mips_semantics() {
        assert_eq!(tdiv(-7, 1), -3); // toward zero, unlike >>
        assert_eq!(-7 >> 1, -4);
        assert_eq!(sat(40000), 0x7fff);
        assert_eq!(sat(-40000), -0x8000);
        assert_eq!(s32(0x1_0000_0001), 1);
        assert_eq!(s32(0xffff_ffff), -1);
    }

    #[test]
    fn identity_matrix_ops() {
        let v = [100, -200, 300];
        assert_eq!(mvec(&IDI, v), v);
        assert_eq!(mmul(&IDI, &IDI), IDI);
        assert_eq!(apply_long(&IDI, [1 << 20, -(1 << 20), 5]), [1 << 20, -(1 << 20), 5]);
        assert_eq!(transpose(&[[1, 2, 3], [4, 5, 6], [7, 8, 9]]), [[1, 4, 7], [2, 5, 8], [3, 6, 9]]);
    }
}
