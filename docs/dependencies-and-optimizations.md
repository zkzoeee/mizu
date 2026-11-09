# Dependency versions, optimization inventory and upgrades

This inventory describes the implementation at `d0bd6cb32b3cc0c9450a6b2e58df4384419a299d`.
Exact pins and patch order are maintained in `dependencies/lock.json`; ordinary
Cargo package resolutions live in each workspace's `Cargo.lock`. This is a
local-state audit, not a claim that the pinned upstream revisions are the latest
available releases.

## Versions and ownership

| Component | Upstream version/base | Our current pin | What is modified |
| --- | --- | --- | --- |
| o1js-to-zkvm | Commit `095a5c0` (2026-05-26); no repository release version asserted | `d0bd6cb32` at this audit | Root verifier, guest/host integration, tools, optional receipt example and documentation |
| Arkworks `algebra` repository | Tag `v0.5.0`, `7ad88c4` | `8bcb64d` | Three commits; source changes in `ark-ff`, `ark-ec`, `ark-poly` |
| o1-labs proof-systems | Workspace version **0.7.0**, official commit `b1d9829` | `e7f63a64d` | Four commits; `mina-curves`, `mina-poseidon`, `poly-commitment`, `kimchi` |
| SP1 fork | **6.10.0**, official commit `d83ed91` | `7d0c4646a` | Four commits; executor, executor-runner, core-machine and curves |
| Other SP1/SLOP crates | **6.10.0**, crates.io | Exact lockfile versions | Unmodified SDK, build, zkVM/lib, JIT, primitives, hypercube, recursion/verifier and SLOP packages |
| crypto-bigint | **0.5.5**, SP1-maintained tag `patch-0.5.5-sp1-6.0.0`, commit `ef85da8668` | Same external commit | Consumed by root and receipt workspace; **not authored or edited by Mizu** |
| Optional Mina fixture exporter | Official Mina commit `e8c2137` | Unchanged Gitlink | Only upstream OCaml fixture-export tooling; default Rust dependencies use our directly pinned proof-systems fork |
| Toolchains | Host Rust **1.92.0**; guest Rust **1.96.0-dev**; SP1 builder commit **d83ed91** | Recorded in benchmark | Tooling, not additional source forks |

**Algebra and Ark are the same family here.** `algebra` is the repository;
`ark-ff`, `ark-ec`, etc. are crates within it. There is no separate Mizu `ark`
repository or dependency named simply `ark`.

All local Arkworks crates used by the verifier are 0.5.0. `ark-serialize` is
path-patched for graph consistency, but its source is unchanged. The local
`ark-ff-asm`, `ark-ff-macros` and `ark-serialize-derive` crates are unchanged too.
`ark-std` and `ark-bn254` 0.5.0 come from crates.io without Mizu modifications.
Older Ark 0.3/0.4 packages also appear in the lockfiles; current default host
`cargo tree -i ark-ff@0.3.0` / `@0.4.2` selects none. Their presence does not mean
our Pickles verifier uses a mixed Ark 0.3/0.4/0.5 field API.

Within proof-systems, `groupmap`, `o1-utils`, `hasher`, `internal-tracing` and
`msm` have no changes against our selected official base. The chosen 0.7.0 base
is the exact revision pinned through Mina by our official o1js-to-zkvm baseline.
Our four optimization commits retain its no-std feature
split and SRS ownership conventions, while aligning its SP1 dependency to 6.10.0.

There are **three external source forks and eleven modified external crates**.
The crypto-bigint override is a fourth external patched source, maintained by
SP1 rather than us. Its tag's `6.0.0` suffix does not make our SP1 version 6.0.

## Separate workspaces

The root, `examples/settlement`, `tools/pasta-proof` and `tools/pasta-guest` have
independent lockfiles. All four lock SP1 to 6.10.0. Their other resolutions are
not identical:

