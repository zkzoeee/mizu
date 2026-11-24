# Build flags: every environment variable that shapes the guest

Reference for the 58 variables listed in
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

47 of the 58 are spelled `ZEKO_*`. **All 58 are this project's own work**,
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
Renaming the 47 has not been done because `option_env!` bakes values at compile
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
| **Compile-time** (`option_env!`) | 56 | Baked into the guest at compile time. Changing one changes the ELF, the `programVKey`, and potentially the shard shapes. |
| **Build-script** (`env::var_os` in a `build.rs`) | 5 | Selects a file or a cache path: `SETTLEMENT_BLOB_CACHE_DIR`, `SETTLEMENT_REBUILD_VERIFIER_BLOB`, `SETTLEMENT_VK_JSON`, `VK_JSON`, `MIZU_PREPARED_VESTA_PATH`. |
| **Run-time** (`env::var`, host only) | 1 | `PASTA_RETAIN` — does not reach the guest (removed from `GUEST_BUILD_ENV`). |

### The one that is not a guest flag

`PASTA_RETAIN` has **no `option_env!` site anywhere** in `crates/`,
`vendor/proof-systems`, `vendor/algebra` or `vendor/sp1`. It is read only at
runtime, by `crates/cycle-runner/src/bin/mirror-records.rs:50`, and it selects
prover-side event retention routing. It has been removed from `GUEST_BUILD_ENV`
in `crates/o1-verifier/build-env.rs` so changing it does not trigger unnecessary
guest rebuilds.

## The recipes are one baseline plus six switches

All ten recipes in `benchmarks/dev/` share an identical **38-setting base**,
every one of them a `ZEKO_*` flag. No recipe omits any of it. Across the ten, only
six settings ever vary:

| setting | varies how |
| --- | --- |
| `MIZU_CANONICAL_POSEIDON` | `1` in nine recipes, `0` in `poseidon-reference` (the control) |
| `MIZU_POSEIDON_PACKED_RHS` | toggled together with the above, in every recipe |
| `MIZU_PASTA_FP_PRECOMPILE` | `pasta-fp`, `pasta-poseidon`, `pasta-poseidon-digest` |
| `MIZU_POSEIDON_PRECOMPILE` | `pasta-poseidon`, `pasta-poseidon-digest` |
| `MIZU_POSEIDON_DEFERRED_MDS` | `deferred-mds`, `poseidon-deferred-mds` |
| `MIZU_CIP_MODE` / `MIZU_STACK_CROSS_FIELD` | `allocation` only |

So the `ZEKO_*` set is a settled baseline that is on everywhere, and **every
experiment this repository has run is driven by the `MIZU_*` flags.** That is
the practical reason the naming matters: the live surface is the Mizu six.

**On 38 versus 40.** `docs/optimizations/README.md` and the area records speak of
"40 retained settings". That is `retained.json`'s own count: the 38 above plus
`MIZU_CANONICAL_POSEIDON=1` and `MIZU_POSEIDON_PACKED_RHS=1`. The 38 here is the
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
| + zero-copy finite SRS, `MIZU_FIXED_PALLAS_COUPLED=1`, `MIZU_SOFT_PASTA_DOUBLE=1` (`w=15`) | 105,862,509 | **-25.78%** total (-18.80% vs 130.37M; -49K `vesta_add`) |
| + zero-copy finite SRS, `MIZU_FIXED_PALLAS_COUPLED=1`, `MIZU_SOFT_PASTA_DOUBLE=1` (`w=16`) | 102,664,876 | **-28.02%** total (-21.25% vs 130.37M) |
| + `MIZU_PREBAKE_NEGATIVES=1` + unchecked recoder/bucket writes (`w=15`) | 85,156,453 | **-40.30%** total (-34.68% vs 130.37M; -49K `vesta_add`) |
| + `MIZU_PREBAKE_NEGATIVES=1` + unchecked recoder/bucket writes (`w=16`) | 84,985,703 | **-40.42%** total (-34.81% vs 130.37M) |

And `PASTA_RETAIN` is a prover-side lever rather than a cycle one: retained
routing takes main cells from 43,922,703,280 to 27,839,873,136 and the Groth16
runs from 138 shards to 84.

## Footguns

Each of these is a real shape in the tree, not a hypothetical:

- **`MIZU_VESTA_PREPARED_LANES` is inverted.** Its guards are
  `option_env!(..).is_none()`, so setting it to *any* value — including `0` —
  turns the prepared-lane route off. Every other flag here is "`1` enables".
