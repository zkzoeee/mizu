//! Build-derived positional Vesta bases. This private view is attached only to
//! the immutable embedded verifier, never to a runtime verifier or proof input.
use alloc::vec::Vec;
use ark_ec::{
    short_weierstrass::{Affine, SWCurveConfig},
    AffineRepr, CurveGroup,
};
use ark_ff::{AdditiveGroup, BigInt, PrimeField};
use bytemuck::{Pod, Zeroable};
use mina_curves::pasta::{Fp, Fq, Vesta};

pub(crate) const MAGIC: &[u8; 8] = b"ZKVFIX01";
pub(crate) const HEADER: usize = 88;
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct EncodedPoint {
    x: [u64; 4],
    y: [u64; 4],
    infinity: u64,
}

#[derive(Clone, Copy)]
enum FixedVestaStorage<'a> {
    Finite {
        coords: &'a [[u64; 8]],
        negatives: Option<&'a [[u64; 8]]>,
    },
    General(&'a [EncodedPoint]),
}

#[derive(Clone, Copy)]
pub(crate) struct FixedVestaSrs<'a> {
    storage: FixedVestaStorage<'a>,
    n: usize,
    lane_shift: usize,
}
impl<'a> FixedVestaSrs<'a> {
    pub(crate) fn len(&self) -> usize {
        self.n
    }
    pub(crate) fn lane_shift(&self) -> usize {
        self.lane_shift
    }
    /// Zero-copy view of all `2 * n` canonical `[u64; 8]` coordinates when every
    /// generator in the baked SRS is finite (true for all production Mina SRSes).
    pub(crate) fn finite_coords(&self) -> Option<&'a [[u64; 8]]> {
        match self.storage {
            FixedVestaStorage::Finite { coords, .. } => Some(coords),
            FixedVestaStorage::General(_) => None,
        }
    }
    /// Optional pre-baked `-G` coordinates borrowed directly from `.rodata`
    /// when compiled with `MIZU_PREBAKE_NEGATIVES=1`.
    pub(crate) fn finite_negatives(&self) -> Option<&'a [[u64; 8]]> {
        match self.storage {
            FixedVestaStorage::Finite { negatives, .. } => negatives,
            FixedVestaStorage::General(_) => None,
        }
    }
    pub(crate) fn point<C: SWCurveConfig>(&self, lane: usize, i: usize) -> Affine<C>
    where
        C::BaseField: PrimeField<BigInt = BigInt<4>>,
    {
        assert_eq!(C::BaseField::MODULUS, Fq::MODULUS, "fixed SRS base field");
        assert!(lane < 2 && i < self.n, "fixed SRS point index");
        match self.storage {
            FixedVestaStorage::Finite { coords, .. } => {
                let c = &coords[lane * self.n + i];
                Affine::new_unchecked(
                    C::BaseField::from_bigint(BigInt([c[0], c[1], c[2], c[3]]))
                        .expect("fixed SRS noncanonical x"),
                    C::BaseField::from_bigint(BigInt([c[4], c[5], c[6], c[7]]))
                        .expect("fixed SRS noncanonical y"),
                )
            }
            FixedVestaStorage::General(entries) => {
                let p = &entries[lane * self.n + i];
                match p.infinity {
                    0 => Affine::new_unchecked(
                        C::BaseField::from_bigint(BigInt(p.x)).expect("fixed SRS noncanonical x"),
                        C::BaseField::from_bigint(BigInt(p.y)).expect("fixed SRS noncanonical y"),
                    ),
                    1 => {
                        assert!(
                            p.x == [0; 4] && p.y == [0; 4],
                            "fixed SRS identity coordinates"
                        );
                        Affine::identity()
                    }
                    _ => panic!("fixed SRS infinity flag"),
                }
            }
        }
    }
}

/// Active positional plane-1 bit shift selected by build flags.
pub(crate) fn configured_lane_shift() -> usize {
    if matches!(option_env!("MIZU_FIXED_VESTA_COUPLED"), Some("1")) {
        match option_env!("MIZU_VESTA_WINDOW") {
            Some("15") => 120,
            Some("14") => 126,
            _ => 128,
        }
    } else {
        128
    }
}

pub(crate) fn encode(g: &[Vesta]) -> Vec<u8> {
    let shift = if g.iter().all(|p| !p.infinity) {
        configured_lane_shift()
    } else {
        128
    };
    encode_with_shift(g, shift)
}

