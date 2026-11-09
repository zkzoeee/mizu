//! CPU execution and cycle measurement only. This binary cannot request a proof.

use anyhow::{anyhow, bail, ensure, Context, Result};
use clap::{Parser, ValueEnum};
use pickles_verifier::types::VerifiableProof;
use pickles_verifier::wire::{
    parse_app_statement_fields, parse_wrap_proof, parse_wrap_vk, OcamlProof,
};
use serde_json::json;
use sha2::{Digest, Sha256};
use sp1_core_executor::{ExecutionReport, GasEstimatingVMEnum, Program, SP1CoreOpts};
use sp1_core_executor_runner::MinimalExecutorRunner;
use sp1_jit::{MinimalTrace, TraceChunkRaw};
use std::{collections::BTreeMap, fs, path::Path, path::PathBuf, sync::Arc, time::Instant};

const GUEST_MEMORY_LIMIT_BYTES: u64 = 2 * 1024 * 1024 * 1024;
// SP1 measures this threshold in 16-byte MemValues. With its fixed and
// proportional slack, one buffer is about 17.8 MiB. Replay is synchronous:
// at most one chunk is alive, and no full execution records are constructed.
const PROFILE_TRACE_CHUNK_ENTRIES: u64 = 1 << 20;

#[derive(Clone, Copy, Debug, ValueEnum)]
enum GuestAbi { Boolean, Settlement, Batch, StatementDigest }

