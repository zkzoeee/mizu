//! CPU-only mirror-root benchmark. No SDK, prover or GPU entry point.
//!
//! Measures what a keccak mirror of the Poseidon ledger costs in SP1 guest
//! cycles, for five mirror constructions, against a native recomputation of
//! the same schedule. The guest is `mirror-operation-program`; the modes and
//! what each one gives up are documented there.
use anyhow::{anyhow, bail, Context, Result};
use ark_ff::{AdditiveGroup, PrimeField};
use clap::Parser;
use mina_curves::pasta::Fp;
use mina_poseidon::{
    constants::PlonkSpongeConstantsKimchi, pasta::fp_kimchi, permutation::poseidon_block_cipher,
    poseidon::ArithmeticSpongeParams,
};
use sha2::{Digest as _, Sha256};
use sha3::Keccak256;
use sp1_core_executor::Program;
use sp1_core_executor_runner::MinimalExecutorRunner;
use std::{fs, path::PathBuf, sync::Arc, time::Instant};

const ACCOUNT_FIELDS: u32 = 16;
const ACCOUNT_BYTES: usize = 320;
const BUCKET_DEPTH: u32 = 16;
const STRIDE: u64 = 0x9E37_79B9_7F4A_7C15;

#[derive(Parser)]
struct Args {
    #[arg(long)]
    elf: PathBuf,
    /// 0 no mirror, 1 full keccak tree, 2 bucketed, 3 hash chain, 4 diff list.
    #[arg(long, value_parser=clap::value_parser!(u32).range(0..=4))]
    mode: u32,
    #[arg(long, default_value_t=1, value_parser=clap::value_parser!(u32).range(1..=256))]
    accounts: u32,
    #[arg(long, default_value_t=35, value_parser=clap::value_parser!(u32).range(1..=40))]
    depth: u32,
    #[arg(long)]
    output: PathBuf,
}

fn poseidon_node(params: &ArithmeticSpongeParams<Fp, 55>, left: Fp, right: Fp) -> Fp {
    let mut state = [left, right, Fp::ZERO];
    poseidon_block_cipher::<Fp, PlonkSpongeConstantsKimchi, 55>(params, &mut state);
    state[0]
}

fn poseidon_leaf(params: &ArithmeticSpongeParams<Fp, 55>, seed: u64) -> Fp {
    let mut state = [Fp::ZERO, Fp::ZERO, Fp::ZERO];
    let mut i = 0u32;
    while i < ACCOUNT_FIELDS {
        state[0] += Fp::from(seed.wrapping_add(i as u64));
        state[1] += Fp::from(seed.wrapping_mul(i as u64 + 1));
        poseidon_block_cipher::<Fp, PlonkSpongeConstantsKimchi, 55>(params, &mut state);
        i += 2;
    }
    state[0]
}

fn poseidon_path(
    params: &ArithmeticSpongeParams<Fp, 55>,
    mut acc: Fp,
    idx: u64,
    depth: u32,
    sibling: Fp,
) -> Fp {
    for level in 0..depth {
        acc = if (idx >> level) & 1 == 0 {
            poseidon_node(params, acc, sibling)
        } else {
            poseidon_node(params, sibling, acc)
        };
    }
    acc
}

fn keccak_node(left: &[u8; 32], right: &[u8; 32]) -> [u8; 32] {
    let mut h = Keccak256::new();
    h.update(left);
    h.update(right);
    h.finalize().into()
}

fn keccak_leaf(seed: u64) -> [u8; 32] {
    let mut h = Keccak256::new();
    let word = seed.to_le_bytes();
    let mut written = 0;
    while written < ACCOUNT_BYTES {
        h.update(word);
        written += word.len();
    }
    h.finalize().into()
}

fn keccak_path(mut acc: [u8; 32], idx: u64, depth: u32, sibling: &[u8; 32]) -> [u8; 32] {
    for level in 0..depth {
        acc = if (idx >> level) & 1 == 0 {
            keccak_node(&acc, sibling)
        } else {
            keccak_node(sibling, &acc)
        };
    }
    acc
}

/// The same schedule the guest runs, recomputed here so a wrong guest fails
/// loudly instead of reporting a cheap number.
fn reference(mode: u32, accounts: u32, depth: u32) -> Vec<u8> {
    let params = fp_kimchi::static_params();
    let mut out = mode.to_le_bytes().to_vec();
    out.extend_from_slice(&accounts.to_le_bytes());
    out.extend_from_slice(&depth.to_le_bytes());

    let mut p = Fp::from(7u64);
    for account in 0..accounts as u64 {
        let idx = account.wrapping_mul(STRIDE);
        let sibling = Fp::from(account.wrapping_add(11));
        let old_leaf = poseidon_leaf(params, account);
        let _check = poseidon_path(params, old_leaf, idx, depth, sibling);
        let new_leaf = poseidon_leaf(params, account.wrapping_add(1 << 20));
        p = poseidon_path(params, new_leaf, idx, depth, sibling);
    }
    for limb in p.into_bigint().as_ref() {
        out.extend_from_slice(&limb.to_le_bytes());
    }

    let mut k = [0u8; 32];
    match mode {
        0 => {}
        1 | 2 => {
            let levels = if mode == 2 {
                depth.min(BUCKET_DEPTH)
            } else {
                depth
            };
            for account in 0..accounts as u64 {
                let idx = account.wrapping_mul(STRIDE);
                let sibling = keccak_leaf(account.wrapping_add(99));
                let old_leaf = keccak_leaf(account);
                let _check = keccak_path(old_leaf, idx, levels, &sibling);
                let new_leaf = keccak_leaf(account.wrapping_add(1 << 20));
                k = keccak_path(new_leaf, idx, levels, &sibling);
            }
        }
        3 => {
            for account in 0..accounts as u64 {
                let idx = account.wrapping_mul(STRIDE);
                let mut h = Keccak256::new();
                h.update(k);
                h.update(idx.to_le_bytes());
                h.update(keccak_leaf(account));
                h.update(keccak_leaf(account.wrapping_add(1 << 20)));
                k = h.finalize().into();
            }
        }
        4 => {
            let mut h = Keccak256::new();
            for account in 0..accounts as u64 {
                let idx = account.wrapping_mul(STRIDE);
                h.update(idx.to_le_bytes());
                h.update(keccak_leaf(account));
                h.update(keccak_leaf(account.wrapping_add(1 << 20)));
            }
            k = h.finalize().into();
        }
        _ => unreachable!(),
    }
    out.extend_from_slice(&k);
    out
}

