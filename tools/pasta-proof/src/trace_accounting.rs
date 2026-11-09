//! CPU-only record accounting with explicit execution acceptance. No trace
//! matrices, commitments, proof generation or cryptographic verification.
use anyhow::{bail, ensure, Context, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use slop_basefold::FriConfig;
use sp1_core_executor::{
    ExecutionRecord, Program, RetainedEventsPreset, SP1CoreOpts, ShardingThreshold, SyscallCode,
};
use sp1_core_machine::{
    io::SP1Stdin,
    riscv::RiscvAir,
    utils::{generate_records_streaming_with_summary, StreamedExecutionSummary},
};
use sp1_hypercube::{
    air::MachineAir, prover::shape_from_record, InnerSC, InteractionKind, MachineVerifier,
    ShardVerifier,
};
use sp1_primitives::{SP1Field, SP1GlobalContext};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::Instant,
};
use strum::IntoEnumIterator;

#[path = "instruction_profile.rs"]
mod instruction_profile;
#[path = "operand_profile.rs"]
mod operand_profile;
#[path = "retention_audit.rs"]
mod retention_audit;
#[path = "input_contract.rs"]
mod input_contract;
#[path = "pasta_retention_audit.rs"]
mod pasta_retention_audit;
#[path = "compressed_point.rs"]
mod compressed_point;
#[path = "poseidon_round.rs"]
mod poseidon_round;

type Air = RiscvAir<SP1Field>;
type Verifier = MachineVerifier<SP1GlobalContext, InnerSC<Air>>;
const LOG_STACKING_HEIGHT: u32 = 21;
const MAX_LOG_ROW_COUNT: usize = 22;

fn sha(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
fn write_json(path: &Path, value: &Value) -> Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, format!("{}\n", serde_json::to_string_pretty(value)?))?;
    Ok(())
}

/// Pinned CPU record preset, independent of ambient SP1 environment variables.
/// A 256 MiB minimal chunk and one slot bound staging on the shared host. This
/// is a new matched preset; do not compare its shard areas with old defaults.
pub fn record_options() -> SP1CoreOpts {
    SP1CoreOpts {
        minimal_trace_chunk_threshold: 1 << 24,
        trace_chunk_slots: 1,
        memory_limit: 2 * 1024 * 1024 * 1024,
        shard_size: 1 << 24,
        sharding_threshold: ShardingThreshold {
            element_threshold: sp1_core_executor::ELEMENT_THRESHOLD,
            height_threshold: sp1_core_executor::HEIGHT_THRESHOLD,
        },
        global_dependencies_opt: true,
        ..SP1CoreOpts::default()
    }
}
fn options_json(opts: &SP1CoreOpts) -> Value {
    let mut value = serde_json::to_value(opts).expect("serialize options");
    value["retained_events_presets"]
        .as_array_mut()
        .expect("preset array")
        .sort_by_key(|v| v.to_string());
    value
}
fn verifier() -> Verifier {
    MachineVerifier::new(ShardVerifier::from_basefold_parameters(
        FriConfig::default_fri_config(),
        LOG_STACKING_HEIGHT,
        MAX_LOG_ROW_COUNT,
        Air::machine(),
    ))
}

#[derive(Clone, Debug)]
pub struct Expectation {
    pub elf_sha256: String,
    pub proof_input_sha256: String,
    pub receipt_input_sha256: String,
    pub cycles: u64,
    pub public_values_sha256: String,
    pub public_values_len: usize,
    pub precompile_calls: BTreeMap<String, u64>,
}
impl Expectation {
    pub fn from_json(value: &Value) -> Result<Self> {
        ensure!(
            value["mode"] == "trace-expectation" && value["version"] == 1,
            "wrong expectation schema"
        );
        let hash = |name: &str| -> Result<String> {
            let text = value[name]
                .as_str()
                .with_context(|| format!("missing {name}"))?;
            ensure!(hex::decode(text)?.len() == 32, "{name} must be SHA256");
            Ok(text.to_ascii_lowercase())
        };
        let cycles = value["cycles"]
            .as_u64()
            .context("missing expected cycles")?;
        let length = value["public_values_len"]
            .as_u64()
            .context("missing expected output length")?;
        ensure!(
            cycles > 0 && length > 0,
            "positive expected cycles and output length required"
        );
        let calls = value["precompile_calls"]
            .as_object()
            .context("expected precompile counts required")?;
        ensure!(
            !calls.is_empty(),
            "expected precompile count map cannot be empty"
        );
        let precompile_calls = calls
            .iter()
            .map(|(name, count)| {
                Ok((
                    name.clone(),
                    count
                        .as_u64()
                        .context("precompile count must be unsigned")?,
                ))
            })
            .collect::<Result<BTreeMap<_, _>>>()?;
        Ok(Self {
            precompile_calls,
            elf_sha256: hash("elf_sha256")?,
            proof_input_sha256: hash("proof_input_sha256")?,
            receipt_input_sha256: hash("receipt_input_sha256")?,
            cycles,
            public_values_sha256: hash("public_values_sha256")?,
            public_values_len: length.try_into()?,
        })
    }
    fn check_inputs(&self, elf: &str, proof: &str, receipt: &str) -> Result<(), &'static str> {
        if elf != self.elf_sha256 {
            return Err("elf_sha256");
        }
        if proof != self.proof_input_sha256 {
            return Err("proof_input_sha256");
        }
        if receipt != self.receipt_input_sha256 {
            return Err("receipt_input_sha256");
        }
        Ok(())
    }
    fn check_record_coverage(
        &self,
        cpu_events: u64,
        calls: &BTreeMap<String, u64>,
    ) -> Result<(), &'static str> {
        if cpu_events != self.cycles {
            return Err("cpu_record_coverage");
        }
        for (name, expected) in &self.precompile_calls {
            if calls.get(name).copied().unwrap_or(0) != *expected {
                return Err("precompile_record_coverage");
            }
        }
        Ok(())
    }
    fn check_execution(&self, summary: &StreamedExecutionSummary) -> Result<(), &'static str> {
        if summary.stopped_early {
            return Err("complete_record_stream");
        }
        let exit = summary.exit_code.ok_or("final_execution_outcome")?;
        let bytes = summary
            .public_values
            .as_deref()
            .ok_or("final_execution_outcome")?;
        if exit != 0 {
            return Err("successful_guest_exit");
        }
        if bytes.len() != self.public_values_len {
            return Err("public_values_length");
        }
        if sha(bytes) != self.public_values_sha256 {
            return Err("public_values_digest");
        }
        if summary.cycles != self.cycles {
            return Err("guest_cycles");
        }
        Ok(())
    }
}