- **`MIZU_VESTA_WINDOW` coupled widths are `16`, `15`, and `14`** (matching
  baked `lane_shift` `128`, `120`, and `126` on packed 64-byte finite SRS tables
  in `canonical_msm.rs` and `fixed_srs.rs`). Any other width falls back to the
  uncoupled `lane_shift=128` path.
- **`MIZU_PALLAS_WINDOW` is parsed as a number** and `.expect()`s, so a
  non-numeric value fails the build rather than being ignored.
- **Two flags are string-valued, not boolean:** `MIZU_INLINE_FINITE_COPY=words`
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
| `MIZU_PREPARED_VESTA_PATH` | — | — | 0/10 | `pickles-verifier/build.rs:9` | Build-script file selection: copies the named prepared-Vesta table into `OUT_DIR/vesta-prepared.bin`. Unset writes an empty file, so the table is absent rather than stale. |

### Field arithmetic

| flag | value | base | recipes | read at | what it selects |
| --- | --- | :-: | :-: | --- | --- |
| `MIZU_CANONICAL_MSM` | `1` | yes | 10/10 | `pickles-verifier/src/lib.rs:250` +1 | Use the canonical MSM entry point for accumulator work instead of arkworks' generic one. |
| `MIZU_CANONICAL_PALLAS` | `1` | yes | 10/10 | `ps:curves/src/pasta/curves/pallas.rs:47` | Route Pallas curve ops through the fork's canonical implementation. |
| `MIZU_DIRECT_MULMOD` | `1` | yes | 10/10 | `pickles-verifier/src/canonical_msm.rs:103` +1 | Call the fixed-modulus mulmod path directly rather than going through generic field multiplication. |
| `MIZU_FUSE_PALLAS_BASE_CONVERSION` | `1` | yes | 10/10 | `ps:curves/src/pasta/canonical_pallas.rs:440` | Fuse the base-field conversion into the Pallas point operation instead of converting separately. |
| `MIZU_FUSE_VESTA_BASE_CONVERSION` | `1` | yes | 10/10 | `pickles-verifier/src/canonical_msm.rs:207` +1 | The Vesta counterpart. Also gates part of the accumulator path in `lib.rs`. |
| `MIZU_HINTED_INVERSE` | `1` | yes | 10/10 | `ps:curves/src/pasta/precompile_spikes.rs:19` | Compute modular inverses from a supplied hint and verify, instead of running the full inversion in the guest. |
| `MIZU_MONT_DIRECT_MULMOD` | `1` | yes | 10/10 | `alg:ff/src/fields/models/fp/montgomery_backend.rs:64` | Same idea one layer down, inside arkworks' Montgomery backend. The only flag read in `vendor/algebra` alone. |
| `MIZU_TRIM_CONVERSIONS` | `1` | yes | 10/10 | `pickles-verifier/src/canonical_msm.rs:223` +1 | Drop redundant Montgomery/bigint conversions around MSM scalar preparation. |

### Pasta MSM and point arithmetic

