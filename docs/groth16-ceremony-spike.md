# Groth16 without a ceremony: a spike

> **Current answer (2026-11-13): no ceremony is needed.** With the default
> (lean) cluster set, `vk_verification` on and the compress shape now pinned
> in-tree, this fork's computed wrap verifying key is byte-identical to
> upstream's baked one (`fce924068aa5a610`), so Succinct's released Groth16
> artifacts apply. Measured, with a negative control — and then exercised: a
> real Groth16 proof was made and verified under those artifacts on 2026-11-12.
> The result is in "The answer" below; "How we got here" records the two wrong
> turns on the way, both the same mistake.

## The question

Does this fork's wrap verifying key equal upstream SP1's? If it does,
`try_install_circuit_artifacts("groth16")` downloads Succinct's released keys and
we need **no ceremony of our own** — and, as a side effect, none of the local
`groth16.Setup` that OOM-killed every machine we could rent on 2026-11-12.

If it does not, a Groth16 ceremony is circuit-specific and has to be re-run on
every change to the recursion layer. That is the cost this spike is trying to
avoid.

## Why the precompiles reach the wrap circuit at all

They very nearly do not. The wrap program is built over `CompressAir` — the
*recursion* machine — not `RiscvAir`, so a new precompile adds no constraints to
it. And the VK-map root travels as a **witnessed public value**, not a circuit
constant:

```
recursion/circuit/src/machine/witness.rs:93    let vk_root = self.vk_root.read(builder);
recursion/circuit/src/machine/compress.rs:199  for (expected, actual) in vk_root.iter().zip_eq(...)
```

So the *identity* of the recursion VKs is irrelevant to the circuit. Exactly one
channel survives, and it is a count, not a key:

```rust
// prover/src/worker/node/core.rs:92
pub fn allowed_vk_height(&self) -> usize {
    let num_shapes = self.inner.verifier.recursion_vks.num_keys();
    num_shapes.next_power_of_two().ilog2() as usize
}
```

`allowed_vk_height` is a Merkle depth, and it *is* baked into the circuit
(`dummy_compose_input(.., allowed_vk_height)`, `max_arity(vk_verification,
allowed_vk_height)`). And the map is padded to a count derived from our own
machine — including on the `vk_verification == false` path, since `dummy()`
routes through the same `from_map`:

```rust
// prover/src/recursion.rs:59-80
let machine = RiscvAir::machine();
let num_shapes = create_all_input_shapes(machine.shape(), max_compose_arity)..len();
let added_len = num_shapes.saturating_sub(map.len());
map.extend(..);   // pad with dummy keys up to num_shapes
```

Adding chip clusters multiplies straight through `create_all_input_shapes`, so
enough new clusters push the padded count across a power-of-two boundary, change
the depth, change the circuit, and invalidate the ceremony.

## Measured, 2026-11-12

`crates/cycle-runner/src/bin/recursion-shapes.rs` against this fork's machine:

```
chips in the machine   140
chip clusters           54
shapes per cluster    6001
Normalize            324054   + Compose 4 + Deferred/Shrink 2
TOTAL VK map         324060   -> next_pow2 2^19 -> depth 19
```

Upstream's depth, two independent ways:

| route | figure | depth |
| --- | --- | --- |
| baked `prover/src/vk_map.bin`, 7,434,488 B at 40-48 B/entry | ~155k-186k keys | **18** |
| our 54 clusters minus our 14 `pasta_retained_clusters` = 40 | 40 x 6001 + 6 = 240,046 | **18** |

The two routes disagree on the key count (186k vs 240k) and agree on the depth,
which is the only thing in the circuit. The first is the more trustworthy of the
two: it is a file we ship rather than an inference from subtraction.

Also observed the same day, which is what prompted all of this: our runtime
template wrap VK hashed to `4806ebe3d6454da2` while the baked `wrap_vk.bin`
hashes to `fce924068aa5a610`, and SP1's S3 fetch for our prefix 404'd — which is
why keys were being built locally.

## The target

Depth 18 holds for a padded count in (2^17, 2^18] = (131,072, 262,144]. At
`clusters * 6001 + 6` that is **any cluster count from 22 to 43**. We are at 54,
so **at least 11 clusters have to go**.

`pasta_retained_clusters` (`core/machine/src/riscv/mod.rs`) has exactly 14
entries — the 7 non-empty subsets of `{Uint256Mul, VestaAdd, PallasAdd}` times
with/without the memory-global pair. Trimming it to 3 lands on 43 exactly:
43 clusters; the real dedup'd count is 249,750, depth 18.

