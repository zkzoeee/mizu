//! Malformed serialized inputs against the real frozen Mina guest. CPU records
//! only; these are guest rejections, not newly generated SP1 proofs.
use super::*;
pub(super) fn selftest(
    elf_path: &Path,
    input_dir: &Path,
    reference: &Path,
    mutations: &Path,
    output: &Path,
) -> Result<()> {
    let expected = Expectation::from_json(&serde_json::from_slice(&fs::read(reference)?)?)?;
    let elf = fs::read(elf_path)?;
    let proof = fs::read(input_dir.join("proof-input.bin"))?;
    let receipt = fs::read(input_dir.join("receipt-input.bin"))?;
    expected
        .check_inputs(&sha(&elf), &sha(&proof), &sha(&receipt))
        .map_err(|c| anyhow::anyhow!("input-contract pin: {c}"))?;
    let manifest: Value = serde_json::from_slice(&fs::read(mutations)?)?;
    ensure!(
        manifest["original_proof_sha256"] == sha(&proof),
        "input-contract original input"
    );
    let program =
        Arc::new(Program::from(&elf).map_err(|e| anyhow::anyhow!("input-contract ELF: {e}"))?);
    let mut results = Vec::new();
    for case in manifest["cases"].as_array().context("mutation cases")? {
        let bytes = fs::read(case["path"].as_str().context("mutation path")?)?;
        let at = case["payload_offset"].as_u64().context("mutation offset")? as usize;
        ensure!(
            sha(&bytes) == case["sha256"] && bytes.len() == proof.len() && at + 33 <= bytes.len(),
            "input-contract mutation identity"
        );
        ensure!(
            proof[..at] == bytes[..at]
                && proof[at + 33..] == bytes[at + 33..]
                && hex::encode(&proof[at..at + 33]) == case["original_payload_hex"]
                && hex::encode(&bytes[at..at + 33]) == case["changed_payload_hex"],
            "input-contract mutation scope"
        );
        let mut stdin = SP1Stdin::new();
        stdin.write_slice(&bytes);
        stdin.write_slice(&receipt);
        let mut calls = BTreeMap::<String, u64>::new();
        let mut records = 0;
        let summary = generate_records_streaming_with_summary::<SP1Field, _>(
            program.clone(),
            stdin,
            record_options(),
            [0; 4],
            |r| {
                records += 1;
                for (sys, _) in r.precompile_events.all_events() {
                    *calls.entry(format!("{:?}", sys.syscall_code)).or_default() += 1;
                }
                true
            },
        )
        .map_err(|e| anyhow::anyhow!("malformed guest execution: {e}"))?;
        ensure!(
            !summary.stopped_early && summary.exit_code == Some(1),
            "input-contract requires recorded nonzero guest exit"
        );
        ensure!(
            summary.public_values.as_deref() == Some(&[]),
            "input-contract committed output on rejected input"
        );
        for name in ["VESTA_ADD", "VESTA_DOUBLE", "PALLAS_ADD", "PALLAS_DOUBLE"] {
            ensure!(
                calls.get(name).copied().unwrap_or(0) == 0,
                "input-contract Pasta syscall before input rejection"
            );
        }
        results.push(json!({"name":case["name"],"curve":case["curve"],"kind":case["kind"],"input_sha256":sha(&bytes),"cycles":summary.cycles,"records":records,"guest_exit_code":summary.exit_code,"public_values_len":0,"precompile_calls":calls,"pasta_calls":0,"passed":true}));
    }
    ensure!(results.len() == 6, "input-contract six-case matrix");
    write_json(
        output,
        &json!({"status":"passed","mode":"compiled-mina-malformed-point-inputs","elf_sha256":sha(&elf),"original_proof_sha256":sha(&proof),"cases":results,"no_proof_generated":true,"caveat":"Each failure is the actual compiled guest's recorded exit1 before any Pasta syscall. It is not cryptographic SP1 verifier evidence."}),
    )
}
