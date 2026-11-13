# Proof pipeline plan: core → compressed → Groth16 wrap

Tasks for getting from today's core proofs to an EVM-verifiable proof.
**Wrap decision: Groth16**, with no ceremony — proved end to end on 2026-11-12.
The gas table and the rejected alternatives are at the end.

## Wrap decision: Groth16, and no ceremony

Groth16 costs ~75,000 gas less per verification than PLONK (1.30x) and 256 B of
proof against 864 B — see the gas table at the end. PLONK was originally chosen
anyway, for one reason: its SRS is universal, so no ceremony is ever needed,
whereas a Groth16 ceremony is circuit-specific and was assumed to need redoing
"every time the circuit changes — which, with custom precompiles in the core
machine, it will."

**That assumption was wrong, and the measurement is on wrap vk bytes.** The
precompiles reach the wrap verifying key through exactly one channel — the padded
VK-map count's power-of-two bucket, which sets a Merkle depth that is in the
circuit — and the fork's larger compress shape does not reach it at all, because
the shrink tables stay inside the same stacking multiples. With the default
(lean) cluster set at depth 18, `vk_verification` on and the compress shape
pinned in-tree, `compress-shape-audit` computes `fce924068aa5a610`: upstream's
baked value, with a depth-19 negative control that correctly mismatches.

So Succinct's released Groth16 artifacts apply, and Groth16 is strictly better
than PLONK here — cheaper gas, smaller proof, and no ceremony either.

**Exercised, not just argued** (2026-11-12 22:42Z,
`benchmarks/pipeline/groth16-20261112/`): one A40, vk verification on against a
67-program partial VK map built from the proof's own shard shapes in 19 s,
Succinct's released v6.1.0 artifacts installed, proof verified. 497 s cold
including a one-off 7.9 GB artifact download, about 6 min warm; raw proof 324 B;
the proof's `groth16_vkey_hash` equals the released `VERIFIER_HASH`.

Details and the two wrong turns taken on the way:
docs/groth16-ceremony-spike.md.

**Three conditions, all in place, and one guard.** The lean cluster default
(depth 18), `vk_verification` on, and the in-tree compress shape. The ceiling is
262,144 keys against the default's 249,750, so roughly two more chip clusters
would tip back to depth 19 and reopen the question; `vk-map-guard` fails when
that happens, and `compress-shape-audit` checks the whole chain.

**What remains for settlement.** Our `vk_root` is
`0x0022bcaf99d2825785f3e498584d8f4aca3e1df07f71c95355c610b069bb8ab0` and
Succinct's deployed verifier pins theirs, so deploy `SP1VerifierGroth16.sol` with
our `VK_ROOT()` — `Groth16Verifier.sol`, the pairing constants, is unchanged —
then gas-profile it (task 13). The partial map covers only this proof's 67
programs; a differently-sharding input needs it extended, which changes the root
and so the contract.

## Where this starts

The record is unambiguous: *"No recursion, no wrap, no Groth16, no recursion VK
map."* Today's output is a CUDA core proof — 82 shards, **132,029,932 proof
bytes**, verified by the complete verifier on the pod and again locally. Nothing
downstream of that exists.

## Phase A — make the custom chips recursion-legal

This phase gates everything else.

1. **Enumerate the shard shapes the fork's machine can emit.** The Pasta
   precompiles add AIR ids — the Poseidon record cites **134 to 139** — and each
   has `*Control` and `*User` variants (`benchmarks/mirror/chip-widths.txt` lists
   them). Every shape the chosen routing can produce has to be in the allowed set.
   **Measured 2026-11-12** (`crates/cycle-runner/src/bin/recursion-shapes.rs`,
   output in `benchmarks/recursion-shapes.txt`). The real clustering is built in
   `RiscvAir::machine()` (`riscv/mod.rs:1040`) — **not** `MachineShape::all`,
   which collapses all 140 chips into one cluster and undercounts by 54x; my
   first run made that mistake.

   Counts from `vk-map-guard`, which calls the real `create_all_input_shapes`.
   (Figures given here before 2026-11-13 came from `recursion-shapes`, which
   iterated `0..=max_preprocessed_multiple` where the real loop starts at 1, so
   it overstated by ~3%. Now fixed.)

   | | clusters | recursion programs | vk_map |
   | --- | ---: | ---: | ---: |
   | stock SP1 (implied, see below) | ~31 | ~186,037 | 7.4 MB |
   | fork with `pasta-full-clusters` | 54 | 313,638 | ~12.6 MB |
   | **this fork, current default (lean)** | **43** | **249,750** | **~10.0 MB** |

   `create_all_input_shapes` emits one `Normalize` program per
   `cluster x 6,001` shapes from the multiple space (max preprocessed multiple
   33, max main multiple 193, padding-col axes contribute 1 each), plus 4
   `Compose`, `Deferred` and `Shrink`. So **the fork needs 1.74x stock SP1's
   recursion programs**, and **27 of 54 clusters touch a Pasta or Uint256
   chip** — 162,027 programs exist only because of this fork.

   Cross-check: stock `vk_map.bin` is 7,434,488 bytes, which at 40 B per entry
   (a 32-byte digest plus a `usize`) is ~185,862 entries, implying ~31.0
   clusters. The arithmetic lands on an integer, which is the reason to trust
   the 6,001 multiplier.

