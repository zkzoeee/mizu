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

## mizu-zakura-host-witness: batched inversion, fixed-limb executor, leaner field ops

- **Status:** retained host-time improvement, pinned as SP1 fork topics
  `e0b57de84` (witness) and `b2c2b02d5` (executor). No guest-cycle change by
  construction.
- **Area and owner:** SP1 fork. `crates/core/machine/src/operations/field/field_op.rs`
  (`mod_inverse`, `batch_mod_inverse`, one `div_rem`),
  `crates/core/machine/src/syscall/precompiles/weierstrass/weierstrass_add.rs`
  (per-chunk batch inversion, discarded trace-pass lookups),
  `crates/curves/src/weierstrass/pasta_arith.rs` (new) and `mod.rs`
  (Pallas/Vesta `EllipticCurve` on the fixed-limb path).
- **Predecessors:** r12 inverse reuse (retained) and optional GMP inversion
  (off). Techniques adapted from Zakura Common's Pasta work: batch inversion
  (Montgomery's trick), fixed four-limb Montgomery arithmetic, and a
  variable-time extended-Euclid inverse. No code or dependency was imported.
- **Mechanism:**
  1. Witness generation populates every addition twice per shard (dependencies,
     then trace), each needing `1 / (q.x - p.x)` by Fermat exponentiation. Rows
     are now inverted per chunk with one exponentiation plus three
     multiplications per row. The `inverse_check` and `slope` columns are filled
     through the existing bit-identical `populate_div_with_inverse`.
  2. The executor's Pasta add and double ran the generic big-integer formulas
     with a square-and-multiply inversion. Canonical, distinct operands now run
     on stack Montgomery arithmetic with a binary extended-Euclid inverse. Every
     other input keeps the generic path, panics included.
  3. Field-op population computes `(carry, result)` with one `div_rem` instead
     of `%`, `-` and `/`, and the add chip's trace pass discards the byte
     lookups that the dependency pass already records.
- **Selection:** always on in the fork; no flag. Doubling witnesses keep the
  per-row path: 481 per proof cannot move the total.
- **Control/candidate:** `pasta-precompile-proof-6-10 --mode r5-cycles
  --time-witness` with `PASTA_WITNESS_DIGEST=1`, retained mainnet ELF, default
  routing, production shard thresholds, 2 GiB memory limit,
  `RAYON_NUM_THREADS=1`, Apple M2 Pro. Harness code identical across arms;
  control built from the stashed fork. One observation per arm.
- **Result (seconds, single thread):**

| Phase | Control | Candidate | Change |
| --- | ---: | ---: | ---: |
| Record generation (execute, trace, dependencies) | 347.20 | 176.04 | −49.3% |
| Pasta dependency generation (separately timed) | 113.70 | 55.74 | −51.0% |
| Pasta trace generation (separately timed) | 97.98 | 34.65 | −64.6% |
| Total streaming run | 558.88 | 266.43 | −52.3% |

  Of the record-generation saving, about 51 s is the batched inversion inside
  the in-record dependency pass and about 112 s the executor path, derived from
  the separately timed dependency delta. Executor-only runs agree: the
  `cycle-runner` execution of the same mainnet guest took 125.0 and 126.0 s
  with the previous fork and 10.6 s with this one (one run, same cycles and
  output). Pasta curve arithmetic was most of the serial executor's time.
- **Correctness evidence:** cycles (142,638,298), records (148), every precompile
  and UInt256 count, and SHA-256 digests of every Pasta chip trace and its
  sorted byte lookups across the whole workload are identical between control
  and candidate. Harness tests compare 600 batched add rows and lookups with
  row-by-row population, batch inverses with single inversions (zeros
  included), and the executor path with the generic path and the integer
  reference. `pasta-fuzz` passed three seeds (see the measurement record).
- **Validation scope:** CPU execution, native and harness tests only. No fresh
  proof, no CUDA run, and no wall-clock proof measurement. The GPU pipeline is
  executor- and CPU-tracegen-paced (r120/r124/r125), so these phases are the
  ones that pace it, but the proof-time effect is unmeasured.
- **Decision and limits:** keep. Run the applicable fresh proof gate before
  claiming any proving-time change. Evidence:
  [summary](../../benchmarks/zakura-ragu-experiments-sp1-6.10.json).

## mizu-proving-pipeline: pipelined producer and concurrent shard proofs

- **Status:** producer retained (SP1 fork `ac99509d0`); concurrent consumer
  retained in the harness, validated on CPU proofs only. The CUDA path uses it
  but has not been compiled or run since the change.
- **Mechanism:** the minimal executor runs on its own thread; each trace chunk
  is spliced, its pieces traced concurrently, the order-dependent tail run in
  execution order, and the chunk's records given dependencies concurrently.
  The consumer keeps up to `PASTA_PROVE_INFLIGHT` shard proofs in flight. Stall
  counters: executor blocked, waiting for a chunk, time in the consumer
  callback (producer blocked), and the consumer's starved and proving time.
- **Selection:** `PASTA_REPLAY_THREADS` (1 = original loop; default available
  parallelism capped at 8), `PASTA_PROVE_INFLIGHT` (default 2 on CUDA),
  `PASTA_R5_INFLIGHT` (record channel, now 4).
- **Result (mainnet producer, M2 Pro, interleaved, two runs each):**

| Replay threads | Producer seconds | Dependencies | Trace | CPU seconds |
| ---: | ---: | ---: | ---: | ---: |
| 1 | 84.1, 84.1 | 50.6 | 14.8 | 166 |
| 4 | 56.6, 55.1 | 35.1 | 8.4 | 181 |
| 8 | 46.6, 46.4 | 28.0 | 6.3 | 184 |

  On the small CPU proof (13 shards), three proofs in flight finish in 10.9 s
  against 16.9 s one at a time.
- **Correctness evidence:** per-record canonical digests identical for all 148
  mainnet records between sequential and pipelined producers (and between two
  sequential runs); both CPU proofs accepted by the complete-execution
  verifier with identical record streams.
- **Limits:** no GPU run, so the wall-clock effect on a pod is unmeasured.
  Earlier runs of the same binary at 242 s were contaminated by other machine
  load; only interleaved runs are reported. Evidence:
  [summary](../../benchmarks/pipeline-sp1-6.10.json).

## mizu-pasta-fp-precompile: fixed-modulus Pallas and Vesta field ops

- **Status:** opt-in measured candidate. SP1 fork `70a005d67`, proof-systems
  `dbe478fc4`, algebra `6f81d7e13`; flag `MIZU_PASTA_FP_PRECOMPILE=1`, recipe
  `benchmarks/dev/pasta-fp.json`. Not in the retained recipe.
- **Mechanism:** `PALLAS_FP_*` / `VESTA_FP_*` syscalls served by SP1's existing
  `FpOpChip` at the Pasta base fields (no new AIR). Each multiply touches 8
  operand words instead of 12, and the modulus is an AIR constant instead of
  a witnessed memory operand. Guest call sites pick the field at compile time
  where the type fixes it; the generic Montgomery path matches the modulus.
- **Result (retained mainnet guest, default routing):**

| Measurement | Control | Candidate | Change |
| --- | ---: | ---: | ---: |
| Guest cycles | 142,638,298 | 139,610,314 | −2.12% |
| `UINT256_MUL` calls | 533,878 | 0 | moved to 400,111 Pallas + 133,767 Vesta |
| Main cells | 43,922,703,280 | 42,414,408,432 | −3.43% |
| Global interaction events | 136,299,445 | 130,836,329 | −4.01% |
| Records | 148 | 145 | −3 |

- **Correctness evidence:** registered costs equal the AIR widths (306/354);
  `pasta-fp-check` runs 144 add/sub/mul operations on random and edge
  operands in both fields: executor results match the integer reference,
  honest rows satisfy the AIR, a tampered result limb is rejected, no column
  survives perturbation, and a 4-shard CPU core proof passes the complete
  verifier against the committed digest. The full guest accepts the mainnet
  proof and rejects a corrupted accumulator; trace accounting accepts the
  complete execution.
- **Limits:** no GPU proof with the new chips and no recursion VK map for the
  new precompile clusters, so it stays opt-in. Not yet measured together with
  the deferred MDS flag, whose saving overlaps (both remove `UINT256_MUL`
  traffic). Evidence: [summary](../../benchmarks/pasta-fp-sp1-6.10.json).
- **Follow-up:** the whole-permutation Poseidon chips that replace these calls
  inside the sponge are recorded in
  [Poseidon](04-poseidon.md#mizu-pasta-poseidon-precompile-the-kimchi-permutation-as-one-syscall).
  The chip's own fuzz layers FP1 to FP4, including what it does with a
  noncanonical operand, are in
  [measurement and tooling](07-measurement-and-tooling.md#mizu-fuzz-expansion-five-more-fuzz-surfaces-and-the-argument-channel);
  `pasta-fp-check` remains the fixed-vector mode.

## mizu-retention-promotion: Pasta and UInt256 retention on the core-proof path

- **Status:** promoted for core proofs; not promoted as SP1's global default.
- **What changed:** the harness's CPU suites take `PASTA_RETAIN=uint` or
  `pasta-uint`; the next GPU run's manifest,
  `benchmarks/gpu/next-core-proof-manifest.json`, makes `retain-pasta-and-uint`
  its primary arm with default routing as the control. Its spending cap is 0:
  it grants nothing until the user sets one. It also needs the expected
  `public-values.bin` (one byte `01`) in its input directory.
- **Result (current boolean guest):** records 148 → 82, main cells
  43,922,703,280 → 27,839,873,136 (−36.6%), Global interaction events −46.9%,
  identical cycles; all 2,542,549 retained calls placed on CPU shards;
  trace accounting accepts the complete execution.
- **Proof evidence:** the 42-case CPU proof matrix under `pasta-uint` routing:
  4 honest proofs accepted (one shard each, against three under default
  routing) and 38 malicious cases rejected, none unexpected. Honest GPU proofs
  of the same routing exist from r114/r120 on the earlier receipt guest.
- **Why not the global default:** flipping `SP1CoreOpts::default` would also
  change the SDK's recursive proving path (`make prove-cpu`), which needs a
  recursion VK map covering the retained CPU clusters. That map is not built;
  building it needs SP1's recursion toolchain and GPU-class compute.
  Evidence: [summary](../../benchmarks/retention-sp1-6.10.json).
