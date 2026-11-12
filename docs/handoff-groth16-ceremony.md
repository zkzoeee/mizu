# Handoff: can this fork reach Groth16 without a ceremony?

> **ANSWERED 2026-11-13: yes.** The brief below concluded a ceremony was
> required; that stopped one link short. `compress-shape-audit` computes the wrap
> vk for the fork's real compress shape and gets upstream's `fce924068aa5a610`,
> with a negative control. See docs/groth16-ceremony-spike.md, "FINAL". The brief
> is kept as written because its traps and tool list are still accurate — except
> that a wrap-vk mismatch is NOT silent when `vk_verification` is on, and the VK
> map can be partial for a first proof.

Self-contained brief. Read the question, not the conclusions — the author of this
brief has reversed himself twice on it, both times by generalising instead of
measuring. Verify everything below by running the tools named.

Repo: `/Users/user/000/_Z/o1js-to-zkvm`, branch `codex/proof-pipeline`.
SP1 fork: `vendor/sp1` submodule, branch `codex/mizu`, SP1 6.10 pin.

## What the project is

A verifier for Mina's Pickles proofs, running as an SP1 guest, so a Mina proof can
be settled on an EVM chain. The fork adds Pasta-curve precompiles (Pallas/Vesta
field ops, point add/double, and the kimchi Poseidon permutation) to SP1's core
machine, which is what makes the verifier affordable — the Poseidon precompile
alone is 120.9x on cycles.

Target: a Groth16 proof, verifiable on an EVM chain. 256 B proof, ~250,000 gas
estimated.

## The blocking question

SP1 publishes Groth16 artifacts (proving/verifying keys) produced by Succinct's
own ceremony, installed by `try_install_circuit_artifacts("groth16")`
(`vendor/sp1/crates/prover/src/build.rs:468`). Those apply only if **our wrap
verifying key equals upstream's**. If it does not, we must run our own Groth16
phase-2 ceremony, and redo it on every change to the recursion layer.

**Measured answer today: our wrap vk does NOT match, so a ceremony is required.**
The question for you is whether that is actually unavoidable.

## The dependency chain, established by reading

The wrap program is built at
`vendor/sp1/crates/prover/src/worker/prover/recursion.rs:1165-1183`:

```rust
let verifier = C::shrink_verifier();
let shrink_proof_shape = SP1RecursionProofShape { shape: RecursionShape::new(shrink_shape) };
let wrap_input = shrink_proof_shape.dummy_input(
    1,
    prover_data.recursion_vks.height(),   // VK-map Merkle height
    verifier.shard_verifier().machine().chips()..,
    verifier.max_log_row_count(), *verifier.fri_config(), verifier.log_stacking_height(),
);
let program = wrap_program_from_input(&recursive_verifier(..), config.vk_verification, &wrap_input);
```

So `wrap vk = f(shrink_shape, VK-map height, fixed recursion config, vk_verification)`.

- `shrink_shape` is `preprocessed_table_heights` of the **shrink program**
  (`recursion.rs:1056-1065`), and the shrink program is built from a
  **compress**-shaped input (`recursion.rs:1025-1048`). So the compress shape
  propagates:
  `compress_shape -> shrink program -> shrink_shape -> wrap_input -> wrap vk`.
- VK-map height is `num_keys().next_power_of_two().ilog2()`
  (`prover/src/worker/node/core.rs:92`), and the map is padded to a count derived
  from `RiscvAir::machine().shape()` (`prover/src/recursion.rs:59-80`), including
  on the `vk_verification == false` path.
- The precompiles add **no constraints** to the wrap circuit: it is built over
  `CompressAir`, the recursion machine, and the VK-map *root* is only a witnessed
  public value (`recursion/circuit/src/machine/witness.rs:93`,
  `machine/compress.rs:199`). Key identity is invisible to the circuit; the count's
  power-of-two bucket is not.

## State of each of the four inputs