/// Counts derived from raw event sources, independently of dependency generators.
/// This detects duplicate/missing generated Global events without generating any
/// dependencies a second time. Each local access contributes one receive/send;
/// each global boundary contributes one event; each sending syscall one event.
fn global_sources(record: &ExecutionRecord) -> BTreeMap<(u8, bool), u64> {
    let mut result = BTreeMap::new();
    let mut add = |kind: InteractionKind, receive: bool, count: usize| {
        if count > 0 {
            result.insert((kind as u8, receive), count as u64);
        }
    };
    let local = record.get_local_mem_events().count();
    add(
        InteractionKind::Memory,
        false,
        local + record.global_memory_initialize_events.len(),
    );
    add(
        InteractionKind::Memory,
        true,
        local + record.global_memory_finalize_events.len(),
    );
    let local = record.get_local_page_prot_events().count();
    add(
        InteractionKind::PageProtAccess,
        false,
        local + record.global_page_prot_initialize_events.len(),
    );
    add(
        InteractionKind::PageProtAccess,
        true,
        local + record.global_page_prot_finalize_events.len(),
    );
    add(
        InteractionKind::Syscall,
        false,
        record
            .syscall_events
            .iter()
            .filter(|(event, _)| event.should_send)
            .count(),
    );
    add(
        InteractionKind::Syscall,
        true,
        record
            .precompile_events
            .all_events()
            .filter(|(event, _)| event.should_send)
            .count(),
    );
    result
}
fn check_global_sources(
    record: &ExecutionRecord,
) -> Result<BTreeMap<(u8, bool), u64>, &'static str> {
    let expected = global_sources(record);
    if expected.values().sum::<u64>() != record.global_interaction_events.len() as u64 {
        return Err("global_event_total");
    }
    let mut actual = BTreeMap::new();
    for event in &record.global_interaction_events {
        *actual.entry((event.kind, event.is_receive)).or_default() += 1;
    }
    if actual != expected {
        return Err("global_event_sources");
    }
    Ok(actual)
}

