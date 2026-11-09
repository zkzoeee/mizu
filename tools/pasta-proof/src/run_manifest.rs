//! Manifest-driven identity pins and prover options for core-proof runs.
//!
//! The `r5-gpu` driver hardcodes the r5 ELF hash and cycle count, so it
//! rejects any other guest. The fix is not to delete those guards: it is to
//! move them into a manifest so a run declares exactly which program, inputs,
//! outcome and per-arm options it is pinned to, and the driver refuses
//! anything else. `spikes/artifacts/r114-gpu-core-three-arm/run-manifest.json`
//! is the first such manifest; it carries the pins verbatim from
//! `r113-nine-task-audit/future-core-proof-plan.json`, which stays unmodified
//! as the historical record of the CPU phase.
//!
//! Two deliberate choices about how options are read:
//!
//! * `SP1CoreOpts` is deserialized **wholesale** from each arm's `opts`
//!   object, not assembled field by field. The requirement is to copy *all*
//!   options from the plan, and a hand-written field list silently drops any
//!   option added later. Deserializing the struct makes that a parse error
//!   instead.
//! * `RetainedEventsPreset` deserializes by name, so an unrecognised preset
//!   fails the load. Retention is the whole independent variable here: if an
//!   unknown name were skipped, the Pasta+UInt arm would quietly become the
//!   default arm and the comparison would be meaningless while still looking
//!   well-formed.
//!
//! This module is not gated on the `cuda` feature, so it compiles and its
//! tests run on a machine with no GPU. That is the point — the CUDA prove
//! path cannot be typechecked locally, so as little logic as possible lives
//! there.

use anyhow::{bail, ensure, Context, Result};
use serde_json::Value;
use sha2::{Digest, Sha256};
use sp1_core_executor::SP1CoreOpts;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

/// Production PCS settings every arm must use, so the arms stay comparable.
pub const EXPECTED_LOG_STACKING_HEIGHT: u32 = 21;
/// Production PCS row-count bound. SP1 asserts `HEIGHT_THRESHOLD <= 1 << 22`,
/// so the fixture suite's 17 is not valid for a real program.
pub const EXPECTED_MAX_LOG_ROW_COUNT: u32 = 22;
/// The manifest names the FRI config by source path rather than by value; the
/// driver resolves it to `core_fri_config()` and refuses any other string.
pub const EXPECTED_FRI: &str = "sp1_primitives::fri_params::core_fri_config()";

/// One routing configuration to prove.
#[derive(Debug, Clone)]
pub struct Arm {
    /// Arm name as the manifest spells it, used in output paths and reports.
    pub name: String,
    /// Every prover option for this arm, deserialized as a whole struct.
    pub opts: SP1CoreOpts,
    /// Record count measured in the CPU phase. A mismatch is reported, never
    /// silently accepted and never "fixed" by moving a threshold.
    pub expected_records: usize,
    /// Precompile call totals, which must be identical across arms: retention
    /// changes *where* events are traced, not how many occur.
    pub expected_precompile_calls: BTreeMap<String, u64>,
    /// The subset of those calls this arm expects to land in CPU shards.
    pub expected_calls_on_cpu: BTreeMap<String, u64>,
    /// Main cells measured in the CPU phase, for the comparison table. A
    /// prover input, not a proving time.
    pub measured_main_cells: u64,
}

/// A run's pinned identity and its arms.
#[derive(Debug, Clone)]
pub struct RunManifest {
    /// Run label, e.g. `r114-gpu-core-three-arm`.
    pub run: String,
    /// Guest cycles every arm must reproduce.
    pub cycles: u64,
    /// Advisory ELF path from the authoring machine. The hash is what binds;
    /// the path will differ on a rented host.
    pub elf_hint: PathBuf,
    /// SHA-256 the ELF must have.
    pub elf_sha256: String,
    /// SHA-256 the proof input must have.
    pub proof_input_sha256: String,
    /// SHA-256 the receipt input must have.
    pub receipt_input_sha256: String,
    /// Exact committed public-values length.
    pub public_values_len: usize,
    /// SHA-256 of those public values.
    pub public_values_sha256: String,
    /// Expected precompile call totals for the whole run.
    pub precompile_calls: BTreeMap<String, u64>,
    /// The arms, in manifest order.
    pub arms: Vec<Arm>,
    /// Total spending cap in USD, carried so the driver can print it and a
    /// reviewer can see what was authorized without a second file.
    pub usd_cap: f64,
}

