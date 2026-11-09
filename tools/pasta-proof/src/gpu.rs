//! CUDA core proving of the compiled four-chip Pasta guest.
//!
//! This module is compiled only under `--features cuda`, which is never
//! enabled on the development Mac, so every CPU mode in `main.rs` builds and
//! behaves exactly as before.
//!
//! The job here is deliberately narrow: prove the *same* compiled guest ELF
//! that `--mode guest` proves on the CPU, with every shard proved on the GPU,
//! and then check the result with the *same* complete-execution verifier. The
//! verifier deliberately runs on the CPU, so a GPU-produced proof is checked
//! by an independent path rather than by the machinery that made it.
//!
//! Nothing in here teaches the GPU about Vesta or Pallas, because nothing
//! needs to. SP1 6.10 still generates these precompile traces on the CPU
//! (`CudaTracegenAir for RiscvAir` answers `true` for `Global` alone), and GPU
//! constraint evaluation is compiled out of each chip's own `Air::eval` into a
//! bytecode program run by a generic device-side VM. The evidence is in
//! `spikes/artifacts/r9-pasta-hardened/gpu-stack-chip-agnosticism-evidence.json`.
//!
//! Core proofs only. No recursion, no wrap, no Groth16: that keeps the
//! recursion verification-key map, whose shapes our four extra chips change,
//! entirely out of scope.

use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fs, path::Path, sync::Arc, time::Instant};

use slop_basefold::{BasefoldVerifier, FriConfig};
use slop_futures::queue::WorkerQueue;
use sp1_core_executor::Program;
use sp1_core_machine::{io::SP1Stdin, utils::generate_records};
use sp1_hypercube::{
    air::MachineAir, prover::SimpleProver, InnerSC, MachineProof, MachineVerifier,
    MachineVerifyingKey, SP1InnerPcs, ShardVerifier,
};
use sp1_primitives::{SP1Field, SP1GlobalContext};

use sp1_gpu_basefold::FriCudaProver;
use sp1_gpu_challenger::DuplexChallenger;
use sp1_gpu_cudart::{cuda_memory_info, PinnedBuffer, TaskScope};
use sp1_gpu_merkle_tree::{CudaTcsProver, Poseidon2SP1Field16CudaProver};
use sp1_gpu_shard_prover::{CudaShardProver, CudaShardProverComponents};

use pasta_precompile_proof::run_manifest::{CoreIdentity, RunManifest};

use crate::{
    canonical_shard_order, committed_digest, core_opts, core_opts_production, expected_commitment,
    guest_input_blob, modulus, multiples, on_curve, sha, verify_complete_execution, write_json,
    Air, Cpu, Curve, Point, Policy, Proof, GUEST_POINTS_PER_CURVE, LOG_STACKING_HEIGHT,
    MAX_LOG_ROW_COUNT, OPS,
};

/// The CUDA core prover specialized to our machine.
///
/// This mirrors `sp1_gpu_prover::CudaProverCoreComponents` exactly. It is
/// specialized to the Pasta machine. SP1 6.10 compiles and uploads machine
/// zerocheck bytecode inside `CudaShardProver::new`; the old caller-side cache
/// argument has been removed. This source still requires a Linux/CUDA build
/// and fresh real-proof validation; neither was performed on the Mac.
async fn new_pasta_cuda_prover(
    verifier: MachineVerifier<SP1GlobalContext, InnerSC<Air>>,
    max_trace_size: usize,
    num_workers: usize,
    recompute_first_layer: bool,
    drop_ldes: bool,
    scope: TaskScope,
) -> Gpu {
    let machine = verifier.machine().clone();

    let log_stacking_height = verifier.log_stacking_height();
    let max_log_row_count = verifier.max_log_row_count();

    let basefold_verifier = BasefoldVerifier::<SP1GlobalContext>::new(*verifier.fri_config(), 2);
    let tcs_prover =
        <Poseidon2SP1Field16CudaProver as CudaTcsProver<SP1GlobalContext>>::new(&scope);
    let basefold_prover =
        FriCudaProver::<SP1GlobalContext, Poseidon2SP1Field16CudaProver, SP1Field>::new(
            tcs_prover,
            basefold_verifier.fri_config,
            log_stacking_height,
        );

    let mut all_interactions = BTreeMap::new();
    for chip in machine.chips().iter() {
        let host_interactions = sp1_gpu_logup_gkr::Interactions::new(chip.sends(), chip.receives());
        let device_interactions = host_interactions.copy_to_device(&scope).unwrap();
        all_interactions.insert(chip.name().to_string(), Arc::new(device_interactions));
    }

    let mut trace_buffers = Vec::with_capacity(num_workers);
    for _ in 0..num_workers {
        trace_buffers.push(PinnedBuffer::<SP1Field>::with_capacity(max_trace_size));
    }
    let trace_buffers = Arc::new(WorkerQueue::new(trace_buffers));

    Gpu::new(
        trace_buffers,
        max_log_row_count as u32,
        basefold_prover,
        machine,
        max_trace_size,
        scope,
        all_interactions,
        recompute_first_layer,
        drop_ldes,
    )
}