#[derive(Parser, Debug)]
#[command(about = "Execute an existing Pickles guest on CPU; never generate a proof")]
struct Args {
    #[arg(long)]
    elf: PathBuf,
    /// Settlement is only for the optional example and its historical reference.
    #[arg(long, value_enum, default_value = "boolean")]
    guest_abi: GuestAbi,
    /// Batch guest only. The optional guest rejects counts outside 1..=8.
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u32).range(0..=9))]
    batch_size: u32,
    /// Which member receives a requested corruption (zero based).
    #[arg(long, default_value_t = 0)]
    corrupt_index: usize,
    #[arg(long)]
    corrupt_statement: bool,
    #[arg(long, default_value = "fixtures/mainnet-blockchain-snark")]
    fixture_dir: PathBuf,
    /// Mutate an accumulator challenge. The guest must reject this input.
    #[arg(long)]
    corrupt_accumulator_challenge: bool,
    /// Mutate the outer IPA response without touching the Vesta accumulator.
    #[arg(long, conflicts_with = "corrupt_accumulator_challenge")]
    corrupt_wrap_opening: bool,
    /// Replay one bounded trace chunk at a time to count precompiles and estimate SP1 gas.
    /// Slower than ordinary execution; generates no proof or full execution trace.
    #[arg(long)]
    profile_precompiles: bool,
    /// Compatibility flag for execution-only Pasta measurements (no proof is generated).
    #[arg(long, conflicts_with = "profile_precompiles")]
    pasta_executor_only: bool,
    /// Write machine-readable measurements, including failures, to this file.
    #[arg(long)]
    output: PathBuf,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let elf = fs::read(&args.elf).with_context(|| format!("read {}", args.elf.display()))?;
    let (mut proof, vk_hash, fixture_hashes) = load_fixture(&args.fixture_dir)?;
    let expected_statement_digest = pickles_verifier::attestation::statement_digest(&vk_hash, &proof.app_state);
    let honest_proof_input = bincode::serialize(&proof)?;
    if args.corrupt_statement {
        *proof.app_state.first_mut().context("empty application statement")? += pickles_verifier::types::StepField::from(1u64);
    }
    if args.corrupt_accumulator_challenge {
        let challenge = proof
            .raw_bulletproof_challenges
            .first_mut()
            .context("proof has no accumulator challenge to corrupt")?;
        *challenge += pickles_verifier::types::StepField::from(1u64);
    }
    if args.corrupt_wrap_opening {
        proof.wrap_proof.proof.z1 += pickles_verifier::types::WrapField::from(1u64);
    }
    let proof_input = bincode::serialize(&proof).context("serialize VerifiableProof")?;
    // The guest reads Option<SettlementWitnessV1>. Every bincode None has the
    // same one-byte encoding, so the execute-only harness needs no settlement
    // or prover dependency just to send this compatibility-fixture input.
    let receipt_input = bincode::serialize(&Option::<()>::None)?;
    ensure!(receipt_input == [0], "unexpected bincode None encoding");

    let started = Instant::now();
    let program = Arc::new(
        Program::from(elf.as_slice()).map_err(|error| anyhow!("disassemble ELF: {error}"))?,
    );
    let mut profile = args.profile_precompiles.then(|| PrecompileProfile::new(program.enable_untrusted_programs));
    let trace_threshold = profile.as_ref().map(|_| PROFILE_TRACE_CHUNK_ENTRIES);
    let mut executor = MinimalExecutorRunner::new(
        program.clone(),
        false,
        trace_threshold,
        GUEST_MEMORY_LIMIT_BYTES,
        0,
    );
    let mut stdin_items = Vec::new();
    if matches!(args.guest_abi, GuestAbi::Batch) {
        ensure!(args.corrupt_index < args.batch_size as usize || args.batch_size == 0, "corruption index outside batch");
        stdin_items.push(bincode::serialize(&args.batch_size)?);
        for i in 0..args.batch_size as usize {
            stdin_items.push(if i == args.corrupt_index { proof_input.clone() } else { honest_proof_input.clone() });
        }
    } else {
        ensure!(args.batch_size == 1 && args.corrupt_index == 0, "batch arguments require batch ABI");
        stdin_items.push(proof_input.clone());
    }
    for item in &stdin_items { executor.with_input(item); }
    if matches!(args.guest_abi, GuestAbi::Settlement) { executor.with_input(&receipt_input); }
    let execution_error = loop {
        match executor.try_execute_chunk() {
            Ok(Some(chunk)) => {
                if let Some(profile) = profile.as_mut() {
                    if let Err(error) = profile.replay(&chunk, program.clone()) {
                        break Some(format!("profile replay: {error}"));
                    }
                }
                // Drop the replay VM and this chunk before executing the next.
            }
            Ok(None) => break None,
            Err(error) => break Some(format!("execute chunk: {error}")),
        }
    };
    let cycles = executor.global_clk();
    let pasta_syscall_counts = executor.pasta_syscall_counts();
    let uint256_mul_count = executor.uint256_mul_count();
    let guest_exit_code = executor.exit_code();
    let stage_cycles: std::collections::BTreeMap<_, _> =
        executor.take_cycle_tracker_totals().into_iter().collect();
    let public_values = executor.into_public_values_stream();
    let elapsed_seconds = started.elapsed().as_secs_f64();

    let profile_error = profile
        .as_ref()
        .and_then(|profile| profile.validate(cycles, guest_exit_code).err());
    let validation = if let Some(error) = execution_error.as_ref() {
        Err(anyhow!("{error}"))
    } else if guest_exit_code != 0 {
        Err(anyhow!("guest exited with status {guest_exit_code}"))
    } else if let Some(error) = profile_error {
        Err(error)
    } else {
        match args.guest_abi {
            GuestAbi::Boolean => validate_boolean_public_values(&public_values),
            GuestAbi::Batch => validate_boolean_public_values(&public_values),
            GuestAbi::StatementDigest => {
                ensure_digest(&public_values, &expected_statement_digest)
            },
            GuestAbi::Settlement => validate_public_values(&public_values, &vk_hash),
        }
    };
    let accepted = validation.is_ok();
    let failure = validation.err().map(|error| error.to_string());
    let mut report = json!({
        "schema_version": 2,
        "guest_abi": match args.guest_abi {
            GuestAbi::Boolean => "boolean", GuestAbi::Settlement => "settlement",
            GuestAbi::Batch => "batch", GuestAbi::StatementDigest => "statement-digest",
        },
        "batch_size": args.batch_size,
        "corrupt_index": args.corrupt_index,
        "corrupt_statement": args.corrupt_statement,
        "expected_statement_digest": hex::encode(expected_statement_digest),
        "stdin_items_sha256": stdin_items.iter().map(|item| hex::encode(Sha256::digest(item))).collect::<Vec<_>>(),
        "mode": "cpu-execute-only",
        "sp1_executor_version": "6.10.0",
        "executor_implementation": "mizu-pasta-core-6.10.0",
        "pasta_executor_only": args.pasta_executor_only,
        "pasta_proving_air_available": true,
        "proof_generated": false,
        "pasta_syscall_counts": {
            "pallas_add": pasta_syscall_counts[0],
            "pallas_double": pasta_syscall_counts[1],
            "vesta_add": pasta_syscall_counts[2],
            "vesta_double": pasta_syscall_counts[3],
        },
        "uint256_mul_count": uint256_mul_count,
        "elf": args.elf,
        "elf_sha256": hex::encode(Sha256::digest(&elf)),
        "fixture_dir": args.fixture_dir,
        "fixture_sha256": fixture_hashes,
        "corrupt_accumulator_challenge": args.corrupt_accumulator_challenge,
        "corrupt_wrap_opening": args.corrupt_wrap_opening,
        "stdin_proof_sha256": hex::encode(Sha256::digest(&proof_input)),
        "cycles": cycles,
        "cycle_metric": "MinimalExecutorRunner::global_clk",
        "stage_cycles": stage_cycles,
        "guest_memory_limit_bytes": GUEST_MEMORY_LIMIT_BYTES,
        "elapsed_seconds": elapsed_seconds,
        "guest_exit_code": guest_exit_code,
        "public_values_len": public_values.len(),
        "public_values_sha256": hex::encode(Sha256::digest(&public_values)),
        "public_values_hex": hex::encode(&public_values),
        "proof_valid": if matches!(args.guest_abi, GuestAbi::StatementDigest) {
            accepted.then_some(true)
        } else { public_values.first().map(|value| *value == 1) },
        "vk_hash": if matches!(args.guest_abi, GuestAbi::Settlement) {
            public_values.get(1..33).map(hex::encode)
        } else { None },
        "expected_vk_hash": hex::encode(vk_hash),
        "accepted": accepted,
        "failure": failure,
    });
    if let Some(profile) = profile {
        report["precompile_profile"] = profile.to_json();
    }
    if let Some(parent) = args
        .output
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    fs::write(
        &args.output,
        format!("{}\n", serde_json::to_string_pretty(&report)?),
    )
    .with_context(|| format!("write {}", args.output.display()))?;
    println!("cycles: {cycles}; elapsed: {elapsed_seconds:.3}s; accepted: {accepted}");
    println!("report: {}", args.output.display());
    if !accepted {
        bail!(
            "{}",
            report["failure"].as_str().unwrap_or("guest rejected input")
        );
    }
    ensure!(
        !args.corrupt_accumulator_challenge && !args.corrupt_wrap_opening && !args.corrupt_statement,
        "corrupted proof input was unexpectedly accepted"
    );
    Ok(())
}

