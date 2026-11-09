use ark_ec::{AffineRepr,CurveGroup};
use ark_ff::{AdditiveGroup,BigInt,Field,PrimeField};
use mina_curves::pasta::{Fp,Fq,Pallas};
use mina_curves::pasta::canonical_pallas::msm_bigint_fixed_prefix;
fn table(g:&[Pallas])->Vec<[u64;9]> {
    g.iter().copied().chain(g.iter().map(|p|p.mul_bigint(BigInt([0,0,1,0])).into_affine()))
        .map(|p|{let mut w=[0;9];if p.infinity {w[8]=1;}else {w[..4].copy_from_slice(&p.x.into_bigint().0);w[4..8].copy_from_slice(&p.y.into_bigint().0);}w}).collect()
}
#[test]
fn fixed_prefix_and_glv_residuals_match_native_for_carry_and_field_boundaries() {
    let g=Pallas::generator();let bases=[g,-g,Pallas::zero(),g];let t=table(&bases);
    let cases=[Fq::ZERO,Fq::ONE,-Fq::ONE,Fq::from(2u64).pow([127]),Fq::from(2u64).pow([128])-Fq::ONE,Fq::from(2u64).pow([254])];
    for offset in 0..cases.len() {
        let ks:Vec<BigInt<4>>=(0..bases.len()).map(|i|cases[(i+offset)%cases.len()].into_bigint()).collect();
        let tail=[-g,g];let tk=[BigInt([u64::MAX;4]),BigInt([3,0,0,0])];let hk=BigInt([7,0,0,0]);
        let expected=bases.iter().zip(&ks).chain(tail.iter().zip(&tk)).fold(g.mul_bigint(hk),|s,(p,k)|s+p.mul_bigint(*k));
        assert_eq!(msm_bigint_fixed_prefix(&t,&ks,&g,hk,&tail,&tk,Some(13)),expected);
    }
}
#[test]
#[should_panic(expected="Pallas table noncanonical x")]
fn rejects_noncanonical_table_coordinate() {
    let g=Pallas::generator();let mut t=table(&[g]);t[0][..4].copy_from_slice(&Fp::MODULUS.0);
    msm_bigint_fixed_prefix(&t,&[BigInt([1,0,0,0])],&g,BigInt([0;4]),&[],&[],None);
}
#[test]
#[should_panic(expected="Pallas table infinity flag")]
fn rejects_invalid_table_flag() {
    let g=Pallas::generator();let mut t=table(&[g]);t[0][8]=2;
    msm_bigint_fixed_prefix(&t,&[BigInt([1,0,0,0])],&g,BigInt([0;4]),&[],&[],None);
}
#[test]
#[should_panic(expected="Pallas fixed scalar canonicality")]
fn rejects_noncanonical_prefix_scalar() {
    let g=Pallas::generator();msm_bigint_fixed_prefix(&table(&[g]),&[Fq::MODULUS],&g,BigInt([0;4]),&[],&[],None);
}
#[test]
#[should_panic(expected="Pallas residual lengths")]
fn rejects_missing_residual_scalar() {
    let g=Pallas::generator();msm_bigint_fixed_prefix(&table(&[g]),&[BigInt([0;4])],&g,BigInt([0;4]),&[g],&[],None);
}
