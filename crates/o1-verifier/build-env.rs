// Every environment variable that changes what the guest ELF compiles to.
//
// Shared by the guest's own build script and by the host's, because cargo
// reruns a build script only when something it declared has changed. The host
// script is what invokes `sp1_build`, so if it does not declare these, a
// `VK_JSON=... make build-rust` from a warm tree silently rebuilds nothing and
// hands back the previous guest -- exit 0, wrong VK. That cost us three
// confused builds on 2026-11-13.
//
// Generated from `option_env!`/`env::var` sites under `crates/`, **and under
// `vendor/proof-systems` and `vendor/algebra`**, plus the `optimization_env`
// keys of `benchmarks/dev/*.json`. Add new flags here.
//
// The vendored trees were missing from that scan until 2026-11-14, which left
// four live `option_env!` flags undeclared -- the same silent-stale-guest hole
// this list exists to close. When adding a flag, grep all three trees:
//   grep -rhoE 'ZEKO_[A-Z_]+|MIZU_[A-Z_]+' crates vendor/proof-systems vendor/algebra --include='*.rs'
// and diff against this list. `ZEKO_VERIFIER_BLOB_CACHE_V1` is NOT a variable:
// it is a cache-version string constant in `crates/o1-verifier/blob_cache.rs`.
pub const GUEST_BUILD_ENV: &[&str] = &[
    "MIZU_BATCH_ACCUMULATOR",
    "MIZU_BATCHED_BUCKET_REDUCE",
    "MIZU_BATCHED_MSM_ACCUM",
    "MIZU_CIP_MODE",
    "MIZU_PASTA_FP_PRECOMPILE",
    "MIZU_POSEIDON_DEFERRED_MDS",
    "MIZU_POSEIDON_PRECOMPILE",
    "MIZU_SOFT_PASTA_DOUBLE",
    "MIZU_STACK_CROSS_FIELD",
    "SETTLEMENT_BLOB_CACHE_DIR",
    "SETTLEMENT_REBUILD_VERIFIER_BLOB",
    "SETTLEMENT_VK_JSON",
    "VK_JSON",
    "MIZU_APPEND_B_POLY",
    "MIZU_BORROW_PALLAS_SRS",
    "MIZU_BORROW_POSITIONAL_SCALARS",
    "MIZU_BORROW_VESTA_SRS",
    "MIZU_CANONICAL_ACCUMULATOR_COEFFICIENTS",
    "MIZU_CANONICAL_MSM",
    "MIZU_CANONICAL_PALLAS",
    "MIZU_CANONICAL_POSEIDON",
    "MIZU_COMPACT_EMBEDDED_BLOB",
    "MIZU_DIRECT_MULMOD",
    "MIZU_FIXED_PALLAS_COUPLED",
    "MIZU_FIXED_PALLAS_SRS",
    "MIZU_FIXED_VESTA_COUPLED",
    "MIZU_FIXED_VESTA_SRS",
    "MIZU_FUSE_PALLAS_BASE_CONVERSION",
    "MIZU_FUSE_VESTA_BASE_CONVERSION",
    "MIZU_HINTED_INVERSE",
    "MIZU_INLINE_FINITE_COPY",
    "MIZU_INTEGER_SCALAR_CHALLENGE",
    "MIZU_INVERSE_HINT_FAULT",
    "MIZU_IPA_BORROW_SRS_PREFIX",
    "MIZU_IPA_CANONICAL_PREFIX",
    "MIZU_IPA_CAPACITY_ASSERT",
    "MIZU_IPA_CAPACITY_REPORT",
    "MIZU_IPA_DIRECT_COEFFICIENTS",
    "MIZU_IPA_EXACT_CAPACITY",
    "MIZU_MONT_DIRECT_MULMOD",
    "MIZU_MSM_FILTER_ZERO",
    "MIZU_MSM_WINDOW_BITS",
    "MIZU_OUTLINE_PALLAS_BUCKETS",
    "MIZU_OUTLINE_VESTA_BUCKETS",
    "MIZU_PACKED_PALLAS_HALVES",
    "MIZU_PALLAS_AFFINE",
    "MIZU_PALLAS_GLV",
    "MIZU_PALLAS_POINT_PRECOMPILE",
    "MIZU_PALLAS_WINDOW",
    "MIZU_POSEIDON_PACKED_RHS",
    "MIZU_PREBAKE_NEGATIVES",
    "MIZU_PRECOMPUTED_SETUP",
    "MIZU_PRECOMPUTED_VK_DIGEST",
    "MIZU_PREPARED_VESTA_PATH",
    "MIZU_TRIM_CONVERSIONS",
    "MIZU_UNINIT_MSM_DIGITS",
    "MIZU_VESTA_AFFINE",
    "MIZU_VESTA_COMPACT",
    "MIZU_VESTA_GLV",
    "MIZU_VESTA_POINT_PRECOMPILE",
    "MIZU_VESTA_PREPARED_LANES",
    "MIZU_VESTA_WINDOW",
    "MIZU_WRAP_SRS_PREFIX",
];