2. **Regenerate the compress shapes**: `vendor/sp1/crates/prover/compress_shape.json`
   and the `_mprotect` variant, via `crates/prover/src/shapes.rs`.
3. **Build the recursion VK map.** The blocker the record names in three places:
   *"That map is not built; building it needs SP1's recursion toolchain and
   GPU-class compute."* Artifacts are `vk_map.bin` (7.4 MB at stock), plus
   `allowed_vk_map.bin` and `dummy_vk_map.bin` per `crates/prover/shapes.sh`.
4. **Routing: there is nothing to choose, and that is the finding.** Both
   routings' clusters are already compiled into `RiscvAir::machine()` — the
   per-precompile `precompile_clusters` (7-8 chips each, clusters 20-38) *and*
   the 14 `pasta_retained_clusters` (35-39 chips each, clusters 40-53). The VK
   map built from this machine therefore covers both, and the 249,750 figure
   already includes them. You cannot halve the build by picking a routing at
   run time.

   Dropping retained routing would mean **removing clusters from the machine
   definition**, which changes the machine and every VK with it. It is worth
   84,014 programs, 26% — a real saving, and a decision to take deliberately
   rather than discover. Note also that the `mprotect` feature adds
   `core_clusters_user`, `precompile_clusters_user` and
   `pasta_retained_clusters_user`; it was **off** in the measured build, and
   `compress_shape_mprotect.json` exists because it changes this set. Settle the
   feature before building, for the same reason.
5. **Stand up fork-local artifact hosting.** `shapes.sh` uploads to the
   `sp1-circuits` S3 bucket keyed by git ref; a fork needs its own location and
   its own ref discipline.

## Phase A can be deferred, and a partial VK map replaces most of it