impl RunManifest {
    /// Parse and validate a run manifest.
    ///
    /// # Errors
    ///
    /// Any missing or malformed field, an unrecognised retention preset, a
    /// non-production PCS setting, zero arms, or duplicate arm names.
    pub fn load(path: &Path) -> Result<Self> {
        let text = fs::read_to_string(path)
            .with_context(|| format!("reading run manifest {}", path.display()))?;
        let root: Value = serde_json::from_str(&text)
            .with_context(|| format!("parsing run manifest {}", path.display()))?;

        let pcs = obj(&root, "production_pcs")?;
        let fri = str_field(pcs, "fri")?;
        ensure!(
            fri == EXPECTED_FRI,
            "manifest FRI config is {fri:?}, expected {EXPECTED_FRI:?}; this driver resolves \
             only the production core config"
        );
        let log_stacking = u64_field(pcs, "log_stacking_height")?;
        let max_log_row = u64_field(pcs, "max_log_row_count")?;
        ensure!(
            log_stacking == u64::from(EXPECTED_LOG_STACKING_HEIGHT),
            "manifest log_stacking_height is {log_stacking}, expected {EXPECTED_LOG_STACKING_HEIGHT}"
        );
        ensure!(
            max_log_row == u64::from(EXPECTED_MAX_LOG_ROW_COUNT),
            "manifest max_log_row_count is {max_log_row}, expected {EXPECTED_MAX_LOG_ROW_COUNT}"
        );

        let precompile_calls = call_map(&root, "precompile_calls")?;

        let arms_value = root
            .get("arms")
            .and_then(Value::as_array)
            .context("manifest has no `arms` array")?;
        ensure!(!arms_value.is_empty(), "manifest declares no arms");

        let mut arms = Vec::with_capacity(arms_value.len());
        for (i, arm) in arms_value.iter().enumerate() {
            let name = str_field(arm, "name")
                .with_context(|| format!("arm {i} has no name"))?
                .to_owned();
            let opts_value = arm
                .get("opts")
                .with_context(|| format!("arm {name:?} has no `opts`"))?
                .clone();
            // Wholesale, so a future option cannot be dropped silently, and an
            // unknown retention preset fails here rather than downgrading the
            // arm to default routing.
            let opts: SP1CoreOpts = serde_json::from_value(opts_value)
                .with_context(|| format!("arm {name:?}: deserializing SP1CoreOpts"))?;
            ensure!(
                opts.global_dependencies_opt,
                "arm {name:?} has global_dependencies_opt = false; r10 measured that every GPU \
                 shard then fails shard_cryptographic_verification while traces, public values \
                 and the verification key all look correct"
            );
            arms.push(Arm {
                name,
                opts,
                expected_records: usize::try_from(u64_field(arm, "expected_records")?)
                    .context("expected_records does not fit usize")?,
                expected_precompile_calls: call_map(arm, "expected_precompile_calls")?,
                expected_calls_on_cpu: call_map(arm, "expected_calls_on_cpu")?,
                measured_main_cells: u64_field(arm, "measured_main_cells")?,
            });
        }

        for (i, a) in arms.iter().enumerate() {
            ensure!(
                !arms[..i].iter().any(|b| b.name == a.name),
                "duplicate arm name {:?}; arm names index output paths",
                a.name
            );
        }

        let usd_cap = obj(&root, "authorization")?
            .get("total_usd_cap")
            .and_then(Value::as_f64)
            .context("authorization.total_usd_cap missing or not a number")?;
        ensure!(
            usd_cap > 0.0,
            "authorization.total_usd_cap must be positive"
        );

        Ok(Self {
            run: str_field(&root, "run")?.to_owned(),
            cycles: u64_field(&root, "cycles")?,
            elf_hint: PathBuf::from(str_field(&root, "elf")?),
            elf_sha256: lower_hex_field(&root, "elf_sha256")?,
            proof_input_sha256: lower_hex_field(&root, "proof_input_sha256")?,
            receipt_input_sha256: lower_hex_field(&root, "receipt_input_sha256")?,
            public_values_len: usize::try_from(u64_field(&root, "public_values_len")?)
                .context("public_values_len does not fit usize")?,
            public_values_sha256: lower_hex_field(&root, "public_values_sha256")?,
            precompile_calls,
            arms,
            usd_cap,
        })
    }

