//! Read-only precompile operand stream from an unchanged guest. This records
//! access metadata, not a proof or an assertion that operations can be reordered.
use super::*;
use sp1_core_executor::{events::PrecompileEvent, Opcode};
use std::io::{BufWriter, Write};

pub(super) struct OperandProfile {
    path: PathBuf,
    writer: BufWriter<fs::File>,
    calls: BTreeMap<String, u64>,
    records: u64,
}
impl OperandProfile {
    pub(super) fn new(path: &Path) -> Result<Self> {
        ensure!(!path.exists(), "operand_output_exists");
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut writer = BufWriter::new(
            fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)?,
        );
        writer.write_all(b"SP1OP001")?;
        Ok(Self {
            path: path.to_owned(),
            writer,
            calls: BTreeMap::new(),
            records: 0,
        })
    }
    pub(super) fn observe(&mut self, record: &ExecutionRecord) -> Result<()> {
        self.records += 1;
        for (sys, event) in record.precompile_events.all_events() {
            let (clk, p, q, previous_p, previous_q) = match event {
                PrecompileEvent::VestaAdd(e) | PrecompileEvent::PallasAdd(e) => {
                    ensure!(
                        e.p.len() == 8
                            && e.q.len() == 8
                            && e.p_memory_records.len() == 8
                            && e.q_memory_records.len() == 8,
                        "operand_add_shape"
                    );
                    ensure!(
                        e.p.iter()
                            .zip(&e.p_memory_records)
                            .all(|(p, m)| *p == m.prev_value)
                            && e.q
                                .iter()
                                .zip(&e.q_memory_records)
                                .all(|(q, m)| *q == m.value),
                        "operand_add_values"
                    );
                    (
                        e.clk,
                        e.p_ptr,
                        e.q_ptr,
                        e.p_memory_records
                            .iter()
                            .map(|m| m.prev_timestamp)
                            .max()
                            .unwrap(),
                        e.q_memory_records
                            .iter()
                            .map(|m| m.prev_timestamp)
                            .max()
                            .unwrap(),
                    )
                }
                PrecompileEvent::VestaDouble(e) | PrecompileEvent::PallasDouble(e) => {
                    ensure!(
                        e.p.len() == 8 && e.p_memory_records.len() == 8,
                        "operand_double_shape"
                    );
                    (
                        e.clk,
                        e.p_ptr,
                        0,
                        e.p_memory_records
                            .iter()
                            .map(|m| m.prev_timestamp)
                            .max()
                            .unwrap(),
                        0,
                    )
                }
                PrecompileEvent::Uint256Mul(e) => {
                    ensure!(
                        e.x.len() == 4
                            && e.y.len() == 4
                            && e.modulus.len() == 4
                            && e.x_memory_records.len() == 4
                            && e.y_memory_records.len() == 4
                            && e.modulus_memory_records.len() == 4,
                        "operand_mul_shape"
                    );
                    (
                        e.clk,
                        e.x_ptr,
                        e.y_ptr,
                        e.x_memory_records
                            .iter()
                            .map(|m| m.prev_timestamp)
                            .max()
                            .unwrap(),
                        e.y_memory_records
                            .iter()
                            .chain(&e.modulus_memory_records)
                            .map(|m| m.prev_timestamp)
                            .max()
                            .unwrap(),
                    )
                }
                _ => continue,
            };
            ensure!(
                sys.clk == clk && sys.arg1 == p && (q == 0 || sys.arg2 == q),
                "operand_syscall_binding"
            );
            let offset = sys
                .pc
                .checked_sub(record.program.pc_base)
                .context("operand_pc_range")?;
            ensure!(
                offset % 4 == 0
                    && record
                        .program
                        .instructions
                        .get((offset / 4) as usize)
                        .is_some_and(|i| i.opcode == Opcode::ECALL),
                "operand_pc_opcode"
            );
            *self
                .calls
                .entry(format!("{:?}", sys.syscall_code))
                .or_default() += 1;
            for word in [
                sys.syscall_code as u32 as u64,
                clk,
                sys.pc,
                p,
                q,
                self.records,
                previous_p,
                previous_q,
            ] {
                self.writer.write_all(&word.to_le_bytes())?;
            }
        }
        Ok(())
    }
    pub(super) fn finish(
        &mut self,
        expected: &BTreeMap<String, u64>,
        complete: bool,
        elf_hash: &str,
    ) -> Result<Value> {
        self.writer.flush()?;
        for name in [
            "VESTA_ADD",
            "VESTA_DOUBLE",
            "PALLAS_ADD",
            "PALLAS_DOUBLE",
            "UINT256_MUL",
        ] {
            ensure!(
                self.calls.get(name).copied().unwrap_or(0)
                    == expected.get(name).copied().unwrap_or(0),
                "operand_count_coverage"
            );
        }
        let bytes = fs::read(&self.path)?;
        ensure!(
            bytes.len() as u64 == 8 + 64 * self.calls.values().sum::<u64>(),
            "operand_byte_coverage"
        );
        Ok(
            json!({"version":1,"complete":complete,"elf_sha256":elf_hash,"path":self.path,"sha256":sha(&bytes),"bytes":bytes.len(),"precompile_calls":self.calls,
          "format":"SP1OP001 then eight little-endian u64 per event: syscall code, clock, PC, p/x pointer, q/y pointer (0 for double), record number, max previous p/x timestamp, max previous q/y/modulus timestamp",
          "caveat":"Record order is not execution order. Sort by clock for reuse analysis. Repeated addresses do not imply safe chaining or lifetime-long bucket occupancy."}),
        )
    }
}

