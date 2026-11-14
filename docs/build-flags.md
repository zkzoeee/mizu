# Build flags: every environment variable that shapes the guest

Reference for the 54 variables listed in
[`crates/o1-verifier/build-env.rs`](../crates/o1-verifier/build-env.rs), which is
the list both build scripts declare to cargo. Generated from the read sites and
the `optimization_env` keys of `benchmarks/dev/*.json`, so it describes what the
tree does rather than what it was meant to do.

Why the list exists at all: cargo reruns a build script only when something it
declared has changed, and the host script is the one that invokes `sp1_build`.
Before both scripts declared these, a `VK_JSON=... make build-rust` from a warm
tree rebuilt nothing and handed back the previous guest — exit 0, wrong VK. That
cost three confused builds on 2026-11-13.

## The naming is a compatibility artifact, not a provenance claim

43 of the 54 are spelled `ZEKO_*`. **All 54 are this project's own work**,
developed here on top of upstream `o1-labs/o1js-to-zkvm`. The prefix is a
compatibility name, which is how
[the dependency inventory](dependencies-and-optimizations.md) already puts it:
"The compatibility `ZEKO_` names are retained in the implementation."

Checked rather than assumed, because an earlier version of this section claimed
these were ported from Zeko's optimization work, and that is wrong:

- **No upstream ref contains any `ZEKO_*` flag.** All seventeen
  `refs/remotes/upstream/*` branches, `main` included, have zero matches for
  `ZEKO_[A-Z_]+` in any `.rs` file.
- **The prefix first appears in this repository's own history**, at `f0e1a1f`
  ("Add strict Pickles wire validation and retained verifier optimizations",
  2026-11-09), which is a descendant of `upstream/main`.

So the starting point was upstream, and every optimization on this list was
measured and retained here. **The only thing this project takes from Zeko is the
settlement idea** — the inner-action tree encoding discussed in
[the diff-proof plan](diff-proof-plan.md). Nothing in the flag surface comes
from it, and Zeko is not a dependency.

The prefix split is therefore chronological, not editorial: `ZEKO_*` is the
older, settled set and `MIZU_*` is what has been added since the naming changed.
Renaming the 43 has not been done because `option_env!` bakes values at compile
time, so it would:

- invalidate every provenance receipt under `benchmarks/provenance/`,
- change `elf_sha256` and therefore the `programVKey` in every benchmark record,
- require all ten `benchmarks/dev/*.json` recipes rewritten together, and
- break reproduction of the recorded cycle counts from their stated recipes.

None of that is hard; it is a change that has to land in one commit with the
receipts regenerated, and it buys naming consistency rather than behaviour.

## Three tiers

| tier | count | what it does |
| --- | ---: | --- |
| **Compile-time** (`option_env!`) | 48 | Baked into the guest at compile time. Changing one changes the ELF, the `programVKey`, and potentially the shard shapes. |
| **Build-script** (`env::var_os` in a `build.rs`) | 5 | Selects a file or a cache path: `SETTLEMENT_BLOB_CACHE_DIR`, `SETTLEMENT_REBUILD_VERIFIER_BLOB`, `SETTLEMENT_VK_JSON`, `VK_JSON`, `ZEKO_PREPARED_VESTA_PATH`. |
| **Run-time** (`env::var`, host only) | 1 | `PASTA_RETAIN` — does not reach the guest. |

### The one that is not a guest flag

`PASTA_RETAIN` has **no `option_env!` site anywhere** in `crates/`,
`vendor/proof-systems`, `vendor/algebra` or `vendor/sp1`. It is read only at
runtime, by `crates/cycle-runner/src/bin/mirror-records.rs:50`, and it selects
prover-side event retention routing. So it does not change the guest ELF, and
listing it in `GUEST_BUILD_ENV` forces a guest rebuild that nothing needs.
Harmless but wasteful, and worth removing from that list — carefully, since the
list is also what stops the silent-stale-guest failure above.

## The recipes are one baseline plus six switches

All ten recipes in `benchmarks/dev/` share an identical **38-setting base**,
every one of them a `ZEKO_*` flag. No recipe omits any of it. Across the ten, only
six settings ever vary:

| setting | varies how |
| --- | --- |
| `ZEKO_CANONICAL_POSEIDON` | `1` in nine recipes, `0` in `poseidon-reference` (the control) |
| `ZEKO_POSEIDON_PACKED_RHS` | toggled together with the above, in every recipe |
| `MIZU_PASTA_FP_PRECOMPILE` | `pasta-fp`, `pasta-poseidon`, `pasta-poseidon-digest` |
| `MIZU_POSEIDON_PRECOMPILE` | `pasta-poseidon`, `pasta-poseidon-digest` |
| `MIZU_POSEIDON_DEFERRED_MDS` | `deferred-mds`, `poseidon-deferred-mds` |
| `MIZU_CIP_MODE` / `MIZU_STACK_CROSS_FIELD` | `allocation` only |