    /// The arm with this name.
    ///
    /// # Errors
    ///
    /// No arm of that name; the message lists the names that do exist.
    pub fn arm(&self, name: &str) -> Result<&Arm> {
        self.arms.iter().find(|a| a.name == name).with_context(|| {
            let names: Vec<&str> = self.arms.iter().map(|a| a.name.as_str()).collect();
            format!("no arm named {name:?}; manifest has {names:?}")
        })
    }

    /// Hash a file and require the pinned value. Returns the hash so callers
    /// can record it.
    ///
    /// # Errors
    ///
    /// Unreadable file, or a hash other than the pinned one.
    pub fn verify_file(&self, what: &str, path: &Path, expected: &str) -> Result<String> {
        let bytes =
            fs::read(path).with_context(|| format!("reading {what} at {}", path.display()))?;
        let got = sha256_hex(&bytes);
        ensure!(
            got == expected,
            "{what} at {} has sha256 {got}, expected {expected}",
            path.display()
        );
        Ok(got)
    }

    /// Verify the frozen ELF by content. The manifest's path is only a hint,
    /// since a rented host stores it elsewhere.
    ///
    /// # Errors
    ///
    /// As [`Self::verify_file`].
    pub fn verify_elf(&self, path: &Path) -> Result<String> {
        self.verify_file("frozen guest ELF", path, &self.elf_sha256)
    }

    /// Verify the two input files.
    ///
    /// # Errors
    ///
    /// As [`Self::verify_file`].
    pub fn verify_inputs(&self, proof_input: &Path, receipt_input: &Path) -> Result<()> {
        self.verify_file("proof input", proof_input, &self.proof_input_sha256)?;
        self.verify_file("receipt input", receipt_input, &self.receipt_input_sha256)?;
        Ok(())
    }

    /// Require the outcome every arm must reproduce: cycles, the exact
    /// committed byte stream, and identical precompile totals.
    ///
    /// Retention moves where an event is traced. It must not change how many
    /// occur, so a difference here means the comparison is not between two
    /// routings of one execution and nothing downstream is comparable.
    ///
    /// # Errors
    ///
    /// Any mismatch, reported with both values.
    pub fn verify_outcome(
        &self,
        cycles: u64,
        public_values: &[u8],
        syscall_counts: &BTreeMap<String, u64>,
    ) -> Result<()> {
        ensure!(
            cycles == self.cycles,
            "guest ran {cycles} cycles, manifest pins {}",
            self.cycles
        );
        ensure!(
            public_values.len() == self.public_values_len,
            "committed {} bytes, manifest pins {}",
            public_values.len(),
            self.public_values_len
        );
        let pv = sha256_hex(public_values);
        ensure!(
            pv == self.public_values_sha256,
            "committed public values hash {pv}, manifest pins {}",
            self.public_values_sha256
        );
        for (op, want) in &self.precompile_calls {
            let got = syscall_counts.get(op).copied().unwrap_or(0);
            ensure!(
                got == *want,
                "{op} ran {got} times, manifest pins {want}; retention must not change how many \
                 calls occur, only where they are traced"
            );
        }
        for op in syscall_counts.keys() {
            ensure!(
                self.precompile_calls.contains_key(op),
                "{op} was used but the manifest pins no count for it"
            );
        }
        Ok(())
    }
}