pub(super) fn selftest(output: &Path) -> Result<()> {
    let mut records = Vec::new();
    let summary = generate_records_streaming_with_summary::<SP1Field, _>(
        fixture(0),
        SP1Stdin::new(),
        fixture_options(),
        [0; 4],
        |r| {
            records.push(r);
            true
        },
    )
    .map_err(|e| anyhow::anyhow!("operand fixture: {e}"))?;
    ensure!(
        summary.cycles == 29 && summary.exit_code == Some(0),
        "operand_fixture_execution"
    );
    let path = output.with_extension("operands.bin");
    let mut profile = OperandProfile::new(&path)?;
    let mut expected = BTreeMap::new();
    expected.insert("UINT256_MUL".to_string(), 3);
    for r in &records {
        profile.observe(r)?;
    }
    let report = profile.finish(&expected, true, "fixture")?;
    ensure!(
        report["bytes"] == 200 && report["complete"] == true,
        "operand_fixture_export"
    );
    let mut rejecting = Vec::new();
    for (index, name) in ["clock", "argument", "pc", "shape", "coverage"]
        .iter()
        .enumerate()
    {
        let path = output.with_extension(format!("bad-{index}.bin"));
        let mut profile = OperandProfile::new(&path)?;
        let mut bad = records.clone();
        let event = bad
            .iter_mut()
            .find_map(|r| {
                r.precompile_events
                    .events
                    .get_mut(&SyscallCode::UINT256_MUL)
                    .filter(|v| !v.is_empty())
            })
            .context("fixture mul events")?;
        match *name {
            "clock" => event[0].0.clk += 1,
            "argument" => event[0].0.arg1 += 8,
            "pc" => event[0].0.pc += 4,
            "shape" => {
                if let PrecompileEvent::Uint256Mul(e) = &mut event[0].1 {
                    e.x.clear();
                }
            }
            "coverage" => {
                event.pop();
            }
            _ => unreachable!(),
        }
        let result = (|| -> Result<()> {
            for r in &bad {
                profile.observe(r)?;
            }
            profile.finish(&expected, true, "fixture")?;
            Ok(())
        })();
        ensure!(result.is_err(), "operand negative accepted: {name}");
        rejecting.push(*name);
    }
    write_json(
        output,
        &json!({"status":"passed","cycles":29,"calls":3,"rejecting_checks":rejecting,"profile":report,"proof_generated":false}),
    )
}