| input | status |
| --- | --- |
| VK-map height | **solved.** Trimming pasta clusters to 3 gives 43 clusters, 249,750 keys, depth 18 == upstream. Now the compile-time default; `pasta-full-clusters` restores 14 (54 clusters, 313,638 keys, depth 19). Cost measured as near-zero: 43 of 54 clusters lose nothing, worst case +2 chips, and **zero** wasted rows. |
| recursion config | untouched by this fork. |
| `vk_verification` | must be `true`. Needs the real VK map built (249,750 programs); every proof made so far uses `without_vk_verification()`, so this is owed for soundness regardless. |
| **shrink shape** | **THE BLOCKER.** See below. |

## The blocker, measured

`vendor/sp1/crates/prover/compress_shape.json` is upstream's pinned compress
shape. This fork overflows it. Two separate discoveries:

```
recursion/machine/src/chips/select.rs:75     got 1087808 for 1087840 rows   (+32)
recursion/machine/src/chips/alu_ext.rs:99    got 795264  for 1115323 rows   (+320,059)
chips/prefix_sum_checks.rs:95                got 224608  for 319560  rows   (+94,952)
```

The first was found by proving; the latter two by a 64-program VK-map sample
(stride 3,902 — an earlier 8-program sample at stride 31,218 missed every large
shape, which is how the wrong conclusion survived a day).

Upstream's *mprotect* variant (`compress_shape_mprotect.json`) pins `ExtAlu`
1,027,424 and `PrefixSumChecks` 274,144 — both still short, so there is no
upstream shape to borrow.

`crates/pipeline-runner/src/bin/shrink-shape-probe.rs` builds the compress-shaped
dummy input and the shrink program for two height sets and compares them, keygen
only, no proving:

```
./target/native-test/shrink-shape-probe vendor/sp1/crates/prover/compress_shape.json \
    --override ExtAlu=1115323 --override PrefixSumChecks=319560 --override Select=1087840

Select only, +32                    dummy_input c8750c918d01f33e..  total_memory 2016023  -> IDENTICAL
ExtAlu + PrefixSumChecks + Select   dummy_input cc05a71c627cbafb..  total_memory 2021271  -> DIFFERENT
```

Why +32 is invisible and +320,059 is not: compress heights reach the input only
through `preprocessed_multiple` and `main_multiple`, each
`ceil(sum_chips(height * width) / 2^log_stacking_height)` with
`log_stacking_height == 20` (`prover/src/shapes.rs:358-382`). A 32-row delta does
not cross a multiple of 2^20; a 320,059-row delta does.

## Your question

**Can the compress shape be made to fit upstream's pinned heights while keeping
the Pasta precompiles?** If yes, the wrap vk matches and no ceremony is needed.

Untested levers, in rough order of promise:

1. **Core shard size / `log_stacking_height`.** The compress program verifies core
   shard proofs; smaller shards mean less per-shard verification work and a
   smaller compress program. Paid in more shards and slower proving — compute for
   trust, which is the right direction.
2. **Maximum cluster size.** The overflow appears on the largest shapes. Our pasta
   clusters extend the ~32-chip core cluster to 35-39 chips. Does the per-shape
   compress program size track the cluster's chip count, the machine's total chip
   count, or the preprocessed/main multiples? The author did not determine this,
   and it decides whether cluster surgery helps at all.
3. **Regenerate the shape properly** with `test_find_recursion_shape` rather than
   hand-bumping. Hand-bumping has now fixed only the first panic, twice, so the
   true required shape is still unknown — there may be further chips that overflow
   on shapes neither sample reached. **Do this first**: the real target is not
   known yet.
4. **Upstreaming the precompiles** so Succinct publishes matching artifacts. Clean
   but slow.

If the answer is no, the live alternatives are: a cheap phase-2 ceremony (single
contributor plus a public beacon, `snarkjs`-style, repeated on every recursion
change), or PLONK, whose SRS is universal so no ceremony is ever needed, at about
+75,000 gas per verification.

