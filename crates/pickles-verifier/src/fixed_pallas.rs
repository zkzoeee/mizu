//! Build-derived positional Pallas bases. This private view is attached only to
//! the immutable embedded verifier, never to a runtime verifier or proof input.
use alloc::vec::Vec;
use ark_ec::{
    short_weierstrass::{Affine, SWCurveConfig},
    AffineRepr, CurveGroup,
};
use ark_ff::{AdditiveGroup, BigInt, PrimeField};
use bytemuck::{Pod, Zeroable};
use mina_curves::pasta::{Fq, Fp, Pallas};

pub(crate) const MAGIC: &[u8; 8] = b"ZKPFIX01";
pub(crate) const HEADER: usize = 88;
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct EncodedPoint {
    x: [u64; 4],
    y: [u64; 4],
    infinity: u64,
}

#[derive(Clone, Copy)]
enum FixedPallasStorage<'a> {
    Finite(&'a [[u64; 8]]),
    General(&'a [EncodedPoint]),
}

#[derive(Clone, Copy)]
pub(crate) struct FixedPallasSrs<'a> {
    storage: FixedPallasStorage<'a>,
    n: usize,
    lane_shift: usize,
}
impl<'a> FixedPallasSrs<'a> {
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
            FixedPallasStorage::Finite(coords) => Some(coords),
            FixedPallasStorage::General(_) => None,
        }
    }
    #[cfg(test)]
    pub(crate) fn point<C: SWCurveConfig>(&self, lane: usize, i: usize) -> Affine<C>
    where
        C::BaseField: PrimeField<BigInt = BigInt<4>>,
    {
        assert_eq!(C::BaseField::MODULUS, Fp::MODULUS, "fixed SRS base field");
        assert!(lane < 2 && i < self.n, "fixed SRS point index");
        match self.storage {
            FixedPallasStorage::Finite(coords) => {
                let c = &coords[lane * self.n + i];
                Affine::new_unchecked(
                    C::BaseField::from_bigint(BigInt([c[0], c[1], c[2], c[3]]))
                        .expect("fixed SRS noncanonical x"),
                    C::BaseField::from_bigint(BigInt([c[4], c[5], c[6], c[7]]))
                        .expect("fixed SRS noncanonical y"),
                )
            }
            FixedPallasStorage::General(entries) => {
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
    pub(crate) fn rows(&self) -> &[[u64; 9]] {
        match self.storage {
            FixedPallasStorage::General(entries) => bytemuck::cast_slice(entries),
            FixedPallasStorage::Finite(_) => panic!("finite Pallas table uses finite_coords"),
        }
    }
}

/// Active positional plane-1 bit shift for the fixed Pallas table.
pub(crate) fn configured_lane_shift() -> usize {
    if matches!(option_env!("MIZU_FIXED_PALLAS_COUPLED"), Some("1"))
        && matches!(option_env!("MIZU_PALLAS_WINDOW"), None | Some("13"))
    {
        130
    } else {
        128
    }
}

pub(crate) fn encode(g: &[Pallas]) -> Vec<u8> {
    let shift = if g.iter().all(|p| !p.infinity) {
        configured_lane_shift()
    } else {
        128
    };
    encode_with_shift(g, shift)
}

pub(crate) fn encode_with_shift(g: &[Pallas], lane_shift: usize) -> Vec<u8> {
    assert!(
        matches!(lane_shift, 128 | 130),
        "unsupported fixed Pallas lane shift {lane_shift}"
    );
    // Host-only: fixture suites encode the same SRS for many VKs. Cache by
    // every generator byte and lane shift so changed-SRS tests remain real.
    #[cfg(feature = "std")]
    {
        use blake2::{Blake2s256, Digest};
        use std::{
            collections::BTreeMap,
            sync::{Mutex, OnceLock},
        };
        static CACHE: OnceLock<Mutex<BTreeMap<[u8; 32], Vec<u8>>>> = OnceLock::new();
        let mut hash = Blake2s256::new();
        hash.update(b"fixed-pallas-srs/v2");
        hash.update((lane_shift as u64).to_le_bytes());
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
            .or_insert_with(|| encode_uncached(g, lane_shift))
            .clone()
    }
    #[cfg(not(feature = "std"))]
    encode_uncached(g, lane_shift)
}

fn encode_uncached(g: &[Pallas], lane_shift: usize) -> Vec<u8> {
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
    let high = <Pallas as AffineRepr>::Group::normalize_batch(&high);
    let all_finite = g.iter().chain(&high).all(|p| !p.infinity);
    let entry_bytes = if all_finite { 64 } else { 72 };
    let mut out = Vec::with_capacity(HEADER + 2 * g.len() * entry_bytes);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&(g.len() as u64).to_le_bytes());
    let meta: u64 = 2u64 | (u64::from(all_finite) << 16) | ((lane_shift as u64) << 32);
    out.extend_from_slice(&meta.to_le_bytes());
    for modulus in [Fp::MODULUS, Fq::MODULUS] {
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

pub(crate) fn decode(bytes: &[u8], expected: usize) -> FixedPallasSrs<'_> {
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
    assert!(flags <= 1, "fixed SRS infinity flag");
    assert!(matches!(lane_shift, 128 | 130), "fixed SRS lane shift");
    let all_finite = flags == 1;
    for (at, modulus) in [(24, Fp::MODULUS), (56, Fq::MODULUS)] {
        assert_eq!(
            core::array::from_fn::<_, 4, _>(|i| word(at + 8 * i)),
            modulus.0,
            "fixed SRS field tag"
        );
    }
    let count = n.checked_mul(2).expect("fixed SRS length overflow");
    let entry_bytes = if all_finite { 64 } else { 72 };
    let size = count
        .checked_mul(entry_bytes)
        .and_then(|x| x.checked_add(HEADER))
        .expect("fixed SRS length overflow");
    assert_eq!(bytes.len(), size, "fixed SRS byte length");
    let storage = if all_finite {
        let coords: &[[u64; 8]] =
            bytemuck::try_cast_slice(&bytes[HEADER..]).expect("fixed SRS alignment");
        FixedPallasStorage::Finite(coords)
    } else {
        let entries: &[EncodedPoint] =
            bytemuck::try_cast_slice(&bytes[HEADER..]).expect("fixed SRS alignment");
        FixedPallasStorage::General(entries)
    };
    FixedPallasSrs {
        storage,
        n,
        lane_shift,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ark_ff::Zero;
    use mina_curves::pasta::{VestaParameters, PallasParameters};
    #[test]
    fn positional_layers_match_native_multiplication_with_identities() {
        let g = Pallas::generator();
        let points = [Pallas::zero(), g, -g, (g * Fq::from(17u64)).into_affine()];
        let bytes = encode(&points);
        let table = decode(&bytes, points.len());
        for (i, p) in points.iter().enumerate() {
            assert_eq!(table.point::<PallasParameters>(0, i), *p);
            assert_eq!(
                table.point::<PallasParameters>(1, i),
                p.mul_bigint(BigInt([0, 0, 1, 0])).into_affine()
            );
        }
    }
    #[test]
    #[should_panic(expected = "fixed SRS field tag")]
    fn rejects_wrong_field_tag() {
        let mut b = encode(&[Pallas::generator()]);
        b[24] ^= 1;
        decode(&b, 1);
    }
    #[test]
    #[should_panic(expected = "fixed SRS byte length")]
    fn rejects_truncated_table() {
        let mut b = encode(&[Pallas::generator()]);
        b.pop();
        decode(&b, 1);
    }
    #[test]
    #[should_panic(expected = "fixed SRS generator count")]
    fn rejects_wrong_length() {
        let b = encode(&[Pallas::generator()]);
        decode(&b, 2);
    }
    #[test]
    #[should_panic(expected = "fixed SRS noncanonical x")]
    fn rejects_noncanonical_coordinate() {
        let mut b = encode(&[Pallas::generator()]);
        for (i, w) in Fp::MODULUS.0.iter().enumerate() {
            b[HEADER + 8 * i..HEADER + 8 * i + 8].copy_from_slice(&w.to_le_bytes());
        }
        decode(&b, 1).point::<PallasParameters>(0, 0);
    }
    #[test]
    #[should_panic(expected = "fixed SRS infinity flag")]
    fn rejects_invalid_identity_flag() {
        let mut b = encode(&[Pallas::zero(), Pallas::generator()]);
        b[HEADER + 64] = 2;
        decode(&b, 2).point::<PallasParameters>(0, 0);
    }
    #[test]
    #[should_panic(expected = "fixed SRS infinity flag")]
    fn rejects_invalid_header_flags() {
        let mut b = encode(&[Pallas::generator()]);
        b[18] = 2;
        decode(&b, 1);
    }
    #[test]
    fn coupled_and_uncoupled_finite_pallas_tables_match_reference() {
        let g = Pallas::generator();
        let bases: Vec<Pallas> = (1..=12u64)
            .map(|k| (g * Fq::from(k * 7 + 5)).into_affine())
            .collect();
        let prefix_fq = [
            Fq::ZERO,
            Fq::from(1u64),
            -Fq::from(1u64),
            Fq::from_bigint(BigInt([u64::MAX, u64::MAX, 0, 0])).unwrap(), // 2^128 - 1
            Fq::from_bigint(BigInt([0, 0, 1, 0])).unwrap(),               // 2^128
            Fq::from_bigint(BigInt([u64::MAX, u64::MAX, 3, 0])).unwrap(), // 2^130 - 1
            Fq::from_bigint(BigInt([0, 0, 4, 0])).unwrap(),               // 2^130
            Fq::from_bigint(BigInt([0x1234, 0x5678, 0x9abc, 0x3fff_ffff_ffff_ffff])).unwrap(),
        ];
        let prefix_ks: Vec<BigInt<4>> = prefix_fq.iter().map(|s| s.into_bigint()).collect();
        let h = (g * Fq::from(31u64)).into_affine();
        let hk = (-Fq::from(17u64)).into_bigint();
        let tail = [
            (g * Fq::from(101u64)).into_affine(),
            Pallas::zero(),
            -(g * Fq::from(101u64)).into_affine(),
        ];
        let tk = [
            Fq::from(123456789u64).into_bigint(),
            Fq::from(42u64).into_bigint(),
            (-Fq::from(999u64)).into_bigint(),
        ];
        let mut expected = h.mul_bigint(hk);
        for (p, k) in bases[..prefix_ks.len()].iter().zip(&prefix_ks) {
            expected += p.mul_bigint(*k);
        }
        for (p, k) in tail.iter().zip(&tk) {
            expected += p.mul_bigint(*k);
        }
        for shift in [128usize, 130usize] {
            let blob = encode_with_shift(&bases, shift);
            let table = decode(&blob, bases.len());
            assert_eq!(table.lane_shift(), shift);
            let coords = table.finite_coords().expect("all-finite Pallas table must be packed");
            let actual = mina_curves::pasta::canonical_pallas::msm_bigint_fixed_prefix_coords(
                coords,
                shift,
                &prefix_ks,
                &h,
                hk,
                &tail,
                &tk,
                Some(13),
            );
            assert_eq!(actual, expected, "Pallas shift={shift} mismatch");
        }
    }
    #[test]
    #[should_panic(expected = "fixed SRS base field")]
    fn rejects_other_curve_field() {
        let b = encode(&[Pallas::generator()]);
        decode(&b, 1).point::<VestaParameters>(0, 0);
    }
}

/// Bound to the immutable verifier's owned SRS buffer. The IPA passes its
/// original generator slice separately from h/tail, so identity is checked
/// without re-reading/copying every generator. Mismatched/larger prefixes use
/// the ordinary backend with all supplied points, never the table.
pub(crate) fn msm(
    bound: &[Pallas], table: &FixedPallasSrs<'_>,
    prefix: &[Pallas], ks: &[BigInt<4>], h: &Pallas, hk: &BigInt<4>,
    tail: &[Pallas], tk: &[BigInt<4>],
) -> <Pallas as AffineRepr>::Group {
    assert_eq!(prefix.len(),ks.len(),"fixed Pallas scalar count");
    assert_eq!(tail.len(),tk.len(),"fixed Pallas tail count");
    if prefix.as_ptr() != bound.as_ptr() || prefix.len()>bound.len() || prefix.len()>table.len() {
        let mut points=Vec::with_capacity(1+prefix.len()+tail.len());
        let mut scalars=Vec::with_capacity(points.capacity());
        points.push(*h);points.extend_from_slice(prefix);points.extend_from_slice(tail);
        scalars.push(*hk);scalars.extend_from_slice(ks);scalars.extend_from_slice(tk);
        return mina_curves::pasta::canonical_pallas::msm_bigint(&points,&scalars);
    }
    let width=option_env!("MIZU_PALLAS_WINDOW").map(|s|s.parse().expect("Pallas fixed window"));
    if let Some(coords) = table.finite_coords() {
        return mina_curves::pasta::canonical_pallas::msm_bigint_fixed_prefix_coords(
            coords,
            table.lane_shift(),
            ks,
            h,
            *hk,
            tail,
            tk,
            width,
        );
    }
    mina_curves::pasta::canonical_pallas::msm_bigint_fixed_prefix(table.rows(),ks,h,*hk,tail,tk,width)
}

#[cfg(test)]
mod binding_tests {
    use super::*;
    use ark_ff::{Field,Zero};
    fn reference(p:&[Pallas], k:&[BigInt<4>], h:&Pallas, hk:BigInt<4>) -> <Pallas as AffineRepr>::Group {
        p.iter().zip(k).fold(h.mul_bigint(hk), |s,(p,k)|s+p.mul_bigint(*k))
    }
    #[test]
    fn fixed_and_mismatched_prefixes_preserve_the_group_equation() {
        let g=Pallas::generator();let bases=[g,-g,Pallas::zero(),(g*Fq::from(17u64)).into_affine()];
        let b=encode(&bases);let table=decode(&b,bases.len());
        let k=[BigInt([1,0,0,0]),(-Fq::ONE).into_bigint(),BigInt([0,0,1,0]),BigInt([u64::MAX,u64::MAX,0,0])];
        let h=(g*Fq::from(31u64)).into_affine();let hk=BigInt([7,0,0,0]);
        assert_eq!(msm(&bases,&table,&bases,&k,&h,&hk,&[],&[]),reference(&bases,&k,&h,hk));
        let mut changed=bases;changed[0]=(changed[0]+g).into_affine();
        assert_eq!(msm(&bases,&table,&changed,&k,&h,&hk,&[],&[]),reference(&changed,&k,&h,hk));
        let small_b=encode(&bases[..2]);let small=decode(&small_b,2);
        assert_eq!(msm(&bases,&small,&bases,&k,&h,&hk,&[],&[]),reference(&bases,&k,&h,hk));
    }
}
