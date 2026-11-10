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
  it grants nothing until the user sets one. The drivers that run it on a
  rented pod, and the measured costs behind their choices, are in
  [the rented-GPU runbook](../gpu-run-on-runpod.md). It also needs the expected
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

## mizu-gpu-core-proof-a40: the first real GPU proof from this repository

- **Status:** validation-only, retained as evidence. One behavioural fix landed
  with it (`PASTA_PROVE_INFLIGHT` defaults to 1); no retained setting changed.
- **Area and owner:** `tools/pasta-proof/Cargo.toml` (`resolver = "1"`),
  `tools/pasta-proof/src/gpu.rs` (three defects), `scripts/gpu/*` (three driver
  defects), `benchmarks/gpu/next-core-proof-manifest.json` (cap set to $2.00).
- **Implementation identity:** root `d985a1a`/`403b064` plus the four fixes
  below, over the fork pins of `mizu-pasta-poseidon-precompile` (SP1
  `a418319a0`, proof-systems `ddb617200`), unchanged by this work. The proved
  bundle is `2863dc0c…`; the harness binary's own hash is in the receipts.
- **Predecessors/corrections:** executes
  [`mizu-retention-promotion`](#mizu-retention-promotion-pasta-and-uint256-retention-on-the-core-proof-path)'s
  prepared manifest, whose "no GPU proof with the new chips" limit this closes
  for the two routing arms. It also qualifies the wall-clock saving that
  wazabi's r120 measured for the same comparison.
- **Mechanism:** nothing optimized. Both manifest arms were proved as SP1 core
  proofs on a rented NVIDIA A40, streamed records, production PCS, and each
  proof was then reloaded from disk in a fresh process under a key derived from
  the pinned ELF — once on the pod and once on the Mac after download.
- **Selection:** `--mode manifest-core-gpu --arm <name>` with
  `PASTA_FRI_CONFIG=core`, stacking 21, row count 22,
  `PASTA_GPU_MAX_TRACE_SIZE=404750336`, `PASTA_GPU_WORKERS=2`,
  `PASTA_R5_INFLIGHT=2`, `PASTA_PROVE_INFLIGHT=1`. Every `SP1CoreOpts` field
  comes from the manifest arm, including `global_dependencies_opt: true`.
- **Control/candidate:** one pod, back to back, same binary, same statement:
  ELF `0cfb3eaf…`, proof input `649da7f4…`, receipt input `6e340b9c…`,
  expected public values `4bf5122f…` (one byte `01`). Host: RunPod
  `2sdy2pearcbmai`, SECURE, EU-SE-1, A40 46,068 MiB, compute capability 8.6,
  driver 595.91.07, host CUDA 13.2, image
  `runpod/pytorch:1.0.2-cu1281-torch280-ubuntu2404`, **Intel Xeon Gold 6342 @
  2.80 GHz**, 9 vCPU allocated (96 visible to `nproc`), 50 GB RAM, 80 GB
  container disk, no network volume. Built with `CUDA_ARCHS=86`, nvcc 12.8,
  rustc 1.92.0, 8 jobs.

| Arm | Shards | Wall | GPU prove loop | Executor | Proof bytes |
| --- | ---: | ---: | ---: | ---: | ---: |
| `default` | 148 | 223.14 s | 193.87 s | 211.33 s | 227,918,740 |
| `retain-pasta-and-uint` | 82 | **168.63 s** | 141.97 s | 159.80 s | 132,029,932 |
| Δ | −44.6% | **−24.4%** | −26.8% | −24.4% | −42.1% |

- **Result:** both arms passed. Shard counts are exactly the CPU phase's
  predictions, 148 and 82. Guest cycles 142,638,298 on both, as pinned. One
  verification key, `eed89d80…`, derived from the ELF and byte-identical to the
  saved one on both arms; one committed digest, `4bf5122f…`. Peak VRAM for the
  retained arm read 18,991 MiB of 46,068 by spot sampling. **Retention's wall
  saving is host-dependent:** r120 measured −11.2% for this same comparison on
  a faster host, this host gives −24.4% from the same −44.6% cut in shards, so
  neither figure may be quoted without its host. Proof bytes per shard are
  1.61 MB (retained) and 1.54 MB (default), so proof size still tracks shard
  count rather than cycles.
- **Correctness evidence:** every proof reloaded and accepted twice by
  `verify_complete_execution` in a fresh process — on the pod (7.04 s and
  9.73 s) and again on the Mac after download (4.03 s and 6.20 s), with
  `saved_key_matches_derived` and `committed_digest_matches` true in all four
  checks and the non-vacuity gate requiring an empty proof to be rejected.
  Both downloaded proofs hash exactly to what the pod reported before
  transfer. All four Pasta chips appear in both proofs.
- **Validation scope:** Linux/CUDA compilation, real CUDA core proving,
  complete-execution verification on the pod and off it, key derivation from
  the pinned ELF. No recursion, no wrap, no Groth16, no recursion VK map.
- **Decision and limits:** kept as the evidence that this tree can prove on a
  GPU at all. **The pipelined consumer is not sound:** at
  `PASTA_PROVE_INFLIGHT=2` four worker threads panic in `sp1-gpu-logup-gkr`'s
  `generate_first_layer` at `main_poly_height(name).unwrap()`, so a chip sits
  in the shard's chip set with no main polynomial on the device. The default is
  now 1, which is serial; r120's ranked idea 3 remains open and unproved. The
  two successful arms were launched by hand rather than through `pod-run.sh`,
  so they carry **no utilization series** and the GPU idle fraction is
  unmeasured for them; the equality of `executor_seconds` (159.800142) and
  `gpu_task_total_seconds` (159.799915) is the fifth consecutive run with that
  signature and is the reason `gpu_prove_seconds` must never be read as GPU
  compute. Cost ≈ **$0.40** of the authorized $2.00, estimated from pod
  timestamps because `list-pod-billing` again returned `recordCount: 0`. The
  pod is terminated and the account has no pods.
- **Source provenance:** receipts in `benchmarks/gpu/runs/20261110T1311Z`
  (including the crashed `PASTA_PROVE_INFLIGHT=2` attempt), summary with every
  hash in
  [gpu-core-proof-a40-sp1-6.10.json](../../benchmarks/gpu-core-proof-a40-sp1-6.10.json),
  proof bytes kept outside Git at `/Users/user/000/_Z/mizu-gpu-proofs-20261110`
  (343 MB). Procedure and costs: [the runbook](../gpu-run-on-runpod.md).
- **Upstream sync:** none. No fork file changed.

### Second draw, same card, another data centre (2026-11-10, 14:36Z)

One arm, no proof download, on a second A40 — pod `a9okellgv9d0hd`, SECURE,
**CA-MTL-1**, $0.59/hr, 16.3 minutes, ~$0.16. It took **nine refused creates**
across A6000/EU-SE-1, A40/EU-SE-1, A40/CA-MTL-1 and RTX 3090/EU-CZ-1 before one
was accepted; the cheap tier's stock is real but it is a race, and re-reading
availability between attempts only wastes the window.

It was meant to vary the host. It did not: RunPod reports the **same Intel Xeon
Gold 6342 @ 2.80 GHz** behind the A40 pool in both data centres, so the run
became a reproducibility check instead.

| | EU-SE-1 | CA-MTL-1 | Δ |
| --- | ---: | ---: | ---: |
| Wall | 168.63 s | **162.02 s** | −3.9% |
| Executor | 159.80 s | 154.16 s | −3.5% |
| GPU prove loop | 141.97 s | 137.58 s | −3.1% |
| Shards | 82 | 82 | same |
| Proof bytes | 132,029,932 | 132,029,932 | same |

−3.9% on identical silicon and an identical statement sits inside r124's
measured 2.6–4.2% within-side spread. The verification key, the committed
digest, the shard count and the byte count are identical; only the proof hash
differs, because FRI draws randomness. The proof was reloaded from disk on the
pod and accepted, with the key derived from the pinned ELF.

**This run lifts the first one's telemetry limit**, because it went through
`pod-run.sh` rather than being launched by hand. Integrating its 809-sample
series over the 162.7 s span: **GPU busy 68.3 s against 94.4 s idle — 58%
idle**, mean utilization 42.0%, median sample 16%, 46.2% of samples at exactly
zero, peak VRAM **19,256 MiB** of 46,068, peak host RSS 16.1 GB. Less extreme
than r123's 82.9% idle on a 5090 and consistent with r120's 33–41% mean: a
faster card could address at most the 68.3 s, while the 94.4 s of waiting is
the executor's.

It also found one more defect, in the drivers rather than the prover:
`manifest-core-verify` iterated every manifest arm, so a one-arm run failed on
the arm it never proved and exited 1 **after** its own proof had passed. The
verify mode now takes an optional `--arm`, and `pod-run.sh` verifies per proved
arm. Evidence:
[gpu-core-proof-a40-repeat-sp1-6.10.json](../../benchmarks/gpu-core-proof-a40-repeat-sp1-6.10.json),
receipts in `benchmarks/gpu/runs/20261110T1436Z-a40-montreal`.

## mizu-record-channel-depth: the record channel was the bottleneck, not either side

- **Status:** retained. `PASTA_R5_INFLIGHT` now defaults to **8**, in the
  harness and in the pod driver. No fork change, no fixture, ELF, FRI or
  verifier change.
- **Area and owner:** `tools/pasta-proof/src/gpu.rs` (the default),
  `scripts/gpu/pod-run.sh` (the knob), `scripts/gpu/sweep-inflight.sh` (new).
- **Implementation identity:** root `26171d4` plus this change, over the fork
  pins of `mizu-pasta-poseidon-precompile`.
- **Predecessors/corrections:** reads the receipts of
  [mizu-gpu-core-proof-a40](#mizu-gpu-core-proof-a40-the-first-real-gpu-proof-from-this-repository)
  and settles wazabi r128's open item ("that `PASTA_R5_INFLIGHT=8` recovers
  r125's 23 s — that is now the obvious experiment and it is untested").
- **Mechanism:** nothing about the proof changed; only how many records may sit
  between the producer and the shard prover. At depth 2 the two halves
  deadlocked against each other by turns — the producer spent ~72 s blocked
  inside the consumer callback on a full channel while the prover starved ~41 s
  on an empty one.
- **Selection:** `PASTA_R5_INFLIGHT`, default 8, overridable. The sweep script
  takes a list of depths and runs them interleaved.
- **Control/candidate:** six passes, one pod (`re9fy8hs6yx5gj`, A40, EU-SE-1,
  Xeon Gold 6342), one arm, same binary and statement, order **2 / 8 / 2 / 8**
  then 16 and 32 — r124's alternation, so host drift lands in the within-side
  spread rather than in the effect.

| Depth | Wall (mean) | Spread | Starved | Producer in callback | Proving | Device busy | Device idle |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 2 | **160.67 s** | 1.42 s (0.9%) | 41.5 s | 72.8 s | 94.1 s | 68.8 s | 57% |
| 8 | **134.67 s** | 1.02 s (0.8%) | 1.0 s | 32.8 s | 107.6 s | 68.6 s | 49% |
| 16 | 133.69 s | — | 1.1 s | 21.2 s | 108.1 s | 68.5 s | 49% |
| 32 | 137.43 s | — | 0.6 s | 9.2 s | 111.5 s | 68.0 s | 51% |

- **Result:** **−16.2% of wall from 2 to 8**, against a within-side spread of
  0.8–0.9%. 16 is indistinguishable from 8 (−0.7%) and 32 is worse (+2.0%), so
  8 is the knee, and the smaller buffer is preferable at equal wall because the
  records sit in host memory. Starvation collapses from 41.5 s to under 1.2 s
  and stays dead. Producer blocking keeps falling monotonically past 8 (73 → 33
  → 21 → 9 s) but buys nothing more, because the consumer is by then
  continuously busy and *is* the critical path.
- **Correctness evidence:** 82 shards and 132,029,932 proof bytes at every
  depth, all six passes accepted by the complete verifier, and the driver's
  pass additionally reloaded from disk in a fresh process under a key derived
  from the pinned ELF. Peak VRAM 18,991 MiB at every depth: the buffer is host
  memory, not device memory.
- **Validation scope:** real CUDA core proofs on Linux, complete-execution
  verification in process for all six and from disk for one. No recursion, no
  wrap, no off-pod verification this run.
- **Decision and limits:** retained as the default. **It also qualifies this
  area's standing advice about the card.** Device busy time is constant at
  68–69 s — identical work — but it now occupies 51% of the wall where it
  occupied 42%, and the ~40 s gap between `consumer_proving` (108 s) and device
  busy (68.5 s) is host-side prover work rather than kernels. So "do not buy a
  bigger GPU" is weakened, not reversed: there is now real headroom on the
  device side, and the next levers are the prover's own host work and the
  producer's dependency pass (51 s), for which wazabi's `PASTA_DEPS_BATCH` does
  not exist in this tree. 16 and 32 have one pass each, so their small deltas
  carry no spread.
- **Source provenance:**
  [gpu-inflight-sweep-a40-sp1-6.10.json](../../benchmarks/gpu-inflight-sweep-a40-sp1-6.10.json),
  receipts in `benchmarks/gpu/runs/20261110T1655Z-inflight-sweep`. ~$0.41 of
  the authorized $2.00; pod terminated, no pods remain.
- **Upstream sync:** none.