#[derive(Default)]
struct ChipTotals {
    rows: u64,
    main_width: u64,
    prep_width: u64,
    shards: u64,
}
#[derive(Default)]
struct Accounting {
    records: usize,
    cpu_events: u64,
    instruction_profile: Option<instruction_profile::InstructionProfile>,
    operand_profile: Option<operand_profile::OperandProfile>,
    precompile_calls: BTreeMap<String, u64>,
    precompile_calls_on_cpu: BTreeMap<String, u64>,
    rows: BTreeMap<String, ChipTotals>,
    excluded_rows: BTreeMap<String, u64>,
    excluded_shards: BTreeMap<String, u64>,
    globals: BTreeMap<(u8, bool), u64>,
    memory_init: u64,
    memory_final: u64,
    main_shape_area: u64,
    prep_shape_area: u64,
    main_padding_cols: u64,
    prep_padding_cols: u64,
    per_record: Vec<Value>,
    last_internal_values_hash: Option<String>,
}
impl Accounting {
    fn observe(&mut self, record: &ExecutionRecord, verifier: &Verifier) -> Result<()> {
        if let Some(profile) = &mut self.instruction_profile { profile.observe(record)?; }
        if let Some(profile) = &mut self.operand_profile { profile.observe(record)?; }
        let globals = check_global_sources(record)
            .map_err(|check| anyhow::anyhow!("record audit failed: {check}"))?;
        for (kind, count) in globals {
            *self.globals.entry(kind).or_default() += count;
        }
        self.records += 1;
        self.cpu_events += record.cpu_event_count as u64;
        let mut record_calls = BTreeMap::new();
        for code in sp1_core_executor::SyscallCode::iter() {
            if code.should_send() != 1 {
                continue;
            }
            if let Some(events) = record.precompile_events.get_events(code) {
                if !events.is_empty() {
                    let name = format!("{code:?}");
                    let count = events.len() as u64;
                    record_calls.insert(name.clone(), count);
                    *self.precompile_calls.entry(name.clone()).or_default() += count;
                    if record.cpu_event_count > 0 {
                        *self.precompile_calls_on_cpu.entry(name).or_default() += count;
                    }
                }
            }
        }

        // Raw event coverage is insufficient if a shape accidentally omits
        // the AIR that must constrain those events. Gate every used arithmetic
        // precompile in this benchmark, including retained CPU-side events.
        for (code, name) in [
            (SyscallCode::UINT256_MUL, "Uint256MulMod"),
            (SyscallCode::VESTA_ADD, "VestaAddAssign"),
            (SyscallCode::VESTA_DOUBLE, "VestaDoubleAssign"),
            (SyscallCode::PALLAS_ADD, "PallasAddAssign"),
            (SyscallCode::PALLAS_DOUBLE, "PallasDoubleAssign"),
        ] {
            let count = record
                .precompile_events
                .get_events(code)
                .map_or(0, |v| v.len());
            if count > 0 {
                let chip = verifier
                    .machine()
                    .chips()
                    .iter()
                    .find(|chip| chip.name() == name)
                    .with_context(|| format!("missing precompile chip {name}"))?;
                ensure!(
                    chip.included(record),
                    "precompile_chip_coverage: {name} excluded"
                );
                ensure!(
                    MachineAir::<SP1Field>::num_rows(chip, record).unwrap_or(0) >= count,
                    "precompile_chip_coverage: {name} rows below event count"
                );
            }
        }

        self.memory_init += record.global_memory_initialize_events.len() as u64;
        self.memory_final += record.global_memory_finalize_events.len() as u64;
        let mut included = 0;
        let mut stored = 0;
        let mut main = 0;
        let mut prep = 0;
        for chip in verifier.machine().chips() {
            let name = chip.name().to_string();
            let rows = MachineAir::<SP1Field>::num_rows(chip, record).unwrap_or(0) as u64;
            if chip.included(record) {
                included += 1;
                let prep_width = MachineAir::<SP1Field>::preprocessed_width(chip) as u64;
                let main_width = chip
                    .cost()
                    .checked_sub(prep_width)
                    .context("chip cost below preprocessed width")?;
                let total = self.rows.entry(name).or_default();
                total.rows += rows;
                total.shards += 1;
                total.main_width = main_width;
                total.prep_width = prep_width;
                stored += rows;
                main += rows * main_width;
                prep += rows * prep_width;
            } else if rows > 0 {
                *self.excluded_rows.entry(name.clone()).or_default() += rows;
                *self.excluded_shards.entry(name).or_default() += 1;
            }
        }
        let shape = shape_from_record(verifier, record)
            .with_context(|| format!("included chips have no supported machine shape: {:?}",
                verifier.machine().chips().iter().filter(|chip|chip.included(record)).map(|chip|chip.name()).collect::<Vec<_>>()))?;
        // Compare the machine-cost width decomposition with the official shape
        // helper, not a per-chip next-power-of-two approximation.
        let height = 1u64 << LOG_STACKING_HEIGHT;
        ensure!(
            main.next_multiple_of(height) == shape.main_area as u64,
            "main shape area differs from stored cells"
        );
        ensure!(
            prep.next_multiple_of(height) == shape.preprocessed_area as u64,
            "preprocessed shape area differs from stored cells"
        );
        self.main_shape_area += shape.main_area as u64;
        self.prep_shape_area += shape.preprocessed_area as u64;
        self.main_padding_cols += shape.main_padding_cols as u64;
        self.prep_padding_cols += shape.preprocessed_padding_cols as u64;
        self.last_internal_values_hash = Some(sha(&bincode::serialize(&record.public_values)?));
        self.per_record.push(json!({"record":self.records,"included_chips":included,
            "cpu_instruction_events":record.cpu_event_count,"precompile_calls":record_calls,
            "shape_cluster_chips":shape.shard_chips.len(),"stored_rows":stored,"main_stored_cells":main,
            "preprocessed_stored_cells":prep,"main_shape_area":shape.main_area,
            "preprocessed_shape_area":shape.preprocessed_area,"main_padding_cols":shape.main_padding_cols,
            "preprocessed_padding_cols":shape.preprocessed_padding_cols,
            "global_interaction_events":record.global_interaction_events.len()}));
        Ok(())
    }
    fn report(&self) -> Value {
        let main: u64 = self.rows.values().map(|v| v.rows * v.main_width).sum();
        let prep: u64 = self.rows.values().map(|v| v.rows * v.prep_width).sum();
        let mut top: Vec<_> = self
            .rows
            .iter()
            .map(|(name, v)| (name, v.rows * v.main_width))
            .collect();
        top.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
        json!({"cpu_instruction_events":self.cpu_events,"precompile_calls":self.precompile_calls,
            "precompile_calls_on_cpu":self.precompile_calls_on_cpu,"totals":{"stored_rows":self.rows.values().map(|v|v.rows).sum::<u64>(),
            "main_cells":main,"preprocessed_cells":prep,"memory_initialize_events":self.memory_init,
            "memory_finalize_events":self.memory_final,"global_interaction_events":self.globals.values().sum::<u64>(),
            "global_sends":self.globals.iter().filter(|((_,r),_)|!*r).map(|(_,n)|n).sum::<u64>(),
            "global_receives":self.globals.iter().filter(|((_,r),_)|*r).map(|(_,n)|n).sum::<u64>(),
            "main_shape_area":self.main_shape_area,"preprocessed_shape_area":self.prep_shape_area,
            "aggregate_main_area_rounding":self.main_shape_area-main,
            "aggregate_preprocessed_area_rounding":self.prep_shape_area-prep,
            "shape_main_padding_columns":self.main_padding_cols,"shape_preprocessed_padding_columns":self.prep_padding_cols,
            "rows_from_chips_reporting_while_excluded":self.excluded_rows.values().sum::<u64>()},
            "per_chip":self.rows.iter().map(|(name,v)|json!({"chip":name,"stored_rows":v.rows,
                "main_width":v.main_width,"preprocessed_width":v.prep_width,"main_cells":v.rows*v.main_width,
                "preprocessed_cells":v.rows*v.prep_width,"shards_included":v.shards})).collect::<Vec<_>>(),
            "global_by_kind_direction":self.globals.iter().map(|((kind,receive),n)|json!({"kind":kind,"is_receive":receive,"events":n})).collect::<Vec<_>>(),
            "shape_cluster_membership":self.excluded_rows.iter().map(|(name,n)|json!({"chip":name,"rows_if_counted":n,"records":self.excluded_shards[name]})).collect::<Vec<_>>(),
            "top_by_main_cells":top.iter().take(20).map(|(name,n)|json!({"chip":name,"main_cells":n,"share":*n as f64/main.max(1) as f64})).collect::<Vec<_>>(),
            "per_record":self.per_record})
    }
}
fn peak_rss_bytes() -> u64 {
    // SAFETY: getrusage writes to a fully allocated native rusage buffer.
    let mut usage: libc::rusage = unsafe { core::mem::zeroed() };
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) } != 0 {
        return 0;
    }
    let raw = usage.ru_maxrss as u64;
    if cfg!(target_os = "macos") {
        raw
    } else {
        raw * 1024
    }
}

pub fn run(
    elf_path: &Path,
    input_dir: &Path,
    output: &Path,
    max_records: Option<usize>,
    reference: &Path,
) -> Result<()> {
    run_with_options(
        elf_path,
        input_dir,
        output,
        max_records,
        reference,
        record_options(),
    )
}

pub fn run_with_options(
    elf_path: &Path,
    input_dir: &Path,
    output: &Path,
    max_records: Option<usize>,
    reference: &Path,
    opts: SP1CoreOpts,
) -> Result<()> {
    run_with_profile(elf_path,input_dir,output,max_records,reference,opts,None,None)
}

