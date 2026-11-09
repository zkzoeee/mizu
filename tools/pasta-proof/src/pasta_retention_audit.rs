//! CPU routing fixture: two distinct call sites on each curve, plus UInt.
use super::*;
use sp1_curves::{
    weierstrass::{
        pasta::{PallasParameters, VestaParameters},
        SwCurve, WeierstrassParameters,
    },
    AffinePoint,
};
fn words<C: WeierstrassParameters>(p: &AffinePoint<SwCurve<C>>) -> [u64; 8] {
    let mut out = [0; 8];
    for (at, n) in [(0, &p.x), (4, &p.y)] {
        let limbs = n.to_u64_digits();
        out[at..at + limbs.len()].copy_from_slice(&limbs);
    }
    out
}
fn fixture() -> (Arc<Program>, Vec<(SyscallCode, u64)>, Vec<u8>) {
    use sp1_core_executor::{Instruction, Opcode};
    const V: u64 = 0x10000;
    const P: u64 = 0x10100;
    const U: u64 = 0x10200;
    let mut code = Vec::new();
    let mut sites = Vec::new();
    for _ in 0..2 {
        for (op, base) in [(SyscallCode::VESTA_ADD, V), (SyscallCode::PALLAS_ADD, P)] {
            load_immediate(&mut code, 10, base);
            load_immediate(&mut code, 11, base + 64);
            load_immediate(&mut code, 5, op as u32 as u64);
            sites.push((op, 32 + 4 * code.len() as u64));
            code.push(Instruction::new(Opcode::ECALL, 5, 10, 11, false, false));
        }
    }
    load_immediate(&mut code, 10, U);
    load_immediate(&mut code, 11, U + 32);
    load_immediate(&mut code, 5, SyscallCode::UINT256_MUL as u32 as u64);
    code.push(Instruction::new(Opcode::ECALL, 5, 10, 11, false, false));
    for (base, len) in [(V, 64), (P, 64), (U, 32)] {
        load_immediate(
            &mut code,
            10,
            sp1_primitives::consts::fd::FD_PUBLIC_VALUES as u64,
        );
        load_immediate(&mut code, 11, base);
        load_immediate(&mut code, 12, len);
        load_immediate(&mut code, 5, SyscallCode::WRITE as u32 as u64);
        code.push(Instruction::new(Opcode::ECALL, 5, 10, 11, false, false));
    }
    load_immediate(&mut code, 10, 0);
    load_immediate(&mut code, 5, SyscallCode::HALT as u32 as u64);
    code.push(Instruction::new(Opcode::ECALL, 5, 10, 11, false, false));
    let mut program = Program::new(code, 32, 32);
    let image = Arc::make_mut(&mut program.memory_image);
    for address in (V..U + 96).step_by(8) {
        image.insert(address, 0);
    }
    let (x, y) = VestaParameters::generator();
    let vg = AffinePoint::<SwCurve<VestaParameters>>::new(x, y);
    let vq = vg.sw_double();
    let vr = vg.sw_add(&vq).sw_add(&vq);
    let (x, y) = PallasParameters::generator();
    let pg = AffinePoint::<SwCurve<PallasParameters>>::new(x, y);
    let pq = pg.sw_double();
    let pr = pg.sw_add(&pq).sw_add(&pq);
    for (base, point) in [
        (V, words(&vg)),
        (V + 64, words(&vq)),
        (P, words(&pg)),
        (P + 64, words(&pq)),
    ] {
        for (i, word) in point.iter().enumerate() {
            image.insert(base + 8 * i as u64, *word);
        }
    }
    image.insert(U, 2);
    image.insert(U + 32, 3);
    image.insert(U + 64, 17);
    let expected = words(&vr)
        .into_iter()
        .chain(words(&pr))
        .chain([6, 0, 0, 0])
        .flat_map(|word| word.to_le_bytes())
        .collect();
    (Arc::new(program), sites, expected)
}
pub(super) fn selftest(output: &Path) -> Result<()> {
    let machine = verifier();
    for cluster in &machine.machine().shape().chip_clusters {
        ensure!(
            32 * cluster.iter().map(|chip| chip.cost()).sum::<u64>()
                <= sp1_core_executor::MAXIMUM_PADDING_AREA,
            "Pasta shape padding budget"
        );
    }
    let (program, sites, expected) = fixture();
    let mut cases = Vec::new();
    let mut control = None;
    for mode in [
        "default",
        "uint",
        "vesta",
        "pallas",
        "both",
        "first-sites",
        "second-sites",
        "unmatched-site",
    ] {
        eprintln!("Pasta retention fixture: {mode}");
        let mut opts = record_options();
        if mode != "default" {
            opts.retained_events_presets
                .insert(RetainedEventsPreset::Uint256MulMod);
        }
        match mode {
            "vesta" => {
                opts.retained_events_presets
                    .insert(RetainedEventsPreset::VestaAdd);
            }
            "pallas" => {
                opts.retained_events_presets
                    .insert(RetainedEventsPreset::PallasAdd);
            }
            "both" => {
                opts.retained_events_presets.extend([
                    RetainedEventsPreset::VestaAdd,
                    RetainedEventsPreset::PallasAdd,
                ]);
            }
            "first-sites" | "second-sites" => {
                let at = if mode == "first-sites" { 0 } else { 2 };
                opts.retained_events_presets.extend([
                    RetainedEventsPreset::VestaAddAtPc { pc: sites[at].1 },
                    RetainedEventsPreset::PallasAddAtPc {
                        pc: sites[at + 1].1,
                    },
                ]);
            }
            "unmatched-site" => {
                opts.retained_events_presets
                    .insert(RetainedEventsPreset::VestaAddAtPc { pc: sites[1].1 });
            }
            _ => {}
        }
        let mut records = Vec::new();
        let summary = generate_records_streaming_with_summary::<SP1Field, _>(
            program.clone(),
            SP1Stdin::new(),
            opts.clone(),
            [0; 4],
            |r| {
                records.push(r);
                true
            },
        )
        .map_err(|e| anyhow::anyhow!("Pasta retention fixture: {e}"))?;
        ensure!(
            !summary.stopped_early
                && summary.exit_code == Some(0)
                && summary.public_values.as_deref() == Some(expected.as_slice()),
            "Pasta fixture outcome"
        );
        retention_audit::memory_history(&records, &program)?;
        let mut stats = Accounting::default();
        let mut placement = Vec::new();
        let mut seen = BTreeMap::new();
        for r in &records {
            stats.observe(r, &machine)?;
            for (sys, _) in r.precompile_events.all_events() {
                // Independent expected placement: do not call the routing predicate
                // under test to decide what a successful fixture should look like.
                let retained = if sys.syscall_code == SyscallCode::UINT256_MUL {
                    mode != "default"
                } else {
                    match mode {
                        "vesta" => sys.syscall_code == SyscallCode::VESTA_ADD,
                        "pallas" => sys.syscall_code == SyscallCode::PALLAS_ADD,
                        "both" => true,
                        "first-sites" => sys.pc == sites[0].1 || sys.pc == sites[1].1,
                        "second-sites" => sys.pc == sites[2].1 || sys.pc == sites[3].1,
                        _ => false,
                    }
                };
                ensure!(
                    (r.cpu_event_count > 0) == retained && sys.should_send == !retained,
                    "Pasta fixture code/PC/interaction placement"
                );
                *seen
                    .entry(format!("{:?}", sys.syscall_code))
                    .or_insert(0u64) += 1;
                placement.push(json!({"code":format!("{:?}",sys.syscall_code),"pc":sys.pc,"retained":retained,"should_send":sys.should_send}));
            }
        }
        ensure!(
            seen == BTreeMap::from([
                ("PALLAS_ADD".to_string(), 2),
                ("VESTA_ADD".to_string(), 2),
                ("UINT256_MUL".to_string(), 1)
            ]),
            "Pasta fixture loss or duplication"
        );
        let identity = (summary.cycles, sha(&expected));
        if let Some(want) = &control {
            ensure!(
                want == &identity,
                "Pasta fixture routing changes cycles/output"
            );
        } else {
            control = Some(identity);
        }
        cases.push(json!({"mode":mode,"cycles":summary.cycles,"records":records.len(),"output_sha256":sha(&expected),"placement":placement,"accounting":stats.report()}));
    }
    write_json(
        output,
        &json!({"status":"passed","cases":cases,"no_proof_generated":true,"caveat":"CPU routing, source/shape and honest arithmetic fixture; not cryptographic proof validation."}),
    )
}