/// What one core-proof run is pinned to, independent of how it is proved.
///
/// This exists so the CUDA prove path takes its identity as a parameter
/// instead of hardcoding one program's hashes. `gpu.rs` cannot be typechecked
/// without a CUDA toolchain, and macOS has none, so every piece of logic that
/// can live outside it does — including this.
#[derive(Debug, Clone)]
pub struct CoreIdentity {
    /// Value for the report's `mode` field.
    pub mode: String,
    /// Human label for the thing being proved: an arm name, or the historical
    /// `r5-both-v16-p13`.
    pub label: String,
    /// SHA-256 the ELF must have.
    pub elf_sha256: String,
    /// Cycles the guest must report.
    pub expected_cycles: u64,
    /// File name for the serialized proof inside the output directory.
    pub proof_file: String,
    /// Line printed on success. The historical r5 path prints
    /// `R5_GPU_PASSED`, which existing scripts grep for, so that string is
    /// preserved rather than generalised away.
    pub pass_marker: String,
    /// Extra report fields merged into the result, for manifest-derived
    /// expectations the historical r5 path does not have.
    pub extra_report: Value,
}

impl CoreIdentity {
    /// The identity for one arm of a manifest-driven run.
    #[must_use]
    pub fn for_arm(manifest: &RunManifest, arm: &Arm) -> Self {
        // Computed here rather than inside `json!`: the macro cannot parse a
        // turbofish or a generic annotation in a nested block.
        let mut presets: Vec<String> = arm
            .opts
            .retained_events_presets
            .iter()
            .map(|p| format!("{p:?}"))
            .collect();
        presets.sort();
        Self {
            mode: "manifest-core-gpu".to_owned(),
            label: arm.name.clone(),
            elf_sha256: manifest.elf_sha256.clone(),
            expected_cycles: manifest.cycles,
            proof_file: format!("{}.proof.bin", arm.name),
            pass_marker: "CORE_PROOF_PASSED".to_owned(),
            extra_report: serde_json::json!({
                "run": manifest.run,
                "arm": arm.name,
                "authorized_usd_cap": manifest.usd_cap,
                "expected_records_from_cpu_phase": arm.expected_records,
                "expected_calls_on_cpu": arm.expected_calls_on_cpu,
                "expected_precompile_calls": arm.expected_precompile_calls,
                "measured_main_cells_from_cpu_phase": arm.measured_main_cells,
                "retained_events_presets": presets,
                // Said plainly in the artifact so no reader has to infer it.
                "proof_type": "core (no recursion, no wrap, no Groth16)",
                "main_cells_are_prover_inputs_not_proving_time": true,
            }),
        }
    }

    /// Verify an ELF's bytes against this identity.
    ///
    /// # Errors
    ///
    /// A hash other than the pinned one.
    pub fn verify_elf_bytes(&self, bytes: &[u8]) -> Result<String> {
        let got = sha256_hex(bytes);
        ensure!(
            got == self.elf_sha256,
            "the ELF is not the pinned program for {}: got {got}, expected {}",
            self.label,
            self.elf_sha256
        );
        Ok(got)
    }

    /// Require the cycle count this run is pinned to.
    ///
    /// # Errors
    ///
    /// Any other cycle count.
    pub fn verify_cycles(&self, cycles: u64) -> Result<()> {
        ensure!(
            cycles == self.expected_cycles,
            "guest ran {cycles} cycles, {} is pinned to {}",
            self.label,
            self.expected_cycles
        );
        Ok(())
    }
}

/// File name of the proof input inside the input directory.
pub const PROOF_INPUT_FILE: &str = "proof-input.bin";
/// File name of the receipt input inside the input directory.
pub const RECEIPT_INPUT_FILE: &str = "receipt-input.bin";
/// File name of the expected committed output, exported from the Mina fixture
/// before any proving.
pub const PUBLIC_VALUES_FILE: &str = "public-values.bin";