/// Read an environment override, falling back to a default.
fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// Prove the compiled guest on the GPU and verify it with the CPU verifier.
///
/// `reference` is an optional `result.json` from a previous CPU `--mode guest`
/// run. When given, the GPU result must agree with it on the verification key,
/// the committed digest and the shard/chip inventory. That comparison is the
/// real point of the run: it shows the GPU reproduces the CPU proof's meaning,
/// not merely that some proof verified.
pub async fn run_guest_gpu(elf_path: &Path, output: &Path, reference: Option<&Path>) -> Result<()> {
    ensure!(!output.exists(), "use a fresh output directory");
    fs::create_dir_all(output)?;
    let started = Instant::now();

    let (free_vram, total_vram) = cuda_memory_info().map_err(|e| anyhow::anyhow!("{e:?}"))?;
    ensure!(
        total_vram > 0,
        "no CUDA device memory reported; refusing to run a GPU proof without a GPU"
    );

    let elf = fs::read(elf_path).context("read the compiled guest ELF")?;
    let program = Arc::new(Program::from(&elf).map_err(|e| anyhow::anyhow!("{e}"))?);

    // Exactly the fixture `--mode guest` uses, so the two runs are comparable.
    let points: Vec<(Curve, Vec<Point>)> = vec![
        (
            Curve::Vesta,
            multiples(Curve::Vesta, GUEST_POINTS_PER_CURVE),
        ),
        (
            Curve::Pallas,
            multiples(Curve::Pallas, GUEST_POINTS_PER_CURVE),
        ),
    ];
    for (curve, list) in &points {
        let m = modulus(*curve);
        for p in list {
            ensure!(on_curve(p, &m), "guest fixture point is off curve");
        }
    }
    let input = guest_input_blob(&points);
    let (expected_bytes, expectation) = expected_commitment(&points, &input)?;
    let expected_digest: [u8; 32] = Sha256::digest(&expected_bytes).into();

    let mut stdin = SP1Stdin::new();
    stdin.write_slice(&input);

    // `global_dependencies_opt` must be on for a GPU proof, and SP1's own
    // `local_gpu_opts()` sets it. The Global chip is the one chip whose main
    // trace is always generated on the device
    // (`supports_device_main_tracegen` returns true unconditionally). With the
    // flag off, the CPU `generate_dependencies` additionally calls
    // `cols.interaction.populate(..)` and emits byte-lookup events for work the
    // device trace does not perform, so the Byte chip's multiplicities no longer
    // balance and every shard fails `shard_cryptographic_verification` — while
    // the traces, public values and committed digest all stay correct.
    let mut opts = core_opts();
    opts.global_dependencies_opt = std::env::var("PASTA_GLOBAL_DEPS_OPT")
        .map(|v| v != "0")
        .unwrap_or(true);
    let global_deps_opt = opts.global_dependencies_opt;

    let trace_start = Instant::now();
    let (records, cycles) =
        generate_records::<SP1Field>(program.clone(), stdin.clone(), opts, [0; 4])?;
    let trace_seconds = trace_start.elapsed().as_secs_f64();

    let mut per_op = BTreeMap::new();
    for op in &OPS {
        let count: usize = records
            .iter()
            .map(|r| r.get_precompile_events(op.syscall).len())
            .sum();
        per_op.insert(op.id, count);
    }
    for op in &OPS {
        ensure!(
            per_op[op.id] > 0,
            "the compiled guest did not exercise {}; all four wrappers must run",
            op.id
        );
    }

    // The PCS parameters are overridable so they can be swept without a rebuild.
    // This matters because SP1's own GPU prover runs the core machine at
    // CORE_LOG_STACKING_HEIGHT = 21, while the CPU suite here was measured at 17,
    // and a stacking height the CUDA FRI prover mishandles would produce a proof
    // whose traces and public values are right but whose PCS opening is not.
    // These default to SP1's own production core-prover parameters, not to the
    // ones the CPU suite was measured at, because the CUDA prover is built for
    // the production ones. The difference that matters is the FRI blowup:
    // `FriConfig::default_fri_config()` is `new(1, 94, 16)` (log_blowup 1), while
    // `core_fri_config()` is `new(2, 124, 16)` (log_blowup 2 = CORE_LOG_BLOWUP).
    // At blowup 1 the GPU produced shard proofs whose traces, public values and
    // committed digest were all correct but whose PCS opening the verifier
    // rejected, at every stacking height from 17 to 21.
    let fri_name = std::env::var("PASTA_FRI_CONFIG").unwrap_or_else(|_| "default".to_string());
    let fri = match fri_name.as_str() {
        "core" => sp1_primitives::fri_params::core_fri_config(),
        "default" => FriConfig::default_fri_config(),
        other => anyhow::bail!("unknown PASTA_FRI_CONFIG {other}; use core or default"),
    };
    let log_stacking = env_usize("PASTA_LOG_STACKING_HEIGHT", LOG_STACKING_HEIGHT as usize) as u32;
    let max_rows = env_usize("PASTA_MAX_LOG_ROW_COUNT", MAX_LOG_ROW_COUNT);
    // The retained CPU run used the default FRI config at 17/17, so the
    // verification key only has to match when every parameter matches it.
    let matches_cpu_params = fri_name == "default"
        && log_stacking == LOG_STACKING_HEIGHT
        && max_rows == MAX_LOG_ROW_COUNT;
    let shard_verifier =
        ShardVerifier::from_basefold_parameters(fri, log_stacking, max_rows, Air::machine());

    // The CPU prover exists only to verify. Building it from the same
    // `ShardVerifier` guarantees both sides agree on the machine, so a key or
    // digest mismatch below would be a real divergence rather than a config
    // difference.
    let cpu_prover = SimpleProver::new(shard_verifier.clone(), Cpu::new(shard_verifier.clone()));

    // Default to the size SP1's own GPU builder uses: ELEMENT_THRESHOLD plus one
    // stacking height. An undersized pinned staging buffer segfaults rather than
    // erroring, which is how the first run on this hardware failed.
    let max_trace_size = env_usize(
        "PASTA_GPU_MAX_TRACE_SIZE",
        ((1usize << 28) + (1usize << 27)) + (1usize << 21),
    );
    let num_workers = env_usize("PASTA_GPU_WORKERS", 2);
    let recompute_first_layer = opts.recompute_gkr_trace;
    let drop_ldes = opts.drop_ldes;
    ensure!(
        !opts.full_size_shards,
        "full_size_shards requires separately validated GPU memory tuning"
    );

    let elf_for_task = elf.len();
    let records_for_task = records.clone();
    let verifier_for_task = shard_verifier.clone();

    // Everything that touches the device has to happen inside the task scope:
    // a `TaskScope` is a weak handle into a pooled CUDA task and is only valid
    // for that task's lifetime.
    let gpu_start = Instant::now();
    let handle = sp1_gpu_cudart::spawn(move |scope| async move {
        let machine_verifier = MachineVerifier::new(verifier_for_task.clone());
        let gpu = new_pasta_cuda_prover(
            machine_verifier,
            max_trace_size,
            num_workers,
            recompute_first_layer,
            false,
            scope.clone(),
        )
        .await;
        let gpu_prover = SimpleProver::new(verifier_for_task, gpu);

        let setup_start = Instant::now();
        let (pk, vk) = gpu_prover.setup(program).await;
        let pk = unsafe { pk.into_inner() };
        let setup_seconds = setup_start.elapsed().as_secs_f64();

        let prove_start = Instant::now();
        let mut shards = Vec::new();
        for record in &records_for_task {
            shards.push(gpu_prover.prove_shard(pk.clone(), record.clone()).await);
        }
        let prove_seconds = prove_start.elapsed().as_secs_f64();

        (vk, shards, setup_seconds, prove_seconds)
    });
    let (vk, shards, setup_seconds, prove_seconds) = handle
        .await
        .map_err(|e| anyhow::anyhow!("the CUDA proving task failed: {e:?}"))?;
    let gpu_seconds = gpu_start.elapsed().as_secs_f64();

    let vk_bytes = bincode::serialize(&vk)?;
    fs::write(output.join("verification-key.bin"), &vk_bytes)?;
    let restored_vk: MachineVerifyingKey<SP1GlobalContext> = bincode::deserialize(&vk_bytes)?;

    let generated_shards = shards.len();
    ensure!(
        generated_shards == records.len(),
        "shard assembly lost shards: {generated_shards} generated from {} execution records",
        records.len()
    );
    let proof = MachineProof {
        shard_proofs: canonical_shard_order(shards),
    };
    ensure!(
        proof.shard_proofs.len() == generated_shards,
        "canonical ordering changed the shard count"
    );

    let proof_bytes = bincode::serialize(&proof)?;
    fs::write(output.join("guest-gpu.proof.bin"), &proof_bytes)?;
    let reloaded: Proof = bincode::deserialize(&proof_bytes)?;
    let serialized_shards = reloaded.shard_proofs.len();
    ensure!(
        serialized_shards == generated_shards && generated_shards == records.len(),
        "shard assembly lost shards: {} records, {generated_shards} generated, {serialized_shards} serialized",
        records.len()
    );

    let mut chip_shards: BTreeMap<String, usize> = BTreeMap::new();
    for shard in reloaded.shard_proofs.iter() {
        for name in shard.opened_values.chips.keys() {
            *chip_shards.entry(name.clone()).or_default() += 1;
        }
    }
    let missing: Vec<_> = OPS
        .iter()
        .filter(|op| !chip_shards.contains_key(op.chip))
        .map(|op| op.chip)
        .collect();
    ensure!(
        missing.is_empty(),
        "the GPU-produced proof is missing Pasta chips {missing:?}; all four must appear"
    );

    let policy = Policy {
        require_successful_exit: true,
        expected_committed_digest: Some(expected_digest),
    };
    let verify_start = Instant::now();
    let complete = verify_complete_execution(&cpu_prover, &restored_vk, &reloaded, &policy);
    let verify_seconds = verify_start.elapsed().as_secs_f64();
    let accepted = complete.is_ok();
    let rejection_check = complete.as_ref().err().map(|e| e.check);
    let error = complete.as_ref().err().map(|e| e.detail.clone());

    let digest = committed_digest(&reloaded)?;
    let digest_matches = digest == expected_digest;

    // Compare against the retained CPU run, when one was given.
    let mut comparison = json!(null);
    let mut comparison_ok = true;
    if let Some(path) = reference {
        let cpu: Value =
            serde_json::from_slice(&fs::read(path).context("read the CPU reference")?)?;
        let cpu_vk = cpu["verification_key_sha256"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let cpu_digest = cpu["committed_public_value_digest_hex"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let cpu_shards = cpu["serialized_shards"].as_u64().unwrap_or_default() as usize;
        let cpu_chip_shards = cpu["chip_shard_counts"].clone();
        let vk_same = cpu_vk == sha(&vk_bytes);
        let digest_same = cpu_digest == hex::encode(digest);
        let shards_same = cpu_shards == serialized_shards;
        let chips_same = cpu_chip_shards == json!(chip_shards);
        comparison_ok = if matches_cpu_params {
            vk_same && digest_same && shards_same && chips_same
        } else {
            digest_same && shards_same && chips_same
        };
        comparison = json!({
            "reference": path.to_string_lossy(),
            "verification_key_matches_cpu": vk_same,
            "committed_digest_matches_cpu": digest_same,
            "shard_count_matches_cpu": shards_same,
            "chip_shard_counts_match_cpu": chips_same,
            "cpu_verification_key_sha256": cpu_vk,
            "gpu_verification_key_sha256": sha(&vk_bytes),
            "note": "the proof bytes themselves are not expected to be identical; the proof system is randomized. What must match is the key, the committed digest and the shard/chip inventory."
        });
    }

    write_json(
        output.join("shard-inventory.json"),
        &json!({
            "execution_records": records.len(),
            "generated_shards": generated_shards,
            "serialized_shards": serialized_shards,
            "chip_shard_counts": chip_shards,
            "pasta_chips_present": OPS.iter().map(|o| o.chip).collect::<Vec<_>>(),
        }),
    )?;

    let report = json!({
        "status": if accepted && digest_matches && comparison_ok { "passed" } else { "failed" },
        "mode": "guest-gpu",
        "proved_on": "cuda",
        "verified_on": "cpu",
        "gpu_total_vram_bytes": total_vram,
        "gpu_free_vram_bytes_at_start": free_vram,
        "gpu_max_trace_size": max_trace_size,
        "log_stacking_height": log_stacking,
        "max_log_row_count": max_rows,
        "fri_config": fri_name,
        "global_dependencies_opt": global_deps_opt,
        "fri_log_blowup": if fri_name == "core" { 2 } else { 1 },
        "parameters_match_retained_cpu_run": matches_cpu_params,
        "gpu_recompute_first_layer": recompute_first_layer,
        "gpu_workers": num_workers,
        "elf_path": elf_path.to_string_lossy(),
        "elf_sha256": sha(&elf),
        "elf_bytes": elf_for_task,
        "input_blob_sha256": sha(&input),
        "guest_cycles": cycles,
        "execution_records": records.len(),
        "generated_shards": generated_shards,
        "serialized_shards": serialized_shards,
        "chip_shard_counts": chip_shards,
        "precompile_events": per_op,
        "expectation": expectation,
        "committed_public_value_digest_hex": hex::encode(digest),
        "independently_recomputed_digest_hex": hex::encode(expected_digest),
        "committed_digest_matches_independent_recomputation": digest_matches,
        "complete_verifier_accepted": accepted,
        "rejection_check": rejection_check,
        "verifier_error": error,
        "proof_bytes": proof_bytes.len(),
        "proof_sha256": sha(&proof_bytes),
        "verification_key_sha256": sha(&vk_bytes),
        "cpu_comparison": comparison,
        "trace_seconds": trace_seconds,
        "gpu_setup_seconds": setup_seconds,
        "gpu_prove_seconds": prove_seconds,
        "gpu_task_total_seconds": gpu_seconds,
        "cpu_verify_seconds": verify_seconds,
        "total_seconds": started.elapsed().as_secs_f64(),
    });
    write_json(output.join("result.json"), &report)?;
    println!("{}", serde_json::to_string_pretty(&report)?);

    ensure!(
        accepted,
        "the GPU-produced guest proof was rejected by the complete verifier: {:?}",
        rejection_check
    );
    ensure!(
        digest_matches,
        "the committed public-value digest does not match the independent recomputation"
    );
    ensure!(
        comparison_ok,
        "the GPU run disagrees with the retained CPU run: {comparison}"
    );
    println!("GUEST_GPU_PASSED");
    Ok(())
}

/// Prove one pinned program on the GPU, core proof only.
///
/// The identity and the executor options come from the caller, so this body
/// serves both the historical r5 candidate and a manifest-driven run without
/// either being a copy of the other. What it will not do is proceed on a
/// program, outcome or option set other than the one it was handed: the ELF
/// hash, the cycle count, the committed byte stream and
/// `global_dependencies_opt` are all enforced here.
///
/// Two things differ from `run_guest_gpu`, both forced by the scale of a real
/// workload:
///
/// 1. Records are streamed. `generate_records` would drain the whole execution
///    into memory first; `generate_records_streaming` hands over one record at a
///    time, and a small bounded channel applies backpressure so execution cannot
///    outrun proving. Execution therefore also overlaps with proving.
/// 2. The PCS parameters default to SP1's production core values rather than the
///    tiny-fixture ones. `CORE_MAX_LOG_ROW_COUNT` is 22 and SP1 asserts
///    `HEIGHT_THRESHOLD <= 1 << CORE_MAX_LOG_ROW_COUNT`, so the 17 the fixture
///    suite uses is not valid for a real program.
///
/// Correctness is judged two ways: the complete-execution verifier must accept,
/// and the committed public-value digest must equal the SHA-256 of the retained
/// 577-byte expected output from the mainnet-blockchain-snark fixture.
async fn prove_core_gpu(
    identity: &CoreIdentity,
    opts: sp1_core_executor::SP1CoreOpts,
    elf_path: &Path,
    input_dir: &Path,
    output: &Path,
) -> Result<()> {
    ensure!(!output.exists(), "use a fresh output directory");
    fs::create_dir_all(output)?;
    let started = Instant::now();

    let (free_vram, total_vram) = cuda_memory_info().map_err(|e| anyhow::anyhow!("{e:?}"))?;
    ensure!(total_vram > 0, "no CUDA device memory reported");

    let elf = fs::read(elf_path).context("read the pinned ELF")?;
    // The guard is not removed, it is supplied: the caller states which
    // program this run is pinned to and anything else is refused.
    let elf_sha = identity.verify_elf_bytes(&elf)?;
    let program = Arc::new(Program::from(&elf).map_err(|e| anyhow::anyhow!("{e}"))?);

    let proof_input = fs::read(input_dir.join("proof-input.bin")).context("proof-input.bin")?;
    let receipt_input =
        fs::read(input_dir.join("receipt-input.bin")).context("receipt-input.bin")?;
    let expected_pv = fs::read(input_dir.join("public-values.bin")).context("public-values.bin")?;
    let expected_digest: [u8; 32] = Sha256::digest(&expected_pv).into();

    // The two raw stdin records, in the order the exporter wrote them.
    let mut stdin = SP1Stdin::new();
    stdin.write_slice(&proof_input);
    stdin.write_slice(&receipt_input);

    // Supplied whole by the caller, so a manifest arm's retention set reaches
    // the executor exactly as written and no option can be dropped here.
    let global_deps_opt = opts.global_dependencies_opt;
    ensure!(
        global_deps_opt,
        "global_dependencies_opt is false: r10 measured that every GPU shard then fails \
         shard_cryptographic_verification while traces, public values and the verification \
         key all look correct"
    );

    let fri_name = std::env::var("PASTA_FRI_CONFIG").unwrap_or_else(|_| "core".to_string());
    let fri = match fri_name.as_str() {
        "core" => sp1_primitives::fri_params::core_fri_config(),
        "default" => FriConfig::default_fri_config(),
        other => anyhow::bail!("unknown PASTA_FRI_CONFIG {other}"),
    };
    let log_stacking = env_usize("PASTA_LOG_STACKING_HEIGHT", 21) as u32;
    let max_rows = env_usize("PASTA_MAX_LOG_ROW_COUNT", 22);

    let shard_verifier =
        ShardVerifier::from_basefold_parameters(fri, log_stacking, max_rows, Air::machine());
    let cpu_prover = SimpleProver::new(shard_verifier.clone(), Cpu::new(shard_verifier.clone()));

    let max_trace_size = env_usize(
        "PASTA_GPU_MAX_TRACE_SIZE",
        ((1usize << 28) + (1usize << 27)) + (1usize << 21),
    );
    let num_workers = env_usize("PASTA_GPU_WORKERS", 2);
    let recompute_first_layer = opts.recompute_gkr_trace;
    let drop_ldes = opts.drop_ldes;
    ensure!(
        !opts.full_size_shards,
        "full_size_shards requires separately validated GPU memory tuning"
    );
    let inflight = env_usize("PASTA_R5_INFLIGHT", 2);

    // Bounded, so a fast executor cannot accumulate records faster than the GPU
    // retires them. This is the whole point of streaming.
    let (tx, mut rx) = tokio::sync::mpsc::channel::<sp1_core_executor::ExecutionRecord>(inflight);
    let exec_program = program.clone();
    let exec_start = Instant::now();
    let exec_handle = tokio::task::spawn_blocking(move || {
        sp1_core_machine::utils::generate_records_streaming::<SP1Field, _>(
            exec_program,
            stdin,
            opts,
            [0; 4],
            move |record| tx.blocking_send(record).is_ok(),
        )
    });

    let verifier_for_task = shard_verifier.clone();
    let gpu_program = program.clone();
    let gpu_start = Instant::now();
    let handle = sp1_gpu_cudart::spawn(move |scope| async move {
        let machine_verifier = MachineVerifier::new(verifier_for_task.clone());
        let gpu = new_pasta_cuda_prover(
            machine_verifier,
            max_trace_size,
            num_workers,
            recompute_first_layer,
            drop_ldes,
            scope.clone(),
        )
        .await;
        let gpu_prover = SimpleProver::new(verifier_for_task, gpu);

        let setup_start = Instant::now();
        let (pk, vk) = gpu_prover.setup(gpu_program).await;
        let pk = unsafe { pk.into_inner() };
        let setup_seconds = setup_start.elapsed().as_secs_f64();

        let prove_start = Instant::now();
        let mut shards = Vec::new();
        while let Some(record) = rx.recv().await {
            shards.push(gpu_prover.prove_shard(pk.clone(), record).await);
        }
        let prove_seconds = prove_start.elapsed().as_secs_f64();
        (vk, shards, setup_seconds, prove_seconds)
    });

    let (vk, shards, setup_seconds, prove_seconds) = handle
        .await
        .map_err(|e| anyhow::anyhow!("the CUDA proving task failed: {e:?}"))?;
    let gpu_seconds = gpu_start.elapsed().as_secs_f64();
    let cycles = exec_handle
        .await
        .map_err(|e| anyhow::anyhow!("the executor task panicked: {e}"))?
        .map_err(|e| anyhow::anyhow!("execution failed: {e}"))?;
    let exec_seconds = exec_start.elapsed().as_secs_f64();

    let vk_bytes = bincode::serialize(&vk)?;
    fs::write(output.join("verification-key.bin"), &vk_bytes)?;
    let restored_vk: MachineVerifyingKey<SP1GlobalContext> = bincode::deserialize(&vk_bytes)?;

    let generated_shards = shards.len();
    ensure!(generated_shards > 0, "no shards were proved");
    let proof = MachineProof {
        shard_proofs: canonical_shard_order(shards),
    };
    ensure!(
        proof.shard_proofs.len() == generated_shards,
        "canonical ordering changed the shard count"
    );

    let proof_bytes = bincode::serialize(&proof)?;
    fs::write(output.join(&identity.proof_file), &proof_bytes)?;
    let reloaded: Proof = bincode::deserialize(&proof_bytes)?;
    ensure!(
        reloaded.shard_proofs.len() == generated_shards,
        "serialization lost shards: {generated_shards} proved, {} serialized",
        reloaded.shard_proofs.len()
    );

    let mut chip_shards: BTreeMap<String, usize> = BTreeMap::new();
    for shard in reloaded.shard_proofs.iter() {
        for name in shard.opened_values.chips.keys() {
            *chip_shards.entry(name.clone()).or_default() += 1;
        }
    }
    let pasta_present: Vec<_> = OPS
        .iter()
        .filter(|op| chip_shards.contains_key(op.chip))
        .map(|op| op.chip)
        .collect();

    let policy = Policy {
        require_successful_exit: true,
        expected_committed_digest: Some(expected_digest),
    };
    let verify_start = Instant::now();
    let complete = verify_complete_execution(&cpu_prover, &restored_vk, &reloaded, &policy);
    let verify_seconds = verify_start.elapsed().as_secs_f64();
    let accepted = complete.is_ok();
    let rejection_check = complete.as_ref().err().map(|e| e.check);
    let error = complete.as_ref().err().map(|e| e.detail.clone());

    let digest = committed_digest(&reloaded)?;
    let digest_matches = digest == expected_digest;

    // The gates require cycle equality. The previous code only reported it,
    // so a different cycle count would still have produced a "passed" artifact.
    identity.verify_cycles(cycles)?;

    let mut report = json!({
        "status": if accepted && digest_matches { "passed" } else { "failed" },
        "mode": identity.mode.as_str(),
        "candidate": identity.label.as_str(),
        "proved_on": "cuda",
        "verified_on": "cpu",
        "proof_type": "core (no recursion, no wrap, no Groth16)",
        "elf_sha256": elf_sha,
        "elf_bytes": elf.len(),
        "guest_cycles": cycles,
        "expected_cycles": identity.expected_cycles,
        "cycles_match_expected": cycles == identity.expected_cycles,
        "generated_shards": generated_shards,
        "chip_shard_counts": chip_shards,
        "pasta_chips_present": pasta_present,
        "committed_public_value_digest_hex": hex::encode(digest),
        "expected_public_values_sha256": hex::encode(expected_digest),
        "expected_public_values_bytes": expected_pv.len(),
        "committed_digest_matches_retained_expected_output": digest_matches,
        "complete_verifier_accepted": accepted,
        "rejection_check": rejection_check,
        "verifier_error": error,
        "proof_bytes": proof_bytes.len(),
        "proof_sha256": sha(&proof_bytes),
        "verification_key_sha256": sha(&vk_bytes),
        "global_dependencies_opt": global_deps_opt,
        "fri_config": fri_name,
        "log_stacking_height": log_stacking,
        "max_log_row_count": max_rows,
        "gpu_total_vram_bytes": total_vram,
        "gpu_free_vram_bytes_at_start": free_vram,
        "gpu_workers": num_workers,
        "records_in_flight": inflight,
        "executor_seconds": exec_seconds,
        "gpu_setup_seconds": setup_seconds,
        "gpu_prove_seconds": prove_seconds,
        "gpu_task_total_seconds": gpu_seconds,
        "cpu_verify_seconds": verify_seconds,
        "total_seconds": started.elapsed().as_secs_f64(),
    });

    // Manifest-derived expectations, merged so the artifact records what the
    // run was pinned to alongside what it measured.
    if let (Some(dst), Some(src)) = (report.as_object_mut(), identity.extra_report.as_object()) {
        for (k, v) in src {
            dst.insert(k.clone(), v.clone());
        }
    }

    write_json(output.join("result.json"), &report)?;
    println!("{}", serde_json::to_string_pretty(&report)?);

    ensure!(
        accepted,
        "the {} GPU core proof was rejected: {:?} {:?}",
        identity.label,
        rejection_check,
        error
    );
    ensure!(
        digest_matches,
        "the committed digest does not match the retained expected output"
    );
    println!("{}", identity.pass_marker);
    Ok(())
}

/// Prove the historical r5 candidate, unchanged.
///
/// Same pins, same options and the same `R5_GPU_PASSED` marker as before the
/// body was parameterised. The one difference is that the cycle count is now
/// enforced rather than only reported, which r5 already satisfied.
pub async fn run_r5_gpu(elf_path: &Path, input_dir: &Path, output: &Path) -> Result<()> {
    let mut opts = core_opts_production();
    opts.global_dependencies_opt = std::env::var("PASTA_GLOBAL_DEPS_OPT")
        .map(|v| v != "0")
        .unwrap_or(true);
    let identity = CoreIdentity {
        mode: "r5-gpu".to_owned(),
        label: "r5-both-v16-p13".to_owned(),
        // From spikes/artifacts/r5-both-v16-p13/metadata.json.
        elf_sha256: "588537b5add70c659100e102f770f728cd9e1b2422abb73f3962d24efc7a0f58".to_owned(),
        expected_cycles: 621_941_587,
        proof_file: "r5.proof.bin".to_owned(),
        pass_marker: "R5_GPU_PASSED".to_owned(),
        extra_report: json!({}),
    };
    prove_core_gpu(&identity, opts, elf_path, input_dir, output).await
}

/// Prove one arm of a manifest-driven run.
///
/// The manifest supplies the program identity, the expected outcome and every
/// `SP1CoreOpts` field for this arm. Settings that must stay matched across
/// arms — FRI config, stacking height, row bound, worker count, in-flight
/// records, trace limit — are not per-arm and come from the shared body, so
/// retention is the only thing that varies.
pub async fn run_manifest_gpu(
    manifest_path: &Path,
    arm_name: &str,
    elf_path: &Path,
    input_dir: &Path,
    output: &Path,
) -> Result<()> {
    let manifest = RunManifest::load(manifest_path)?;
    let arm = manifest.arm(arm_name)?;
    let identity = CoreIdentity::for_arm(&manifest, arm);
    prove_core_gpu(&identity, arm.opts.clone(), elf_path, input_dir, output).await
}
