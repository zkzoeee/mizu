# Changed files by upstream repository

Implementation snapshot: `d0bd6cb32b3cc0c9450a6b2e58df4384419a299d`. Generated from Git diffs against the official bases in `dependencies/lock.json`. Subsequent documentation-only edits are outside this snapshot. `A` = added, `M` = modified. Files within forks are relative to that fork, not the root consumer.

## o1js-to-zkvm — 118 changed files

Location: `.`. Official base: `095a5c0a46b903b7b126c857c9910567b16e2e6b`. Our pin: `d0bd6cb32b3cc0c9450a6b2e58df4384419a299d`.

```text
M .github/workflows/ci.yml
M .gitignore
M .gitmodules
A AGENTS.md
M Cargo.lock
M Cargo.toml
A LICENSE-APACHE
A LICENSE-MIT
M Makefile
M README.md
A benchmarks/core-sp1-6.10.json
A benchmarks/reference/settlement-sp1-6.10.json
A benchmarks/retained.json
A crates/cycle-runner/Cargo.toml
A crates/cycle-runner/src/bin/export-fixture.rs
A crates/cycle-runner/src/bin/operation-bench-runner.rs
A crates/cycle-runner/src/bin/prepare-vesta.rs
A crates/cycle-runner/src/main.rs
M crates/o1-verifier-host/Cargo.toml
M crates/o1-verifier-host/build.rs
M crates/o1-verifier-host/src/main.rs
M crates/o1-verifier/Cargo.toml
A crates/o1-verifier/blob_cache.rs
M crates/o1-verifier/build.rs
A crates/o1-verifier/src/atomic_shims.rs
M crates/o1-verifier/src/main.rs
M crates/pickles-verifier/Cargo.toml
A crates/pickles-verifier/build.rs
A crates/pickles-verifier/src/canonical_coefficients.rs
A crates/pickles-verifier/src/canonical_msm.rs
M crates/pickles-verifier/src/convert.rs
M crates/pickles-verifier/src/deferred.rs
A crates/pickles-verifier/src/fixed_pallas.rs
A crates/pickles-verifier/src/fixed_srs.rs
M crates/pickles-verifier/src/lib.rs
M crates/pickles-verifier/src/serialize.rs
A crates/pickles-verifier/src/setup_data.rs
A crates/pickles-verifier/src/statement.rs
M crates/pickles-verifier/src/types.rs
A crates/pickles-verifier/src/vesta_glv.rs
A crates/pickles-verifier/src/vesta_spikes.rs
M crates/pickles-verifier/src/wire.rs
A crates/pickles-verifier/tests/affine_spikes.rs
A crates/pickles-verifier/tests/b_poly.rs
A crates/pickles-verifier/tests/canonical_ipa.rs
A crates/pickles-verifier/tests/canonical_poseidon.rs
A crates/pickles-verifier/tests/fixed_pallas_prefix.rs
A crates/pickles-verifier/tests/ipa_borrowed_prefix.rs
A crates/pickles-verifier/tests/legacy_sponge_constants.rs
A crates/pickles-verifier/tests/mont_direct_spike.rs
A crates/pickles-verifier/tests/msm_spikes.rs
A crates/pickles-verifier/tests/pallas_field_spikes.rs
A crates/pickles-verifier/tests/pallas_msm_spikes.rs
A crates/pickles-verifier/tests/pallas_spikes.rs
A crates/pickles-verifier/tests/pasta_input_contract.rs
A crates/pickles-verifier/tests/point_spikes.rs
A crates/pickles-verifier/tests/positional_recoding.rs
A crates/pickles-verifier/tests/precompile_spikes.rs
A crates/pickles-verifier/tests/scalar_challenge.rs
A crates/pickles-verifier/tests/sponge_constants.rs
A crates/pickles-verifier/tests/srs_storage.rs
A crates/pickles-verifier/tests/verifier_blob_cache.rs
A dependencies/README.md
A dependencies/bootstrap.py
A dependencies/bundles/algebra.bundle
A dependencies/bundles/proof-systems.bundle
A dependencies/bundles/sp1.bundle
A dependencies/inventory.json
A dependencies/lock.json
A dependencies/verify.py
A docs/dependencies-and-optimizations.md
A docs/file-changes.md
A docs/modules.md
A docs/validation.md
A examples/settlement/Cargo.lock
A examples/settlement/Cargo.toml
A examples/settlement/README.md
A examples/settlement/program/Cargo.toml
A examples/settlement/program/build.rs
A examples/settlement/program/src/lib.rs
A examples/settlement/program/src/main.rs
A examples/settlement/program/src/receipt.rs
A examples/settlement/types/Cargo.toml
A examples/settlement/types/src/lib.rs
A fixtures/ocaml-compat/README.md
A fixtures/ocaml-compat/app_statement.json
A fixtures/ocaml-compat/proof.serde.json
A fixtures/ocaml-compat/public_input_skeleton.json
A fixtures/ocaml-compat/settlement.json
A fixtures/ocaml-compat/statement.json
A fixtures/ocaml-compat/vk.serde.json
M rust-toolchain.toml
A scripts/benchmark.py
A tools/pasta-guest/Cargo.lock
A tools/pasta-guest/Cargo.toml
A tools/pasta-guest/src/main.rs
A tools/pasta-proof/.gitignore
A tools/pasta-proof/Cargo.lock
A tools/pasta-proof/Cargo.toml
A tools/pasta-proof/README.md
A tools/pasta-proof/src/bin/trace-accounting.rs
A tools/pasta-proof/src/compressed_point.rs
A tools/pasta-proof/src/gpu.rs
A tools/pasta-proof/src/input_contract.rs
A tools/pasta-proof/src/instruction_profile.rs
A tools/pasta-proof/src/lib.rs
A tools/pasta-proof/src/main.rs
A tools/pasta-proof/src/operand_profile.rs
A tools/pasta-proof/src/pasta_retention_audit.rs
A tools/pasta-proof/src/poseidon_round.rs
A tools/pasta-proof/src/retention_audit.rs
A tools/pasta-proof/src/run_manifest.rs
A tools/pasta-proof/src/trace_accounting.rs
A tools/pasta-proof/tests/fixtures/public-values.bin
A tools/pasta-proof/tests/fixtures/run-manifest.json
A vendor/algebra
A vendor/proof-systems
A vendor/sp1
```