| Package | Root / receipt example | Standalone Pasta tools |
| --- | --- | --- |
| crypto-bigint | SP1 Git patch, 0.5.5 | crates.io 0.5.5 |
| num-bigint 0.4 line | 0.4.6 | 0.4.8 |
| serde | 1.0.228 | 1.0.229 |
| rayon | 1.10.0 | 1.12.0 in proof harness; guest has no Rayon entry |
| rug / GMP bindings | Not in these lockfiles | Optional proof-harness `gmp` feature, locked rug 1.30.0 / gmp-mpfr-sys 1.7.1 |

These packages are unmodified. A package appearing in a lockfile, especially an
optional GPU/GMP dependency, does not establish that its feature was enabled in
the measured CPU execution. Per-workspace Ark/SP1 versions, external Git sources
and selected library resolutions are in `dependencies/inventory.json`.

## Retained optimizations

The measured retained configuration is `benchmarks/retained.json`: **40 configured
compile-time settings**. Normal `cargo build` does not automatically turn these
settings on; `scripts/benchmark.py` applies them. The compatibility `ZEKO_` names
are retained in the implementation. Multiple settings compose or supersede one
another, so these are not forty independently measured speedups.

The tables abbreviate repeated parent directories. The complete file list and
flag-to-file/line map use exact repository-relative paths in the appendices and
JSON inventory.

