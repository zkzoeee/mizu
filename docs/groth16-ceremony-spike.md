# Groth16 without a ceremony: a spike

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

## Stages

Each stage is free and needs no GPU. Build time dominates.

### Stage 0 — upstream's depth. DONE

Depth 18, by both routes in the table above.

### Stage 1 — trim, and confirm our depth drops. ~15 min

Gate the 14 clusters down to 3, rebuild `recursion-shapes`, expect
`chip clusters 43` and `TOTAL 258049`.

**Kill:** a count outside [22, 43] means this approach cannot reach depth 18.
Stop.

### Stage 1 RESULT, 2026-11-12: passed

The lean set is now the **compile-time default**; `pasta-full-clusters` restores
the original 14. It is a cargo feature, not an env var, because the choice
changes the machine: prover and verifier must agree, and a runtime switch would
let them disagree silently. (It was an env var during the spike, promoted once
the result stood.)

Counts are from `crates/pipeline-runner/src/bin/vk-map-guard.rs`, which calls the
real `create_all_input_shapes` and dedups through a `BTreeSet` exactly as
`from_map` does (`prover/src/recursion.rs:63-67`):

| arm | clusters | VK map keys | next_pow2 | depth |
| --- | --- | --- | --- | --- |
| `pasta-full-clusters` (14) | 54 | 313,638 | 2^19 | 19 |
| **default, lean (3)** | 43 | **249,750** | 2^18 | **18** |
| upstream, est. from `vk_map.bin` | - | ~155k-186k | 2^18 | **18** |

Headroom on the default is **12,394 keys**, roughly two clusters.

**Correction.** Earlier figures here, and in commits cc81871e2, f16f511 and
912875c, said 324,060 and 258,049, and 912875c blamed the gap on shapes shared
between clusters not being dedup'd. **That explanation was wrong.**
`create_all_input_shapes` returns no duplicates at all — its `len()` and its
dedup'd length are both 249,750. The real cause was an off-by-one in
`recursion-shapes`, which iterated `0..=max_preprocessed_multiple` where the
real loop (`shapes.rs:711`) starts at 1, adding a phantom band worth ~193 shapes
per cluster: 193 x 43 = 8,299 against the observed 8,293 gap. Fixed in the tool.

The depth conclusions are unchanged. `vk-map-guard` is authoritative because it
calls the real function, and it exits non-zero when the depth leaves 18 — which
matters because nothing else checks it (stage 3).

### Stage 2 — REDESIGNED, and the news is worse

Three corrections to the plan above, all found before spending any GPU time.

**1. `test_wrap_vk` is vacuous.** `verify.rs:93` is the *only* assignment to
`SP1Verifier::wrap_vk` and it deserializes `WRAP_VK_BYTES`, so the test compares
the baked bytes with themselves and cannot fail. Exposing `wrap_vk()` would
report nothing. The only computed wrap vk in the tree is `shrink_wrap(..)?.vk`
(`build.rs:821`), which needs a real compressed proof — so it is *not*
keygen-only as the plan claimed.

**2. `vk_verification` makes the obvious experiment self-defeating.** It is a
direct parameter to `wrap_program_from_input`, and upstream's baked vk was
generated with it **true**. But proving needs it **false** here, because our
recursion programs are not in the baked map. So:

- `true`  -> vk comparable to upstream, proving fails before `shrink_wrap`.
- `false` -> proving works, vk incomparable to upstream by construction.

Renting a GPU to run a `--mode wrap-vk` built on `without_vk_verification()`
would have produced a guaranteed mismatch that proved nothing.

**3. The compress shape does reach the wrap vk.** The real construction site
(`worker/prover/recursion.rs:1165-1183`) enumerates every input:

```rust
let wrap_input = shrink_proof_shape.dummy_input(
    1,
    prover_data.recursion_vks.height(),   // VK map tree height
    verifier.shard_verifier().machine().chips()..,
    verifier.max_log_row_count(), *verifier.fri_config(), verifier.log_stacking_height(),
);
let program = wrap_program_from_input(&recursive_verifier(..), config.vk_verification, &wrap_input);
```

so wrap vk = f(shrink shape, VK-map height, fixed recursion config,
vk_verification). And `shrink_shape` is `preprocessed_table_heights` of the
*shrink program* (`recursion.rs:1056-1065`), which is itself built from a
compress-shaped input. The chain is therefore

```
compress_shape -> shrink program -> shrink_shape -> wrap_input -> wrap vk
```

**So the precompiles reach the wrap vk through two channels, not one:**

| channel | fixable? |
| --- | --- |
| VK-map height, 19 vs 18 | yes — trim to <=43 clusters, done in stage 1 |
| compress shape -> shrink shape | **not while keeping the chips** |

The second is the hard one. Our `Select` bump exists because the compress program
has to verify core shard proofs for 140 chips, and trimming clusters does not
reduce the chip count. Upstream's non-mprotect `compress_shape.json` pins
`Select` at 1,087,808; we need 1,087,840. (Upstream's *mprotect* shape happens to
pin exactly 1,087,840, but its other heights differ, so it is not a drop-in.)