fn run_with_profile(
    elf_path: &Path,
    input_dir: &Path,
    output: &Path,
    max_records: Option<usize>,
    reference: &Path,
    opts: SP1CoreOpts,
    profile_path: Option<&Path>,
    operand_path: Option<&Path>,
) -> Result<()> {
    ensure!(max_records != Some(0), "max-records must be positive");
    let expected = Expectation::from_json(&serde_json::from_slice(&fs::read(reference)?)?)?;
    let started = Instant::now();
    let elf = fs::read(elf_path)?;
    let proof = fs::read(input_dir.join("proof-input.bin"))?;
    let receipt = fs::read(input_dir.join("receipt-input.bin"))?;
    let elf_hash = sha(&elf);
    let proof_hash = sha(&proof);
    let receipt_hash = sha(&receipt);
    expected
        .check_inputs(&elf_hash, &proof_hash, &receipt_hash)
        .map_err(|c| anyhow::anyhow!("input gate failed: {c}"))?;
    let program = Arc::new(Program::from(&elf).map_err(|e| anyhow::anyhow!("ELF decode: {e}"))?);
    for preset in &opts.retained_events_presets {
        let pc = match preset {
            RetainedEventsPreset::VestaAddAtPc { pc } | RetainedEventsPreset::PallasAddAtPc { pc } => *pc,
            _ => continue,
        };
        let offset = pc.checked_sub(program.pc_base).context("retained PC below program")?;
        ensure!(offset%4==0 && program.instructions.get((offset/4) as usize).is_some_and(|i|i.opcode==sp1_core_executor::Opcode::ECALL),"retained PC must name an ECALL in this pinned ELF");
    }
    let mut stdin = SP1Stdin::new();
    stdin.write_slice(&proof);
    stdin.write_slice(&receipt);
    let opts_json = options_json(&opts);
    let verifier = verifier();
    let mut totals = Accounting::default();
    if profile_path.is_some() { totals.instruction_profile=Some(instruction_profile::InstructionProfile::new(&program)?); }
    if let Some(path)=operand_path { totals.operand_profile=Some(operand_profile::OperandProfile::new(path)?); }
    let profile_program=program.clone();
    let mut callback_error = None;
    let mut progress = Instant::now();
    let summary = generate_records_streaming_with_summary::<SP1Field, _>(
        program,
        stdin,
        opts,
        [0; 4],
        |record| {
            if let Err(error) = totals.observe(&record, &verifier) {
                callback_error = Some(error.to_string());
                return false;
            }
            if progress.elapsed().as_secs() >= 30 {
                eprintln!("trace: {} records consumed", totals.records);
                progress = Instant::now();
            }
            !max_records.is_some_and(|cap| totals.records >= cap)
        },
    )
    .map_err(|e| anyhow::anyhow!("record execution: {e}"))?;
    if let Some(error) = callback_error {
        bail!("{error}");
    }
    // Pin inputs through execution too, so concurrent edits cannot be silently
    // associated with a report for a different ELF/input pair.
    ensure!(
        sha(&fs::read(elf_path)?) == elf_hash
            && sha(&fs::read(input_dir.join("proof-input.bin"))?) == proof_hash
            && sha(&fs::read(input_dir.join("receipt-input.bin"))?) == receipt_hash,
        "execution inputs changed"
    );
    let acceptance = expected
        .check_execution(&summary)
        .and_then(|()| expected.check_record_coverage(totals.cpu_events, &totals.precompile_calls));
    let rejection = acceptance.err();
    let mut report = totals.report();
    let object = report.as_object_mut().unwrap();
    object.extend(json!({"mode":"trace-accounting","version":3,"accepted":rejection.is_none(),
        "complete":!summary.stopped_early,"incomplete_reason":if summary.stopped_early {Some("consumer stopped before the complete record stream was confirmed")}else{None},
        "rejection_check":rejection,"elf":elf_path,"elf_sha256":elf_hash,"proof_input_sha256":proof_hash,
        "receipt_input_sha256":receipt_hash,"guest_exit_code":summary.exit_code,"cycles":summary.cycles,
        "public_values_sha256":summary.public_values.as_ref().map(|b|sha(b)),"public_values_len":summary.public_values.as_ref().map(Vec::len),
        "expected_public_values_sha256":expected.public_values_sha256,"expected_cycles":expected.cycles,
        "last_record_internal_public_values_sha256":totals.last_internal_values_hash,"shards":totals.records,
        "wall_seconds":started.elapsed().as_secs_f64(),"peak_rss_bytes":peak_rss_bytes(),"opts":opts_json,
        "shape_config":{"log_stacking_height":LOG_STACKING_HEIGHT,"max_log_row_count":MAX_LOG_ROW_COUNT},
        "global_source_audit_passed":true,
        "caveats":["No trace matrix, commitment or proof generated. Stored cells and official shape areas are not measured proving time.",
            "Shape areas and padding-column fields come directly from SP1 shape_from_record; no synthetic per-chip power-of-two padding is reported.",
            "Per-chip totals do not attribute the surrounding instruction/memory traffic to program regions.",
            "The final serialized internal public-values hash is diagnostic; successful exit and actual guest committed bytes gate acceptance.",
            "This pinned 256 MiB-chunk, one-slot preset differs from historical v2 defaults. Compare only matched v3 options.",
            "Portable memory_limit bounds memory entries, not host RSS; staging and replay overhead are additional."]}).as_object().unwrap().clone());
    if let Some(path)=profile_path {
        report["instruction_profile"]=if rejection.is_none() && !summary.stopped_early {
            totals.instruction_profile.as_ref().unwrap().finish(&profile_program,summary.cycles,&elf_hash,path)?
        } else {json!({"complete":false,"cpu_instruction_events":totals.cpu_events,"reason":"execution or coverage gate rejected"})};
    }
    if let Some(profile)=&mut totals.operand_profile {
        report["operand_profile"]=profile.finish(&totals.precompile_calls,rejection.is_none() && !summary.stopped_early,&elf_hash)?;
    }
    write_json(output, &report)?;
    println!(
        "trace accepted={} complete={} cycles={} records={}",
        rejection.is_none(),
        !summary.stopped_early,
        summary.cycles,
        totals.records
    );
    if let Some(check) = rejection {
        bail!("trace acceptance failed: {check}");
    }
    Ok(())
}