So the `ZEKO_*` set is a settled baseline that is on everywhere, and **every
experiment this repository has run is driven by the `MIZU_*` flags.** That is
the practical reason the naming matters: the live surface is the Mizu six.

**On 38 versus 40.** `docs/optimizations/README.md` and the area records speak of
"40 retained settings". That is `retained.json`'s own count: the 38 above plus
`ZEKO_CANONICAL_POSEIDON=1` and `ZEKO_POSEIDON_PACKED_RHS=1`. The 38 here is the
intersection across all ten `benchmarks/dev/` recipes, which excludes those two
only because `poseidon-reference` sets them to `0`. Same set, different question.

**Three more configs carry the retained 40**, outside `benchmarks/dev/`:
`benchmarks/retained.json`, `core-proof-systems-0.7-sp1-6.10.json` and
`core-sp1-6.10.json`. Their `optimization_env` blocks are byte-identical to
`benchmarks/dev/retained.json`, so they add no settings to this reference — but
they are the ones the measurement records and the dependency inventory cite, so
a flag change has to reach all four.

### What the switches are worth, measured

Against this repo's canonical base of 142,638,298 cycles on the pinned mainnet
fixture:

| configuration | cycles | delta |
| --- | ---: | ---: |
| canonical (neither precompile) | 142,638,298 | — |
| `MIZU_PASTA_FP_PRECOMPILE` | 139,610,314 | **-2.12%** |
| + `MIZU_POSEIDON_PRECOMPILE` | 130,367,324 | **-8.60%** total |

And `PASTA_RETAIN` is a prover-side lever rather than a cycle one: retained
routing takes main cells from 43,922,703,280 to 27,839,873,136 and the Groth16
runs from 138 shards to 84.

## Footguns

Each of these is a real shape in the tree, not a hypothetical:

- **`ZEKO_VESTA_PREPARED_LANES` is inverted.** Its guards are
  `option_env!(..).is_none()`, so setting it to *any* value — including `0` —
  turns the prepared-lane route off. Every other flag here is "`1` enables".
- **`ZEKO_VESTA_WINDOW` is compared to the literal `16`** at
  `canonical_msm.rs:317`, which gates `ZEKO_FIXED_VESTA_COUPLED`. Another width
  does not error; it silently takes a different path.
- **`ZEKO_PALLAS_WINDOW` is parsed as a number** and `.expect()`s, so a
  non-numeric value fails the build rather than being ignored.
- **Two flags are string-valued, not boolean:** `ZEKO_INLINE_FINITE_COPY=words`
  and `MIZU_CIP_MODE=borrowed|fused`. `=1` is not a valid value for either.
- **`VK_JSON` wins over `SETTLEMENT_VK_JSON`**, which is a fallback only.
- **`option_env!` bakes the value at compile time.** Exporting a flag and
  rerunning the host without a rebuild changes nothing, which is exactly the
  failure the `GUEST_BUILD_ENV` list exists to prevent.

## Full reference

Area tags match the files under [`docs/optimizations/`](optimizations/). **base** marks the 38 settings every recipe shares; **recipes** counts how many of the ten set the flag at all.

### Build plumbing — these select inputs, not optimizations

| flag | value | base | recipes | read at | what it selects |
| --- | --- | :-: | :-: | --- | --- |
| `SETTLEMENT_BLOB_CACHE_DIR` | — | — | 0/10 | `o1-verifier/build.rs:44` | Directory for the cached `verifier.bin` blob, so a rebuild can reuse the baked SRS/VK blob instead of regenerating it. |
| `SETTLEMENT_REBUILD_VERIFIER_BLOB` | — | — | 0/10 | `o1-verifier/build.rs:47` | `1` forces the verifier blob to be rebuilt even when the cache would hit. |
| `SETTLEMENT_VK_JSON` | — | — | 0/10 | `o1-verifier-host/build.rs:32` +1 | Fallback VK path, used only when `VK_JSON` is unset. Both build scripts consult them in the same order. |
| `VK_JSON` | — | — | 0/10 | `o1-verifier-host/build.rs:31` +1 | Path to the wrap verification key JSON baked into the guest. Takes precedence over `SETTLEMENT_VK_JSON`. Changing it changes the guest ELF and therefore the `programVKey`. |
| `ZEKO_PREPARED_VESTA_PATH` | — | — | 0/10 | `pickles-verifier/build.rs:9` | Build-script file selection: copies the named prepared-Vesta table into `OUT_DIR/vesta-prepared.bin`. Unset writes an empty file, so the table is absent rather than stale. |

