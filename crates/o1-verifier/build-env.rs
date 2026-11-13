// Every environment variable that changes what the guest ELF compiles to.
//
// Shared by the guest's own build script and by the host's, because cargo
// reruns a build script only when something it declared has changed. The host
// script is what invokes `sp1_build`, so if it does not declare these, a
// `VK_JSON=... make build-rust` from a warm tree silently rebuilds nothing and
// hands back the previous guest -- exit 0, wrong VK. That cost us three
// confused builds on 2026-11-13.
//
// Generated from `option_env!`/`env::var` sites under `crates/` and the
// `optimization_env` keys of `benchmarks/dev/*.json`. Add new flags here.
pub const GUEST_BUILD_ENV: &[&str] = &[
    "MIZU_BATCH_ACCUMULATOR",
    "MIZU_CIP_MODE",
    "MIZU_PASTA_FP_PRECOMPILE",
    "MIZU_POSEIDON_DEFERRED_MDS",
    "MIZU_POSEIDON_PRECOMPILE",
    "MIZU_STACK_CROSS_FIELD",
    "PASTA_RETAIN",
    "SETTLEMENT_BLOB_CACHE_DIR",
    "SETTLEMENT_REBUILD_VERIFIER_BLOB",
    "SETTLEMENT_VK_JSON",
    "VK_JSON",
    "ZEKO_APPEND_B_POLY",
    "ZEKO_BORROW_PALLAS_SRS",
    "ZEKO_BORROW_POSITIONAL_SCALARS",
    "ZEKO_BORROW_VESTA_SRS",
    "ZEKO_CANONICAL_ACCUMULATOR_COEFFICIENTS",
    "ZEKO_CANONICAL_MSM",
    "ZEKO_CANONICAL_PALLAS",
    "ZEKO_CANONICAL_POSEIDON",
    "ZEKO_DIRECT_MULMOD",
    "ZEKO_FIXED_PALLAS_SRS",
    "ZEKO_FIXED_VESTA_COUPLED",
    "ZEKO_FIXED_VESTA_SRS",
    "ZEKO_FUSE_PALLAS_BASE_CONVERSION",
    "ZEKO_FUSE_VESTA_BASE_CONVERSION",
    "ZEKO_HINTED_INVERSE",
    "ZEKO_INLINE_FINITE_COPY",
    "ZEKO_INTEGER_SCALAR_CHALLENGE",
    "ZEKO_IPA_BORROW_SRS_PREFIX",
    "ZEKO_IPA_CANONICAL_PREFIX",
    "ZEKO_IPA_DIRECT_COEFFICIENTS",
    "ZEKO_IPA_EXACT_CAPACITY",
    "ZEKO_MONT_DIRECT_MULMOD",
    "ZEKO_MSM_FILTER_ZERO",
    "ZEKO_OUTLINE_PALLAS_BUCKETS",
    "ZEKO_OUTLINE_VESTA_BUCKETS",
    "ZEKO_PACKED_PALLAS_HALVES",
    "ZEKO_PALLAS_AFFINE",
    "ZEKO_PALLAS_GLV",
    "ZEKO_PALLAS_POINT_PRECOMPILE",
    "ZEKO_PALLAS_WINDOW",
    "ZEKO_POSEIDON_PACKED_RHS",
    "ZEKO_PRECOMPUTED_SETUP",
    "ZEKO_PRECOMPUTED_VK_DIGEST",
    "ZEKO_PREPARED_VESTA_PATH",
    "ZEKO_TRIM_CONVERSIONS",
    "ZEKO_UNINIT_MSM_DIGITS",
    "ZEKO_VESTA_AFFINE",
    "ZEKO_VESTA_COMPACT",
    "ZEKO_VESTA_GLV",
    "ZEKO_VESTA_POINT_PRECOMPILE",
    "ZEKO_VESTA_PREPARED_LANES",
    "ZEKO_VESTA_WINDOW",
    "ZEKO_WRAP_SRS_PREFIX",
];