/// Check every identity the manifest pins that can be checked without
/// proving, and describe the arms.
///
/// This runs before renting anything and again on the rented host as the
/// pre-flight, so a hash mismatch or a malformed manifest costs nothing
/// instead of being discovered part-way through a paid run. It deliberately
/// does not touch a GPU or generate a proof.
///
/// # Errors
///
/// A manifest that will not load, a missing file, or any hash other than the
/// pinned one.
pub fn preflight(manifest_path: &Path, elf: Option<&Path>, input_dir: &Path) -> Result<Value> {
    let m = RunManifest::load(manifest_path)?;

    let elf_path = elf.map_or_else(|| m.elf_hint.clone(), Path::to_path_buf);
    let elf_sha = m.verify_elf(&elf_path)?;

    let proof_input = input_dir.join(PROOF_INPUT_FILE);
    let receipt_input = input_dir.join(RECEIPT_INPUT_FILE);
    m.verify_inputs(&proof_input, &receipt_input)?;

    // The expected output is an input to the gate, not a product of the run:
    // it was exported from the Mina fixture before any proving, so comparing
    // the manifest's digest against it here means the later outcome check is
    // anchored to a file rather than to a string.
    let public_values = input_dir.join(PUBLIC_VALUES_FILE);
    let expected_output = if public_values.is_file() {
        let sha = m.verify_file(
            "expected public values",
            &public_values,
            &m.public_values_sha256,
        )?;
        let len = fs::metadata(&public_values)?.len();
        ensure!(
            len == m.public_values_len as u64,
            "expected public values file is {len} bytes, manifest pins {}",
            m.public_values_len
        );
        serde_json::json!({ "path": public_values, "sha256": sha, "len": len })
    } else {
        serde_json::json!({ "path": public_values, "present": false })
    };

    let default_cells = m.arms.first().map(|a| a.measured_main_cells);
    let arms: Vec<Value> = m
        .arms
        .iter()
        .map(|a| {
            let mut presets: Vec<String> = a
                .opts
                .retained_events_presets
                .iter()
                .map(|p| format!("{p:?}"))
                .collect();
            presets.sort();
            serde_json::json!({
                "name": a.name,
                "retained_events_presets": presets,
                "expected_records": a.expected_records,
                "expected_calls_on_cpu": a.expected_calls_on_cpu,
                "measured_main_cells": a.measured_main_cells,
                // Record-input cells from the CPU phase. Not proving time, and
                // not a VRAM prediction.
                "main_cells_vs_default_pct": default_cells.map(|d| {
                    #[allow(clippy::cast_precision_loss)]
                    let pct = (a.measured_main_cells as f64 - d as f64) / d as f64 * 100.0;
                    (pct * 1000.0).round() / 1000.0
                }),
            })
        })
        .collect();

    Ok(serde_json::json!({
        "mode": "manifest-check",
        "proof_generated": false,
        "run": m.run,
        "manifest": manifest_path,
        "manifest_sha256": sha256_hex(&fs::read(manifest_path)?),
        "authorized_usd_cap": m.usd_cap,
        "cycles": m.cycles,
        "elf": { "path": elf_path, "sha256": elf_sha },
        "proof_input": { "path": proof_input, "sha256": m.proof_input_sha256 },
        "receipt_input": { "path": receipt_input, "sha256": m.receipt_input_sha256 },
        "expected_output": expected_output,
        "precompile_calls": m.precompile_calls,
        "production_pcs": {
            "fri": EXPECTED_FRI,
            "log_stacking_height": EXPECTED_LOG_STACKING_HEIGHT,
            "max_log_row_count": EXPECTED_MAX_LOG_ROW_COUNT,
        },
        "arms": arms,
    }))
}

/// SHA-256 of a byte slice as lower-case hex.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    hex::encode(h.finalize())
}

fn obj<'a>(v: &'a Value, key: &str) -> Result<&'a Value> {
    v.get(key).with_context(|| format!("missing `{key}`"))
}

fn str_field<'a>(v: &'a Value, key: &str) -> Result<&'a str> {
    v.get(key)
        .and_then(Value::as_str)
        .with_context(|| format!("`{key}` missing or not a string"))
}

fn u64_field(v: &Value, key: &str) -> Result<u64> {
    v.get(key)
        .and_then(Value::as_u64)
        .with_context(|| format!("`{key}` missing or not an unsigned integer"))
}

fn lower_hex_field(v: &Value, key: &str) -> Result<String> {
    let s = str_field(v, key)?;
    ensure!(
        s.len() == 64
            && s.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "`{key}` is not a 64-character lower-case hex digest: {s:?}"
    );
    Ok(s.to_owned())
}