**Superseded in part by the 2026-11-12 run, which used neither switch.** A
*partial* VK map — the shapes in one core proof plus the fixed tail — is built in
19 s on an A40 and proves with `vk_verification` ON
(`node.build_vks(Either::Left(indices), chunk)` plus `with_vk_map_path`, which is
SP1's own `test_build_vk_map` flow). That is the route to take: it keeps the
soundness the two switches below throw away, and it costs seconds.

The switches remain documented because every development proof before that run
used them, so any proof bytes from before 2026-11-12 carry this caveat:

- **`build_vk_map(dummy: true, …)`** returns `dummy_vk_map(machine)` and
  `return`s immediately (`shapes.rs:485`) — no compilation, no setup, no GPU.
- **`VerifierRecursionVks { vk_verification: false, … }`** switches off the
  membership check: `if self.vk_verification() && !self.recursion_vks.verify(…)`
  (`verify.rs:578`). SP1's own comment: *"This `vk_verification` must be only
  turned off for testing purposes."* It also disables SP1's wrap-vk check, which
  is why those runs silently produced `4806ebe3d6454da2` instead of upstream's
  key.

So the ordering changes:

**Milestone 1 — the pipeline, with the map stubbed.** Dummy map plus
`vk_verification: false`, then core → compress → shrink → wrap. Produces a real
PLONK proof that verifies against its own VK, and hands over the three numbers
the settlement design is waiting on: proof size, EVM verification gas, and
per-stage latency. **Costs no VK map build.** What it does *not* give is
soundness — with a dummy map, nothing binds the recursion program that produced
the proof, so a prover could substitute a different one. Development only, and
it must be labelled that way wherever its outputs are quoted.

**Milestone 2 — the real map, sharded.** `build_vk_map` takes
`indices: Option<Vec<usize>>` and an explicit `merkle_tree_height`, so the
249,750-program build splits across workers and runs incrementally rather than
as one long job. That is what `shapes.sh`'s S3 upload is for. It also means the
build is resumable, which matters at this size.

Do Milestone 1 first. It de-risks Phases B, C and D, and it prices the wrap
before anyone commits GPU budget to Phase A task 3.

## What a first attempt actually hit (2026-11-12, $0.32 spent, no proof produced)

Milestone 1 was attempted on a rented CPU pod — cpu3g, 16 vCPU, 64 GB, EU-CZ-1,
$0.64/hr, pod `2km3q7z1rgark2`. **No proof was produced.** Five things blocked
it, in the order they appeared, and all five are prerequisites this plan was
missing.

### 1. CPU core proving needs 96–128 GB, not 64

Two runs, both killed by the cgroup OOM killer (`oom_kill 1`, peak 59.6 GB):

| run | threads | survived |
| --- | --- | --- |
| 1 | uncapped (load average 184 on 16 cores) | 2 min 37 s |
| 2 | `RAYON_NUM_THREADS=4` | 5 min 39 s |

The 30-second memory curve for the capped run:

```
12 GB → 18 GB → 46 GB → 46 → 47 → 48 → 48 → 49 → 55 → killed
```

The thread cap doubled time-to-death but **did not lower the ceiling**: the
46–49 GB plateau held for three minutes at four threads. So the working set is
inherent, not parallelism overhead — a claim made from two 50-second samples
earlier in the session was wrong, and the curve is what refuted it. The
development Mac, at 16 GB, was never a candidate.

### 2. The committed compress shape is too small for this fork's machine

```
recursion/machine/src/chips/select.rs:75:
fixed height is too small: got height 1087808 for number of rows 1087840
```

1,087,808 is exactly the `Select` entry in `compress_shape.json`. The fork needs
1,087,840 — **one 32-row block more**. Bumping that single entry by hand got the
run past the panic, which scopes Phase A task 2: the gap is small, not sprawling.
Do it properly with SP1's own tool rather than by hand:

```bash
cargo test --release -p sp1-prover --features experimental -- test_find_recursion_shape --include-ignored
```

### 3. `ProofMode::Core` is not recursion-free

The panic above fired on `--mode core`, in the recursion machine. SP1's local
node normalizes regardless of mode, so **there is no cheap recursion-free
calibration run**. Any first attempt pays the recursion path's prerequisites.

### 4. Three host prerequisites, none visible from macOS

- **`protoc`** — `sp1-prover-types` generates protobuf types at build time via
  prost-build. `apt-get install protobuf-compiler`. Never surfaced locally
  because the Mac already has it.
- **the `profiling` feature on x86_64 Linux** — `vendor/sp1/crates/core/runner/build.rs:10`
  asserts it, and `sp1-prover` reaches that crate transitively without it. A
  `-p` build does not inherit another member's feature selection, so the
  depending crate must declare it. The assert passes on aarch64-darwin, which is
  why the Mac build said nothing.
- **resolver 1 for anything CUDA** — see below.

**The pipeline path cannot be validated on macOS.** The Mac builds binaries the
target platform rejects, twice over. Plan for a Linux box from the start.

### 5. The CUDA path needs its own workspace, not a patch-list edit

`tools/pasta-proof` is a separate workspace with `resolver = "1"` deliberately,
and its comment says why: resolver 2 does not apply a feature enabled on a
normal dependency to the same crate in the *build* graph, and `sp1-gpu-sys`
build-depends on `sp1-core-executor`, so the runner's assert fires and the CUDA
build dies. Its warning is worth repeating: *"Do not satisfy the assert with
`CARGO_FEATURE_PROFILING` instead: that silences the guard while leaving the
unpatched native executor linked."*

The root workspace is `resolver = "2"`. So:

- **CPU driver** → root workspace is correct (`crates/pipeline-runner`, built and
  working; the root's live lock sidesteps `vendor/sp1`'s stale one).
- **CUDA driver** → needs a **third standalone workspace**, resolver 1, with its
  own `[patch.crates-io]` closure over `../../vendor/sp1/...`, depending on
  `sp1-gpu-prover` — which `tools/pasta-proof` avoided precisely because it pulls
  the recursion and gnark stack a core proof never touches. Building it needs a
  CUDA devel image for `sp1-gpu-sys`'s kernels; the comparable fat build measured
  12 min 31 s.

### What this says about the route

The GPU prover is the only one proven for this arm — eleven runs, 100.8 s and
18,991 MiB of peak VRAM — and its memory profile is nothing like the CPU one.
Against a measured CPU requirement of 96–128 GB, the GPU path is the better
target even though it costs the extra workspace. Estimated A40 cost for build
plus run: **$0.25–0.35**.

## The CUDA driver: `tools/pipeline-cuda`

Written 2026-11-12, **not yet compiled or run** — it cannot be built on macOS,
by the same resolver/assert constraint that makes it a separate workspace.

- A standalone workspace with `resolver = "1"`, excluded from the root, for the
  reason `tools/pasta-proof` is: `sp1-gpu-sys` build-depends on
  `sp1-core-executor`, and resolver 2 would drop the `profiling` feature in the
  build graph.
- 48 `[patch.crates-io]` entries at `../../vendor/...`. Not because the fork
  modified 48 crates — it modifies four — but to force **one** resolution:
  `sp1-prover` patched to a path takes its `workspace = true` deps from the
  fork's workspace while the crates.io `sp1-gpu-*` crates would take
  `sp1-hypercube` from the registry, and two of those in one graph is a type
  error.
- Depends on `sp1-gpu-prover`, which `tools/pasta-proof` deliberately avoids
  because it pulls `sp1-prover` and the recursion and gnark stack. Compress and
  wrap need exactly that.
- Proves inside `sp1_gpu_cudart::spawn`, since a `TaskScope` is valid only for
  its task's lifetime.
- `local_gpu_opts()` **panics below 24 GB of VRAM** and sets
  `shard_size = 1 << 24`, so a GPU run has fewer, larger shards than the 82 the
  CPU default produced.

### Milestone 1 is DONE on CUDA (2026-11-12): a compressed proof, verified

A40, EU-SE-1, $0.59/hr, pod `tirknbwxnncb2w`, **ECC Enabled** — so comparable to
the 107–111 s A40 cluster, not the 96.32 s ECC-off outlier. Reports in
`benchmarks/pipeline/`.

| | core | compressed |
| --- | ---: | ---: |
| cycles | 142,638,298 | 142,638,298 |
| proof bytes | 212,657,158 | **1,272,578** |
| prove | 158.2 s | 229.8 s |
| **verify** | 9.02 s | **0.17 s** |
| verified | true | **true** |
| peak VRAM observed | — | 24,383 MiB |

**The size collapse is 167x and the verify speedup is 52x.** That 0.17 s is the
point of the exercise: one proof to check instead of a shard set.

Two figures that must not be compared with the GPU harness's records:

- **212,657,158 core bytes against the harness's 132,029,932** for the same
  guest. `local_gpu_opts()` sets `shard_size = 1 << 24` and its own sharding
  threshold, so this run shards differently. Same cycles, different proof shape.
- **158.2 s of core prove against the harness's ~100 s.** This path goes through
  the worker/controller with normalize; the harness hand-assembles a core shard
  prover and does not.

**24,383 MiB of peak VRAM** is a third more than the 18,991 MiB the eleven
core-only harness runs peaked at, which is why `local_gpu_opts()` panics below
24 GB. The recursion tree would not fit on a 24 GB card with any headroom —
so an A5000 or 3090 is out for this stage regardless of price.

**This is a development artifact.** `vk_verification` is off, so nothing binds
the recursion program that produced the proof; the caveat is a field in both
report JSONs. And `compress_shape.json`'s `Select` was patched by hand from
1,087,808 to 1,087,840 — the same 32-row gap seen on CPU, which confirms the
requirement is a property of the machine and guest rather than the backend.
Regenerate it properly before any of this counts.

### What it took to get there, beyond the plan

Four PATH-or-package prerequisites, none visible from macOS, and the protobuf
one is **distro-dependent**:

| | needed for | caught on |
| --- | --- | --- |
| `protoc` | `sp1-prover-types`' prost-build | Debian pod |
| `libprotobuf-dev` | protobuf well-known types (`google/protobuf/empty.proto`) | **Ubuntu pod only** |
| explicit `/root/.cargo/bin` | cargo over SSH | both pods |
| explicit `/usr/local/cuda/bin` | `nvcc` over SSH, even on a `-devel` image | CUDA pod |

sshd does not inherit the image's Docker `ENV`, which is why two of those are
PATH problems rather than missing software.

**RESOLVED 2026-11-12.** Both drivers now carry a `native-gnark` feature
(`sp1-prover/native-gnark`), which the Groth16 wrap needs for the same reason
PLONK did: without it the gnark FFI compiles against its Docker backend and a
container with no Docker fails at wrap time. The pod also needs a Go toolchain at
or above the go.mod directive (1.24.0 — 1.23.3 is too old) and libclang for
bindgen, which `build-essential` does not provide.

### How to build and run it on a pod

Needs a CUDA **devel** image for `sp1-gpu-sys`'s kernels — the comparable fat
build measured 12 min 31 s — plus `protobuf-compiler`, and Rust 1.92.0 to match
`rust-toolchain.toml`. An A40 or A6000 at $0.59/hr satisfies the 24 GB floor;
peak VRAM for the core arm was 18,991 MiB.

```bash
# on the pod, from the repo root
apt-get install -y protobuf-compiler
cd tools/pipeline-cuda && cargo build --release
./target/release/pipeline-cuda --elf <pinned ELF> --input-dir <dir with the two .bin inputs> \
  --mode compressed --output /root/compressed.json
```

Run `--mode core` first: it is the cheapest exercise of the whole path, and the
2026-11-12 attempt showed the first failures come from prerequisites rather
than from proving. Expect the compress shape to need regenerating before
anything succeeds (see blocker 2 above).

## Phase B — compressed proving

6. **Add a compress mode to the harness.** `tools/pasta-proof` has
   `manifest-core-gpu` and `manifest-core-verify` only — there is no recursive
   path today.
7. **Budget the recursion tree.** 82 shards compress pairwise; measure wall-clock
   and VRAM per level. Peak VRAM for core proving is 18,991 MiB of 46,068, so
   there is headroom, but recursion nodes are a different shape.
8. **Retest the known GPU hazard.** At `PASTA_PROVE_INFLIGHT=2`, four worker
   threads panic in `sp1-gpu-logup-gkr`'s `generate_first_layer` at
   `main_poly_height(name).unwrap()` — a chip sits in the shard's chip set with no
   main polynomial on the device. The default is 1, which is serial. If that class
   of bug recurs in the recursion prover it blocks this phase.
9. **Verify a compressed proof** with the complete verifier, keeping the
   non-vacuity gate (an empty proof must be rejected) and the committed-digest
   check that core proving already enforces.
10. **Record the size collapse.** 132 MB core against a few hundred KB compressed
    is the whole reason for the phase.

## Phase C — shrink, wrap, EVM verifier

11. **Build the gnark FFI** (`miza-sp1-runtime/Dockerfile.gnark-ffi`).
12. **Match upstream's wrap vk, then install Succinct's Groth16 artifacts.**
    This task used to read "generate the PLONK wrap circuit and its keys for this
    fork", on the premise that extra core chips change the compress circuit → its
    VK → what shrink and wrap verify → the wrap circuit. **That premise was
    wrong** (docs/groth16-ceremony-spike.md): the chips reach the wrap vk only
    through the padded VK-map count's power-of-two bucket, and the compress-shape
    bump is invisible to the shrink program.

    So the work is not generating keys but *not needing to*: build with the
    default cluster set (43 clusters, depth 18), leave
    `SP1_CIRCUIT_MODE` unset so the production path runs, and confirm
    `try_install_circuit_artifacts("groth16")` serves keys that verify. No
    ceremony, no 1-of-N assumption, and none of the 107.4 GiB the local setup
    needed.

    Still unproven: no end-to-end byte comparison of our computed wrap vk against
    upstream's `fce924068aa5a610`. That needs `vk_verification = true`, hence the
    real VK map — task 3. Until then the chain is complete by derivation plus two
    measurements, not observed. And because the download is keyed by
    `SP1_CIRCUIT_VERSION` rather than the vk hash, a mismatch would be silent
    until a verification failure, so this must be confirmed before it is trusted.
13. **Deploy and gas-profile the generated Solidity verifier.** Needed for real,
    not estimated: the settlement economics key off it.
14. **Re-pin `SP1_CIRCUIT_VERSION`** and record artifact hashes beside the
    existing manifest pins.

## Phase D — end-to-end validation

15. **One full pipeline run**: core → compress → shrink → wrap, with the
    committed digest preserved and checked at every stage.
16. **Verify the wrapped proof on an EVM testnet**, and keep the receipt.
17. **Per-stage cost and latency on GPU**, so the settlement cadence rests on
    measurement. Core proving is 100–110 s and $0.0225–0.0759 per run today;
    compression and wrap are unmeasured.
18. **Negative tests at every stage** — a corrupted accumulator and a corrupted
    opening must fail where they fail for core proving now.

## Two sequencing rules that cost money if ignored

**Freeze the chip set before Phase C.** Shapes, the VK map and the wrap keys are
all downstream of the core machine's chips. Adding a Poseidon commitment check,
changing a precompile or flipping routing *after* the wrap keys exist means
redoing Phase A and Phase C. The diff-proof work ([diff-proof-plan.md](diff-proof-plan.md))
and any precompile change belong **before** this pipeline work.

**Phase A is not a cents-scale job.** Core proofs cost $0.02–0.08 each; a VK map
build needs SP1's recursion toolchain and GPU-class compute, and it is the
prerequisite for everything downstream. Budget it, and settle task 4's routing
question first so it is built once.

## The gas comparison (which favours Groth16)

Proof sizes are exact, from `vendor/sp1/crates/verifier/src/constants.rs` and the
two converters at the SP1 6.10 pin. Operation counts are read from
`groth16/verify.rs` and `plonk/kzg.rs`. Only the Solidity overhead above the
precompile floor is estimated.

| | Groth16 | PLONK |
| --- | ---: | ---: |
| raw proof | **256 B** | **864 B** |
| on-chain blob | 356 B | 964 B |
| pairing pairs | 4 | 2 |
| pairing gas | 181,000 | 113,000 |
| ecMul count | 5 | 20–25 |
| ecMul gas | 30,000 | 120,000–150,000 |
| precompile floor | 211,750 | 239,470–269,470 |
| floor + calldata | 217,446 | 254,894–284,894 |
| **with Solidity overhead (est.)** | **~250,000** | **~300,000–350,000** |

PLONK costs about **75,000 gas more per verification, 1.30x** — roughly 10.8M
gas/day at a 10-minute cadence. The original plan accepted that cost to avoid a
Groth16 phase-2 ceremony, its 1-of-N honesty assumption, transcript and beacon,
and a full redo on every circuit change.

**That trade does not exist for this fork.** Its wrap vk matches upstream's, so
Succinct's released Groth16 artifacts install and there is no ceremony to avoid.
The 75,000 gas would be paid for nothing. Hence Groth16, now proved end to end.

Note the structure, which is counter-intuitive: pairings favour PLONK by 68,000
gas, and everything else favours Groth16 by 125,720, almost entirely scalar
multiplications. "Groth16 is cheap because it is just pairings" is backwards —
it does *more* pairing work and wins anyway.

## Rejected: Stylus for the verifier

Spike 07 measured this and the answer is settled. Stylus is **3.3x cheaper per
Poseidon hash** (14,873 against the EVM's 48,723) but carries a **2,233,850 gas
fixed overhead per call** against the EVM's 24,983 — 89x — and Test 6 showed
caching removes **0.95%** of it. The crossover stays at about 51 hashes.

For a PLONK verifier it is worse than that, because of *what* Stylus
accelerates. Scalar multiplications and pairings are precompiles, priced
identically on Arbitrum, so Stylus has nothing to improve there — only the
~55,000 gas of field arithmetic and transcript hashing above the floor:

```
PLONK on the EVM      ~325,000   (precompiles 270,000 + solidity 55,000)
PLONK on Stylus     ~2,520,000   (fixed 2,233,850 + precompiles 270,000 + solidity/3.3)
                                 -> 7.8x worse for one verification
```

Stylus saves only ~38,000 gas per verification, so the fixed cost amortizes
after about **58 verifications in a single call** — which no settlement design
here produces. Stylus wins on workloads that are pure field arithmetic in bulk;
PLONK verification is mostly precompile calls, so there is almost nothing for it
to accelerate.