Those 14 exist to route the `PASTA_RETAIN` presets, so this is a real trade and
not a free win — see stage 4.

## How we got here, and the two wrong turns

The path ran through four stages: confirm upstream's VK-map depth is 18; trim the
pasta clusters so ours is too; test whether the compress shape reaches the wrap
vk; and price the trim. The stage write-ups are gone — the results below
supersede them, and `git log` has the detail — but two mistakes are worth keeping
because both were the same mistake.

**Wrong turn one.** The `Select` height bump is +32 rows, and a probe showed it
invisible to the shrink program. I generalised that to "the compress shape is
invisible to the wrap vk". It is not: a 64-program VK-map sample then panicked on
`ExtAlu` needing +320,059 rows and `PrefixSumChecks` +94,952, four orders of
magnitude past what I had tested. The 8-program sample that missed them used a
stride of 31,218.

**Wrong turn two.** Measuring those real deltas showed they *do* change the
shrink program, and I concluded a ceremony was required. That stopped one link
short: the wrap program sees the shrink proof only through
`ceil(area / 2^18)` stacking multiples, and the larger shrink tables stay inside
the same multiple.

Both errors were reasoning one link past the last measurement. The fix was to
measure the final artifact — the wrap vk bytes — which is what
`compress-shape-audit` now does, with a negative control so it can fail.

## Traps already paid for

- **`test_wrap_vk` is vacuous.** `verify.rs:93` is the only assignment to
  `SP1Verifier::wrap_vk` and it deserializes the baked `WRAP_VK_BYTES`, so the
  test compares those bytes with themselves and cannot fail.
- **A wrap vk computed with `without_vk_verification()` is not comparable to
  upstream's**, which was generated with it on, since `vk_verification` is a
  direct parameter to `wrap_program_from_input`. With verification off, every
  development run here produced `4806ebe3d6454da2` and nothing complained.
- **With verification on, SP1 does check.** The wrap prover's lazy init compares
  its computed vk against `WRAP_VK_BYTES` and returns a fatal "Wrap vk mismatch"
  (`worker/prover/recursion.rs:659-680`). It ran in the real proof and did not
  fire.
- **The VK map need not be complete for a first proof.** SP1's own
  `test_build_vk_map` builds a partial map from the shapes in one core proof plus
  the last 12, and proves against it via `with_vk_map_path`.
- `vendor/sp1`'s own `Cargo.lock` does not satisfy its workspace, so
  `cargo --locked` fails inside it, and the standing rule forbids running cargo
  there without `--locked`. Work from the root workspace.
- SP1's bundled test ELF is 32-bit and fails setup on this rv64 fork; pass a
  fork-built guest to `compress-shape-audit`.

## Tools

| tool | what it does |
| --- | --- |
| `compress-shape-audit` | per-cluster compress needs, the shrink-shape link, and the real wrap vk with a depth-19 negative control. Exits non-zero unless the in-tree arm matches and the control does not. ~20 min single-threaded; pass a fork-built ELF. |
| `vk-map-guard` | the padded VK-map count and resulting depth, from the real `create_all_input_shapes`. Seconds. Exits non-zero off depth 18. |
| `pipeline-runner` / `pipeline-cuda` `--mode vk-map` | builds the partial map for one workload and writes it for `--vk-map`. |
| `recursion-shapes` | per-cluster breakdown and which clusters hold Pasta chips. |

Run `vk-map-guard` after any change to chips or clusters, and
`compress-shape-audit` after any change to the compress shape, before spending
on a pod.

## The answer, 2026-11-13: no ceremony is needed — measured on wrap vk bytes

`crates/pipeline-runner/src/bin/compress-shape-audit.rs` does three things with
SP1's own builders and no proving.

**1. Per-cluster compress needs**, mirroring the `#[cfg(test)]`
`compute_compress_shape` (`shapes.rs:198`). Self-check: recomputing over the
clusters with no Pasta chip reproduces upstream's pinned `compress_shape.json`
exactly, all eight heights. Then, over all 43 clusters:

| clusters | what | over upstream's shape |
| --- | --- | --- |
| 41, 42 | retained `[Uint256Mul, VestaAdd, PallasAdd]` (+ memory-global), 37/39 chips, 5,239/5,299 columns | `ExtAlu` +334,157, `PrefixSumChecks` +94,952 |
| 37, 38 | Pallas / Vesta Poseidon (control + permute), 2,834 columns | `ExtAlu` +3,221 |
| the other 39, incl. retained `[PallasAdd]` and every Pasta add/double/fp-op cluster | | nothing |