pub fn cli() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut elf = None;
    let mut input = None;
    let mut output = None;
    let mut reference = None;
    let mut max_records = None;
    let mut selftest = false;
    let mut retention_selftest = false;
    let mut retain_uint256 = false;
    let mut profile_path = None;
    let mut profile_selftest = false;
    let mut operand_path = None;
    let mut operand_selftest = false;
    let mut retention_audit = false;
    let mut input_contract = false;
    let mut mutations = None;
    let mut pasta_retention = Vec::new();
    let mut pasta_retention_selftest = false;
    let mut compressed_selftest = false;
    let mut poseidon_selftest = false;
    let mut poseidon_params = None;
    let mut compressed_samples = None;
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--poseidon-round-selftest" {poseidon_selftest=true;i+=1;continue;}
        if args[i] == "--compressed-point-selftest" {compressed_selftest=true;i+=1;continue;}
        if args[i] == "--pasta-retention-selftest" {pasta_retention_selftest=true;i+=1;continue;}
        if args[i] == "--input-contract-selftest" {input_contract=true;i+=1;continue;}
        if args[i] == "--retention-audit-selftest" {retention_audit=true;i+=1;continue;}
        if args[i] == "--operand-profile-selftest" {operand_selftest=true;i+=1;continue;}
        if args[i] == "--instruction-profile-selftest" {profile_selftest=true;i+=1;continue;}
        if args[i] == "--selftest" {
            selftest = true;
            i += 1;
            continue;
        }
        if args[i] == "--retention-selftest" {
            retention_selftest = true;
            i += 1;
            continue;
        }
        if args[i] == "--retain-uint256-mul" {
            retain_uint256 = true;
            i += 1;
            continue;
        }
        let value = args
            .get(i + 1)
            .with_context(|| format!("{} needs a value", args[i]))?;
        match args[i].as_str() {
            "--elf" => elf = Some(PathBuf::from(value)),
            "--input" => input = Some(PathBuf::from(value)),
            "--output" => output = Some(PathBuf::from(value)),
            "--reference" => reference = Some(PathBuf::from(value)),
            "--max-records" => max_records = Some(value.parse::<usize>()?),
            "--instruction-profile" => profile_path = Some(PathBuf::from(value)),
            "--operand-profile" => operand_path = Some(PathBuf::from(value)),
            "--mutations" => mutations = Some(PathBuf::from(value)),
            "--retain-pasta-add" => pasta_retention.push(value.clone()),
            "--poseidon-parameters" => poseidon_params = Some(PathBuf::from(value)),
            "--compressed-samples" => compressed_samples = Some(PathBuf::from(value)),
            other => bail!("unexpected argument {other}"),
        }
        i += 2;
    }
    let output = output.context("--output required")?;
    let modes=[selftest,retention_selftest,profile_selftest,operand_selftest,retention_audit,input_contract,pasta_retention_selftest,compressed_selftest,poseidon_selftest];
    let selected_modes=modes.into_iter().filter(|mode|*mode).count();
    ensure!(selected_modes<=1,"select exactly one selftest mode");
    ensure!(input_contract || mutations.is_none(),"--mutations requires input-contract-selftest");
    ensure!(poseidon_selftest || poseidon_params.is_none(),"--poseidon-parameters requires poseidon-round-selftest");
    ensure!(compressed_selftest || compressed_samples.is_none(),"--compressed-samples requires compressed-point-selftest");
    if selected_modes==1 && !input_contract {
        ensure!(elf.is_none() && input.is_none() && reference.is_none() && max_records.is_none() && profile_path.is_none() && operand_path.is_none() && !retain_uint256 && pasta_retention.is_empty(),"selftest does not accept execution/routing arguments");
    }
    if poseidon_selftest {
        ensure!(!selftest && !profile_selftest && !retention_selftest && !operand_selftest && !retention_audit && !input_contract && !pasta_retention_selftest && !compressed_selftest && !retain_uint256 && profile_path.is_none() && operand_path.is_none() && pasta_retention.is_empty(),"select one selftest mode");
        return poseidon_round::selftest(&poseidon_params.context("--poseidon-parameters required")?,&output);
    }
    if compressed_selftest {
        ensure!(!selftest && !profile_selftest && !retention_selftest && !operand_selftest && !retention_audit && !input_contract && !pasta_retention_selftest && !retain_uint256 && profile_path.is_none() && operand_path.is_none() && pasta_retention.is_empty(),"select one selftest mode");
        return compressed_point::selftest(compressed_samples.as_deref(),&output);
    }
    if pasta_retention_selftest {
        ensure!(!selftest && !profile_selftest && !retention_selftest && !operand_selftest && !retention_audit && !input_contract && !retain_uint256 && profile_path.is_none() && operand_path.is_none() && pasta_retention.is_empty(),"select one selftest mode");
        return pasta_retention_audit::selftest(&output);
    }
    if input_contract {
        ensure!(!selftest && !profile_selftest && !retention_selftest && !operand_selftest && !retention_audit && !retain_uint256 && profile_path.is_none() && operand_path.is_none(),"select one selftest mode");
        return input_contract::selftest(&elf.context("--elf required")?,&input.context("--input required")?,&reference.context("--reference required")?,&mutations.context("--mutations required")?,&output);
    }
    if retention_audit {
        ensure!(!selftest && !profile_selftest && !retention_selftest && !operand_selftest && !retain_uint256 && profile_path.is_none() && operand_path.is_none(),"select one selftest mode");
        return retention_audit::selftest(&output);
    }
    if operand_selftest {
        ensure!(!selftest && !profile_selftest && !retention_selftest && !retain_uint256 && profile_path.is_none() && operand_path.is_none(),"select one selftest mode");
        return operand_profile::selftest(&output);
    }
    if profile_selftest {
        ensure!(!selftest && !retention_selftest && !retain_uint256 && profile_path.is_none(),"select one selftest mode");
        return instruction_profile::selftest(&output);
    }
    if selftest {
        ensure!(
            !retain_uint256 && !retention_selftest,
            "select one selftest mode"
        );
        return run_selftest(&output);
    }
    if retention_selftest {
        return run_retention_selftest(&output);
    }
    let mut opts = record_options();
    if retain_uint256 {
        opts.retained_events_presets
            .insert(RetainedEventsPreset::Uint256MulMod);
    }
    for spec in pasta_retention {
        let (curve,pc) = if let Some((curve,pc))=spec.split_once(':') {
            let pc=if let Some(hex)=pc.strip_prefix("0x"){u64::from_str_radix(hex,16)?}else{pc.parse()?};
            (curve,Some(pc))
        } else {(spec.as_str(),None)};
        let preset=match (curve,pc) {
            ("vesta",None)=>RetainedEventsPreset::VestaAdd,
            ("pallas",None)=>RetainedEventsPreset::PallasAdd,
            ("vesta",Some(pc))=>RetainedEventsPreset::VestaAddAtPc {pc},
            ("pallas",Some(pc))=>RetainedEventsPreset::PallasAddAtPc {pc},
            _=>bail!("retain-pasta-add must be vesta[:PC] or pallas[:PC]"),
        };
        opts.retained_events_presets.insert(preset);
    }
    if let Some(path)=&profile_path {
        ensure!(Some(path)!=elf.as_ref() && Some(path)!=reference.as_ref() && *path!=output,"profile path collides with an input/output");
        ensure!(!path.exists() && !path.with_extension("pc.bin").exists(),"profile output already exists");
    }
    if let Some(path)=&operand_path {
        ensure!(Some(path)!=elf.as_ref() && Some(path)!=reference.as_ref() && *path!=output && Some(path)!=profile_path.as_ref(),"operand path collides with input/output");
    }
    run_with_profile(
        &elf.context("--elf required")?,
        &input.context("--input required")?,
        &output,
        max_records,
        &reference.context("--reference expectation required")?,
        opts,
        profile_path.as_deref(),
        operand_path.as_deref(),
    )
}