**Open question, and the one worth answering next:** does a larger compress
`Select` height actually change the shrink program's *preprocessed* table
heights? Preprocessed tables are the program's own instruction/constant tables.
If the recursion DSL unrolls proportionally to the verified shape, they change
and a ceremony is unavoidable while we keep the chips. If they do not, the
compress bump is invisible to the wrap vk and the cluster trim alone suffices.

That check is keygen of the shrink program for both compress shapes — no
proving, no GPU, locally runnable — but it needs `wrap_program_from_input`,
`recursive_verifier` and the recursion prover's `setup` reachable from outside
`sp1-prover`, i.e. three `pub(crate)` -> `pub` changes in the fork.

Note also that full *empirical* confirmation needs `vk_verification = true`,
which needs the real VK map built (249,750 programs on the lean arm) — Phase A
task 3. So the derivation above is as far as reasoning alone can go.

### Stage 2 (superseded) — the original decisive comparison

`test_wrap_vk` (`prover/src/build.rs:839`) already asserts the computed wrap VK
equals the baked bytes. It cannot be run as-is: `cargo metadata --locked
--offline` in `vendor/sp1` fails with *"cannot update the lock file ... because
--locked was passed"*, the fork's lock does not satisfy its own workspace, and
the standing rule forbids running cargo there without `--locked`. The root
workspace cannot substitute because it does not resolve `sp1-prover`'s
dev-dependencies.

**The unlock:** add a `pub fn wrap_vk()` beside the existing `#[cfg(test)]
pub(crate)` one and call it from a small binary in the **root** workspace.
`WRAP_VK_BYTES` is already `pub`. No dev-dependencies, so the root lock resolves
it unchanged and neither lockfile is touched.

`wrap_vk()` does keygen only — no proving — so `Select` can be set to upstream's
height even though proving would fail with it. That makes a clean 2x2:

| clusters | `Select` | isolates |
| --- | --- | --- |
| 54 | 1,087,840 | baseline; expect mismatch |
| 43 | 1,087,840 | the compress shape |
| 43 | 1,087,808 (upstream) | **the hoped-for match** |
| 54 | 1,087,808 | the clusters |

`compress_shape.json` is `include_bytes!`, so each row forces an `sp1-prover`
rebuild (~5 min).

**Kill:** if row 3 mismatches, something beyond depth and shape diverges and a
ceremony is unavoidable. Report rather than keep hunting.

### Stage 2 RESULT, 2026-11-12: the compress bump is invisible

`crates/pipeline-runner/src/bin/shrink-shape-probe.rs`, keygen-free and
proving-free, needing no fork change at all: `pub use components::*` (lib.rs:12)
re-exports the components, `shrink_program_from_input` and `recursive_verifier`
are already `pub`, and `SP1RecursionProofShape` derives `Deserialize` so both
shapes come straight from the JSON.

```
compress machine: 8 chips, log_stacking_height 20, max_log_row_count 21
vk map height held at 18 for both arms (the probe varies only the compress shape)

upstream  Select 1087808   dummy_input c8750c918d01f33e..  total_memory 2016023
ours      Select 1087840   dummy_input c8750c918d01f33e..  total_memory 2016023

dummy_input identical: true   total_memory identical: true   event_counts identical: true
```

So the bump cannot move `shrink_shape`, the wrap input, or the wrap vk. The
reason is visible in `shapes.rs:358-382`: compress heights enter only through
`ceil(sum(height * width) / 2^20)`, and a 32-row delta does not straddle a
multiple of 2^20. And `git diff` confirms `Select` is the *only* height we
changed, so our shrink program is identical to upstream's.

**The VK-map height is therefore the only channel**, and stage 1's trim brings
it to upstream's 18. Every input to the wrap program then matches upstream:

| input to wrap vk | ours vs upstream |
| --- | --- |
| shrink shape | identical — measured above |
| VK-map height | 18 == 18, with the default (lean) cluster set |
| recursion config (chips, fri, stacking) | untouched by this fork |
| `vk_verification` | must be `true`; production needs it anyway |

**Conclusion: SP1 custom STARK chips do not by themselves require a new Groth16
ceremony — provided the chip clusters keep the padded VK-map count inside
upstream's power-of-two bucket.** For this fork that means at most 43 clusters,
and the default 54 exceeds it. The claim that prompted this spike is therefore
correct in mechanism and correct in conclusion *once the cluster count is
managed*; it is not automatic.

### REVERSED, 2026-11-13: a ceremony IS required

Stage 2 concluded the compress shape was invisible to the wrap vk. **That
conclusion was wrong**, and the error was generalising from an unrepresentative
delta.

Stage 2 tested the `Select` bump alone: +32 rows. But a 64-program VK-map sample
(stride 3,902, vs the 8-program sample's stride 31,218, which missed every large
shape) panicked on two *other* chips:

```
alu_ext.rs:99            fixed height is too small: got 795264  for 1115323 rows
prefix_sum_checks.rs:95  fixed height is too small: got 224608  for 319560  rows
```

So this fork needs `ExtAlu` +320,059 rows and `PrefixSumChecks` +94,952, not +32.
Upstream's *mprotect* shape pins 1,027,424 and 274,144 — both still short, so
there is no upstream shape to borrow either.

`shrink-shape-probe`, generalised to arbitrary `--override Chip=Height`:

| arm | dummy_input sha256 | shrink total_memory |
| --- | --- | --- |
| Select only, +32 | `c8750c918d01f33e..` | 2,016,023 |
| ExtAlu + PrefixSumChecks + Select | `cc05a71c627cbafb..` | 2,021,271 |

Different input, different program, different `event_counts`. The chain
`compress_shape -> shrink program -> shrink_shape -> wrap_input -> wrap vk`
therefore carries this change through, so our wrap vk cannot equal upstream's
`fce924068aa5a610` while the fork's chips need a larger compress shape — and
they do, because the compress program must verify core shard proofs for 140
chips.

**Consequences.**

- Ceremony-free Groth16 is **not** available to this fork. A phase-2 ceremony
  over the 27.6M-constraint wrap circuit is required, and must be redone on every
  change to the recursion layer.
- The lean cluster default (43 clusters, depth 18) was necessary but not
  sufficient; with the wrap vk diverging anyway, matching upstream's VK-map depth
  buys nothing. Keeping or reverting it is close to cost-free either way (stage 4:
  zero wasted rows, at most +2 chips), but its stated justification is gone.
- The real VK map is still required, for soundness: every proof so far carries
  `without_vk_verification()`. It simply no longer buys ceremony avoidance, which
  was most of the case for spending on it.
- **PLONK's universal SRS now looks like the right call after all** — which is
  what `docs/proof-pipeline-plan.md` originally decided, on exactly this reasoning,
  before the reversal in f16f511. The gas table favours Groth16 by ~75,000 gas per
  verification; a repeated ceremony is the price of that.
- The compress shape still needs regenerating properly with
  `test_find_recursion_shape`. Hand-bumping fixed only the first panic each time,
  twice now.

### Stage 3 RESULT: production downloads by version, and will not warn you

`try_install_circuit_artifacts` (`build.rs:468`) keys the download by
`SP1_CIRCUIT_VERSION` — `v6.1.0` in this tree — **not** by the wrap vk hash.
Only the dev path is hash-keyed (`<prefix>-groth16-dev`).

So in production mode SP1 downloads artifacts for the pinned circuit version
whatever our wrap circuit is. A wrap-vk mismatch is therefore **silent at
download time** and surfaces only as a verification failure later. Matching the
wrap vk is a correctness requirement, not an optimisation, and nothing in the
pipeline checks it for you.

### What is still not proven

The chain above is complete by derivation plus the two measurements, but no
end-to-end byte comparison of our computed wrap vk against
`fce924068aa5a610` has been made. That needs `vk_verification = true`, which
needs the real VK map built — 249,750 programs on the lean arm — i.e. Phase A
task 3. Until then: strong, link-by-link, but not observed.

### Stage 3 (superseded) — prove the payoff. ~10 min

With `SP1_CIRCUIT_MODE` unset, confirm the production path actually fetches
Groth16 artifacts for our wrap VK. A 404 here would mean the VK matches but
nothing usable is published — worth knowing before claiming success.

### Stage 4 RESULT, 2026-11-12: the trim wastes zero rows

`crates/cycle-runner/src/bin/cluster-cost.rs`. The premise of this stage -- that
coarser clusters waste cells -- was **wrong**, and the code says so:
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

### Stage 4 (superseded) — price the trim. Conditional, ~30 min

Only if 1-3 succeed. Coarser clusters waste rows: a `PallasAdd`-only shard gets
proved in a cluster that also allocates `Uint256Mul` and `VestaAdd`. Measure with
`mirror-records` and the chip-width/cells accounting, then decide whether
ceremony-free Groth16 is worth the proving overhead.

## What this spike does not do

- It does not produce a Groth16 proof, or a PLONK one. As of 2026-11-12 we have
  neither: the A40 was OOM-killed in `plonk.Setup` at the 46.6 GiB cgroup limit,
  and the 238 GiB CPU run was stopped by choice at 1 h 04 min, 30 h of CPU time,
  107.4 GiB peak, still pre-wrap.
- It does not build the real VK map (249,750 programs on the default set).
  Production needs that for
  soundness regardless, and `without_vk_verification()` is itself a divergence:
  `vk_verification` is a direct parameter to `wrap_program_from_input`.
- It says nothing about Groth16 proof size or verifier gas.

## Note on PLONK

None of this applies to PLONK, whose SRS is universal — the non-dev path calls
`DownloadAndSaveAztecIgnitionSrs`. PLONK never needs a new ceremony however far
the circuit diverges; it only ever costs compute. That asymmetry is the reason
this spike exists only for Groth16.