| Optimization | Selection / effect | Primary implementation files |
| --- | --- | --- |
| Serial arithmetic schedules in zkVM | Always selected on `target_os=zkvm`; avoids Rayon/thread queries in MSM, batch inversion and polynomial evaluation | `vendor/algebra/ec/src/scalar_mul/variable_base/mod.rs`; `vendor/algebra/ff/src/fields/mod.rs`; `vendor/algebra/poly/src/polynomial/univariate/dense.rs` |
| Direct Montgomery multiplication/squaring and conversion | `MONT_DIRECT_MULMOD`; uses exact Pasta modulus matching and prepacked UINT256 operands where Montgomery arithmetic remains | `vendor/algebra/ff/src/fields/models/fp/montgomery_backend.rs` |
| Canonical-residue Pasta arithmetic | `CANONICAL_MSM`, `CANONICAL_PALLAS`, `DIRECT_MULMOD`; keeps coordinates canonical across long computations, avoiding repeated Montgomery corrections | `crates/pickles-verifier/src/canonical_msm.rs`; `vendor/proof-systems/curves/src/pasta/canonical_pallas.rs`; `canonical_words.rs` |
| Checked inverse hints | `HINTED_INVERSE`; obtains inverse hints but checks canonicality and the inverse equation in the guest | `vendor/proof-systems/curves/src/pasta/precompile_spikes.rs` |
| Pallas GLV and affine/Pasta MSM backend | `PALLAS_GLV`, `PALLAS_AFFINE`, `PALLAS_POINT_PRECOMPILE`; retained window 13 | `vendor/proof-systems/curves/src/pasta/pallas_glv.rs`; `canonical_pallas.rs`; `point_spikes.rs`; `curves/pallas.rs` |
| Vesta GLV and affine/Pasta MSM backend | `VESTA_GLV`, `VESTA_AFFINE`, `VESTA_POINT_PRECOMPILE`; retained window 16 | `crates/pickles-verifier/src/vesta_glv.rs`; `vesta_spikes.rs`; `canonical_msm.rs` |
| Fused base/scalar conversions | `FUSE_PALLAS_BASE_CONVERSION`, `FUSE_VESTA_BASE_CONVERSION`, `TRIM_CONVERSIONS`; removes intermediate representation buffers and conversions | `crates/pickles-verifier/src/canonical_msm.rs`; `vendor/proof-systems/curves/src/pasta/canonical_pallas.rs` |
| Fixed positional SRS tables | `FIXED_PALLAS_SRS`, `FIXED_VESTA_SRS`, `FIXED_VESTA_COUPLED`, `BORROW_POSITIONAL_SCALARS`; precomputes immutable positional bases and couples scalar/table access | `crates/pickles-verifier/src/fixed_pallas.rs`; `fixed_srs.rs`; `canonical_msm.rs`; `serialize.rs`; `crates/o1-verifier/build.rs` |
| Borrowed immutable SRS storage | `BORROW_PALLAS_SRS`, `BORROW_VESTA_SRS`; borrows trusted embedded bytes/generators rather than copying them | `crates/pickles-verifier/src/serialize.rs`; `types.rs`; `vendor/proof-systems/poly-commitment/src/generators.rs`; `ipa.rs` |
| Smaller/faster MSM working sets | `MSM_FILTER_ZERO`, `PACKED_PALLAS_HALVES`, `UNINIT_MSM_DIGITS`, `INLINE_FINITE_COPY=words`, outlined Pallas/Vesta bucket helpers; window-major digits, bounded buffers and cheaper point copies | `vendor/algebra/ec/src/scalar_mul/variable_base/mod.rs`; `vendor/proof-systems/curves/src/pasta/canonical_pallas.rs`; `point_spikes.rs` |
| Canonical accumulator coefficients | `CANONICAL_ACCUMULATOR_COEFFICIENTS`; generates the same challenge polynomial while keeping its coefficients canonical | `crates/pickles-verifier/src/canonical_coefficients.rs`; `lib.rs` |
| IPA allocation/prefix/coefficient paths | `IPA_EXACT_CAPACITY`, `IPA_BORROW_SRS_PREFIX`, `IPA_CANONICAL_PREFIX`, `IPA_DIRECT_COEFFICIENTS`; bounds allocations, borrows the needed generator prefix and avoids coefficient round trips | `vendor/proof-systems/poly-commitment/src/ipa.rs`; `canonical_ipa.rs` |
| In-place challenge-polynomial expansion | `APPEND_B_POLY`; appends into the destination allocation | `vendor/proof-systems/poly-commitment/src/commitment.rs` |
| Single verifier MSM path | Replaces parallel half-MSM splitting in verifier commitments and routes public-input / combined commitments through the optimized Pasta hook | `vendor/proof-systems/poly-commitment/src/commitment.rs`; `vendor/proof-systems/kimchi/src/verifier.rs` |
| Canonical Poseidon with prepared multipliers | `CANONICAL_POSEIDON`, `POSEIDON_PACKED_RHS`; preserves the exact 55-round Kimchi permutation while preparing canonical constants/RHS operands | `vendor/proof-systems/poseidon/src/canonical.rs`; `permutation.rs`; `pasta/{fp,fq}_{kimchi,legacy}.rs` |
| Integer scalar-challenge conversion | `INTEGER_SCALAR_CHALLENGE`; computes the same scalar challenge through integer operations | `vendor/proof-systems/poseidon/src/sponge.rs` |
| Build-time verifier setup and VK digest | `PRECOMPUTED_SETUP`, `PRECOMPUTED_VK_DIGEST`; moves immutable linearization/alpha-registry and transcript-digest work to the build | `crates/pickles-verifier/src/setup_data.rs`; `serialize.rs`; `types.rs`; `vendor/proof-systems/kimchi/src/alphas.rs`; `verifier.rs`; `crates/o1-verifier/build.rs` |
| Proof-sized wrap SRS prefix | `WRAP_SRS_PREFIX`; decodes/borrows the needed opening prefix without changing the VK's logical max domain | `crates/o1-verifier/src/main.rs`; `crates/pickles-verifier/src/serialize.rs`; `vendor/proof-systems/poly-commitment/src/ipa.rs` |
| Avoid unused permutation-cell maps | Uses `Shifts::shifts_only` where only coset shifts are needed | `vendor/proof-systems/kimchi/src/circuits/polynomials/permutation.rs`; `crates/pickles-verifier/src/types.rs` |
| Cached verifier-blob generation | Host build-time improvement; fingerprints generator executable, VK and both SRS files before reusing generated blobs | `crates/o1-verifier/build.rs`; `blob_cache.rs`; `crates/pickles-verifier/tests/verifier_blob_cache.rs` |