pub(crate) fn encode_with_shift(g: &[Vesta], lane_shift: usize) -> Vec<u8> {
    assert!(
        matches!(lane_shift, 120 | 126 | 128),
        "unsupported fixed Vesta lane shift {lane_shift}"
    );
    let prebake_negatives = matches!(option_env!("MIZU_PREBAKE_NEGATIVES"), Some("1"));
    // Host-only: fixture suites encode the same SRS for many VKs. Cache by
    // every generator byte, lane shift, and negative-prebake setting.
    #[cfg(feature = "std")]
    {
        use blake2::{Blake2s256, Digest};
        use std::{
            collections::BTreeMap,
            sync::{Mutex, OnceLock},
        };
        static CACHE: OnceLock<Mutex<BTreeMap<[u8; 32], Vec<u8>>>> = OnceLock::new();
        let mut hash = Blake2s256::new();
        hash.update(b"fixed-vesta-srs/v3");
        hash.update((lane_shift as u64).to_le_bytes());
        hash.update([u8::from(prebake_negatives)]);
        for p in g {
            for word in p.x.0 .0.into_iter().chain(p.y.0 .0) {
                hash.update(word.to_le_bytes());
            }
            hash.update([u8::from(p.infinity)]);
        }
        let key: [u8; 32] = hash.finalize().into();
        let mut cache = CACHE
            .get_or_init(|| Mutex::new(BTreeMap::new()))
            .lock()
            .unwrap();
        cache
            .entry(key)
            .or_insert_with(|| encode_uncached(g, lane_shift, prebake_negatives))
            .clone()
    }
    #[cfg(not(feature = "std"))]
    encode_uncached(g, lane_shift, prebake_negatives)
}

fn encode_uncached(g: &[Vesta], lane_shift: usize, prebake_negatives: bool) -> Vec<u8> {
    let high: Vec<_> = g
        .iter()
        .map(|p| {
            let mut p = p.into_group();
            for _ in 0..lane_shift {
                p.double_in_place();
            }
            p
        })
        .collect();
    let high = <Vesta as AffineRepr>::Group::normalize_batch(&high);
    let all_finite = g.iter().chain(&high).all(|p| !p.infinity);
    let with_negatives = all_finite && prebake_negatives;
    let entry_bytes = if all_finite { 64 } else { 72 };
    let plane_mult = if with_negatives { 4 } else { 2 };
    let flags: u64 = if with_negatives {
        3
    } else if all_finite {
        1
    } else {
        0
    };
    let mut out = Vec::with_capacity(HEADER + plane_mult * g.len() * entry_bytes);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&(g.len() as u64).to_le_bytes());
    let meta: u64 = 2u64 | (flags << 16) | ((lane_shift as u64) << 32);
    out.extend_from_slice(&meta.to_le_bytes());
    for modulus in [Fq::MODULUS, Fp::MODULUS] {
        for word in modulus.0 {
            out.extend_from_slice(&word.to_le_bytes());
        }
    }
    if all_finite {
        for p in g.iter().chain(&high) {
            let x = p.x.into_bigint().0;
            let y = p.y.into_bigint().0;
            let coords = [x[0], x[1], x[2], x[3], y[0], y[1], y[2], y[3]];
            out.extend_from_slice(bytemuck::bytes_of(&coords));
        }
        if with_negatives {
            for p in g.iter().chain(&high) {
                let np = -*p;
                let x = np.x.into_bigint().0;
                let y = np.y.into_bigint().0;
                let coords = [x[0], x[1], x[2], x[3], y[0], y[1], y[2], y[3]];
                out.extend_from_slice(bytemuck::bytes_of(&coords));
            }
        }
    } else {
        for p in g.iter().chain(&high) {
            let encoded = if p.infinity {
                EncodedPoint {
                    x: [0; 4],
                    y: [0; 4],
                    infinity: 1,
                }
            } else {
                EncodedPoint {
                    x: p.x.into_bigint().0,
                    y: p.y.into_bigint().0,
                    infinity: 0,
                }
            };
            out.extend_from_slice(bytemuck::bytes_of(&encoded));
        }
    }
    out
}

