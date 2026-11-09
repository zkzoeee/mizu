//! Synthetic opening records exercise the verifier only; no proofs are generated.
use ark_ec::{AffineRepr, CurveGroup};
use ark_ff::{AdditiveGroup, Field, PrimeField, UniformRand};
use groupmap::GroupMap;
use mina_curves::pasta::{Fq, Pallas, PallasParameters};
use mina_poseidon::{constants::PlonkSpongeConstantsKimchi, pasta::FULL_ROUNDS,
    sponge::DefaultFqSponge, FqSponge};
use poly_commitment::{commitment::{b_poly_coefficients, shift_scalar, BatchEvaluationProof,
    CommitmentCurve, Evaluation}, ipa::{endos, OpeningProof, SRS}, PolyComm};
use rand_chacha::ChaCha20Rng;
use rand_core::{RngCore, SeedableRng};
use std::cell::Cell;

type Sponge = DefaultFqSponge<PallasParameters, PlonkSpongeConstantsKimchi, FULL_ROUNDS>;
type Opening = OpeningProof<Pallas, FULL_ROUNDS>;
type Batch<'a> = BatchEvaluationProof<'a, Pallas, Sponge, Opening, FULL_ROUNDS>;
fn point(n: u64) -> Pallas { (Pallas::generator() * Fq::from(n)).into_affine() }
fn batch(openings: &[Opening]) -> Vec<Batch<'_>> {
    openings.iter().enumerate().map(|(i, opening)| BatchEvaluationProof {
        sponge: Sponge::new(mina_poseidon::pasta::fp_kimchi::static_params()),
        opening, evaluation_points: vec![Fq::from(3u64), Fq::from(7u64)],
        polyscale: Fq::from(11u64), evalscale: Fq::from(13u64),
        evaluations: vec![Evaluation { commitment: PolyComm { chunks: vec![point(31), Pallas::zero(), point(37)] },
            evaluations: vec![vec![Fq::ONE; 3]; 2] }],
        combined_inner_product: Fq::from(19u64 + i as u64),
    }).collect()
}
#[test]
fn borrowed_prefix_preserves_coefficients_tail_order_and_rng_for_batch_geometries() {
    let mut srs = SRS::<Pallas>::default();
    srs.g = (1..=16).map(point).collect();srs.g[2]=Pallas::zero();srs.g[3]=-srs.g[0];srs.h=point(23);
    let map = <Pallas as CommitmentCurve>::Map::setup();
    for rounds in [vec![], vec![0], vec![1], vec![3], vec![0,3], vec![3,1]] {
        let openings: Vec<Opening> = rounds.iter().enumerate().map(|(i,&r)| Opening {
            lr: (0..r).map(|j|(point(41+2*j as u64),point(42+2*j as u64))).collect(),
            delta: point(61+i as u64),sg:point(71+i as u64),
            z1:Fq::from(29u64+i as u64),z2:Fq::from(43u64+i as u64),
        }).collect();
        let active=rounds.iter().map(|r|1usize<<r).max().unwrap_or(0);
        let mut expected_rng=ChaCha20Rng::from_seed([93;32]);
        let rand_base=Fq::rand(&mut expected_rng);let sg_base=Fq::rand(&mut expected_rng);
        let (mut rand_i,mut sg_i)=(Fq::ONE,Fq::ONE);
        let mut expected_h=Fq::ZERO;let mut expected_prefix=vec![Fq::ZERO;active];let mut expected_tail=Vec::new();
        for mut proof in batch(&openings) {
            proof.sponge.absorb_fr(&[shift_scalar::<Pallas>(proof.combined_inner_product)]);
            let (x,y)=map.to_group(proof.sponge.challenge_fq());let u=Pallas::of_coordinates(x,y);
            let challenges=proof.opening.challenges::<Sponge>(&endos::<Pallas>().1,&mut proof.sponge);
            let coefficients=b_poly_coefficients(&challenges.chal);
            for (dst,src) in expected_prefix.iter_mut().zip(coefficients) {*dst+=sg_i*src;}
            expected_h-=rand_i*proof.opening.z2;
            expected_tail.extend([proof.opening.sg,u]);
            for (l,r) in &proof.opening.lr {expected_tail.extend([*l,*r]);}
            expected_tail.extend_from_slice(&proof.evaluations[0].commitment.chunks);
            expected_tail.extend([u,proof.opening.delta]);rand_i*=rand_base;sg_i*=sg_base;
        }
        let called=Cell::new(false);
        let msm=|prefix:&[Pallas], ks:&[ark_ff::BigInt<4>], h:&Pallas, hk:&ark_ff::BigInt<4>, tail:&[Pallas], tk:&[ark_ff::BigInt<4>]| {
            assert!(!called.replace(true));assert_eq!(prefix.as_ptr(),srs.g.as_ptr());
            assert_eq!(prefix,&srs.g[..active]);assert_eq!(*h,srs.h);
            assert_eq!(*hk,expected_h.into_bigint());
            assert_eq!(ks,expected_prefix.iter().map(|k|k.into_bigint()).collect::<Vec<_>>());
            assert_eq!(tail,expected_tail);assert_eq!(tail.len(),tk.len());
            // Independent native group sum rather than another optimized MSM.
            prefix.iter().zip(ks).chain(tail.iter().zip(tk))
                .fold(h.mul_bigint(*hk),|sum,(p,k)|sum+p.mul_bigint(*k))
        };
        let mut segmented_rng=ChaCha20Rng::from_seed([93;32]);let mut ordinary_rng=segmented_rng.clone();
        let segmented=srs.verify_with_msm(&map,&mut batch(&openings),&mut segmented_rng,Some(&msm));
        let ordinary=srs.verify(&map,&mut batch(&openings),&mut ordinary_rng);
        assert!(called.get());assert_eq!(segmented,ordinary);
        let expected_next=expected_rng.next_u64();
        assert_eq!(segmented_rng.next_u64(),expected_next);
        assert_eq!(ordinary_rng.next_u64(),expected_next);
    }
}
