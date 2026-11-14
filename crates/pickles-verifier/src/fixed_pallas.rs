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
pub(crate) struct FixedPallasSrs<'a> {
    entries: &'a [EncodedPoint],
    n: usize,
}
impl FixedPallasSrs<'_> {
    pub(crate) fn len(&self) -> usize {
        self.n
    }
    #[cfg(test)]
    pub(crate) fn point<C: SWCurveConfig>(&self, lane: usize, i: usize) -> Affine<C>
    where
        C::BaseField: PrimeField<BigInt = BigInt<4>>,
    {
        assert_eq!(C::BaseField::MODULUS, Fp::MODULUS, "fixed SRS base field");
        assert!(lane < 2 && i < self.n, "fixed SRS point index");
        let p = &self.entries[lane * self.n + i];
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
    pub(crate) fn rows(&self) -> &[[u64;9]] {
        bytemuck::cast_slice(self.entries)
    }

}

pub(crate) fn encode(g: &[Pallas]) -> Vec<u8> {
    // Host-only: fixture suites encode the same SRS for many VKs. Cache by
    // every generator byte, not pointer/length, so changed-SRS tests remain real.
    #[cfg(feature = "std")]
    {
        use blake2::{Blake2s256, Digest};
        use std::{
            collections::BTreeMap,
            sync::{Mutex, OnceLock},
        };
        static CACHE: OnceLock<Mutex<BTreeMap<[u8; 32], Vec<u8>>>> = OnceLock::new();
        let mut hash = Blake2s256::new();
        hash.update(b"fixed-pallas-srs/v1");
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
            .or_insert_with(|| encode_uncached(g))
            .clone()
    }
    #[cfg(not(feature = "std"))]
    encode_uncached(g)
}

fn encode_uncached(g: &[Pallas]) -> Vec<u8> {
    let high: Vec<_> = g
        .iter()
        .map(|p| {
            let mut p = p.into_group();
            for _ in 0..128 {
                p.double_in_place();
            }
            p
        })
        .collect();
    let high = <Pallas as AffineRepr>::Group::normalize_batch(&high);
    let mut out = Vec::with_capacity(HEADER + 2 * g.len() * 72);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&(g.len() as u64).to_le_bytes());
    out.extend_from_slice(&2u64.to_le_bytes());
    for modulus in [Fp::MODULUS, Fq::MODULUS] {
        for word in modulus.0 {
            out.extend_from_slice(&word.to_le_bytes());
        }
    }
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
    assert_eq!(word(16), 2, "fixed SRS lanes");
    for (at, modulus) in [(24, Fp::MODULUS), (56, Fq::MODULUS)] {
        assert_eq!(
            core::array::from_fn::<_, 4, _>(|i| word(at + 8 * i)),
            modulus.0,
            "fixed SRS field tag"
        );
    }
    let count = n.checked_mul(2).expect("fixed SRS length overflow");
    let size = count
        .checked_mul(72)
        .and_then(|x| x.checked_add(HEADER))
        .expect("fixed SRS length overflow");
    assert_eq!(bytes.len(), size, "fixed SRS byte length");
    let entries = bytemuck::try_cast_slice(&bytes[HEADER..]).expect("fixed SRS alignment");
    FixedPallasSrs { entries, n }
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
        let mut b = encode(&[Pallas::generator()]);
        b[HEADER + 64] = 2;
        decode(&b, 1).point::<PallasParameters>(0, 0);
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