## algebra — 22 changed files

Location: `vendor/algebra`. Official base: `7ad88c46e859a94ab8e0b19fd8a217c3dc472f1c`. Our pin: `8bcb64d5234ecec84c18524fed907dfc22c81eb6`.

```text
M Cargo.toml
M ec/Cargo.toml
M ec/src/hashing/map_to_curve_hasher.rs
M ec/src/lib.rs
M ec/src/models/short_weierstrass/mod.rs
M ec/src/models/twisted_edwards/mod.rs
M ec/src/pairing.rs
A ec/src/scalar_mul/fixed_base.rs
M ec/src/scalar_mul/glv.rs
M ec/src/scalar_mul/mod.rs
A ec/src/scalar_mul/variable_base/affine_spike.rs
M ec/src/scalar_mul/variable_base/mod.rs
M ec/src/scalar_mul/variable_base/stream_pippenger.rs
M ec/src/scalar_mul/wnaf.rs
M ff/Cargo.toml
M ff/src/fields/arithmetic.rs
M ff/src/fields/mod.rs
M ff/src/fields/models/cubic_extension.rs
M ff/src/fields/models/fp/montgomery_backend.rs
M ff/src/fields/models/fp4.rs
M ff/src/lib.rs
M poly/src/polynomial/univariate/dense.rs
```

## proof-systems — 28 changed files

Location: `vendor/proof-systems`. Official base: `b1d9829fa0b2bb8e15494df1e5e0c6741394075b`. Our pin: `e7f63a64d8dff5d9c9bc6809dbf1e706f61fef6a`.

```text
M Cargo.toml
M curves/Cargo.toml
A curves/src/pasta/canonical_pallas.rs
A curves/src/pasta/canonical_words.rs
M curves/src/pasta/curves/pallas.rs
M curves/src/pasta/mod.rs
A curves/src/pasta/pallas_glv.rs
A curves/src/pasta/point_spikes.rs
A curves/src/pasta/precompile_spikes.rs
M kimchi/src/alphas.rs
M kimchi/src/circuits/constraints.rs
M kimchi/src/circuits/polynomials/permutation.rs
M kimchi/src/verifier.rs
A poly-commitment/src/canonical_ipa.rs
M poly-commitment/src/commitment.rs
A poly-commitment/src/generators.rs
M poly-commitment/src/ipa.rs
M poly-commitment/src/lib.rs
M poly-commitment/src/precomputed_srs.rs
M poly-commitment/src/utils.rs
A poseidon/src/canonical.rs
M poseidon/src/lib.rs
M poseidon/src/pasta/fp_kimchi.rs
M poseidon/src/pasta/fp_legacy.rs
M poseidon/src/pasta/fq_kimchi.rs
M poseidon/src/pasta/fq_legacy.rs
M poseidon/src/permutation.rs
M poseidon/src/sponge.rs
```

## sp1 — 30 changed files

Location: `vendor/sp1`. Official base: `d83ed91f96701209613720708f36ffce0e98ed33`. Our pin: `7d0c4646a47fba76ff4d9aeffee18eb884c4ebc1`.