So the whole overflow is two clusters. The fork's real compress shape, after the
arity-4 compose fixed point:

```
upstream  BaseAlu=592384 ExtAlu=795264  MemoryConst=492320 MemoryVar=661984 Poseidon2WideDeg3=158368 PrefixSumChecks=224608 Select=1087808
fork      BaseAlu=592384 ExtAlu=1129440 MemoryConst=492480 MemoryVar=664288 Poseidon2WideDeg3=158880 PrefixSumChecks=319584 Select=1087808
```

That is now pinned in `vendor/sp1/crates/prover/compress_shape.json`, with
`Select` at the 1,087,840 real proving asked for.

**2. The unmeasured link.** For each compress shape: build the shrink program,
take its real preprocessed table heights exactly as `preprocessed_table_heights`
does (`hypercube/src/prover/shard.rs:363`), build the wrap dummy input.

| compress shape | compress multiples (pre/main, x 2^20) | shrink area (pre/main, x 2^18) | main slack left |
| --- | --- | --- | --- |
| upstream pinned | 34 / 52 | 33 / 50 | 101,456 cells |
| fork, in-tree | 37 / 58 | 33 / 50 | 45,072 cells |

Six more compress multiples add 56,384 cells to the shrink main area and it stays
inside the same 2^18 multiple. The wrap dummy input is byte-identical
(`405178201d07b5cb..`).

**3. The wrap vk itself**, computed the way `WrapProver::new` does
(`worker/prover/recursion.rs:1165-1190`): compile `wrap_program_from_input` (made
`pub` for this) and run setup on `CpuWrapProverBuilder`'s prover. No proof, no VK
map — the wrap program depends on the map's *height*, not its contents.

| arm | VK-map depth | computed wrap vk | vs upstream `fce924068aa5a610` |
| --- | --- | --- | --- |
| upstream pinned shape | 18 | `fce924068aa5a610` | MATCH |
| fork shape, recomputed | 18 | `fce924068aa5a610` | MATCH |
| **in-tree `compress_shape.json`** | 18 | `fce924068aa5a610` | **MATCH** |
| negative control | 19 | `fad44bcd347d83dc` | MISMATCH, as it must |

The first row also answers the question the handoff brief flagged as the real
risk — whether the four-input chain is exhaustive. This fork's tree, given
upstream's shape, reproduces upstream's baked vk byte for byte; a fifth
fork-specific input would have shown up as a mismatch there. The control row
shows the tool can fail (unlike `test_wrap_vk`), and that depth 18 — the lean
cluster default — is necessary, not incidental.

**What ceremony-free Groth16 requires of this fork, all now in place:**