| flag | value | base | recipes | read at | what it selects |
| --- | --- | :-: | :-: | --- | --- |
| `MIZU_INLINE_FINITE_COPY` | `words` | yes | 10/10 | `ps:curves/src/pasta/point_spikes.rs:238` | **String-valued, not `1`.** Recipes set `words`, selecting word-wise inline copying of finite-field limbs. |
| `MIZU_MSM_FILTER_ZERO` | `1` | yes | 10/10 | `alg:ec/src/scalar_mul/variable_base/mod.rs:188` +1 | Skip zero scalars before bucketing. Read in both the proof-systems and algebra forks. |
| `MIZU_MSM_WINDOW_BITS` | `10`, `12`, `14` | — | 0/10 | `ps:curves/src/pasta/canonical_pallas.rs:229` +1 | MSM window override, read in proof-systems **and** algebra. **Panics** on any value other than unset, `10`, `12` or `14`. No recipe sets it. |
| `MIZU_OUTLINE_PALLAS_BUCKETS` | `1` | yes | 10/10 | `ps:curves/src/pasta/point_spikes.rs:690` | Move Pallas bucket accumulation out of line to cut code size in the hot loop. |
| `MIZU_OUTLINE_VESTA_BUCKETS` | `1` | yes | 10/10 | `ps:curves/src/pasta/point_spikes.rs:689` | The Vesta counterpart; both read at the same `point_spikes.rs` match. |
| `MIZU_PACKED_PALLAS_HALVES` | `1` | yes | 10/10 | `ps:curves/src/pasta/canonical_pallas.rs:443` | Pack the two halves of a Pallas base-field element together through the conversion. |
| `MIZU_PALLAS_AFFINE` | `1` | yes | 10/10 | `ps:curves/src/pasta/canonical_pallas.rs:209` | Keep Pallas MSM inputs in affine form rather than projective. |
| `MIZU_PALLAS_GLV` | `1` | yes | 10/10 | `ps:curves/src/pasta/canonical_pallas.rs:210` | GLV scalar decomposition for Pallas, halving scalar bit length. |
| `MIZU_PALLAS_POINT_PRECOMPILE` | `1` | yes | 10/10 | `ps:curves/src/pasta/canonical_pallas.rs:212` | Use the SP1 `PALLAS_ADD` executor precompile for point addition. |
| `MIZU_PALLAS_WINDOW` | `13` | yes | 10/10 | `pickles-verifier/src/fixed_pallas.rs:240` +1 | Pallas fixed-window width, parsed as a number. The recipes use `13`; a non-numeric value panics at build time. |
| `MIZU_SOFT_PASTA_DOUBLE` | `1` | — | 0/10 | `ps:curves/src/pasta/point_spikes.rs:384` | Software affine doubling via canonical Fp/Fq arithmetic + checked inverse hints. Eliminates all 481 `VestaDouble`/`PallasDouble` syscalls and their `WeierstrassDoubleAssign` AIR shards. |
| `MIZU_UNINIT_MSM_DIGITS` | `1` | yes | 10/10 | `ps:curves/src/pasta/point_spikes.rs:575` | Allocate the MSM digit buffer uninitialised instead of zeroing it first. |
| `MIZU_VESTA_AFFINE` | `1` | yes | 10/10 | `pickles-verifier/src/canonical_msm.rs:273` +1 | The Vesta counterpart. |
| `MIZU_VESTA_GLV` | `1` | yes | 10/10 | `pickles-verifier/src/canonical_msm.rs:208` +4 | The Vesta counterpart. |
| `MIZU_VESTA_POINT_PRECOMPILE` | `1` | yes | 10/10 | `pickles-verifier/src/canonical_msm.rs:209` +4 | Use the SP1 `VESTA_ADD` executor precompile. |
| `MIZU_VESTA_WINDOW` | `16` (`15`, `14`) | yes | 10/10 | `pickles-verifier/src/canonical_msm.rs:317` +2 | Vesta window width (`16`, `15`, or `14` with `MIZU_FIXED_VESTA_COUPLED=1`, selecting baked `lane_shift` `128`, `120`, or `126` on packed finite tables). |

### IPA and SRS