```text
M crates/core/executor/Cargo.toml
M crates/core/executor/src/air.rs
M crates/core/executor/src/artifacts/rv64im_complexity.json
M crates/core/executor/src/artifacts/rv64im_complexity_mprotect.json
M crates/core/executor/src/artifacts/rv64im_costs.json
M crates/core/executor/src/events/precompiles/mod.rs
M crates/core/executor/src/minimal.rs
M crates/core/executor/src/minimal/arch/portable/mod.rs
M crates/core/executor/src/minimal/ecall.rs
M crates/core/executor/src/record.rs
M crates/core/executor/src/retain.rs
M crates/core/executor/src/syscall_code.rs
M crates/core/executor/src/vm.rs
M crates/core/executor/src/vm/gas.rs
M crates/core/executor/src/vm/syscall.rs
M crates/core/executor/src/vm/syscall/precompiles/weierstrass/add.rs
M crates/core/executor/src/vm/syscall/precompiles/weierstrass/double.rs
M crates/core/machine/Cargo.toml
M crates/core/machine/src/operations/field/field_op.rs
M crates/core/machine/src/riscv/mod.rs
M crates/core/machine/src/syscall/precompiles/weierstrass/weierstrass_add.rs
M crates/core/machine/src/syscall/precompiles/weierstrass/weierstrass_double.rs
M crates/core/machine/src/utils/prove.rs
M crates/core/runner/Cargo.toml
M crates/core/runner/build.rs
M crates/core/runner/src/portable.rs
M crates/curves/Cargo.toml
M crates/curves/src/lib.rs
M crates/curves/src/weierstrass/mod.rs
A crates/curves/src/weierstrass/pasta.rs
```

## Retained benchmark flags and implementation references

Values come from `benchmarks/retained.json`. References include callers/build scripts as well as implementation sites; setting a flag does not establish an independent additive speedup.

| Setting | Value | References |
| --- | --- | --- |

| `ZEKO_APPEND_B_POLY` | `1` | `vendor/proof-systems/poly-commitment/src/commitment.rs:547` |

| `ZEKO_BORROW_PALLAS_SRS` | `1` | `crates/o1-verifier/build.rs:37`; `crates/pickles-verifier/src/lib.rs:727`; `crates/pickles-verifier/src/serialize.rs:491`; `examples/settlement/program/build.rs:38` |

| `ZEKO_BORROW_POSITIONAL_SCALARS` | `1` | `crates/o1-verifier/build.rs:34`; `crates/pickles-verifier/src/canonical_msm.rs:316`; `examples/settlement/program/build.rs:35` |

| `ZEKO_BORROW_VESTA_SRS` | `1` | `crates/pickles-verifier/src/lib.rs:749`; `crates/pickles-verifier/src/serialize.rs:487` |

| `ZEKO_CANONICAL_ACCUMULATOR_COEFFICIENTS` | `1` | `crates/pickles-verifier/src/lib.rs:237` |

| `ZEKO_CANONICAL_MSM` | `1` | `crates/pickles-verifier/src/lib.rs:238,259` |

| `ZEKO_CANONICAL_PALLAS` | `1` | `vendor/proof-systems/curves/src/pasta/curves/pallas.rs:47` |

| `ZEKO_CANONICAL_POSEIDON` | `1` | `vendor/proof-systems/poseidon/src/permutation.rs:151` |

| `ZEKO_DIRECT_MULMOD` | `1` | `crates/pickles-verifier/src/canonical_msm.rs:103`; `vendor/proof-systems/curves/src/pasta/canonical_pallas.rs:106` |

| `ZEKO_FIXED_PALLAS_SRS` | `1` | `crates/o1-verifier/build.rs:36,70`; `crates/pickles-verifier/src/lib.rs:832`; `crates/pickles-verifier/src/serialize.rs:277,490`; `examples/settlement/program/build.rs:37,71` |

| `ZEKO_FIXED_VESTA_COUPLED` | `1` | `crates/o1-verifier/build.rs:33`; `crates/pickles-verifier/src/canonical_msm.rs:311`; `examples/settlement/program/build.rs:34` |

| `ZEKO_FIXED_VESTA_SRS` | `1` | `crates/o1-verifier/build.rs:32,69`; `crates/pickles-verifier/src/lib.rs:692`; `crates/pickles-verifier/src/serialize.rs:273,489`; `examples/settlement/program/build.rs:33,70` |

| `ZEKO_FUSE_PALLAS_BASE_CONVERSION` | `1` | `vendor/proof-systems/curves/src/pasta/canonical_pallas.rs:436` |

| `ZEKO_FUSE_VESTA_BASE_CONVERSION` | `1` | `crates/pickles-verifier/src/canonical_msm.rs:202`; `crates/pickles-verifier/src/lib.rs:239` |