### Field arithmetic

| flag | value | base | recipes | read at | what it selects |
| --- | --- | :-: | :-: | --- | --- |
| `ZEKO_CANONICAL_MSM` | `1` | yes | 10/10 | `pickles-verifier/src/lib.rs:250` +1 | Use the canonical MSM entry point for accumulator work instead of arkworks' generic one. |
| `ZEKO_CANONICAL_PALLAS` | `1` | yes | 10/10 | `ps:curves/src/pasta/curves/pallas.rs:47` | Route Pallas curve ops through the fork's canonical implementation. |
| `ZEKO_DIRECT_MULMOD` | `1` | yes | 10/10 | `pickles-verifier/src/canonical_msm.rs:103` +1 | Call the fixed-modulus mulmod path directly rather than going through generic field multiplication. |
| `ZEKO_FUSE_PALLAS_BASE_CONVERSION` | `1` | yes | 10/10 | `ps:curves/src/pasta/canonical_pallas.rs:440` | Fuse the base-field conversion into the Pallas point operation instead of converting separately. |
| `ZEKO_FUSE_VESTA_BASE_CONVERSION` | `1` | yes | 10/10 | `pickles-verifier/src/canonical_msm.rs:207` +1 | The Vesta counterpart. Also gates part of the accumulator path in `lib.rs`. |
| `ZEKO_HINTED_INVERSE` | `1` | yes | 10/10 | `ps:curves/src/pasta/precompile_spikes.rs:19` | Compute modular inverses from a supplied hint and verify, instead of running the full inversion in the guest. |
| `ZEKO_MONT_DIRECT_MULMOD` | `1` | yes | 10/10 | `alg:ff/src/fields/models/fp/montgomery_backend.rs:64` | Same idea one layer down, inside arkworks' Montgomery backend. The only flag read in `vendor/algebra` alone. |
| `ZEKO_TRIM_CONVERSIONS` | `1` | yes | 10/10 | `pickles-verifier/src/canonical_msm.rs:223` +1 | Drop redundant Montgomery/bigint conversions around MSM scalar preparation. |

### Pasta MSM and point arithmetic

| flag | value | base | recipes | read at | what it selects |
| --- | --- | :-: | :-: | --- | --- |
| `ZEKO_INLINE_FINITE_COPY` | `words` | yes | 10/10 | `ps:curves/src/pasta/point_spikes.rs:238` | **String-valued, not `1`.** Recipes set `words`, selecting word-wise inline copying of finite-field limbs. |
| `ZEKO_MSM_FILTER_ZERO` | `1` | yes | 10/10 | `alg:ec/src/scalar_mul/variable_base/mod.rs:188` +1 | Skip zero scalars before bucketing. Read in both the proof-systems and algebra forks. |
| `ZEKO_OUTLINE_PALLAS_BUCKETS` | `1` | yes | 10/10 | `ps:curves/src/pasta/point_spikes.rs:690` | Move Pallas bucket accumulation out of line to cut code size in the hot loop. |
| `ZEKO_OUTLINE_VESTA_BUCKETS` | `1` | yes | 10/10 | `ps:curves/src/pasta/point_spikes.rs:689` | The Vesta counterpart; both read at the same `point_spikes.rs` match. |
| `ZEKO_PACKED_PALLAS_HALVES` | `1` | yes | 10/10 | `ps:curves/src/pasta/canonical_pallas.rs:443` | Pack the two halves of a Pallas base-field element together through the conversion. |
| `ZEKO_PALLAS_AFFINE` | `1` | yes | 10/10 | `ps:curves/src/pasta/canonical_pallas.rs:209` | Keep Pallas MSM inputs in affine form rather than projective. |
| `ZEKO_PALLAS_GLV` | `1` | yes | 10/10 | `ps:curves/src/pasta/canonical_pallas.rs:210` | GLV scalar decomposition for Pallas, halving scalar bit length. |
| `ZEKO_PALLAS_POINT_PRECOMPILE` | `1` | yes | 10/10 | `ps:curves/src/pasta/canonical_pallas.rs:212` | Use the SP1 `PALLAS_ADD` executor precompile for point addition. |
| `ZEKO_PALLAS_WINDOW` | `13` | yes | 10/10 | `pickles-verifier/src/fixed_pallas.rs:240` +1 | Pallas fixed-window width, parsed as a number. The recipes use `13`; a non-numeric value panics at build time. |
| `ZEKO_UNINIT_MSM_DIGITS` | `1` | yes | 10/10 | `ps:curves/src/pasta/point_spikes.rs:426` | Allocate the MSM digit buffer uninitialised instead of zeroing it first. |
| `ZEKO_VESTA_AFFINE` | `1` | yes | 10/10 | `pickles-verifier/src/canonical_msm.rs:273` +1 | The Vesta counterpart. |
| `ZEKO_VESTA_GLV` | `1` | yes | 10/10 | `pickles-verifier/src/canonical_msm.rs:208` +4 | The Vesta counterpart. |
| `ZEKO_VESTA_POINT_PRECOMPILE` | `1` | yes | 10/10 | `pickles-verifier/src/canonical_msm.rs:209` +4 | Use the SP1 `VESTA_ADD` executor precompile. |
| `ZEKO_VESTA_WINDOW` | `16` | yes | 10/10 | `pickles-verifier/src/canonical_msm.rs:317` +1 | Vesta window width, recipes use `16`. **Matched against the literal `16`** at `canonical_msm.rs:317`, so another width silently takes a different path. |