## SP1-side support and host optimizations

| Change | Scope | Files |
| --- | --- | --- |
| Pallas/Vesta add and double syscalls, curve definitions and AIR registration | Enables the guest's Pasta acceleration; eight trusted/user AIR variants, syscall/event/cost/retention plumbing | `vendor/sp1/crates/curves/src/weierstrass/pasta.rs`; `crates/core/executor/src/{air.rs,syscall_code.rs,minimal/ecall.rs,events/precompiles/mod.rs,retain.rs}`; `crates/core/machine/src/riscv/mod.rs`; Weierstrass add/double implementation files |
| Inverse reuse during witness generation | Host witness-time optimization; does not reduce guest instruction cycles | `vendor/sp1/crates/core/machine/src/operations/field/field_op.rs`; `syscall/precompiles/weierstrass/weierstrass_add.rs` |
| Optional GMP extended-Euclid inversion | `gmp` / `bigint-rug` feature; **off** in default CPU checks and the core cycle benchmark | `vendor/sp1/crates/curves/src/lib.rs`; `crates/core/machine/src/operations/field/field_op.rs`; `tools/pasta-proof/Cargo.toml` |
| Stream records with touched memory/page state | Bounded host memory for record/proof harness modes; not a guest-cycle saving | `vendor/sp1/crates/core/machine/src/utils/prove.rs`; `crates/core/executor/src/record.rs`; `vm.rs`; `vm/gas.rs` |
| Preserve tied records during ordering | Correctness: keeps every item rather than losing tied keys | `vendor/sp1/crates/core/machine/src/utils/prove.rs` |
| Portable executor and syscall counters | Execution/measurement infrastructure; portable backend is forced, and x86_64 Linux consumers must enable profiling | `vendor/sp1/crates/core/runner/build.rs`; `src/portable.rs`; `crates/core/executor/src/minimal.rs`; `minimal/arch/portable/mod.rs` |
| Registry-consumer manifest adapter | Avoids duplicate SP1/SLOP Rust types while patching four SP1 crates into the released SDK graph | Four SP1 crate `Cargo.toml` files, separate fork commit `59fa996a5` |
| Low-memory host execution and reproducible measurements | Executes without requesting proofs, pins fixture/output/source identities, bounds memory and optional trace replay | `crates/o1-verifier-host/src/main.rs`; `crates/cycle-runner/src/main.rs`; `scripts/benchmark.py` |

These host-time, memory, correctness and integration changes must not be presented
as additional guest-cycle speedups. The active core benchmark executes without
proving records; streaming/GMP/GPU improvements are not exercised by that number.

## Retained experiments and non-performance changes

The source also retains optional experiments that are not selected by the
retained configuration: generic MSM window sweeps (`ZEKO_MSM_WINDOW_BITS`),
Vesta prepared-lane tables (`ZEKO_VESTA_PREPARED_LANES`), alternative compact
backends, the generic Arkworks affine-batch-inversion implementation, and
`debug-log` features in `ark-ff` / `ark-ec`. Their presence is not evidence that
they contributed to 142,650,295 cycles. The guest's `mina-curves/sp1` Cargo
feature is available but not enabled by the current host build script or the
retained benchmark configuration.

Wire canonicality checks, application-statement binding, deterministic lookup
ordering, the Montgomery squaring predicate correction, and invalid-proof
rejection are correctness behavior. Some, including deterministic lookup ordering,
are now provided by the selected upstream base rather than local patches. Primary files are
`crates/pickles-verifier/src/{wire.rs,convert.rs,statement.rs,types.rs,lib.rs}`,
`vendor/proof-systems/kimchi/src/circuits/lookup/{lookups.rs,tables/mod.rs}`, and
`vendor/algebra/ff/src/fields/models/fp/montgomery_backend.rs`.
Receipt derivation, test fixtures, CI, licensing and documentation are not
arithmetic optimizations. Receipt types/logic live only in `examples/settlement`.