struct PrecompileProfile {
    report: ExecutionReport,
    page_protect_enabled: bool,
    opts: SP1CoreOpts,
    chunks: u64,
    max_chunk_entries: u64,
    replay_seconds: f64,
    last_progress: Instant,
}

impl PrecompileProfile {
    fn new(page_protect_enabled: bool) -> Self {
        // Explicitly pin settings read from the environment by SP1's default
        // constructor. Retained-event presets have no environment override.
        let opts = SP1CoreOpts {
            minimal_trace_chunk_threshold: PROFILE_TRACE_CHUNK_ENTRIES,
            trace_chunk_slots: 1,
            memory_limit: GUEST_MEMORY_LIMIT_BYTES,
            shard_size: 1 << 24,
            sharding_threshold: sp1_core_executor::ShardingThreshold {
                element_threshold: sp1_core_executor::ELEMENT_THRESHOLD,
                height_threshold: sp1_core_executor::HEIGHT_THRESHOLD,
            },
            ..SP1CoreOpts::default()
        };
        Self {
            report: ExecutionReport::default(),
            page_protect_enabled,
            opts,
            chunks: 0,
            max_chunk_entries: 0,
            replay_seconds: 0.0,
            last_progress: Instant::now(),
        }
    }

    fn replay(&mut self, chunk: &TraceChunkRaw, program: Arc<Program>) -> Result<()> {
        let started = Instant::now();
        // This is the same trace-replay API as SP1 6.10's execute worker. A zero
        // nonce suffices for execution statistics: no proof is produced.
        let mut vm = GasEstimatingVMEnum::new(chunk, program, [0; 4], self.opts.clone());
        self.report += vm
            .execute()
            .map_err(|error| anyhow!("gas estimating VM: {error}"))?;
        self.chunks += 1;
        self.max_chunk_entries = self.max_chunk_entries.max(chunk.num_mem_reads());
        self.replay_seconds += started.elapsed().as_secs_f64();
        ensure!(
            self.report.total_instruction_count() == chunk.global_clk_end(),
            "replay instruction count differs from executor clock at chunk {}",
            self.chunks
        );
        if self.last_progress.elapsed().as_secs() >= 30 {
            eprintln!(
                "precompile profile: {} chunks; {} guest instructions replayed",
                self.chunks,
                self.report.total_instruction_count()
            );
            self.last_progress = Instant::now();
        }
        Ok(())
    }