### IPA and SRS

| flag | value | base | recipes | read at | what it selects |
| --- | --- | :-: | :-: | --- | --- |
| `ZEKO_APPEND_B_POLY` | `1` | yes | 10/10 | `ps:poly-commitment/src/commitment.rs:547` | Append the b-polynomial commitment in the IPA batch rather than committing it separately. |
| `ZEKO_BORROW_PALLAS_SRS` | `1` | yes | 10/10 | `pickles-verifier/src/lib.rs:807` +1 | Borrow the baked Pallas SRS instead of cloning it into the guest heap. |
| `ZEKO_BORROW_POSITIONAL_SCALARS` | `1` | yes | 10/10 | `pickles-verifier/src/canonical_msm.rs:321` | Borrow positional scalars for the accumulator MSM rather than materialising a vector. |
| `ZEKO_BORROW_VESTA_SRS` | `1` | yes | 10/10 | `pickles-verifier/src/lib.rs:829` +1 | The Vesta counterpart. |
| `ZEKO_CANONICAL_ACCUMULATOR_COEFFICIENTS` | `1` | yes | 10/10 | `pickles-verifier/src/lib.rs:249` | Compute accumulator coefficients canonically; gated together with `ZEKO_CANONICAL_MSM` and `ZEKO_FUSE_VESTA_BASE_CONVERSION`. |
| `ZEKO_FIXED_PALLAS_SRS` | `1` | yes | 10/10 | `o1-verifier/build.rs:62` +3 | Use the compile-time fixed Pallas SRS tables. Also one of the two flags that make `build.rs` bake the verifier blob. |
| `ZEKO_FIXED_VESTA_COUPLED` | `1` | yes | 10/10 | `pickles-verifier/src/canonical_msm.rs:316` | Use the coupled fixed-Vesta table, valid only with `ZEKO_VESTA_WINDOW=16`. |
| `ZEKO_FIXED_VESTA_SRS` | `1` | yes | 10/10 | `o1-verifier/build.rs:61` +3 | The Vesta counterpart, likewise consulted by `build.rs`. |
| `ZEKO_IPA_BORROW_SRS_PREFIX` | `1` | yes | 10/10 | `ps:poly-commitment/src/ipa.rs:432` | Borrow the SRS prefix the IPA opening needs instead of copying it. |
| `ZEKO_IPA_CANONICAL_PREFIX` | `1` | yes | 10/10 | `ps:poly-commitment/src/ipa.rs:440` | Use the canonical prefix routine, passed as a flag into the batch call. |
| `ZEKO_IPA_DIRECT_COEFFICIENTS` | `1` | yes | 10/10 | `ps:poly-commitment/src/ipa.rs:508` | Build IPA coefficients directly rather than via the generic polynomial path. |
| `ZEKO_IPA_EXACT_CAPACITY` | `1` | yes | 10/10 | `ps:poly-commitment/src/ipa.rs:415` | Size the IPA buffer to the exact length needed instead of a growth-factor reservation. |
| `ZEKO_WRAP_SRS_PREFIX` | `1` | yes | 10/10 | `o1-verifier/src/main.rs:29` | Deserialise the wrap proof eagerly against the baked SRS prefix, in `o1-verifier/src/main.rs`. |

### Poseidon and the sponge

