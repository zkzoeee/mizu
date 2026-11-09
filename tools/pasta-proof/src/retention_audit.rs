//! Independent CPU record checks, explicitly not AIR or proof verification.
use super::*;
use num::{BigUint, One, Zero};
use sp1_core_executor::events::{PrecompileEvent, SyscallEvent, Uint256MulEvent};

fn integer(words: &[u64]) -> BigUint {
    BigUint::from_bytes_le(
        &words
            .iter()
            .flat_map(|w| w.to_le_bytes())
            .collect::<Vec<_>>(),
    )
}
pub(super) fn memory_history(records: &[ExecutionRecord], program: &Program) -> Result<()> {
    let mut balance: BTreeMap<(u64, u64, u64), i64> = BTreeMap::new();
    // Program-image words are initialized by MemoryProgram, not by the dynamic
    // MemoryGlobalInit event stream (emit_globals deliberately excludes them).
    for (&address, &value) in program.memory_image.iter() {
        *balance.entry((address, 0, value)).or_default() += 1;
    }
    for record in records {
        for local in record.get_local_mem_events() {
            *balance
                .entry((
                    local.addr,
                    local.initial_mem_access.timestamp,
                    local.initial_mem_access.value,
                ))
                .or_default() -= 1;
            *balance
                .entry((
                    local.addr,
                    local.final_mem_access.timestamp,
                    local.final_mem_access.value,
                ))
                .or_default() += 1;
        }
        for e in &record.global_memory_initialize_events {
            ensure!(
                e.timestamp == 0
                    && e.value == program.memory_image.get(&e.addr).copied().unwrap_or(0),
                "uint_history_initial_image"
            );
            *balance.entry((e.addr, e.timestamp, e.value)).or_default() += 1;
        }
        for e in &record.global_memory_finalize_events {
            *balance.entry((e.addr, e.timestamp, e.value)).or_default() -= 1;
        }
    }
    ensure!(balance.values().all(|n| *n == 0), "uint_history_balance");
    Ok(())
}
fn check(sys: &SyscallEvent, e: &Uint256MulEvent) -> Result<()> {
    ensure!(
        e.x.len() == 4
            && e.y.len() == 4
            && e.modulus.len() == 4
            && e.x_memory_records.len() == 4
            && e.y_memory_records.len() == 4
            && e.modulus_memory_records.len() == 4,
        "uint_audit_shape"
    );
    ensure!(
        sys.syscall_code == SyscallCode::UINT256_MUL
            && sys.clk == e.clk
            && sys.arg1 == e.x_ptr
            && sys.arg2 == e.y_ptr,
        "uint_audit_binding"
    );
    ensure!(e.x_ptr % 8 == 0 && e.y_ptr % 8 == 0, "uint_audit_alignment");
    ensure!(
        e.x.iter()
            .zip(&e.x_memory_records)
            .all(|(x, m)| *x == m.prev_value)
            && e.y
                .iter()
                .zip(&e.y_memory_records)
                .all(|(y, m)| *y == m.value)
            && e.modulus
                .iter()
                .zip(&e.modulus_memory_records)
                .all(|(p, m)| *p == m.value),
        "uint_audit_values"
    );
    ensure!(
        e.y_memory_records
            .iter()
            .chain(&e.modulus_memory_records)
            .all(|m| m.timestamp == e.clk && m.prev_timestamp < m.timestamp)
            && e.x_memory_records
                .iter()
                .all(|m| m.timestamp == e.clk + 1 && m.prev_timestamp < m.timestamp),
        "uint_audit_timestamps"
    );
    let modulus = integer(&e.modulus);
    let modulus = if modulus.is_zero() {
        BigUint::one() << 256
    } else {
        modulus
    };
    let output = integer(
        &e.x_memory_records
            .iter()
            .map(|m| m.value)
            .collect::<Vec<_>>(),
    );
    ensure!(
        output == (integer(&e.x) * integer(&e.y)) % modulus,
        "uint_audit_arithmetic"
    );
    Ok(())
}
fn program(rounds: usize, alias: bool, zero_mod: bool) -> Arc<Program> {
    use sp1_core_executor::{Instruction, Opcode};
    const BASE: u64 = 0x10000;
    let mut code = Vec::new();
    let rhs = if alias { BASE } else { BASE + 32 };
    for _ in 0..rounds {
        load_immediate(&mut code, 10, BASE);
        load_immediate(&mut code, 11, rhs);
        load_immediate(&mut code, 5, SyscallCode::UINT256_MUL as u32 as u64);
        code.push(Instruction::new(Opcode::ECALL, 5, 10, 11, false, false));
    }
    load_immediate(
        &mut code,
        10,
        sp1_primitives::consts::fd::FD_PUBLIC_VALUES as u64,
    );
    load_immediate(&mut code, 11, BASE);
    load_immediate(&mut code, 12, 32);
    load_immediate(&mut code, 5, SyscallCode::WRITE as u32 as u64);
    code.push(Instruction::new(Opcode::ECALL, 5, 10, 11, false, false));
    load_immediate(&mut code, 10, 0);
    load_immediate(&mut code, 5, SyscallCode::HALT as u32 as u64);
    code.push(Instruction::new(Opcode::ECALL, 5, 10, 11, false, false));
    let mut p = Program::new(code, 32, 32);
    let image = Arc::make_mut(&mut p.memory_image);
    for address in (BASE..BASE + 128).step_by(8) {
        image.insert(address, 0);
    }
    image.insert(BASE, 2);
    if !alias {
        image.insert(rhs, 3);
    }
    image.insert(rhs + 32, if zero_mod { 0 } else { 17 });
    Arc::new(p)
}
pub(super) fn selftest(output: &Path) -> Result<()> {
    let machine = verifier();
    let mut results = Vec::new();
    let mut negatives = Vec::new();
    for alias in [false, true] {
        for zero_mod in [false, true] {
            for boundary in [false, true] {
                let rounds = if boundary { 4097 } else { 37 };
                let p = program(rounds, alias, zero_mod);
                let modulus = if zero_mod {
                    BigUint::one() << 256
                } else {
                    BigUint::from(17u64)
                };
                let mut oracle = BigUint::from(2u64);
                for _ in 0..rounds {
                    let rhs = if alias {
                        oracle.clone()
                    } else {
                        BigUint::from(3u64)
                    };
                    oracle = (oracle * rhs) % &modulus;
                }
                let mut expected = oracle.to_bytes_le();
                expected.resize(32, 0);
                let mut pair = Vec::new();
                for retained in [false, true] {
                    let mut opts = fixture_options();
                    if retained {
                        opts.retained_events_presets
                            .insert(RetainedEventsPreset::Uint256MulMod);
                    }
                    if boundary {
                        opts.sharding_threshold.height_threshold = 8192;
                        opts.minimal_trace_chunk_threshold = 1 << 14;
                    }
                    let mut records = Vec::new();
                    let summary = generate_records_streaming_with_summary::<SP1Field, _>(
                        p.clone(),
                        SP1Stdin::new(),
                        opts,
                        [0; 4],
                        |r| {
                            records.push(r);
                            true
                        },
                    )
                    .map_err(|e| anyhow::anyhow!("uint boundary fixture: {e}"))?;
                    ensure!(
                        !summary.stopped_early
                            && summary.exit_code == Some(0)
                            && summary.public_values.as_deref() == Some(expected.as_slice()),
                        "uint_fixture_outcome"
                    );
                    memory_history(&records, &p)?;
                    if results.is_empty() && !retained {
                        for name in [
                            "history-read-value",
                            "history-final-clock",
                            "history-address",
                            "history-initial-image",
                        ] {
                            let mut bad = records.clone();
                            if name == "history-initial-image" {
                                let event = bad
                                    .iter_mut()
                                    .find_map(|r| r.global_memory_initialize_events.first_mut())
                                    .context("history initial event")?;
                                event.value ^= 1;
                            } else {
                                let local = bad
                                    .iter_mut()
                                    .find_map(|r| r.cpu_local_memory_access.first_mut())
                                    .context("history local event")?;
                                match name {
                                    "history-read-value" => local.initial_mem_access.value ^= 1,
                                    "history-final-clock" => local.final_mem_access.timestamp += 1,
                                    _ => local.addr ^= 8,
                                }
                            }
                            let want = if name == "history-initial-image" {
                                "uint_history_initial_image"
                            } else {
                                "uint_history_balance"
                            };
                            let error = memory_history(&bad, &p).unwrap_err().to_string();
                            ensure!(error == want, "history negative {name}: {error}");
                            negatives.push(
                                json!({"case":name,"expected_check":want,"actual_error":error}),
                            );
                        }
                    }
                    let mut count = 0;
                    let mut cpu_records = 0;
                    let mut stats = Accounting::default();
                    for r in &records {
                        stats.observe(r, &machine)?;
                        cpu_records += usize::from(r.cpu_event_count > 0);
                        for (sys, event) in r.precompile_events.all_events() {
                            if let PrecompileEvent::Uint256Mul(e) = event {
                                check(sys, e)?;
                                count += 1;
                                if !negatives.iter().any(|n| n["case"] == "binding") {
                                    for name in [
                                        "binding",
                                        "previous-value",
                                        "read-value",
                                        "modulus-value",
                                        "read-clock",
                                        "write-clock",
                                        "previous-clock",
                                        "result",
                                        "shape",
                                    ] {
                                        let mut bad = e.clone();
                                        let mut s = *sys;
                                        match name {
                                            "binding" => s.arg1 += 8,
                                            "previous-value" => {
                                                bad.x_memory_records[0].prev_value ^= 1
                                            }
                                            "read-value" => bad.y_memory_records[0].value ^= 1,
                                            "modulus-value" => {
                                                bad.modulus_memory_records[0].value ^= 1
                                            }
                                            "read-clock" => bad.y_memory_records[0].timestamp += 1,
                                            "write-clock" => bad.x_memory_records[0].timestamp += 1,
                                            "previous-clock" => {
                                                bad.x_memory_records[0].prev_timestamp =
                                                    bad.x_memory_records[0].timestamp
                                            }
                                            "result" => bad.x_memory_records[0].value ^= 1,
                                            "shape" => bad.x.pop().map(|_| ()).unwrap(),
                                            _ => unreachable!(),
                                        }
                                        let want = match name {
                                            "binding" => "uint_audit_binding",
                                            "previous-value" | "read-value" | "modulus-value" => {
                                                "uint_audit_values"
                                            }
                                            "read-clock" | "write-clock" | "previous-clock" => {
                                                "uint_audit_timestamps"
                                            }
                                            "result" => "uint_audit_arithmetic",
                                            _ => "uint_audit_shape",
                                        };
                                        let error = check(&s, &bad).unwrap_err().to_string();
                                        ensure!(error == want, "uint negative {name}: {error}");
                                        negatives.push(json!({"case":name,"expected_check":want,"actual_error":error}));
                                    }
                                }
                            }
                        }
                    }
                    ensure!(count == rounds, "uint_fixture_event_count");
                    if boundary {
                        ensure!(cpu_records > 1, "uint_fixture_shard_boundary_not_exercised");
                    }
                    let on_cpu = stats
                        .precompile_calls_on_cpu
                        .get("UINT256_MUL")
                        .copied()
                        .unwrap_or(0);
                    ensure!(
                        on_cpu == if retained { rounds as u64 } else { 0 },
                        "uint_fixture_routing"
                    );
                    pair.push(json!({"retained":retained,"cycles":summary.cycles,"cpu_records":cpu_records,"records":records.len(),"calls":count,"output_sha256":sha(&expected),"accounting":stats.report()}));
                }
                ensure!(
                    pair[0]["cycles"] == pair[1]["cycles"],
                    "uint_fixture_cycle_invariance"
                );
                results.push(json!({"alias":alias,"zero_modulus":zero_mod,"forced_boundaries":boundary,"variants":pair}));
            }
        }
    }
    write_json(
        output,
        &json!({"status":"passed","mode":"uint-retention-record-audit","cases":results,"negative_checks":negatives,"proof_generated":false,"cryptographic_verifier_acceptance_claimed":false,"caveat":"Independent host record consistency and execution oracle checks; malicious-record rejections here are not AIR or cryptographic verifier rejections."}),
    )
}
