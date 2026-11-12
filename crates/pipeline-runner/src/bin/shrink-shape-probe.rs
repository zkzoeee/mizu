//! Does the compress shape's `Select` bump reach the wrap verifying key?
//!
//! Stage 2 of `docs/groth16-ceremony-spike.md`. The wrap vk is
//!
//!   wrap vk = f(shrink shape, VK-map height, fixed recursion config, vk_verification)
//!
//! and `shrink_shape` is `preprocessed_table_heights` of the *shrink program*,
//! which is built from a **compress**-shaped dummy input
//! (`worker/prover/recursion.rs:1025-1048`). So this fork's bump of the compress
//! `Select` height from upstream's 1,087,808 to 1,087,840 is a candidate cause of
//! wrap-vk divergence, which would make upstream's released Groth16 artifacts
//! inapplicable and a ceremony ours to run.
//!
//! The compress heights enter the input through only two quantities
//! (`shapes.rs:358-382`):
//!
//! ```text
//! preprocessed_multiple = ceil( sum_chips(height * preprocessed_width) / 2^log_stacking_height )
//! main_multiple         = ceil( sum_chips(height * width)              / 2^log_stacking_height )
//! ```
//!
//! A 32-row delta divided by a stacking height of 2^21 changes those only if the
//! sums straddle a multiple of the stacking height. This probe settles it rather
//! than arguing from probability: it builds the dummy input and the shrink
//! program for both heights and compares them.
//!
//! Holds the VK-map height fixed, so the only variable is the compress shape.
//! Keygen-free and proving-free -- no GPU, no artifacts, no network.
use std::collections::BTreeSet;

use anyhow::{Context, Result};
use sp1_prover::{
    recursion::{recursive_verifier, shrink_program_from_input},
    shapes::SP1RecursionProofShape,
    CpuSP1ProverComponents, SP1ProverComponents,
};

/// Any value, as long as it is the same for both arms: this probe varies the
/// compress shape alone. The real height is `recursion_vks.height()`, which the
/// cluster trim moves from 19 to 18 and which is the *other* channel.
const FIXED_VK_MAP_HEIGHT: usize = 18;

fn sha(bytes: &[u8]) -> String {
    hex::encode(<sha2::Sha256 as sha2::Digest>::digest(bytes))
}

fn main() -> Result<()> {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "vendor/sp1/crates/prover/compress_shape.json".to_string());
    let base = std::fs::read_to_string(&path).with_context(|| format!("read {path}"))?;

    let verifier = CpuSP1ProverComponents::compress_verifier();
    let shard_verifier = verifier.shard_verifier();
    let chips: BTreeSet<_> = shard_verifier.machine().chips().iter().cloned().collect();
    // These three come from the MachineVerifier, not the ShardVerifier -- as the
    // real construction site does (worker/prover/recursion.rs:1033-1040).
    let max_log_row_count = verifier.max_log_row_count();
    let fri_config = *verifier.fri_config();
    let log_stacking_height = verifier.log_stacking_height() as usize;

    println!("compress machine: {} chips", chips.len());
    println!("log_stacking_height {log_stacking_height}, max_log_row_count {max_log_row_count}");
    println!("vk map height held at {FIXED_VK_MAP_HEIGHT} for both arms\n");

    // Arm A is the file as upstream pins it; arm B applies the overrides the
    // fork actually needs. Pass them as --override Chip=Height.
    let overrides: Vec<(String, u64)> = std::env::args()
        .collect::<Vec<_>>()
        .windows(2)
        .filter(|w| w[0] == "--override")
        .map(|w| {
            let (k, v) = w[1].split_once('=').expect("--override wants Chip=Height");
            (k.to_string(), v.parse().expect("height must be a number"))
        })
        .collect();
    if overrides.is_empty() {
        println!("no --override given; pass e.g. --override ExtAlu=1115323");
    }
    println!("overrides: {overrides:?}\n");

    let mut results = Vec::new();
    for (label, apply) in [("upstream", false), ("fork", true)] {
        let mut json = base.clone();
        if apply {
            for (chip, height) in &overrides {
                let pat = format!("\"{chip}\": ");
                let at = json.find(&pat).unwrap_or_else(|| panic!("{chip} not in the shape"));
                let start = at + pat.len();
                let end = start
                    + json[start..]
                        .find(|c: char| !c.is_ascii_digit())
                        .expect("height digits");
                json = format!("{}{}{}", &json[..start], height, &json[end..]);
            }
        }
        let shape: SP1RecursionProofShape =
            serde_json::from_str(&json).with_context(|| format!("parse {label} shape"))?;

        let input = shape.dummy_input(
            1,
            FIXED_VK_MAP_HEIGHT,
            chips.clone(),
            max_log_row_count,
            fri_config,
            log_stacking_height,
        );
        let input_sha = sha(&bincode::serialize(&input).context("serialize input")?);

        let program = shrink_program_from_input(&recursive_verifier(shard_verifier), true, &input);
        let inner = program.into_inner();
        let event_counts = format!("{:?}", inner.event_counts);

        println!("{label:9}");
        for (chip, _) in &overrides {
            println!("  {chip:18} {}", shape.shape.height_of_name(chip).unwrap());
        }
        println!("  dummy_input sha256   {input_sha}");
        println!("  shrink total_memory  {}", inner.total_memory);
        println!("  shrink event_counts  {}", sha(event_counts.as_bytes()));
        results.push((label, input_sha, inner.total_memory, event_counts));
    }

    let (a, b) = (&results[0], &results[1]);
    let same_input = a.1 == b.1;
    let same_memory = a.2 == b.2;
    let same_events = a.3 == b.3;
    println!(
        "\ndummy_input identical: {same_input}\nshrink total_memory identical: {same_memory}\nshrink event_counts identical: {same_events}"
    );

    if same_input && same_memory && same_events {
        println!(
            "\nVERDICT: the compress Select bump is INVISIBLE to the shrink program.\n\
             It therefore cannot move shrink_shape, the wrap input, or the wrap vk.\n\
             The VK-map height (cluster count) is then the only channel, and the\n\
             trim in MIZU_LEAN_PASTA_CLUSTERS is sufficient for ceremony-free Groth16."
        );
    } else {
        println!(
            "\nVERDICT: the compress Select bump CHANGES the shrink program.\n\
             It will move shrink_shape and therefore the wrap vk, so trimming\n\
             clusters is NOT sufficient: a Groth16 ceremony stays ours to run\n\
             while this fork needs a larger compress shape than upstream pins."
        );
    }
    Ok(())
}