// CPU-only fixture checks are implemented below; neither this module nor its
// dedicated binary exposes a proving route.

fn load_immediate(code: &mut Vec<sp1_core_executor::Instruction>, register: u8, value: u64) {
    use sp1_core_executor::{Instruction, Opcode};
    let high = value & !0xfff;
    let low = value & 0xfff;
    assert!(low < 0x800);
    code.push(Instruction::new(
        Opcode::LUI,
        register,
        high,
        high,
        true,
        true,
    ));
    if low != 0 {
        code.push(Instruction::new(
            Opcode::ADDI,
            register,
            register as u64,
            low,
            false,
            true,
        ));
    }
}
fn fixture(exit: u64) -> Arc<Program> {
    use sp1_core_executor::{Instruction, Opcode, SyscallCode};
    const BASE: u64 = 0x10000;
    let mut code = Vec::new();
    for _ in 0..3 {
        load_immediate(&mut code, 10, BASE);
        load_immediate(&mut code, 11, BASE + 32);
        load_immediate(&mut code, 5, SyscallCode::UINT256_MUL as u32 as u64);
        code.push(Instruction::new(Opcode::ECALL, 5, 10, 11, false, false));
    }
    // Publish the actual first result word. 2*3^3 mod17=3, independently of
    // SP1's arithmetic implementation. No COMMIT metadata is substituted for IO.
    load_immediate(
        &mut code,
        10,
        sp1_primitives::consts::fd::FD_PUBLIC_VALUES as u64,
    );
    load_immediate(&mut code, 11, BASE);
    load_immediate(&mut code, 12, 8);
    load_immediate(&mut code, 5, SyscallCode::WRITE as u32 as u64);
    code.push(Instruction::new(Opcode::ECALL, 5, 10, 11, false, false));
    load_immediate(&mut code, 10, exit);
    load_immediate(&mut code, 5, SyscallCode::HALT as u32 as u64);
    code.push(Instruction::new(Opcode::ECALL, 5, 10, 11, false, false));
    let mut program = Program::new(code, 32, 32);
    let image = Arc::make_mut(&mut program.memory_image);
    for address in (BASE..BASE + 96).step_by(8) {
        image.insert(address, 0);
    }
    image.insert(BASE, 2);
    image.insert(BASE + 32, 3);
    image.insert(BASE + 64, 17);
    Arc::new(program)
}
fn fixture_options() -> SP1CoreOpts {
    let mut opts = record_options();
    opts.minimal_trace_chunk_threshold = 64;
    opts.retained_events_presets.clear();
    opts
}
fn fixture_expectation(cycles: u64) -> Expectation {
    Expectation {
        elf_sha256: sha(b"synthetic-fixture"),
        proof_input_sha256: sha(&[]),
        receipt_input_sha256: sha(&[]),
        cycles,
        public_values_sha256: sha(&3u64.to_le_bytes()),
        public_values_len: 8,
        precompile_calls: BTreeMap::from([("UINT256_MUL".to_string(), 3)]),
    }
}
fn rejected(check: Result<(), &'static str>, expected: &str) -> Result<Value> {
    ensure!(
        check == Err(expected),
        "expected rejection {expected}, got {check:?}"
    );
    Ok(json!({"expected_check":expected,"actual_check":check.err(),"passed":true}))
}

