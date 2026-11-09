# SP1 execution, AIRs and host proving work

Status: SP1 6.10.0 plus the pinned Pasta fork. Guest cycles, CPU record work,
host witness cost and actual proof wall time are separate measurements.
Historical proof success applies to the exact archived ELF/source/key, not to
every later guest with the same syscall counts.

## Work records

| Work | Historical rounds | Scope and current disposition |
| --- | --- | --- |
| Pasta add/double syscalls and constrained chips | [r5–r11](round-index.md#r5) | r5 initially executed only. Subsequent rounds added AIR/event/memory/complete-verifier checks and historical CPU/GPU core proofs. Stock SP1 has no matching Pasta chips. |
| Reuse witness denominator inverses | [r12](round-index.md#r12) | Retained host witness optimization; guest-cycle change zero. Compute one inverse for the inverse check and slope witness. |
| Optional GMP extended-Euclid inversion | [r12](round-index.md#r12) | Historical per-inversion microbenchmark: Fp/Fq ~32.2/33.2 μs → 1.2/1.3 μs. Optional `gmp` / `bigint-rug` remains off in the retained core benchmark; this is not a 26× end-to-end proof claim. |
| Bounded streaming and complete outcomes | [r13](round-index.md#r13), [r71](round-index.md#r71) | Retain touched memory/page state across streams, bounded records, exit/output checks and lossless ordering. Host memory/correctness work, not extra guest-cycle savings. |
| UInt256/Pasta CPU retention layouts | [r84/r105/r107](round-index.md#r107) | Optional routing experiments. Lower main cells/global events are historical record metrics; core cycle-only execution does not exercise proving layouts. |
| Raw Pasta input contract | [r106](round-index.md#r106) | Preserve canonicality, point provenance and malformed-input checks; distinguish guest guards from AIR constraints. |
| SP1 6.10 migration | [r121/r122](round-index.md#r122) | Historical receipt guest 142,692,781 → 142,692,323 (−458). r121 was execution-only; r122 ported the recording/AIR path. Current native layout also has a separate registry adapter and portable counters commit. |
| Historical full proofs | [r119/r120/r123](round-index.md#r123) | r119 reload verification; r120 three 6.1 routing arms; r123 fresh 6.10 proof gates and full receipt guest. These precede the current upstream-based boolean/0.7 checkpoint. |
| Matched-host SP1 comparison | [r124](round-index.md#r124) | Mean 308.35 s (6.1) vs 308.90 s (6.10); +0.18% within observed spread. Cross-host r120/r123 timing gap was not a version effect. |
| Dependency-batch experiments | [r125/r126](round-index.md#r125) | External work, absent from current `prove.rs`. r125 corrects r126's CPU-sweep recommendation: width 8 cut dependencies but not total wall time enough, while memory doubled. Not imported. |
| Consumer split | [r127](round-index.md#r127) | Only a manifest and scripts at the capture. No completed result located; no import or speedup claimed. |

## Implementation and correctness

Early proof artifacts need their corrections: the r7 compiled-guest inventory
contained only `VestaDoubleAssign`, and weak per-shard verification did not
establish complete execution. r8/r9 add the complete verifier, full chip/record
checks and independent program-derived keys on reload. Do not promote an early
"proof passed" label without the corresponding corrected verification scope.

- SP1 [Pasta curves](../../vendor/sp1/crates/curves/src/weierstrass/pasta.rs),
  [syscalls](../../vendor/sp1/crates/core/executor/src/syscall_code.rs),
  [AIR registry](../../vendor/sp1/crates/core/machine/src/riscv/mod.rs), and
  [witness field operations](../../vendor/sp1/crates/core/machine/src/operations/field/field_op.rs).
- [Streaming/proving utilities](../../vendor/sp1/crates/core/machine/src/utils/prove.rs),
  [execution records](../../vendor/sp1/crates/core/executor/src/record.rs), and
  [portable execution](../../vendor/sp1/crates/core/runner/src/portable.rs).
- [Pasta validation harness](../../tools/pasta-proof/README.md) and
  [raw guest](../../tools/pasta-guest/src/main.rs).

Current local validation: 249 native tests, no-std, host/core/receipt guest builds,
valid execution and two rejected corruptions; see
[the 0.7 upgrade](../proof-systems-upgrade.md). No fresh proof or CUDA run was
performed for that exact new guest. Historical GPU proof records do not close
that current validation gap.

## Upgrade boundary

Keep SDK/build/zkVM/lib/JIT/primitives/core/SLOP versions aligned. Review syscall
IDs, eight trusted/user AIR variants, events/cost/retention registration, memory
continuity, portable-runner behavior and the external crypto-bigint patch.
Record new honest/adversarial proofs and from-disk key derivation separately
when explicitly run. Prototype compressed-point/Poseidon AIRs and chain ideas
remain outside the registered production accelerator set.

## Retained controls and commit ownership

No additional setting from the 40-flag retained recipe is assigned to this area.
Optional harness/routing/GMP settings do not become defaults through this record.

Pinned topic commits (in their respective repositories):

- sp1 `0fde0939b` — Add Pasta curves and modular inversion support.
- sp1 `7d7b32b15` — Register Pasta execution, AIRs and lossless record streaming.
- sp1 `59fa996a5` — Keep the Pasta crates compatible with registry SP1 consumers.
- sp1 `7d0c4646a` — Expose portable execution counters for cycle measurements.

Root-specific code stays in the o1js-to-zkvm module series. Current pins and
ordered commits are in [the dependency lock](../../dependencies/lock.json).
Historical evidence and current implementation are connected by this record,
not by preserving the original experimental commit history.