| flag | value | base | recipes | read at | what it selects |
| --- | --- | :-: | :-: | --- | --- |
| `MIZU_APPEND_B_POLY` | `1` | yes | 10/10 | `ps:poly-commitment/src/commitment.rs:547` | Append the b-polynomial commitment in the IPA batch rather than committing it separately. |
| `MIZU_BORROW_PALLAS_SRS` | `1` | yes | 10/10 | `pickles-verifier/src/lib.rs:807` +1 | Borrow the baked Pallas SRS instead of cloning it into the guest heap. |
| `MIZU_BORROW_POSITIONAL_SCALARS` | `1` | yes | 10/10 | `pickles-verifier/src/canonical_msm.rs:324` | Borrow positional scalars for the accumulator MSM rather than materialising a vector. |
| `MIZU_BORROW_VESTA_SRS` | `1` | yes | 10/10 | `pickles-verifier/src/lib.rs:829` +1 | The Vesta counterpart. |
| `MIZU_CANONICAL_ACCUMULATOR_COEFFICIENTS` | `1` | yes | 10/10 | `pickles-verifier/src/lib.rs:249` | Compute accumulator coefficients canonically; gated together with `MIZU_CANONICAL_MSM` and `MIZU_FUSE_VESTA_BASE_CONVERSION`. |
| `MIZU_COMPACT_EMBEDDED_BLOB` | `1` | — | 0/10 | `pickles-verifier/src/serialize.rs:321` +1 | Strip unused legacy Montgomery `VestaSrs` (when `MIZU_FIXED_VESTA_SRS=1`) and Lagrange tail beyond `wrap_vk.public` from `encode_embedded_verifier_blob` (~5.9 MB saved, keeping pre-baked ELFs `< 32 MiB`). |
| `MIZU_FIXED_PALLAS_COUPLED` | `1` | — | 0/10 | `pickles-verifier/src/fixed_pallas.rs:95` | Bake finite fixed Pallas SRS with `lane_shift=130` for coupled width-13 recoding (`msm_pallas_finite_prefix`) directly over `.rodata` `&[[u64; 8]]`. |
| `MIZU_FIXED_PALLAS_SRS` | `1` | yes | 10/10 | `o1-verifier/build.rs:62` +3 | Use the compile-time fixed Pallas SRS tables (64-byte finite `.rodata` layout for all-finite SRSes). |
| `MIZU_FIXED_VESTA_COUPLED` | `1` | yes | 10/10 | `pickles-verifier/src/canonical_msm.rs:316` | Use the coupled fixed-Vesta table (`MIZU_VESTA_WINDOW=16|15|14` with matching `lane_shift=128|120|126`). |
| `MIZU_FIXED_VESTA_SRS` | `1` | yes | 10/10 | `o1-verifier/build.rs:61` +3 | The Vesta counterpart, likewise consulted by `build.rs`. |
| `MIZU_IPA_BORROW_SRS_PREFIX` | `1` | yes | 10/10 | `ps:poly-commitment/src/ipa.rs:432` | Borrow the SRS prefix the IPA opening needs instead of copying it. |
| `MIZU_IPA_CANONICAL_PREFIX` | `1` | yes | 10/10 | `ps:poly-commitment/src/ipa.rs:440` | Use the canonical prefix routine, passed as a flag into the batch call. |
| `MIZU_IPA_DIRECT_COEFFICIENTS` | `1` | yes | 10/10 | `ps:poly-commitment/src/ipa.rs:508` | Build IPA coefficients directly rather than via the generic polynomial path. |
| `MIZU_IPA_EXACT_CAPACITY` | `1` | yes | 10/10 | `ps:poly-commitment/src/ipa.rs:415` | Size the IPA buffer to the exact length needed instead of a growth-factor reservation. |
| `MIZU_PREBAKE_NEGATIVES` | `1` | — | 0/10 | `pickles-verifier/src/fixed_srs.rs:124` +1 | Bake `-plane0`/`-plane1` into all-finite fixed Vesta/Pallas SRS tables (`flags=3`) in `.rodata`, eliminating 12.6 MB of heap `negatives` writes and 196K runtime negations. |
| `MIZU_WRAP_SRS_PREFIX` | `1` | yes | 10/10 | `o1-verifier/src/main.rs:29` | Deserialise the wrap proof eagerly against the baked SRS prefix, in `o1-verifier/src/main.rs`. |

### Poseidon and the sponge

| flag | value | base | recipes | read at | what it selects |
| --- | --- | :-: | :-: | --- | --- |
| `MIZU_PASTA_FP_PRECOMPILE` | `1` | — | 3/10 | `pickles-verifier/src/canonical_msm.rs:113` +4 | Fixed-modulus Pallas/Vesta field-op precompiles. -2.12% of cycles alone. The widest flag in the tree: read in `crates/`, proof-systems and algebra. |
| `MIZU_POSEIDON_DEFERRED_MDS` | `1` | — | 2/10 | `ps:poseidon/src/canonical.rs:54` | Defer the MDS multiply via `sum3_prepared_products_deferred`. Opt-in candidate, not in the retained recipe. |
| `MIZU_POSEIDON_PRECOMPILE` | `1` | — | 2/10 | `ps:poseidon/src/canonical.rs:70` | Routes the kimchi sponge to the SP1 `PALLAS_POSEIDON`/`VESTA_POSEIDON` executor precompiles. Worth a further -6.62% of cycles on top of the Fp precompile. |
| `MIZU_CANONICAL_POSEIDON` | `0`, `1` | — | 10/10 | `ps:poseidon/src/permutation.rs:151` | The fork's canonical Poseidon permutation. On in nine recipes; explicitly `0` in `poseidon-reference`, which is the control. |
| `MIZU_INTEGER_SCALAR_CHALLENGE` | `1` | yes | 10/10 | `ps:poseidon/src/sponge.rs:194` | Derive sponge scalar challenges as integers, avoiding a field round-trip. |
| `MIZU_POSEIDON_PACKED_RHS` | `0`, `1` | — | 10/10 | `ps:poseidon/src/canonical.rs:152` +1 | Packed right-hand side in the Poseidon MDS multiply. Toggled with `MIZU_CANONICAL_POSEIDON` in every recipe. |

### Verifier setup

| flag | value | base | recipes | read at | what it selects |
| --- | --- | :-: | :-: | --- | --- |
| `MIZU_PRECOMPUTED_SETUP` | `1` | yes | 10/10 | `o1-verifier/build.rs:60` +1 | Bake the verifier setup at build time instead of deriving it in the guest. |
| `MIZU_PRECOMPUTED_VK_DIGEST` | `1` | yes | 10/10 | `o1-verifier/src/main.rs:30` +1 | Bake the VK digest too, so the guest does not recompute it. |

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