pub fn run_selftest(output: &Path) -> Result<()> {
    use sp1_core_executor::SyscallCode;
    let started = Instant::now();
    let program = fixture(0);
    let opts = fixture_options();
    let mut records = Vec::new();
    let summary = generate_records_streaming_with_summary::<SP1Field, _>(
        program.clone(),
        SP1Stdin::new(),
        opts.clone(),
        [0; 4],
        |record| {
            records.push(record);
            true
        },
    )
    .map_err(|e| anyhow::anyhow!("fixture execution: {e}"))?;
    let expected = fixture_expectation(summary.cycles);
    expected
        .check_execution(&summary)
        .map_err(|c| anyhow::anyhow!("honest fixture rejected: {c}"))?;
    let calls: usize = records
        .iter()
        .map(|r| r.get_precompile_events(SyscallCode::UINT256_MUL).len())
        .sum();
    ensure!(calls == 3, "expected exactly three UInt events");
    let verifier = verifier();
    let mut accounting = Accounting::default();
    for record in &records {
        accounting.observe(record, &verifier)?;
    }
    expected
        .check_record_coverage(accounting.cpu_events, &accounting.precompile_calls)
        .map_err(|c| anyhow::anyhow!("honest record coverage failed: {c}"))?;
    ensure!(
        accounting
            .globals
            .get(&(InteractionKind::Syscall as u8, false))
            == Some(&3),
        "three syscall sends required"
    );
    ensure!(
        accounting
            .globals
            .get(&(InteractionKind::Syscall as u8, true))
            == Some(&3),
        "three syscall receives required"
    );
    let mut phantom = 0;
    for (name, count) in &accounting.excluded_rows {
        if name.contains("Vesta") || name.contains("Pallas") {
            phantom += *count;
            ensure!(
                !accounting.rows.contains_key(name),
                "unused Pasta chip leaked into included totals"
            );
        }
    }
    ensure!(
        phantom > 0,
        "fixture must challenge the old excluded-row bug"
    );
    ensure!(
        accounting
            .per_record
            .iter()
            .all(|r| r["included_chips"].as_u64().unwrap()
                < (verifier.machine().chips().len() as u64)),
        "fixture falsely included every chip"
    );
    let mut legacy = Accounting::default();
    let mut legacy_error = None;
    let legacy_cycles = sp1_core_machine::utils::generate_records_streaming::<SP1Field, _>(
        program.clone(),
        SP1Stdin::new(),
        opts.clone(),
        [0; 4],
        |record| {
            if let Err(error) = legacy.observe(&record, &verifier) {
                legacy_error = Some(error.to_string());
                return false;
            }
            true
        },
    )
    .map_err(|e| anyhow::anyhow!("legacy wrapper: {e}"))?;
    ensure!(
        legacy_error.is_none()
            && legacy_cycles == summary.cycles
            && legacy.report()["totals"] == accounting.report()["totals"],
        "legacy stream API changed record counts/cycles"
    );
    let mut cases = Vec::new();
    let cap = generate_records_streaming_with_summary::<SP1Field, _>(
        program,
        SP1Stdin::new(),
        opts.clone(),
        [0; 4],
        |_| false,
    )
    .map_err(|e| anyhow::anyhow!("capped fixture: {e}"))?;
    ensure!(
        cap.exit_code.is_none() && cap.public_values.is_none(),
        "stopped stream exposed final output"
    );
    cases.push(rejected(
        expected.check_execution(&cap),
        "complete_record_stream",
    )?);
    let unknown = StreamedExecutionSummary {
        cycles: summary.cycles,
        exit_code: None,
        public_values: None,
        stopped_early: false,
    };
    cases.push(rejected(
        expected.check_execution(&unknown),
        "final_execution_outcome",
    )?);
    let failed = generate_records_streaming_with_summary::<SP1Field, _>(
        fixture(1),
        SP1Stdin::new(),
        opts,
        [0; 4],
        |_| true,
    )
    .map_err(|e| anyhow::anyhow!("failing fixture: {e}"))?;
    ensure!(
        failed.exit_code == Some(1) && !failed.stopped_early,
        "actual failing guest not observed"
    );
    cases.push(rejected(
        expected.check_execution(&failed),
        "successful_guest_exit",
    )?);
    let fake = |bytes: Vec<u8>, cycles: u64| StreamedExecutionSummary {
        cycles,
        public_values: Some(bytes),
        exit_code: Some(0),
        stopped_early: false,
    };
    cases.push(rejected(
        expected.check_execution(&fake(vec![3], summary.cycles)),
        "public_values_length",
    )?);
    cases.push(rejected(
        expected.check_execution(&fake(4u64.to_le_bytes().to_vec(), summary.cycles)),
        "public_values_digest",
    )?);
    cases.push(rejected(
        expected.check_execution(&fake(
            summary.public_values.clone().unwrap(),
            summary.cycles + 1,
        )),
        "guest_cycles",
    )?);
    cases.push(rejected(
        expected.check_inputs(
            "wrong",
            &expected.proof_input_sha256,
            &expected.receipt_input_sha256,
        ),
        "elf_sha256",
    )?);
    cases.push(rejected(
        expected.check_inputs(
            &expected.elf_sha256,
            "wrong",
            &expected.receipt_input_sha256,
        ),
        "proof_input_sha256",
    )?);
    cases.push(rejected(
        expected.check_inputs(&expected.elf_sha256, &expected.proof_input_sha256, "wrong"),
        "receipt_input_sha256",
    )?);
    cases.push(rejected(
        expected.check_record_coverage(accounting.cpu_events - 1, &accounting.precompile_calls),
        "cpu_record_coverage",
    )?);
    let wrong_calls = BTreeMap::from([("UINT256_MUL".to_string(), 2)]);
    cases.push(rejected(
        expected.check_record_coverage(accounting.cpu_events, &wrong_calls),
        "precompile_record_coverage",
    )?);
    let source = records
        .iter()
        .find(|r| !r.global_interaction_events.is_empty())
        .context("fixture lacks Global events")?;
    let mut changed = source.clone();
    changed
        .global_interaction_events
        .extend(source.global_interaction_events.iter().cloned());
    cases.push(rejected(
        check_global_sources(&changed).map(|_| ()),
        "global_event_total",
    )?);
    let mut changed = source.clone();
    changed.global_interaction_events.pop();
    cases.push(rejected(
        check_global_sources(&changed).map(|_| ()),
        "global_event_total",
    )?);
    let mut changed = source.clone();
    changed.global_interaction_events[0].kind = 255;
    cases.push(rejected(
        check_global_sources(&changed).map(|_| ()),
        "global_event_sources",
    )?);
    let mut changed = source.clone();
    changed.global_interaction_events[0].is_receive =
        !changed.global_interaction_events[0].is_receive;
    cases.push(rejected(
        check_global_sources(&changed).map(|_| ()),
        "global_event_sources",
    )?);
    for (case, name) in cases.iter_mut().zip([
        "consumer_cap",
        "missing_final_outcome",
        "actual_nonzero_exit",
        "truncated_output",
        "altered_output",
        "wrong_cycles",
        "wrong_elf",
        "wrong_proof_input",
        "wrong_receipt_input",
        "missing_cpu_event",
        "missing_precompile_event",
        "duplicate_global_events",
        "dropped_global_event",
        "altered_global_kind",
        "altered_global_direction",
    ]) {
        case["case"] = json!(name);
    }
    let report = json!({"status":"passed","mode":"trace-accounting-selftest","proof_generated":false,
        "expected_uint_calls":3,"actual_uint_calls":calls,"expected_public_values_hex":hex::encode(3u64.to_le_bytes()),
        "actual_public_values_sha256":sha(summary.public_values.as_ref().unwrap()),"cycles":summary.cycles,
        "records":records.len(),"excluded_pasta_rows_not_counted":phantom,"legacy_stream_equivalent":true,
        "global_source_audit_passed":true,"shape_config":{"log_stacking_height":LOG_STACKING_HEIGHT,"max_log_row_count":MAX_LOG_ROW_COUNT},
        "honest_accounting":accounting.report(),"negative_gates":cases,"negative_gate_count":cases.len(),
        "wall_seconds":started.elapsed().as_secs_f64(),
        "note":"CPU execution fixture only. It writes a known arithmetic result to the actual public stream; no proof or complete-verifier acceptance is claimed."});
    write_json(output, &report)?;
    println!(
        "trace selftest passed: three UInt calls, {} records, {} pinned rejection checks",
        records.len(),
        cases.len()
    );
    Ok(())
}

