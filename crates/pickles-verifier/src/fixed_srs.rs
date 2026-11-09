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
pub(crate) struct FixedVestaSrs<'a> {
    entries: &'a [EncodedPoint],
    n: usize,
}
impl FixedVestaSrs<'_> {
    pub(crate) fn len(&self) -> usize {
        self.n
    }
    pub(crate) fn point<C: SWCurveConfig>(&self, lane: usize, i: usize) -> Affine<C>
    where
        C::BaseField: PrimeField<BigInt = BigInt<4>>,
    {
        assert_eq!(C::BaseField::MODULUS, Fq::MODULUS, "fixed SRS base field");
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
}

pub(crate) fn encode(g: &[Vesta]) -> Vec<u8> {
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
        hash.update(b"fixed-vesta-srs/v1");
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

fn encode_uncached(g: &[Vesta]) -> Vec<u8> {
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
    let high = <Vesta as AffineRepr>::Group::normalize_batch(&high);
    let mut out = Vec::with_capacity(HEADER + 2 * g.len() * 72);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&(g.len() as u64).to_le_bytes());
    out.extend_from_slice(&2u64.to_le_bytes());
    for modulus in [Fq::MODULUS, Fp::MODULUS] {
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

pub(crate) fn decode(bytes: &[u8], expected: usize) -> FixedVestaSrs<'_> {
    assert!(
        bytes.len() >= HEADER && &bytes[..8] == MAGIC,
        "fixed SRS header"
    );
    let word = |at| u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap());
    let n = usize::try_from(word(8)).expect("fixed SRS length overflow");
    assert_eq!(n, expected, "fixed SRS generator count");
    assert_eq!(word(16), 2, "fixed SRS lanes");
    for (at, modulus) in [(24, Fq::MODULUS), (56, Fp::MODULUS)] {
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
    FixedVestaSrs { entries, n }
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
        let mut b = encode(&[Vesta::generator()]);
        b[HEADER + 64] = 2;
        decode(&b, 1).point::<VestaParameters>(0, 0);
    }
    #[test]
    #[should_panic(expected = "fixed SRS base field")]
    fn rejects_other_curve_field() {
        let b = encode(&[Vesta::generator()]);
        decode(&b, 1).point::<PallasParameters>(0, 0);
    }
}