| flag | value | base | recipes | read at | what it selects |
| --- | --- | :-: | :-: | --- | --- |
| `MIZU_PASTA_FP_PRECOMPILE` | `1` | — | 3/10 | `pickles-verifier/src/canonical_msm.rs:113` +4 | Fixed-modulus Pallas/Vesta field-op precompiles. -2.12% of cycles alone. The widest flag in the tree: read in `crates/`, proof-systems and algebra. |
| `MIZU_POSEIDON_DEFERRED_MDS` | `1` | — | 2/10 | `ps:poseidon/src/canonical.rs:54` | Defer the MDS multiply via `sum3_prepared_products_deferred`. Opt-in candidate, not in the retained recipe. |
| `MIZU_POSEIDON_PRECOMPILE` | `1` | — | 2/10 | `ps:poseidon/src/canonical.rs:70` | Routes the kimchi sponge to the SP1 `PALLAS_POSEIDON`/`VESTA_POSEIDON` executor precompiles. Worth a further -6.62% of cycles on top of the Fp precompile. |
| `ZEKO_CANONICAL_POSEIDON` | `0`, `1` | — | 10/10 | `ps:poseidon/src/permutation.rs:151` | The fork's canonical Poseidon permutation. On in nine recipes; explicitly `0` in `poseidon-reference`, which is the control. |
| `ZEKO_INTEGER_SCALAR_CHALLENGE` | `1` | yes | 10/10 | `ps:poseidon/src/sponge.rs:194` | Derive sponge scalar challenges as integers, avoiding a field round-trip. |
| `ZEKO_POSEIDON_PACKED_RHS` | `0`, `1` | — | 10/10 | `ps:poseidon/src/canonical.rs:152` +1 | Packed right-hand side in the Poseidon MDS multiply. Toggled with `ZEKO_CANONICAL_POSEIDON` in every recipe. |

### Verifier setup

| flag | value | base | recipes | read at | what it selects |
| --- | --- | :-: | :-: | --- | --- |
| `ZEKO_PRECOMPUTED_SETUP` | `1` | yes | 10/10 | `o1-verifier/build.rs:60` +1 | Bake the verifier setup at build time instead of deriving it in the guest. |
| `ZEKO_PRECOMPUTED_VK_DIGEST` | `1` | yes | 10/10 | `o1-verifier/src/main.rs:30` +1 | Bake the VK digest too, so the guest does not recompute it. |

### Prover runtime

| flag | value | base | recipes | read at | what it selects |
| --- | --- | :-: | :-: | --- | --- |
| `PASTA_RETAIN` | — | — | 0/10 | `cycle-runner/src/bin/mirror-records.rs:50` +1 | Prover-side event retention routing (`uint` / `pasta-uint`). Read with `env::var` by the runners, not `option_env!` — see *The one that is not a guest flag*. |

### OpenVM-inspired experiments

| flag | value | base | recipes | read at | what it selects |
| --- | --- | :-: | :-: | --- | --- |
| `MIZU_BATCH_ACCUMULATOR` | — | — | 0/10 | `pickles-verifier/src/lib.rs:110` | Batch the accumulator check. Code path exists; **no recipe enables it.** |
| `MIZU_CIP_MODE` | `borrowed` | — | 1/10 | `pickles-verifier/src/deferred.rs:304` | **String-valued.** Combined-inner-product strategy: `borrowed` or `fused`. Only `allocation` sets it. |
| `MIZU_STACK_CROSS_FIELD` | `1` | — | 1/10 | `pickles-verifier/src/deferred.rs:375` | Stack-allocate cross-field bytes in deferred-value expansion. |

### Paths no recipe enables

| flag | value | base | recipes | read at | what it selects |
| --- | --- | :-: | :-: | --- | --- |
| `ZEKO_VESTA_COMPACT` | — | — | 0/10 | `pickles-verifier/src/canonical_msm.rs:274` | Alternative compact Vesta representation, OR-ed with `ZEKO_VESTA_AFFINE`. No recipe sets it. |
| `ZEKO_VESTA_PREPARED_LANES` | — | — | 0/10 | `pickles-verifier/src/canonical_msm.rs:210` +4 | **Inverted: presence disables.** The guarded paths require `option_env!(..).is_none()`, so setting it to any value turns off the prepared-lane MSM route. No recipe sets it. |

## Adding a flag

1. Read it with `option_env!` if the guest's behaviour depends on it, and with
   `env::var` only if it is genuinely host-side.
2. Add it to `GUEST_BUILD_ENV` in `crates/o1-verifier/build-env.rs`. Both build
   scripts include that file, so one edit covers the guest and the host.
3. Set it in whichever `benchmarks/dev/*.json` recipe exercises it, and record a
   receipt under `benchmarks/provenance/` from `make build-guest` rather than
   `make build-rust`.
4. Regenerate this document's tables from the sources rather than editing them by
   hand; every column here is derived.