## Upgrade workflow

Treat an upgrade as a reviewed dependency change, not an unbounded `cargo update`.
The component version and our fork revision are distinct: a crate still reports
0.5.0 / 0.7.0 / 6.10.0 even though our pinned commit adds local changes.

1. Choose the official new base and preserve the old base/tip. Work on an upgrade
   branch. For the root, fetch `upstream` and rebase the root module series onto
   the chosen o1js-to-zkvm commit. Keep the upstream guest/host paths.
2. For a dependency fork, replay only that fork's topic commits onto its selected
   upstream base. Review `git range-diff OLD_BASE..OLD_TIP NEW_BASE..NEW_TIP` and
   the changed-file appendix. Do not transplant another full source snapshot.
3. Resolve related API changes as one migration. Arkworks major changes require
   coherent versions across fields, curves, polynomial, serialization and macro
   crates and the proof-systems users. Preserve proof-systems' no-std feature split,
   SRS ownership and authenticated setup interfaces when porting local changes.
4. An SP1 upgrade must align the SDK, guest runtime/lib, build crate, executor,
   machine, curves, JIT, primitives, SLOP and enabled GPU crates. Update the
   SP1 references in the algebra and proof-systems fork manifests too. Review
   our syscall IDs, AIR/event/cost/retention registration, portable-runner adapter
   and all three codebases' package versions. Review the external crypto-bigint
   override against the new SP1 release instead of blindly keeping/removing it.
5. Update each affected workspace manifest and lockfile separately: root,
   `examples/settlement`, `tools/pasta-proof`, `tools/pasta-guest`. Update
   `install.sh`, toolchain requirements and `benchmarks/retained.json`'s SP1
   version/builder commit. Keep benchmark flags and fixture hashes explicit.
6. Test native verifier/tool/receipt/harness suites, no-std, guest and host
   compilation, fresh honest/corrupt guest execution and identical public output
   for the same ABI. Re-measure cycles; compare the same fixture, flags, compiler
   and output contract. SP1 circuit/AIR updates additionally require fresh honest
   and adversarial proofs and Linux/CUDA validation on an authorized host. New
   circuit versions may require new proving/verifying keys and proof artifacts.
7. Advance the tested fork branch, regenerate its bundle from the matching ref,
   and update the root Gitlink plus `lock.json` base/revision/tree, patch list and
   bundle hash in one dependency-bump commit. The bundle ref must match the
   manifest's `branch`. Verify bootstrap from a fresh checkout. Regenerate this
   inventory after the upgrade.

The forks remain local until published. Once actual fork URLs are known, update
`.gitmodules` to those URLs; keep official repositories as `upstream`. Bootstrap
already reconstructs our unpublished commits from bundles. Root upstream syncs
and the three fork upgrades can be reviewed independently, except for explicit
API/version coupling above.

## Validation and measurement scope

The retained boolean guest measured **142,650,295 cycles** with 249 native tests
passing, successful guest/host and harness compilation, fresh-checkout
restoration, and a rejected corrupted accumulator challenge. The old receipt
measurements used a different 577-byte output and are not direct per-optimization
comparisons. No new per-toggle ablation was performed for this inventory; do not
sum hypothetical savings. Fresh real proofs and Linux/CUDA validation remain
pending. See `benchmarks/core-proof-systems-0.7-sp1-6.10.json` and `docs/proof-systems-upgrade.md`.

## File and flag appendices

[Every changed file, grouped by upstream repository](file-changes.md) lists the
exact diff baseline and status. [Machine-readable inventory](../dependencies/inventory.json)
contains full commit IDs, all forty configured flags and their file/line
references, and the four lockfile version inventories. These generated lists
cover the committed implementation snapshot stated at the top; subsequent
documentation-only edits are outside that snapshot.