1. the default (lean) cluster set, so the VK map pads to 249,750 keys, depth 18
   (upstream's map is exactly 185,862 keys, depth 18);
2. `vk_verification` on — which production needs anyway;
3. the in-tree compress shape, whose shrink tables stay inside upstream's
   stacking multiples.

**Corrections to earlier sections.**

- Stage 3 said a wrap-vk mismatch is silent. Not with `vk_verification` on: the
  wrap prover's lazy init compares its computed vk with `WRAP_VK_BYTES` and
  returns a fatal "Wrap vk mismatch" (`worker/prover/recursion.rs:659-680`). It
  is silent only with verification off, which is how every development run so
  far went through with `4806ebe3d6454da2`.
- The VK map is not a 249,750-program prerequisite for a first proof. SP1's own
  `test_build_vk_map` builds a *partial* map — the shapes that appear in one core
  proof plus the last 12 (compose, deferred, shrink) — and proves against it via
  `with_vk_map_path`; `from_map` pads the rest. Dozens of programs, not a quarter
  of a million. The full map is for generality later, since a shape missing from
  the map cannot be proved until it is added, and adding changes the root.
- The root is ours and that is fine: `VkRoot` is one of the gnark circuit's five
  public inputs, and the Solidity template carries `VK_ROOT()` as a substituted
  constant. We deploy Succinct's Groth16 key material with our own `VK_ROOT`.

**Done, 2026-11-12 22:42Z: a real Groth16 proof, verified, under Succinct's
keys.** Records in `benchmarks/pipeline/groth16-20261112/`. One A40 pod
($0.59/hr, 46.6 GiB, about 24 minutes billed, about $0.24), `tools/pipeline-cuda`
built with `--features native-gnark`:

1. `--mode vk-map` proved core (138 shards, 155.6 s), found 55 distinct shard
   shapes, added the last 12 shapes and built 67 vks in **19.1 s** on the GPU —
   0.285 s per program against 13.3 s on one CPU core. A 2,688-byte map.
2. `--mode groth16 --vk-map ..` with `SP1_CIRCUIT_MODE` unset: vk verification on
   against that map, the wrap-vk check ran and did not fire, SP1 downloaded
   `v6.1.0-groth16.tar.gz` (7.9 GB) and gnark proved.

```
Groth16: 142638298 cycles, verified true, prove 497.4 s (incl. the one-off artifact download)
  STARK pipeline, core -> wrap     ~247 s
  artifact download + extract      ~135 s, once per machine
  gnark load + prove               ~112 s   (15,972,262 constraints; prover 65.3 s)
  raw proof 324 B, encoded 352 B   peak RSS 34.9 GiB, peak VRAM 24,767 MiB
```

The proof's `groth16_vkey_hash` is
`4388a21c687fdd5f218d7e3d13190cac4c5355818d3605fd5fb811df468ee696`, which is
exactly `VERIFIER_HASH()` in the `SP1VerifierGroth16.sol` Succinct ships in that
tarball. So the keys are theirs, from their ceremony, and none of the 107.4 GiB
local setup was needed.

**What is ours is the root.** The proof's `vk_root` public input is
`0x0022bcaf99d2825785f3e498584d8f4aca3e1df07f71c95355c610b069bb8ab0`; Succinct's
contract pins `0x002f850e..`, so *their deployed* verifier would reject this
proof. Deploy `SP1VerifierGroth16.sol` with `VK_ROOT()` set to ours;
`Groth16Verifier.sol`, the pairing constants, is unchanged.

**The partial map's limit.** It holds exactly the 67 programs this proof needs. A
different Pickles proof that shards differently would need shapes outside it, the
map would have to be extended, and that changes the root and so the contract.
The full map removes that: 249,750 programs at the measured GPU rate is about
19.8 hours on one A40, about $12.

**Not yet done:** the contract deployed and gas-profiled on a chain; and a second,
different input proved against the same map, which is what would show how stable
the shard shapes are.

**Margin.** 45,072 cells of shrink main-area slack. Compress growth of roughly
five more stacking multiples would cross it, so a new wide chip in a retained
cluster could. Dropping clusters 41 and 42 would restore almost all of the
101,456, at the cost of retaining only `PallasAdd` in core shards. Re-run
`compress-shape-audit` after any change to chips, clusters or the compress shape:
it exits non-zero unless the in-tree arm matches and the control does not.

## Production downloads by version, and will not warn you

`try_install_circuit_artifacts` (`build.rs:468`) keys the download by
`SP1_CIRCUIT_VERSION` — `v6.1.0` in this tree — **not** by the wrap vk hash.
Only the dev path is hash-keyed (`<prefix>-groth16-dev`).

So in production mode SP1 downloads artifacts for the pinned circuit version
whatever our wrap circuit is. A wrap-vk mismatch is therefore **silent at
download time** and surfaces only as a verification failure later. Matching the
wrap vk is a correctness requirement, not an optimisation, and nothing in the
pipeline checks it for you.

## The cluster trim wastes zero rows

The premise — that coarser clusters waste cells — was **wrong**, and the code
says so (measured with a tool since retired, `cluster-cost`; see `git log`):
`smallest_cluster` picks the cluster (`prover/trace.rs:157`), and a chip in it
with no events becomes `PaddedMle::zeros(width, max_log_row_count)` whose `inner`
is `None`, so `num_real_entries()` is 0 (`slop/multilinear/src/padded.rs:64-125`)
and the jagged prover's row count for it is 0 (`slop/jagged/src/prover.rs:110`).

So there are no wasted rows at all. The only cost is per-chip constant overhead,
and it is small:

```
extra chips per shard, over all 54 full-machine clusters:
  +0 chips   43 clusters
  +1 chips    6 clusters
  +2 chips    5 clusters

worst case: +2 zero-row chips (clusters 40-43, 45)
```

43 of 54 clusters lose nothing. The worst affected shard carries 2 extra named
traces, row-count entries and constraint evaluations, and zero extra rows.

**Verdict: the trim is effectively free**, so ceremony-free Groth16 costs
essentially nothing in proving. The real cost is the lost headroom -- 43 is the
exact boundary.

## Note on PLONK

None of this applies to PLONK, whose SRS is universal — the non-dev path calls
`DownloadAndSaveAztecIgnitionSrs`. PLONK never needs a new ceremony however far
the circuit diverges; it only ever costs compute. That asymmetry is the reason
this spike exists only for Groth16.