/// Exact byte length of an encoded fixed Vesta SRS prefix starting at `bytes`.
pub(crate) fn encoded_byte_len(bytes: &[u8]) -> usize {
    assert!(
        bytes.len() >= HEADER && &bytes[..8] == MAGIC,
        "fixed SRS header"
    );
    let word = |at| u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap());
    let n = usize::try_from(word(8)).expect("fixed SRS length overflow");
    let flags = (word(16) >> 16) & 0xffff;
    assert!(matches!(flags, 0 | 1 | 3), "fixed SRS infinity flag");
    let entry_bytes = if flags == 0 { 72 } else { 64 };
    let plane_mult = if flags == 3 { 4 } else { 2 };
    n.checked_mul(plane_mult)
        .and_then(|c| c.checked_mul(entry_bytes))
        .and_then(|s| s.checked_add(HEADER))
        .expect("fixed SRS length overflow")
}

pub(crate) fn decode(bytes: &[u8], expected: usize) -> FixedVestaSrs<'_> {
    assert!(
        bytes.len() >= HEADER && &bytes[..8] == MAGIC,
        "fixed SRS header"
    );
    let word = |at| u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap());
    let n = usize::try_from(word(8)).expect("fixed SRS length overflow");
    assert_eq!(n, expected, "fixed SRS generator count");
    let meta = word(16);
    let lanes = meta & 0xffff;
    let flags = (meta >> 16) & 0xffff;
    let raw_shift = (meta >> 32) as usize;
    let lane_shift = if raw_shift == 0 { 128 } else { raw_shift };
    assert_eq!(lanes, 2, "fixed SRS lanes");
    assert!(matches!(flags, 0 | 1 | 3), "fixed SRS infinity flag");
    assert!(
        matches!(lane_shift, 120 | 126 | 128),
        "fixed SRS lane shift"
    );
    let all_finite = flags != 0;
    let with_negatives = flags == 3;
    for (at, modulus) in [(24, Fq::MODULUS), (56, Fp::MODULUS)] {
        assert_eq!(
            core::array::from_fn::<_, 4, _>(|i| word(at + 8 * i)),
            modulus.0,
            "fixed SRS field tag"
        );
    }
    let plane_mult = if with_negatives { 4 } else { 2 };
    let count = n.checked_mul(plane_mult).expect("fixed SRS length overflow");
    let entry_bytes = if all_finite { 64 } else { 72 };
    let size = count
        .checked_mul(entry_bytes)
        .and_then(|x| x.checked_add(HEADER))
        .expect("fixed SRS length overflow");
    assert_eq!(bytes.len(), size, "fixed SRS byte length");
    let storage = if all_finite {
        let all_coords: &[[u64; 8]] =
            bytemuck::try_cast_slice(&bytes[HEADER..]).expect("fixed SRS alignment");
        let two_n = n.checked_mul(2).expect("fixed SRS length overflow");
        if with_negatives {
            FixedVestaStorage::Finite {
                coords: &all_coords[..two_n],
                negatives: Some(&all_coords[two_n..]),
            }
        } else {
            FixedVestaStorage::Finite {
                coords: all_coords,
                negatives: None,
            }
        }
    } else {
        let entries: &[EncodedPoint] =
            bytemuck::try_cast_slice(&bytes[HEADER..]).expect("fixed SRS alignment");
        FixedVestaStorage::General(entries)
    };
    FixedVestaSrs {
        storage,
        n,
        lane_shift,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ark_ff::Zero;
    use mina_curves::pasta::{PallasParameters, VestaParameters};
    #[test]
    fn positional_layers_match_native_multiplication_with_identities() {
        let g = Vesta::generator();
        let points = [Vesta::zero(), g, -g, (g * Fp::from(17u64)).into_affine()];
        let bytes = encode(&points);
        let table = decode(&bytes, points.len());
        for (i, p) in points.iter().enumerate() {
            assert_eq!(table.point::<VestaParameters>(0, i), *p);
            assert_eq!(
                table.point::<VestaParameters>(1, i),
                p.mul_bigint(BigInt([0, 0, 1, 0])).into_affine()
            );
        }
    }
    #[test]
    #[should_panic(expected = "fixed SRS field tag")]
    fn rejects_wrong_field_tag() {
        let mut b = encode(&[Vesta::generator()]);
        b[24] ^= 1;
        decode(&b, 1);
    }
    #[test]
    #[should_panic(expected = "fixed SRS byte length")]
    fn rejects_truncated_table() {
        let mut b = encode(&[Vesta::generator()]);
        b.pop();
        decode(&b, 1);
    }
    #[test]
    #[should_panic(expected = "fixed SRS generator count")]
    fn rejects_wrong_length() {
        let b = encode(&[Vesta::generator()]);
        decode(&b, 2);
    }
    #[test]
    #[should_panic(expected = "fixed SRS noncanonical x")]
    fn rejects_noncanonical_coordinate() {
        let mut b = encode(&[Vesta::generator()]);
        for (i, w) in Fq::MODULUS.0.iter().enumerate() {
            b[HEADER + 8 * i..HEADER + 8 * i + 8].copy_from_slice(&w.to_le_bytes());
        }
        decode(&b, 1).point::<VestaParameters>(0, 0);
    }
    #[test]
    #[should_panic(expected = "fixed SRS infinity flag")]
    fn rejects_invalid_identity_flag() {
        // Test the general-table entry infinity check (triggered when table contains identity).
        let mut b = encode(&[Vesta::zero(), Vesta::generator()]);
        b[HEADER + 64] = 2;
        decode(&b, 2).point::<VestaParameters>(0, 0);
    }
    #[test]
    #[should_panic(expected = "fixed SRS infinity flag")]
    fn rejects_invalid_header_flags() {
        let mut b = encode(&[Vesta::generator()]);
        b[18] = 2;
        decode(&b, 1);
    }
    #[test]
    fn coupled_finite_vesta_windows_16_15_14_match_reference() {
        use mina_curves::pasta::{canonical_words::CanonicalPasta, point_spikes};
        let g = Vesta::generator();
        let bases: Vec<Vesta> = (1..=16u64)
            .map(|k| (g * Fp::from(k * k + 3)).into_affine())
            .collect();
        let mut seed = 0x51e7_a015_1416u64;
        let mut scalars_fp = vec![
            Fp::ZERO,
            Fp::from(1u64),
            -Fp::from(1u64),
            Fp::from_bigint(BigInt([u64::MAX, (1u64 << 56) - 1, 0, 0])).unwrap(), // 2^120 - 1
            Fp::from_bigint(BigInt([0, 1u64 << 56, 0, 0])).unwrap(),              // 2^120
            Fp::from_bigint(BigInt([u64::MAX, (1u64 << 62) - 1, 0, 0])).unwrap(), // 2^126 - 1
            Fp::from_bigint(BigInt([u64::MAX, u64::MAX, 0, 0])).unwrap(),         // 2^128 - 1
            Fp::from_bigint(BigInt([0, 0, 1, 0])).unwrap(),                       // 2^128
            Fp::from_bigint(BigInt([u64::MAX, u64::MAX, u64::MAX, (1u64 << 48) - 1])).unwrap(), // 2^240 - 1
            Fp::from_bigint(BigInt([0, 0, 0, 1u64 << 48])).unwrap(),              // 2^240
            Fp::from_bigint(BigInt([u64::MAX, u64::MAX, u64::MAX, (1u64 << 60) - 1])).unwrap(), // 2^252 - 1
            Fp::from_bigint(BigInt([0, 0, 0, 1u64 << 60])).unwrap(),              // 2^252
        ];
        while scalars_fp.len() < bases.len() {
            let mut limbs = [0u64; 4];
            for w in &mut limbs {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                *w = seed;
            }
            limbs[3] &= (1u64 << 62) - 1;
            if let Some(f) = Fp::from_bigint(BigInt(limbs)) {
                scalars_fp.push(f);
            }
        }
        let expected: <Vesta as AffineRepr>::Group = bases
            .iter()
            .zip(&scalars_fp)
            .fold(Zero::zero(), |acc, (b, s)| acc + b.mul_bigint(s.into_bigint()));
        let canonical: Vec<_> = scalars_fp
            .iter()
            .map(|s| CanonicalPasta::<false>::from_limbs(s.into_bigint().0).unwrap())
            .collect();

        for (width, shift) in [(16usize, 128usize), (15, 120), (14, 126)] {
            let blob = encode_with_shift(&bases, shift);
            let table = decode(&blob, bases.len());
            assert_eq!(table.lane_shift(), shift);
            let coords = table.finite_coords().expect("all-finite table must be packed");
            let actual = point_spikes::msm_vesta_finite_borrowed::<VestaParameters>(
                coords, &canonical, width, shift,
            );
            assert_eq!(actual, expected, "width={width} shift={shift} mismatch");
        }
    }
    #[test]
    #[should_panic(expected = "fixed SRS base field")]
    fn rejects_other_curve_field() {
        let b = encode(&[Vesta::generator()]);
        decode(&b, 1).point::<PallasParameters>(0, 0);
    }
}