/// Permutations the schedule must perform, derived from the shape alone. The
/// cycle figures are only meaningful divided by these.
fn permutation_counts(mode: u32, accounts: u64, depth: u64) -> (u64, u64) {
    let leaf_p = (ACCOUNT_FIELDS as u64 + 1) / 2;
    let leaf_k = (ACCOUNT_BYTES as u64) / 136 + 1;
    let poseidon = accounts * (2 * depth + 2 * leaf_p);
    let keccak = match mode {
        0 => 0,
        1 => accounts * (2 * depth + 2 * leaf_k),
        2 => accounts * (2 * depth.min(BUCKET_DEPTH as u64) + 2 * leaf_k),
        // chain: two leaf hashes plus one 104-byte absorb per account
        3 => accounts * (2 * leaf_k + 1),
        // diff list: two leaf hashes per account, one 72-byte-per-account sponge
        4 => accounts * 2 * leaf_k + (accounts * 72) / 136 + 1,
        _ => 0,
    };
    (poseidon, keccak)
}

fn main() -> Result<()> {
    let args = Args::parse();
    let expected = reference(args.mode, args.accounts, args.depth);
    let input = bincode::serialize(&(args.mode, args.accounts, args.depth))?;

    let elf = fs::read(&args.elf).context("read mirror guest")?;
    let started = Instant::now();
    let program = Arc::new(
        Program::from(elf.as_slice()).map_err(|e| anyhow!("disassemble mirror guest: {e}"))?,
    );
    let mut runner = MinimalExecutorRunner::new(program, false, None, 2 * 1024 * 1024 * 1024, 0);
    runner.with_input(&input);
    let failure = loop {
        match runner.try_execute_chunk() {
            Ok(Some(_)) => {}
            Ok(None) => break None,
            Err(error) => break Some(format!("execute: {error}")),
        }
    };
    let cycles = runner.global_clk();
    let exit = runner.exit_code();
    let stages = runner.take_cycle_tracker_totals();
    let uint256 = runner.uint256_mul_count();
    let pasta = runner.pasta_syscall_counts();
    let actual = runner.into_public_values_stream();
    let accepted = failure.is_none() && exit == 0 && actual == expected;

    let (perm_p, perm_k) = permutation_counts(args.mode, args.accounts as u64, args.depth as u64);
    let report = serde_json::json!({
        "mode": "cpu-mirror-benchmark",
        "proof_generated": false,
        "mirror_mode": args.mode,
        "mirror_name": match args.mode {
            0 => "no-mirror-poseidon-only",
            1 => "full-keccak-merkle-mirror",
            2 => "bucketed-keccak-mirror",
            3 => "keccak-hash-chain",
            _ => "keccak-diff-list",
        },
        "accounts": args.accounts,
        "depth": args.depth,
        "account_fields": ACCOUNT_FIELDS,
        "account_bytes": ACCOUNT_BYTES,
        "elf_sha256": hex::encode(Sha256::digest(&elf)),
        "input_sha256": hex::encode(Sha256::digest(&input)),
        "cycles": cycles,
        "stage_cycles": stages,
        "derived_poseidon_permutations": perm_p,
        "derived_keccak_permutations": perm_k,
        "uint256_mul_count": uint256,
        "pasta_syscall_counts": pasta,
        "guest_exit_code": exit,
        "accepted": accepted,
        "failure": failure,
        "public_values_len": actual.len(),
        "public_values_sha256": hex::encode(Sha256::digest(&actual)),
        "expected_public_values_sha256": hex::encode(Sha256::digest(&expected)),
        "keccak_implementation": "sha3 software; SP1's KECCAK_PERMUTE precompile is NOT patched in, so every keccak figure is an upper bound",
        "elapsed_seconds": started.elapsed().as_secs_f64(),
    });
    if let Some(parent) = args.output.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(
        &args.output,
        format!("{}\n", serde_json::to_string_pretty(&report)?),
    )?;
    println!(
        "mode {} ({} accounts, depth {}): {cycles} cycles; accepted: {accepted}",
        args.mode, args.accounts, args.depth
    );
    if !accepted {
        bail!("mirror benchmark failed: {}", args.output.display());
    }
    Ok(())
}