### Diagnostics and adversarial tests

Not optimizations. These were **undeclared until 2026-11-14** — see the note
below.

| flag | value | base | recipes | read at | what it selects |
| --- | --- | :-: | :-: | --- | --- |
| `MIZU_INVERSE_HINT_FAULT` | `zero`, `modulus` | — | 0/10 | `ps:curves/src/pasta/precompile_spikes.rs:131` | **TEST ONLY.** Corrupts the inverse hint before the guest's checks, to build adversarial ELFs. Effective only with `MIZU_HINTED_INVERSE` on. Its own comment warns these runs "must never be reported as successful runs". |
| `MIZU_IPA_CAPACITY_ASSERT` | `1` | — | 0/10 | `ps:poly-commitment/src/ipa.rs:628` +1 | Asserts the IPA point and scalar buffers never reallocate: `assert_eq!(points.capacity(), ..)`. A guard for `MIZU_IPA_EXACT_CAPACITY`, not a speedup. |
| `MIZU_IPA_CAPACITY_REPORT` | `1` | — | 0/10 | `ps:poly-commitment/src/ipa.rs:622` +1 | `eprintln!`s an `IPA_CAPACITY {...}` JSON line with the point/scalar capacities. Measurement instrumentation. |

**How these were missed, and why it mattered.** `GUEST_BUILD_ENV`'s own comment
said it was generated from sites "under `crates/`" — and these three, plus
`MIZU_MSM_WINDOW_BITS`, live in `vendor/proof-systems` and `vendor/algebra`.
All four are `option_env!`, so they bake into the guest ELF, and an undeclared
one means changing it from a warm tree hands back the **previous** guest at
exit 0: exactly the failure that cost three confused builds on 2026-11-13 and
the reason the list exists. Declared now, and the comment says to grep all three
trees.

### The `ZEKO_` names that are genuinely Zeko's

The 2026-11-14 rename was exact-name against the flag list, never a prefix sed,
because **eleven `ZEKO_` names here are not ours and must not be renamed.** Nine
are wire-format domain separators whose value is hashed, so changing one changes
a hash on the wire, and `AGENTS.md` protects them as "compatibility data, not
project branding":

`ZEKO_RAW_INNER_ACTION_LEAF_V2`, `ZEKO_INNER_ACTION_FIELDS_V2`,
`ZEKO_INNER_ACTION_NODE_V2`, `ZEKO_ASSET_RECORD_BATCH_LEAF_V2`,
`ZEKO_ASSET_RECORD_BATCH_NODE_V1`, `ZEKO_NATIVE_WITHDRAWAL_LEAF_V2`,
`ZEKO_ERC20_WITHDRAWAL_LEAF_V4`, `ZEKO_ERC20_ASSET_V1`,
`ZEKO_ERC20_ASSET_RECORD_V1` — all in `examples/settlement`.

The tenth is `ZEKO_SETTLEMENT_FIXTURE_DIR` in `fixtures/ocaml-compat/README.md`:
Zeko's own variable, present in `zeko-labs/ethereum-settlement` `origin/main`,
used there in a command that runs **their** tooling. No code of ours reads it.

The eleventh is ours but is not a variable at all: `ZEKO_VERIFIER_BLOB_CACHE_V1`
is a cache-version string constant in `crates/o1-verifier/blob_cache.rs`. A
`ZEKO_[A-Z_]+` grep catches it, which is exactly why the rename ran from an
explicit list.

**So a `ZEKO_` hit in this tree is now either Zeko's or a historical record.**
Measurement records under `benchmarks/` keep the old names deliberately: they
document what was run, under the names it was run with.

### Paths no recipe enables

| flag | value | base | recipes | read at | what it selects |
| --- | --- | :-: | :-: | --- | --- |
| `MIZU_VESTA_COMPACT` | — | — | 0/10 | `pickles-verifier/src/canonical_msm.rs:274` | Alternative compact Vesta representation, OR-ed with `MIZU_VESTA_AFFINE`. No recipe sets it. |
| `MIZU_VESTA_PREPARED_LANES` | — | — | 0/10 | `pickles-verifier/src/canonical_msm.rs:210` +4 | **Inverted: presence disables.** The guarded paths require `option_env!(..).is_none()`, so setting it to any value turns off the prepared-lane MSM route. No recipe sets it. |

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

