//! Native wall-clock timing for the Pallas MSM schedules.
//!
//! Not a correctness test — `#[ignore]`d, run explicitly:
//!
//! ```sh
//! RAYON_NUM_THREADS=1 CARGO_BUILD_JOBS=1 cargo test --profile native-test --locked \
//!   -p pickles-verifier --test pallas_msm_native_timing -- --ignored --nocapture
//! ```
//!
//! Why this exists. Mizu's MSM work was measured in SP1 cycles, a cost model
//! where a Pasta add is ~98 cycle-equivalents. o1js's prover runs the same
//! crates natively (`kimchi-napi`) or in WASM (`kimchi-wasm`), so the question
//! "does our GLV help o1js" needs a native wall-clock answer, which no existing
//! harness produces. `poly-commitment`'s own `--bench msm` cannot answer it:
//! it calls arkworks' `VariableBaseMSM::msm`, while our schedules live in the
//! standalone `canonical_pallas::msm_bigint`, which a caller must opt into.
//!
//! Every arm computes the same MSM and the results are asserted equal, so a
//! timing difference cannot come from one arm doing less work.
use ark_ec::{AffineRepr, CurveGroup, VariableBaseMSM};
use ark_ff::{BigInt, PrimeField};
use mina_curves::pasta::{
    canonical_pallas::{msm_bigint_with_config, MsmSpikeConfig},
    Fq, Pallas, ProjectivePallas,
};
use std::time::{Duration, Instant};

fn splitmix(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Distinct bases by repeated addition, normalised in one batch -- a per-point
/// `into_affine` would pay an inversion each and dominate the measurement.
fn inputs(n: usize) -> (Vec<Pallas>, Vec<BigInt<4>>) {
    let g = ProjectivePallas::from(Pallas::generator());
    let mut acc = g;
    let mut projective = Vec::with_capacity(n);
    for _ in 0..n {
        acc += g;
        projective.push(acc);
    }
    let bases = ProjectivePallas::normalize_batch(&projective);
    let mut state = 0x243F_6A88_85A3_08D3u64;
    let scalars = (0..n)
        .map(|_| {
            let mut bytes = [0u8; 32];
            for chunk in bytes.chunks_mut(8) {
                chunk.copy_from_slice(&splitmix(&mut state).to_le_bytes());
            }
            Fq::from_le_bytes_mod_order(&bytes).into_bigint()
        })
        .collect();
    (bases, scalars)
}

fn best_of<F: FnMut() -> ProjectivePallas>(reps: usize, mut f: F) -> (Duration, ProjectivePallas) {
    let mut best = Duration::MAX;
    let mut out = ProjectivePallas::from(Pallas::generator());
    for _ in 0..reps {
        let t = Instant::now();
        out = f();
        best = best.min(t.elapsed());
    }
    (best, out)
}

#[test]
#[ignore = "timing benchmark, not a correctness check"]
fn glv_against_affine_and_arkworks() {
    let affine_only = MsmSpikeConfig {
        filter_zero: true,
        trim_conversions: true,
        affine: true,
        ..Default::default()
    };
    // GLV needs affine reduction or the point backend, per the field's own doc.
    let affine_glv = MsmSpikeConfig {
        glv: true,
        ..affine_only
    };

    println!("threads={:?}", std::thread::available_parallelism());
    println!(
        "{:>6}  {:>12}  {:>12}  {:>12}  {:>9}  {:>9}",
        "size", "arkworks", "affine", "affine+glv", "glv/ark", "glv/affine"
    );
    for log in [12usize, 14, 16] {
        let n = 1 << log;
        let (bases, scalars) = inputs(n);
        let reps = if log >= 16 { 3 } else { 5 };

        let (t_ark, r_ark) = best_of(reps, || {
            <Pallas as AffineRepr>::Group::msm_bigint(&bases, &scalars)
        });
        let (t_aff, r_aff) = best_of(reps, || {
            msm_bigint_with_config(&bases, &scalars, affine_only)
        });
        let (t_glv, r_glv) = best_of(reps, || {
            msm_bigint_with_config(&bases, &scalars, affine_glv)
        });

        // The whole measurement is void if the arms disagree.
        assert_eq!(r_ark, r_aff, "affine schedule disagrees with arkworks");
        assert_eq!(r_ark, r_glv, "glv schedule disagrees with arkworks");

        let ms = |d: Duration| d.as_secs_f64() * 1e3;
        println!(
            "2^{:<4}  {:>10.2}ms  {:>10.2}ms  {:>10.2}ms  {:>8.3}x  {:>8.3}x",
            log,
            ms(t_ark),
            ms(t_aff),
            ms(t_glv),
            ms(t_glv) / ms(t_ark),
            ms(t_glv) / ms(t_aff)
        );
    }
}
