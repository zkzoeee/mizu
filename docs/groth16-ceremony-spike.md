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
43 x 6001 + 6 = 258,049, depth 18.

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

Gated behind `MIZU_LEAN_PASTA_CLUSTERS` (`core/machine/src/riscv/mod.rs`),
default unchanged so `PASTA_RETAIN` routing is untouched.

| arm | clusters | VK map keys | next_pow2 | depth |
| --- | --- | --- | --- | --- |
| default (14 pasta clusters) | 54 | 324,060 | 2^19 | 19 |
| `MIZU_LEAN_PASTA_CLUSTERS=1` (3) | 43 | 258,049 | 2^18 | **18** |
| upstream, est. from `vk_map.bin` | - | ~185,862 | 2^18 | **18** |

43 is the exact boundary: 258,049 against a 262,144 ceiling, 4,095 keys of
margin, well under one cluster. **Zero headroom** — any future chip cluster tips
back to depth 19. Trim to 42 or fewer if that matters.

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
which needs the real VK map built (258,049 programs on the lean arm) — Phase A
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

### Stage 3 — prove the payoff. ~10 min

With `SP1_CIRCUIT_MODE` unset, confirm the production path actually fetches
Groth16 artifacts for our wrap VK. A 404 here would mean the VK matches but
nothing usable is published — worth knowing before claiming success.

### Stage 4 — price the trim. Conditional, ~30 min

Only if 1-3 succeed. Coarser clusters waste rows: a `PallasAdd`-only shard gets
proved in a cluster that also allocates `Uint256Mul` and `VestaAdd`. Measure with
`mirror-records` and the chip-width/cells accounting, then decide whether
ceremony-free Groth16 is worth the proving overhead.

## What this spike does not do

- It does not produce a Groth16 proof, or a PLONK one. As of 2026-11-12 we have
  neither: the A40 was OOM-killed in `plonk.Setup` at the 46.6 GiB cgroup limit,
  and the 238 GiB CPU run was stopped by choice at 1 h 04 min, 30 h of CPU time,
  107.4 GiB peak, still pre-wrap.
- It does not build the real VK map (324,060 programs). Production needs that for
  soundness regardless, and `without_vk_verification()` is itself a divergence:
  `vk_verification` is a direct parameter to `wrap_program_from_input`.
- It says nothing about Groth16 proof size or verifier gas.

## Note on PLONK

None of this applies to PLONK, whose SRS is universal — the non-dev path calls
`DownloadAndSaveAztecIgnitionSrs`. PLONK never needs a new ceremony however far
the circuit diverges; it only ever costs compute. That asymmetry is the reason
this spike exists only for Groth16.
