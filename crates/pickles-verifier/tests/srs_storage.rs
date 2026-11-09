//! Container/wire/basis checks only; this suite never generates an opening or
//! SP1 proof. Existing Mina fixtures exercise verification separately.
use ark_ec::{AffineRepr, CurveGroup};
use ark_ff::{One, Zero};
use ark_poly::{
    univariate::DensePolynomial, DenseUVPolynomial, EvaluationDomain, Radix2EvaluationDomain,
};
use mina_curves::pasta::{Fq, Pallas};
use poly_commitment::{generators::Generators, ipa::SRS, SRS as _};
use serde::{Deserialize, Serialize};
use serde_with::serde_as;

fn points() -> Vec<Pallas> {
    let g = Pallas::generator();
    vec![g, -g, Pallas::zero(), (g + g).into_affine()]
}
fn static_points() -> &'static [Pallas] {
    Box::leak(points().into_boxed_slice())
}
#[test]
fn static_generators_borrow_clone_and_clone_on_write_preserve_ownership() {
    let raw = static_points();
    let borrowed = Generators::borrowed(raw);
    assert!(borrowed.is_borrowed());
    assert_eq!(borrowed.as_ptr(), raw.as_ptr());
    assert_eq!(borrowed.len(), 4);
    let mut changed = borrowed.clone();
    assert_eq!(changed.as_ptr(), raw.as_ptr());
    changed[0] = Pallas::zero();
    assert!(!changed.is_borrowed());
    assert_ne!(changed.as_ptr(), raw.as_ptr());
    assert_eq!(raw, points());
    assert_eq!(borrowed.as_slice(), raw);
    assert_eq!(changed[0], Pallas::zero());
    changed.to_mut().truncate(2);
    changed.to_mut().push(Pallas::generator());
    assert_eq!(changed.len(), 3);
    let owned = Generators::from(points());
    let ptr = owned.as_ptr();
    let moved = owned.into_owned();
    assert_eq!(ptr, moved.as_ptr());
    assert_eq!(borrowed.into_owned(), raw);
    let empty = Generators::<Pallas>::default();
    assert!(!empty.is_borrowed());
    assert!(empty.is_empty());
}
#[serde_as]
#[derive(Serialize, Deserialize)]
struct LegacySrs {
    #[serde_as(as = "Vec<o1_utils::serialization::SerdeAs>")]
    g: Vec<Pallas>,
    #[serde_as(as = "o1_utils::serialization::SerdeAs")]
    h: Pallas,
}
#[test]
fn owned_and_borrowed_srs_keep_the_exact_legacy_json_and_binary_bytes() {
    let mut owned = SRS::<Pallas>::default();
    owned.g = points().into();
    owned.h = Pallas::generator();
    let mut borrowed = owned.clone();
    borrowed.g = Generators::borrowed(static_points());
    let legacy = LegacySrs {
        g: points(),
        h: owned.h,
    };
    for bytes in [
        postcard::to_allocvec(&owned).unwrap(),
        postcard::to_allocvec(&borrowed).unwrap(),
    ] {
        assert_eq!(bytes, postcard::to_allocvec(&legacy).unwrap());
        let decoded: SRS<Pallas> = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(decoded, owned);
        assert!(!decoded.g.is_borrowed());
        let prior: LegacySrs = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(prior.g, points());
        assert_eq!(prior.h, owned.h);
    }
    let json = serde_json::to_vec(&legacy).unwrap();
    assert_eq!(serde_json::to_vec(&owned).unwrap(), json);
    assert_eq!(serde_json::to_vec(&borrowed).unwrap(), json);
    let decoded: SRS<Pallas> = serde_json::from_slice(&json).unwrap();
    assert_eq!(decoded, owned);
    assert!(!decoded.g.is_borrowed());
    let empty: SRS<Pallas> =
        postcard::from_bytes(&postcard::to_allocvec(&SRS::<Pallas>::default()).unwrap()).unwrap();
    assert!(empty.g.is_empty());
    assert!(!empty.g.is_borrowed());
}
#[test]
fn borrowed_srs_preserves_full_and_prefix_basis_and_polynomial_commitments() {
    let mut owned = SRS::<Pallas>::default();
    owned.g = points().into();
    owned.h = Pallas::generator();
    let raw = static_points();
    let mut borrowed = SRS::<Pallas>::default();
    borrowed.g = Generators::borrowed(raw);
    borrowed.h = owned.h;
    let domain = Radix2EvaluationDomain::<Fq>::new(4).unwrap();
    let old = owned.get_lagrange_basis(domain);
    let new = borrowed.get_lagrange_basis(domain);
    assert_eq!(*old, *new);
    assert_eq!(new.len(), 4);
    for n in [0, 1, 3, 4] {
        assert_eq!(
            *owned.get_lagrange_basis_prefix(domain, n),
            *borrowed.get_lagrange_basis_prefix(domain, n)
        );
        assert_eq!(borrowed.get_lagrange_basis(domain).len(), 4);
    }
    for c in [
        vec![],
        vec![Fq::one()],
        vec![Fq::from(2u64); 3],
        vec![Fq::from(7u64); 9],
    ] {
        let polynomial = DensePolynomial::from_coefficients_vec(c);
        for chunks in [1, 2] {
            assert_eq!(
                owned.commit_non_hiding(&polynomial, chunks),
                borrowed.commit_non_hiding(&polynomial, chunks)
            );
        }
    }
    assert!(borrowed.g.is_borrowed());
    assert_eq!(borrowed.g.as_ptr(), raw.as_ptr());
}