fn call_map(v: &Value, key: &str) -> Result<BTreeMap<String, u64>> {
    let Some(entry) = v.get(key) else {
        bail!("missing `{key}`");
    };
    let table = entry
        .as_object()
        .with_context(|| format!("`{key}` is not an object"))?;
    let mut out = BTreeMap::new();
    for (k, val) in table {
        let n = val
            .as_u64()
            .with_context(|| format!("`{key}.{k}` is not an unsigned integer"))?;
        out.insert(k.clone(), n);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sp1_core_executor::RetainedEventsPreset;

    fn manifest_path() -> PathBuf {
        // The tests run from the crate directory; the manifest lives in the
        // focused test-fixture directory.
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/run-manifest.json")
    }

    fn manifest() -> RunManifest {
        RunManifest::load(&manifest_path()).expect("the committed run manifest must load")
    }

    #[test]
    fn old_options_fail_closed_instead_of_silently_defaulting_new_fields() {
        let mut raw: Value =
            serde_json::from_str(&fs::read_to_string(manifest_path()).unwrap()).unwrap();
        raw["arms"][0]["opts"]
            .as_object_mut()
            .unwrap()
            .remove("gas_trace_chunk_threshold");
        let path = std::env::temp_dir().join(format!(
            "pasta610-missing-option-{}.json",
            std::process::id()
        ));
        fs::write(&path, serde_json::to_vec(&raw).unwrap()).unwrap();
        let error = RunManifest::load(&path).unwrap_err();
        let _ = fs::remove_file(path);
        assert!(format!("{error:#}").contains("gas_trace_chunk_threshold"));
    }

    #[test]
    fn the_run_manifest_loads_and_pins_the_frozen_identity() {
        let m = manifest();
        assert_eq!(m.cycles, 142_692_781);
        assert_eq!(
            m.elf_sha256,
            "992a8e5f149c0748db389699811905984bee182c82b98f2fcc787d6ef6517be1"
        );
        assert_eq!(
            m.proof_input_sha256,
            "649da7f45438f33c2b407e91ab110686347c8989e0ad5076d078819af9875cb1"
        );
        assert_eq!(
            m.receipt_input_sha256,
            "6e340b9cffb37a989ca544e6bb780a2c78901d3fb33738768511a30617afa01d"
        );
        assert_eq!(m.public_values_len, 577);
        assert_eq!(
            m.public_values_sha256,
            "e28098de97d14ccdf98aa05896114825a6cf004a8868d42976f4634c4904ef50"
        );
        assert_eq!(m.usd_cap, 2.0);
    }

    #[test]
    fn the_three_arms_carry_the_plans_retention_sets() {
        let m = manifest();
        let names: Vec<&str> = m.arms.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["default", "retain-uint", "retain-pasta-and-uint"]);

        let base = [
            RetainedEventsPreset::Bls12381Field,
            RetainedEventsPreset::Bn254Field,
            RetainedEventsPreset::Poseidon2,
            RetainedEventsPreset::Sha256,
            RetainedEventsPreset::U256Ops,
        ];

        let default = m.arm("default").unwrap();
        assert_eq!(default.opts.retained_events_presets.len(), base.len());
        for p in base {
            assert!(default.opts.retained_events_presets.contains(&p));
        }
        // The default preset already contains U256Ops. That is a different
        // thing from retaining Uint256MulMod, which is what arm 2 adds.
        assert!(!default
            .opts
            .retained_events_presets
            .contains(&RetainedEventsPreset::Uint256MulMod));

        let uint = m.arm("retain-uint").unwrap();
        assert!(uint
            .opts
            .retained_events_presets
            .contains(&RetainedEventsPreset::Uint256MulMod));
        assert!(!uint
            .opts
            .retained_events_presets
            .contains(&RetainedEventsPreset::VestaAdd));

        let both = m.arm("retain-pasta-and-uint").unwrap();
        for p in [
            RetainedEventsPreset::Uint256MulMod,
            RetainedEventsPreset::VestaAdd,
            RetainedEventsPreset::PallasAdd,
        ] {
            assert!(both.opts.retained_events_presets.contains(&p));
        }
        // Doubles stay on their planned routing; nothing retains them.
        assert_eq!(both.opts.retained_events_presets.len(), base.len() + 3);
    }

    #[test]
    fn every_arm_shares_the_prover_settings_that_keep_them_comparable() {
        let m = manifest();
        let first = &m.arms[0].opts;
        for a in &m.arms {
            assert_eq!(a.opts.shard_size, first.shard_size, "{}", a.name);
            assert_eq!(a.opts.memory_limit, first.memory_limit, "{}", a.name);
            assert_eq!(
                a.opts.trace_chunk_slots, first.trace_chunk_slots,
                "{}",
                a.name
            );
            assert_eq!(
                a.opts.minimal_trace_chunk_threshold, first.minimal_trace_chunk_threshold,
                "{}",
                a.name
            );
            assert_eq!(
                a.opts.sharding_threshold, first.sharding_threshold,
                "{}",
                a.name
            );
            assert!(a.opts.global_dependencies_opt, "{}", a.name);
        }
    }

    #[test]
    fn precompile_totals_are_identical_across_arms() {
        let m = manifest();
        for a in &m.arms {
            assert_eq!(
                a.expected_precompile_calls, m.precompile_calls,
                "arm {} expects different totals; retention moves events, it does not add or \
                 remove calls",
                a.name
            );
        }
        assert_eq!(m.precompile_calls["UINT256_MUL"], 533_878);
        assert_eq!(m.precompile_calls["VESTA_ADD"], 1_310_683);
        assert_eq!(m.precompile_calls["VESTA_DOUBLE"], 112);
        assert_eq!(m.precompile_calls["PALLAS_ADD"], 697_988);
        assert_eq!(m.precompile_calls["PALLAS_DOUBLE"], 369);
    }

    #[test]
    fn cpu_placement_grows_with_retention_and_never_covers_doubles() {
        let m = manifest();
        assert!(m.arm("default").unwrap().expected_calls_on_cpu.is_empty());
        assert_eq!(
            m.arm("retain-uint").unwrap().expected_calls_on_cpu,
            BTreeMap::from([("UINT256_MUL".to_owned(), 533_878)])
        );
        assert_eq!(
            m.arm("retain-pasta-and-uint")
                .unwrap()
                .expected_calls_on_cpu,
            BTreeMap::from([
                ("PALLAS_ADD".to_owned(), 697_988),
                ("UINT256_MUL".to_owned(), 533_878),
                ("VESTA_ADD".to_owned(), 1_310_683),
            ])
        );
        for a in &m.arms {
            for op in a.expected_calls_on_cpu.keys() {
                assert!(!op.ends_with("_DOUBLE"), "arm {} retains {op}", a.name);
            }
        }
    }

    #[test]
    fn expected_records_fall_as_retention_grows() {
        let m = manifest();
        assert_eq!(m.arm("default").unwrap().expected_records, 149);
        assert_eq!(m.arm("retain-uint").unwrap().expected_records, 134);
        assert_eq!(m.arm("retain-pasta-and-uint").unwrap().expected_records, 82);
    }

    #[test]
    fn an_unknown_retention_preset_fails_the_load_rather_than_downgrading_the_arm() {
        // The whole experiment is retention. A preset name this build does not
        // know must be an error: skipping it would turn the Pasta+UInt arm into
        // the default arm while every other field still looked right.
        let mut root: Value =
            serde_json::from_str(&fs::read_to_string(manifest_path()).unwrap()).unwrap();
        root["arms"][2]["opts"]["retained_events_presets"]
            .as_array_mut()
            .unwrap()
            .push(Value::String("NotAPresetThisBuildKnows".to_owned()));
        let tmp = std::env::temp_dir().join("mizu-run-manifest-unknown-preset.json");
        fs::write(&tmp, serde_json::to_vec(&root).unwrap()).unwrap();
        let err = RunManifest::load(&tmp).expect_err("an unknown preset must not load");
        let text = format!("{err:#}");
        assert!(
            text.contains("retain-pasta-and-uint"),
            "the error should name the arm: {text}"
        );
        let _ = fs::remove_file(&tmp);
    }

    #[test]
    fn global_dependencies_opt_false_fails_the_load() {
        // r10: with this off, every GPU shard failed
        // shard_cryptographic_verification while traces, public values, chip
        // inventory and the verification key were all correct. It cost a
        // 49-minute rental to find, so it is a load-time guard now.
        let mut root: Value =
            serde_json::from_str(&fs::read_to_string(manifest_path()).unwrap()).unwrap();
        root["arms"][0]["opts"]["global_dependencies_opt"] = Value::Bool(false);
        let tmp = std::env::temp_dir().join("mizu-run-manifest-no-global-deps.json");
        fs::write(&tmp, serde_json::to_vec(&root).unwrap()).unwrap();
        let err = RunManifest::load(&tmp).expect_err("global_dependencies_opt = false must fail");
        assert!(format!("{err:#}").contains("shard_cryptographic_verification"));
        let _ = fs::remove_file(&tmp);
    }

    #[test]
    fn a_non_production_pcs_setting_fails_the_load() {
        for (key, value) in [
            ("log_stacking_height", Value::from(17)),
            ("max_log_row_count", Value::from(17)),
        ] {
            let mut root: Value =
                serde_json::from_str(&fs::read_to_string(manifest_path()).unwrap()).unwrap();
            root["production_pcs"][key] = value;
            let tmp = std::env::temp_dir().join(format!("mizu-run-manifest-bad-{key}.json"));
            fs::write(&tmp, serde_json::to_vec(&root).unwrap()).unwrap();
            let err = RunManifest::load(&tmp)
                .unwrap_err_or_else_message(&format!("{key} must be pinned"));
            assert!(err.contains(key), "{err}");
            let _ = fs::remove_file(&tmp);
        }
    }

    /// The real 577 bytes, exported from the Mina fixture before any proving.
    /// Reading them here does double duty: the outcome gate's later checks are
    /// only reachable once the digest passes, and it ties the manifest's
    /// pinned digest to an artifact that exists on disk rather than to a
    /// string nobody has compared against anything.
    fn real_public_values() -> Vec<u8> {
        let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/public-values.bin");
        fs::read(&p).unwrap_or_else(|e| panic!("reading {}: {e}", p.display()))
    }

    #[test]
    fn the_pinned_digest_matches_the_exported_public_values_on_disk() {
        let m = manifest();
        let pv = real_public_values();
        assert_eq!(pv.len(), m.public_values_len);
        assert_eq!(sha256_hex(&pv), m.public_values_sha256);
    }

    #[test]
    fn the_outcome_gate_accepts_the_real_outcome() {
        let m = manifest();
        m.verify_outcome(m.cycles, &real_public_values(), &m.precompile_calls)
            .expect("the pinned outcome must pass its own gate");
    }

    #[test]
    fn the_outcome_gate_rejects_wrong_cycles_output_and_call_counts() {
        let m = manifest();
        let good_pv = real_public_values();
        let counts: BTreeMap<String, u64> = m.precompile_calls.clone();

        // One byte flipped in an otherwise correct stream must fail the digest.
        let mut flipped = good_pv.clone();
        flipped[0] ^= 1;
        let err = format!(
            "{:#}",
            m.verify_outcome(m.cycles, &flipped, &counts).unwrap_err()
        );
        assert!(err.contains("public values hash"), "{err}");

        let err = format!(
            "{:#}",
            m.verify_outcome(m.cycles + 1, &good_pv, &counts)
                .unwrap_err()
        );
        assert!(err.contains("142692781"), "{err}");

        let err = format!(
            "{:#}",
            m.verify_outcome(m.cycles, &[0u8; 576], &counts)
                .unwrap_err()
        );
        assert!(err.contains("576"), "{err}");

        let mut short = counts.clone();
        short.insert("VESTA_ADD".to_owned(), 1_310_682);
        let err = format!(
            "{:#}",
            m.verify_outcome(m.cycles, &good_pv, &short).unwrap_err()
        );
        assert!(err.contains("only where they are traced"), "{err}");

        let mut extra = counts;
        extra.insert("SOME_OTHER_SYSCALL".to_owned(), 1);
        let err = format!(
            "{:#}",
            m.verify_outcome(m.cycles, &good_pv, &extra).unwrap_err()
        );
        assert!(err.contains("SOME_OTHER_SYSCALL"), "{err}");
    }

    /// Small helper so the PCS test reads as one line per case.
    trait UnwrapErrMessage {
        fn unwrap_err_or_else_message(self, what: &str) -> String;
    }

    impl UnwrapErrMessage for Result<RunManifest> {
        fn unwrap_err_or_else_message(self, what: &str) -> String {
            match self {
                Ok(_) => panic!("{what}"),
                Err(e) => format!("{e:#}"),
            }
        }
    }
}