/// Independent small CPU routing test. No trace matrices or proofs are built.
pub fn run_retention_selftest(output: &Path) -> Result<()> {
    let machine = verifier();
    // The executor reserves this padding budget when deciding shard limits.
    // New cluster combinations must fit it even before any large CPU run.
    for cluster in &machine.machine().shape().chip_clusters {
        let columns: u64 = cluster.iter().map(|chip| chip.cost()).sum();
        ensure!(
            32 * columns <= sp1_core_executor::MAXIMUM_PADDING_AREA,
            "retention shape exceeds executor padding budget"
        );
    }
    let mut cases = Vec::new();
    for retained in [false, true] {
        let mut opts = fixture_options();
        if retained {
            opts.retained_events_presets
                .insert(RetainedEventsPreset::Uint256MulMod);
        }
        let mut records = Vec::new();
        let summary = generate_records_streaming_with_summary::<SP1Field, _>(
            fixture(0),
            SP1Stdin::new(),
            opts.clone(),
            [0; 4],
            |record| {
                records.push(record);
                true
            },
        )
        .map_err(|e| anyhow::anyhow!("retention fixture execution: {e}"))?;
        let expected = fixture_expectation(29);
        expected
            .check_execution(&summary)
            .map_err(|c| anyhow::anyhow!("retention outcome failed: {c}"))?;
        let mut accounting = Accounting::default();
        for record in &records {
            accounting.observe(record, &machine)?;
        }
        expected
            .check_record_coverage(accounting.cpu_events, &accounting.precompile_calls)
            .map_err(|c| anyhow::anyhow!("retention coverage failed: {c}"))?;
        let on_cpu = accounting
            .precompile_calls_on_cpu
            .get("UINT256_MUL")
            .copied()
            .unwrap_or(0);
        ensure!(
            on_cpu == if retained { 3 } else { 0 },
            "retention fixture CPU placement"
        );
        for receive in [false, true] {
            let n = accounting
                .globals
                .get(&(InteractionKind::Syscall as u8, receive))
                .copied()
                .unwrap_or(0);
            ensure!(
                n == if retained { 0 } else { 3 },
                "retention fixture syscall routing"
            );
        }
        cases.push(json!({"retained":retained,"cycles":summary.cycles,
            "exit_code":summary.exit_code,"output_sha256":sha(summary.public_values.as_deref().unwrap()),
            "opts":options_json(&opts),"accounting":accounting.report()}));
    }
    let before = cases[0]["accounting"]["totals"]["global_interaction_events"]
        .as_u64()
        .unwrap();
    let after = cases[1]["accounting"]["totals"]["global_interaction_events"]
        .as_u64()
        .unwrap();
    ensure!(
        after < before,
        "shared UInt operand buffers must reduce Global events"
    );
    write_json(
        output,
        &json!({"mode":"trace-retention-selftest","version":1,"status":"passed",
        "cases":cases,"global_events_before":before,"global_events_after":after,
        "no_proof_generated":true,"cryptographic_verifier_acceptance_claimed":false}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn outcome_gates_reject_cap_exit_output_and_cycle_changes() {
        let expected = fixture_expectation(100);
        let good = || StreamedExecutionSummary {
            cycles: 100,
            exit_code: Some(0),
            public_values: Some(3u64.to_le_bytes().to_vec()),
            stopped_early: false,
        };
        assert_eq!(expected.check_execution(&good()), Ok(()));
        let mut altered = good();
        altered.stopped_early = true;
        assert_eq!(
            expected.check_execution(&altered),
            Err("complete_record_stream")
        );
        let mut altered = good();
        altered.exit_code = Some(1);
        assert_eq!(
            expected.check_execution(&altered),
            Err("successful_guest_exit")
        );
        let mut altered = good();
        altered.public_values.as_mut().unwrap().pop();
        assert_eq!(
            expected.check_execution(&altered),
            Err("public_values_length")
        );
        let mut altered = good();
        altered.public_values.as_mut().unwrap()[0] ^= 1;
        assert_eq!(
            expected.check_execution(&altered),
            Err("public_values_digest")
        );
        let mut altered = good();
        altered.cycles += 1;
        assert_eq!(expected.check_execution(&altered), Err("guest_cycles"));
    }
    #[test]
    fn expectation_schema_requires_pinned_hashes_and_positive_work() {
        let value = json!({"mode":"trace-expectation","version":1,"elf_sha256":sha(b"elf"),
            "proof_input_sha256":sha(b"proof"),"receipt_input_sha256":sha(b"receipt"),"cycles":1,
            "public_values_sha256":sha(b"output"),"public_values_len":6,"precompile_calls":{"UINT256_MUL":3}});
        assert!(Expectation::from_json(&value).is_ok());
        for field in [
            "elf_sha256",
            "proof_input_sha256",
            "receipt_input_sha256",
            "public_values_sha256",
        ] {
            let mut bad = value.clone();
            bad[field] = json!("not a digest");
            assert!(Expectation::from_json(&bad).is_err());
        }
        for field in ["cycles", "public_values_len"] {
            let mut bad = value.clone();
            bad[field] = json!(0);
            assert!(Expectation::from_json(&bad).is_err());
        }
    }
}