## Tools that already exist

Build with `CARGO_BUILD_JOBS=1 RAYON_NUM_THREADS=1 cargo build --locked
--profile native-test -p pipeline-runner --bin <name>` (shared 16 GB Mac, one
heavy build at a time).

| tool | what it does |
| --- | --- |
| `shrink-shape-probe` | compress shape -> shrink program diff, keygen only. `--override Chip=Height`. |
| `vk-map-guard` | real `create_all_input_shapes` count and the resulting depth; exits non-zero off depth 18. |
| `vk-map-sample` | times a strided sample of `build_vks` and extrapolates. **Use a small stride**; a large one misses the big shapes. |
| `recursion-shapes` (cycle-runner) | per-cluster breakdown and which clusters hold Pasta chips. |
| `pipeline-runner` | CPU pipeline driver, `--mode core|compressed|groth16`. |
| `tools/pipeline-cuda` | GPU driver, separate workspace (resolver 1). **Never compiled on this Mac — CUDA does not build here.** |

## Traps already paid for

- **`test_wrap_vk` is vacuous.** `verify.rs:93` is the only assignment to
  `SP1Verifier::wrap_vk` and it deserializes the baked `WRAP_VK_BYTES`, so the test
  compares those bytes with themselves and cannot fail. The only computed wrap vk
  is `shrink_wrap(..)?.vk` (`build.rs:821`), which needs a real compressed proof.
- **A wrap vk computed with `without_vk_verification()` is not comparable to
  upstream's**, which was generated with it on, since `vk_verification` is a direct
  parameter to `wrap_program_from_input`. `true` cannot prove here (our recursion
  programs are not in the baked map); `false` is incomparable. Do not spend GPU
  time on that comparison.
- **The production artifact download keys by `SP1_CIRCUIT_VERSION`** (`v6.1.0`
  here), **not** by the wrap vk hash. Only the dev path is hash-keyed. A wrap-vk
  mismatch is therefore **silent at download time** and surfaces only as a
  verification failure.
- `vendor/sp1`'s own `Cargo.lock` does not satisfy its workspace, so
  `cargo --locked` fails inside it; and the standing rule forbids running cargo
  there without `--locked`. Work from the root workspace.
- Never `cargo fmt --all` the fork.
- No new external crate dependency in any `Cargo.toml` without asking the user.

## Hardware notes

- GPU capacity was exhausted on 2026-11-12: 11 consecutive create refusals across
  A40 x4/x2, A6000 x2, 5090 x2, H100 x3, RTX PRO 6000. CPU pods were HIGH
  availability at 32 vCPU in six datacenters.
- The catalog's `minRam` is a placement filter, not an allocation: a community 5090
  advertising 92 GB delivered a 42.8 GiB cgroup. Trust `gpu.memory` in the create
  response.
- `gnark`'s local Groth16/PLONK setup needs **107.4 GiB peak** and is unrelated to
  proving cost; it OOM-killed a 46.6 GiB A40 inside `plonk.Setup`. Not needed at
  all if upstream's artifacts apply.
- VK-map build cost: 13.265 s/program over 8 programs, but those 8 all avoided the
  large shapes, so treat it as a floor, not an estimate. Peak RSS 2.4-2.7 GiB, so
  memory-optimized hardware is wasted; compute-optimized at 2 GB/vCPU is ample.
  Cost is core-hours x $/vCPU-hr, so pod count is a free wall-clock knob. RunPod
  `cpu3c` at $0.03/vCPU-hr and DigitalOcean CPU-Optimized at ~$0.030/vCPU-hr are
  within noise of each other.

## Read next

- `docs/groth16-ceremony-spike.md` — the full investigation, including the
  "REVERSED" section that supersedes its own earlier stages.
- `docs/proof-pipeline-plan.md` — the pipeline plan. Its wrap decision has been
  reversed twice; the header says where it stands.