    fn validate(&self, cycles: u64, exit_code: u32) -> Result<()> {
        ensure!(self.chunks > 0, "profiling produced no trace chunks");
        ensure!(
            self.report.total_instruction_count() == cycles,
            "profile instruction count differs from executor clock"
        );
        ensure!(
            self.report.exit_code == u64::from(exit_code),
            "profile exit code differs from executor exit code"
        );
        Ok(())
    }

    fn to_json(&self) -> serde_json::Value {
        let syscall_counts: BTreeMap<_, _> = self
            .report
            .syscall_counts
            .iter()
            .filter(|(_, count)| **count > 0)
            .map(|(code, count)| (format!("{code:?}"), *count))
            .collect();
        let opcode_counts: BTreeMap<_, _> = self
            .report
            .opcode_counts
            .iter()
            .filter(|(_, count)| **count > 0)
            .map(|(opcode, count)| (format!("{opcode:?}"), *count))
            .collect();
        let mut air_rows = BTreeMap::<String, u64>::new();
        for (code, count) in self.report.syscall_counts.iter() {
            if *count > 0 {
                if let Some(air) = code.as_air_id() {
                    *air_rows.entry(air.to_string()).or_default() +=
                        count * air.rows_per_event() as u64;
                    if let Some(control) = air.control_air_id(self.page_protect_enabled) {
                        *air_rows.entry(control.to_string()).or_default() += count;
                    }
                }
            }
        }
        json!({
            "method": "SP1 6.10 GasEstimatingVMEnum synchronous bounded trace replay",
            "syscall_counts": syscall_counts,
            "syscall_count_scope": "Precompile invocations with mapped SP1 AIRs; excludes ordinary HALT, WRITE, HINT and COMMIT ecalls",
            "total_precompile_invocations": self.report.total_syscall_count(),
            "precompile_air_rows_unpadded": air_rows,
            "opcode_counts": opcode_counts,
            "replayed_instructions": self.report.total_instruction_count(),
            "replay_exit_code": self.report.exit_code,
            "sp1_gas_estimate": self.report.gas(),
            "gas_estimate_scope": "SP1 6.10 normalized gas model; chunk boundaries affect memory accounting. Not proof time, proof cost, or Ethereum gas",
            "trace_chunk_threshold_entries": PROFILE_TRACE_CHUNK_ENTRIES,
            "trace_buffer_capacity_bytes": sp1_jit::trace_capacity(Some(PROFILE_TRACE_CHUNK_ENTRIES)),
            "max_live_trace_chunks": 1,
            "trace_chunks_replayed": self.chunks,
            "max_observed_chunk_entries": self.max_chunk_entries,
            "replay_seconds": self.replay_seconds,
            "sp1_core_options": self.opts,
            "proof_nonce": ([0u32; 4]),
            "memory_limit_scope": "2 GiB guest-address entry limit; trace buffer and host overhead are additional, not an RSS cap",
        })
    }
}

fn load_fixture(directory: &Path) -> Result<(VerifiableProof, [u8; 32], serde_json::Value)> {
    let read = |name: &str| -> Result<String> {
        let path = directory.join(name);
        fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))
    };
    let vk_json = read("vk.serde.json")?;
    let proof_json = read("proof.serde.json")?;
    let skeleton_json = read("public_input_skeleton.json")?;
    let app_json = read("app_statement.json")?;
    let wrap_vk = parse_wrap_vk(&vk_json).map_err(|error| anyhow!("parse wrap VK: {error}"))?;
    let wrap_proof =
        parse_wrap_proof(&proof_json).map_err(|error| anyhow!("parse wrap proof: {error}"))?;
    let ocaml = OcamlProof::parse(&skeleton_json)
        .map_err(|error| anyhow!("parse public-input skeleton: {error}"))?;
    let app_statement = parse_app_statement_fields(&app_json)
        .map_err(|error| anyhow!("parse app statement: {error}"))?;
    let proof = ocaml
        .into_verifiable(wrap_proof, &wrap_vk, &app_statement)
        .map_err(|error| anyhow!("convert VerifiableProof: {error}"))?;
    let vk_hash: [u8; 32] = Sha256::digest(vk_json.as_bytes()).into();
    let hashes = json!({
        "vk.serde.json": hex::encode(vk_hash),
        "proof.serde.json": hex::encode(Sha256::digest(proof_json.as_bytes())),
        "public_input_skeleton.json": hex::encode(Sha256::digest(skeleton_json.as_bytes())),
        "app_statement.json": hex::encode(Sha256::digest(app_json.as_bytes())),
    });
    Ok((proof, vk_hash, hashes))
}

fn validate_boolean_public_values(actual: &[u8]) -> Result<()> {
    ensure!(actual == [1], "guest must commit exactly boolean true");
    Ok(())
}

