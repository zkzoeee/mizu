# Measurement, audits and development tooling

Status: retained CPU measurement and correctness tooling, with historical
accounting corrections explicitly recorded. A profile is diagnostic evidence;
it is not automatically the timing of an uninstrumented ELF.

## Work records

| Work | Historical rounds | Evidence rule / retained result |
| --- | --- | --- |
| Decompose shared MSM and field cost | [r13/r14/r17/r18](round-index.md#r18) | Separate execution from witness generation and confirm operation counts before choosing a target. |
| Profile GLV, carry geometry and actual selected windows | [r33–r41](round-index.md#r35), [r59/r60](round-index.md#r60) | Apply barrier/calibration corrections; do not infer live width choices from unreachable code. |
| Stage boundaries and frozen instruction attribution | [r42/r45/r48/r64/r80/r88/r94/r96/r104](round-index.md#r94) | Frozen-ELF instruction totals and separately instrumented diagnostic guests are different evidence classes. |
| Correct record accounting | [r53/r54/r71/r72](round-index.md#r54) | Generate dependencies once; count only included chips; distinguish stored main cells, official shape area and physical prover allocation. |
| Native/build loop and checked blob cache | [r61/r66/r67](round-index.md#r67) | Faster development and cache correctness, with no claimed guest-cycle gain. |
| Small-operation benchmark foundation | [r68](round-index.md#r68) | Compare independent arithmetic answers, cold startup and larger workloads before full-guest timing. |
| Completion/correctness audits | [r103/r113/r114/r118/r119](round-index.md#r118) | Preserve both retained and failed variants, source/config/ELF/runner identities, exact bytes and verification outcomes. |
| Matched versions and pipeline throughput | [r121–r126](round-index.md#r124) | Same host/options and repeated runs are required for version/wall-time attribution. CPU accounting and real producer-consumer pipelines can have different bottlenecks. |

The corrected r72 comparison used its own matched producer/options: r31
427,791,614 → current-at-r72 252,012,129 cycles; stored main cells
98,162,838,112 → 57,029,321,632. It is historical CPU record accounting, not
new proving-time evidence and not the current 0.7 guest's cell count.

## Implementation and reproducibility

- [Retained recipe](../../benchmarks/retained.json),
  [build/execution script](../../scripts/benchmark.py) and
  [cycle runner](../../crates/cycle-runner/src/main.rs).
- [Trace accounting](../../tools/pasta-proof/src/trace_accounting.rs),
  [instruction profile](../../tools/pasta-proof/src/instruction_profile.rs),
  [run manifest](../../tools/pasta-proof/src/run_manifest.rs), and
  [retention audit](../../tools/pasta-proof/src/retention_audit.rs).
- [Guest blob cache](../../crates/o1-verifier/blob_cache.rs) and its
  [tests](../../crates/pickles-verifier/tests/verifier_blob_cache.rs).

## Current comparable checkpoints

| Checkpoint | SP1 / proof-systems | ABI | Cycles |
| --- | --- | --- | ---: |
| Historical r111 | 6.1 / original historical source | 577-byte receipt | 142,692,781 |
| Historical r121/r122 runtime rebuild | 6.10 / original historical source | 577-byte receipt | 142,692,323 |
| Consolidated Mizu | 6.10 / optimized 0.4.1 | one-byte boolean | 142,660,047 |
| Current Mizu | 6.10 / optimized 0.7.0 | one-byte boolean | 142,650,295 |

The final two checkpoints are a matched comparison: −9,752 cycles (−0.006836%),
same fixture, serialized input, 40 settings, toolchains, output and arithmetic
calls. Receipt-to-boolean changes are not a pure optimization ablation. See
[the current measurement](../../benchmarks/core-proof-systems-0.7-sp1-6.10.json).

For every future record, retain the control and candidate's actual source/ELF,
compiler/builder, flags, fixture/input/output hashes, metric, acceptance/rejection
results and memory/options. Report failed/incomplete attempts as such. Do not
sum deltas measured against different controls or substitute shape estimates for
wall-clock proof time. Use [the record template](TEMPLATE.md).

## Retained controls and commit ownership

No additional setting from the 40-flag retained recipe is assigned to this area.
Optional harness/routing/GMP settings do not become defaults through this record.

Root-specific code stays in the o1js-to-zkvm module series. Current pins and
ordered commits are in [the dependency lock](../../dependencies/lock.json).
Historical evidence and current implementation are connected by this record,
not by preserving the original experimental commit history.
