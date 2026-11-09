//! Host-only positional precomputation for the fixed Vesta accumulator SRS.
//!
//! This is a 2/4-lane radix-2 positional table, not an Udon alpha table. For
//! width = 256 / lanes, layer j stores [2^(j * width)]G_i. All records are
//! layer-major and contain canonical little-endian x/y limbs, without flags.
//! The generator reads the same production SRS as the settlement build script.
//! It neither builds nor runs an SP1 guest and never generates a proof.

use anyhow::{ensure, Context, Result};
use ark_ec::{AffineRepr, CurveGroup};
use ark_ff::{AdditiveGroup, PrimeField};
use clap::Parser;
use mina_curves::pasta::{ProjectiveVesta, Vesta};
use pickles_verifier::types::STEP_IPA_ROUNDS;
use poly_commitment::precomputed_srs::get_srs;
use std::{
    fs::{self, File},
    io::{BufWriter, Write},
    path::PathBuf,
    time::Instant,
};

const MAGIC: &[u8; 8] = b"ZKVESTA1";
const NORMALIZATION_BATCH_SIZE: usize = 1024;

#[derive(Debug, Parser)]
#[command(about = "Generate a host-only 2/4-lane positional Vesta SRS table")]
struct Args {
    /// Number of scalar lanes: 2 uses 128-bit chunks, 4 uses 64-bit chunks.
    #[arg(long, value_parser = parse_lanes)]
    lanes: usize,
    #[arg(long)]
    output: PathBuf,
}

fn parse_lanes(value: &str) -> std::result::Result<usize, String> {
    match value {
        "2" => Ok(2),
        "4" => Ok(4),
        _ => Err("lanes must be 2 or 4".to_owned()),
    }
}

fn main() -> Result<()> {
    let args = Args::parse();
    let started = Instant::now();
    let srs = get_srs::<Vesta>();
    let n = 1usize << STEP_IPA_ROUNDS;
    ensure!(
        srs.g.len() >= n,
        "production Vesta SRS has fewer than {n} bases"
    );
    if let Some(parent) = args.output.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let file =
        File::create(&args.output).with_context(|| format!("create {}", args.output.display()))?;
    let mut writer = BufWriter::new(file);
    write_prepared_table(&srs.g[..n], args.lanes, &mut writer)?;
    writer.flush().context("flush prepared Vesta table")?;
    println!(
        "wrote {}: {} lanes, {n} bases, {} bytes, {:.3}s",
        args.output.display(),
        args.lanes,
        24 + args.lanes * n * 64,
        started.elapsed().as_secs_f64(),
    );
    Ok(())
}

/// Write the 24-byte header followed by layer-major, 64-byte affine records.
///
/// Memory is bounded by one affine layer plus a 1024-point normalization batch.
/// Retaining each normalized layer lets the next one reuse its positional shift.
fn write_prepared_table<W: Write>(bases: &[Vesta], lanes: usize, writer: &mut W) -> Result<()> {
    ensure!(lanes == 2 || lanes == 4, "lanes must be 2 or 4");
    ensure!(!bases.is_empty(), "prepared table needs at least one base");
    ensure!(
        bases.iter().all(|point| !point.infinity),
        "SRS contains an identity point"
    );
    writer.write_all(MAGIC)?;
    writer.write_all(&(lanes as u64).to_le_bytes())?;
    writer.write_all(&(bases.len() as u64).to_le_bytes())?;

    let width = 256 / lanes;
    let mut layer = bases.to_vec();
    for lane in 0..lanes {
        for batch in layer.chunks_mut(NORMALIZATION_BATCH_SIZE) {
            if lane != 0 {
                let mut projective: Vec<_> = batch.iter().map(|point| point.into_group()).collect();
                for point in &mut projective {
                    for _ in 0..width {
                        point.double_in_place();
                    }
                }
                batch.copy_from_slice(&ProjectiveVesta::normalize_batch(&projective));
            }
            for point in batch.iter() {
                // The odd prime-order group preserves nonidentity under 2^width.
                ensure!(
                    !point.infinity,
                    "positional precomputation produced identity"
                );
                for coordinate in [point.x, point.y] {
                    for limb in coordinate.into_bigint().0 {
                        writer.write_all(&limb.to_le_bytes())?;
                    }
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ark_ff::{BigInt, Field, Zero};
    use mina_curves::pasta::{Fp, Fq};

    fn read_coordinate(bytes: &[u8]) -> Fq {
        let limbs = core::array::from_fn(|i| {
            u64::from_le_bytes(bytes[i * 8..(i + 1) * 8].try_into().unwrap())
        });
        Fq::from_bigint(BigInt(limbs)).expect("coordinate is canonical")
    }

    #[test]
    fn eight_point_tables_have_the_expected_layers_and_encoding() {
        let generator = Vesta::generator();
        let bases: Vec<_> = (1..=8)
            .map(|i| (generator * Fp::from(i as u64)).into_affine())
            .collect();
        for lanes in [2, 4] {
            let mut bytes = Vec::new();
            write_prepared_table(&bases, lanes, &mut bytes).unwrap();
            assert_eq!(&bytes[..8], MAGIC);
            assert_eq!(
                u64::from_le_bytes(bytes[8..16].try_into().unwrap()),
                lanes as u64
            );
            assert_eq!(u64::from_le_bytes(bytes[16..24].try_into().unwrap()), 8);
            assert_eq!(bytes.len(), 24 + lanes * 8 * 64);
            for lane in 0..lanes {
                let multiplier = Fp::from(2u64).pow([(lane * (256 / lanes)) as u64]);
                for (index, base) in bases.iter().enumerate() {
                    let offset = 24 + (lane * bases.len() + index) * 64;
                    let actual = Vesta::new_unchecked(
                        read_coordinate(&bytes[offset..offset + 32]),
                        read_coordinate(&bytes[offset + 32..offset + 64]),
                    );
                    assert!(actual.is_on_curve());
                    assert_eq!(actual, (*base * multiplier).into_affine());
                }
            }
        }
    }

    #[test]
    fn invalid_table_inputs_are_rejected_before_the_header() {
        for (bases, lanes) in [
            (vec![Vesta::generator()], 3),
            (Vec::new(), 2),
            (vec![Vesta::zero()], 4),
        ] {
            let mut bytes = Vec::new();
            assert!(write_prepared_table(&bases, lanes, &mut bytes).is_err());
            assert!(bytes.is_empty());
        }
    }
}