| `ZEKO_HINTED_INVERSE` | `1` | `vendor/proof-systems/curves/src/pasta/precompile_spikes.rs:19` |

| `ZEKO_INLINE_FINITE_COPY` | `words` | `vendor/proof-systems/curves/src/pasta/point_spikes.rs:238` |

| `ZEKO_INTEGER_SCALAR_CHALLENGE` | `1` | `vendor/proof-systems/poseidon/src/sponge.rs:194` |

| `ZEKO_IPA_BORROW_SRS_PREFIX` | `1` | `vendor/proof-systems/poly-commitment/src/ipa.rs:432` |

| `ZEKO_IPA_CANONICAL_PREFIX` | `1` | `vendor/proof-systems/poly-commitment/src/ipa.rs:440` |

| `ZEKO_IPA_DIRECT_COEFFICIENTS` | `1` | `vendor/proof-systems/poly-commitment/src/ipa.rs:508` |

| `ZEKO_IPA_EXACT_CAPACITY` | `1` | `vendor/proof-systems/poly-commitment/src/ipa.rs:415` |

| `ZEKO_MONT_DIRECT_MULMOD` | `1` | `vendor/algebra/ff/src/fields/models/fp/montgomery_backend.rs:64` |

| `ZEKO_MSM_FILTER_ZERO` | `1` | `vendor/algebra/ec/src/scalar_mul/variable_base/mod.rs:188`; `vendor/proof-systems/curves/src/pasta/canonical_pallas.rs:220` |

| `ZEKO_OUTLINE_PALLAS_BUCKETS` | `1` | `vendor/proof-systems/curves/src/pasta/point_spikes.rs:690` |

| `ZEKO_OUTLINE_VESTA_BUCKETS` | `1` | `vendor/proof-systems/curves/src/pasta/point_spikes.rs:689` |

| `ZEKO_PACKED_PALLAS_HALVES` | `1` | `crates/o1-verifier/build.rs:35`; `examples/settlement/program/build.rs:36`; `vendor/proof-systems/curves/src/pasta/canonical_pallas.rs:439` |

| `ZEKO_PALLAS_AFFINE` | `1` | `vendor/proof-systems/curves/src/pasta/canonical_pallas.rs:205` |

| `ZEKO_PALLAS_GLV` | `1` | `vendor/proof-systems/curves/src/pasta/canonical_pallas.rs:206` |

| `ZEKO_PALLAS_POINT_PRECOMPILE` | `1` | `vendor/proof-systems/curves/src/pasta/canonical_pallas.rs:208` |

| `ZEKO_PALLAS_WINDOW` | `13` | `crates/pickles-verifier/src/fixed_pallas.rs:240`; `vendor/proof-systems/curves/src/pasta/canonical_pallas.rs:209` |

| `ZEKO_POSEIDON_PACKED_RHS` | `1` | `vendor/proof-systems/poseidon/src/canonical.rs:134,149` |

| `ZEKO_PRECOMPUTED_SETUP` | `1` | `crates/o1-verifier/build.rs:31,68`; `crates/pickles-verifier/src/serialize.rs:488`; `examples/settlement/program/build.rs:32,69` |

| `ZEKO_PRECOMPUTED_VK_DIGEST` | `1` | `crates/o1-verifier/src/main.rs:30,36`; `examples/settlement/program/src/main.rs:56,62` |

| `ZEKO_TRIM_CONVERSIONS` | `1` | `crates/pickles-verifier/src/canonical_msm.rs:218`; `vendor/proof-systems/curves/src/pasta/canonical_pallas.rs:207` |

| `ZEKO_UNINIT_MSM_DIGITS` | `1` | `vendor/proof-systems/curves/src/pasta/point_spikes.rs:426` |

| `ZEKO_VESTA_AFFINE` | `1` | `crates/pickles-verifier/src/canonical_msm.rs:268`; `crates/pickles-verifier/src/vesta_spikes.rs:58` |

| `ZEKO_VESTA_GLV` | `1` | `crates/pickles-verifier/src/canonical_msm.rs:203,219,228,239`; `crates/pickles-verifier/src/lib.rs:240` |

| `ZEKO_VESTA_POINT_PRECOMPILE` | `1` | `crates/pickles-verifier/src/canonical_msm.rs:204,241,270`; `crates/pickles-verifier/src/lib.rs:241`; `crates/pickles-verifier/src/vesta_spikes.rs:49` |

| `ZEKO_VESTA_WINDOW` | `16` | `crates/pickles-verifier/src/canonical_msm.rs:312`; `crates/pickles-verifier/src/vesta_spikes.rs:11` |

| `ZEKO_WRAP_SRS_PREFIX` | `1` | `crates/o1-verifier/src/main.rs:29`; `examples/settlement/program/src/main.rs:55` |