fn ensure_digest(actual: &[u8], expected: &[u8; 32]) -> Result<()> {
    ensure!(actual == expected, "guest must commit exactly the expected statement/VK digest");
    Ok(())
}

fn validate_public_values(actual: &[u8], vk_hash: &[u8; 32]) -> Result<()> {
    // The no-settlement-witness path emits exactly: true, the baked VK hash,
    // two empty eight-field states and one empty action state (577 bytes).
    // Compare every byte, preventing a different receipt/guest from being
    // counted as a successful verification of this compatibility fixture.
    let mut expected = vec![0u8; 577];
    expected[0] = 1;
    expected[1..33].copy_from_slice(vk_hash);
    ensure!(
        actual == expected,
        "public values do not match the fixture VK and compatibility receipt"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{validate_public_values, validate_boolean_public_values, PrecompileProfile, GUEST_MEMORY_LIMIT_BYTES};
    use sp1_core_executor::{Instruction, Opcode, Program, SyscallCode};
    use sp1_core_executor_runner::MinimalExecutorRunner;
    use std::sync::Arc;

    #[test]
    fn boolean_output_rejects_false_and_extra_bytes() {
        assert!(validate_boolean_public_values(&[1]).is_ok());
        for bad in [vec![], vec![0], vec![1, 0], vec![2]] { assert!(validate_boolean_public_values(&bad).is_err()); }
    }

    #[test]
    fn receipt_validation_checks_validity_key_state_and_length() {
        let key = [7u8; 32];
        let mut receipt = vec![0; 577];
        receipt[0] = 1;
        receipt[1..33].copy_from_slice(&key);
        assert!(validate_public_values(&receipt, &key).is_ok());
        for offset in [0, 1, 33, 576] {
            let mut changed = receipt.clone();
            changed[offset] ^= 1;
            assert!(validate_public_values(&changed, &key).is_err());
        }
        assert!(validate_public_values(&receipt[..576], &key).is_err());
        receipt.push(0);
        assert!(validate_public_values(&receipt, &key).is_err());
    }

    #[test]
    fn bounded_replay_counts_precompiles_across_chunk_boundaries() {
        // Three Uint256MulMod invocations: 2 * 3^3 mod 17 = 3. Syscall
        // metadata is in guest memory so these are real executor dispatches.
        let mut instructions = vec![
            Instruction::new(Opcode::LD, 5, 0, 1536, false, true),
            Instruction::new(Opcode::ADDI, 10, 0, 1024, false, true),
            Instruction::new(Opcode::ADDI, 11, 0, 1056, false, true),
        ];
        for index in 0..3 {
            if index > 0 {
                instructions.push(Instruction::new(Opcode::LD, 5, 0, 1536, false, true));
            }
            instructions.push(Instruction::new(Opcode::ECALL, 5, 10, 11, false, false));
        }
        sp1_core_executor::add_halt(&mut instructions);
        let mut program = Program::new(instructions, 32, 32);
        let memory = Arc::make_mut(&mut program.memory_image);
        for address in (1024..1120).step_by(8) {
            memory.insert(address, 0);
        }
        memory.insert(1024, 2);
        memory.insert(1056, 3);
        memory.insert(1088, 17);
        memory.insert(1536, SyscallCode::UINT256_MUL as u64);
        let program = Arc::new(program);
        let mut expected_cycles = None;
        for threshold in [None, Some(1), Some(32), Some(1 << 20)] {
            let mut executor = MinimalExecutorRunner::new(
                program.clone(),
                false,
                threshold,
                GUEST_MEMORY_LIMIT_BYTES,
                0,
            );
            let mut profile = PrecompileProfile::new(false);
            while let Some(chunk) = executor.try_execute_chunk().unwrap() {
                profile.replay(&chunk, program.clone()).unwrap();
            }
            let cycles = executor.global_clk();
            assert_eq!(executor.get_memory_value(1024).value, 3);
            assert_eq!(*expected_cycles.get_or_insert(cycles), cycles);
            if threshold.is_some() {
                profile.validate(cycles, executor.exit_code()).unwrap();
                assert_eq!(profile.report.syscall_counts[SyscallCode::UINT256_MUL], 3);
                assert_eq!(profile.report.total_syscall_count(), 3);
                assert_eq!(profile.report.opcode_counts[Opcode::ECALL], 4);
                let json = profile.to_json();
                assert_eq!(json["precompile_air_rows_unpadded"]["Uint256MulMod"], 3);
                assert!(profile.report.gas().unwrap() > 0);
            }
        }
    }
}
