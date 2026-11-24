# Pasta MSM, GLV and memory scheduling

Status: retained shared Pallas/Vesta MSM backend. Current retained windows are
Pallas 13 and Vesta 16, with adaptive behavior for small MSMs. Scalar bounds,
signed-digit carries, exceptional points and fixed-table geometry are part of
correctness, not optional tuning assumptions.

## Work records

| Work | Historical rounds | Recorded outcome |
| --- | --- | --- |
| Canonical coordinates, affine reduction, signed buckets and GLV | [r1–r5](round-index.md#r2) | Early arithmetic baseline; custom Pasta operations then move the r4 combined 1,974,602,537-cycle guest to 621,941,587 with the 16/13 pair. |
| Window-major digits | [r18/r20](round-index.md#r20) | One change: 621,941,587 → 594,350,168. r18's findings and r20's ledger describe the same improvement. |
| Avoid positive-digit point copies | [r21](round-index.md#r21) | 594,350,168 → 545,144,012. Borrow a positive base; materialize negation only where required. |
| Flag-only bucket reset and cached negatives | [r25](round-index.md#r25), [r26](round-index.md#r26) | 536,083,919 → 517,077,466 → 478,237,799. Identity coordinates remain unread until initialized. |
| Hoisted limbs/stride/modulus, indexed comparisons, straddle guard and const recoding | [r27–r31](round-index.md#r27) | 478,237,799 → 427,791,614 across the sequence. The final kernel retains the mechanisms, not every intermediate layout. |
| Bound-derived digit slots, two-limb Vesta GLV, prepared builder, i16 digits and fixed bucket view | [r35–r41](round-index.md#r35) | The A/B/C/D series reduced the fixed 427,791,614 control; r41 corrects carry/bound claims. Earlier Pallas r40 shortening was superseded by r58. |
| Centered Pallas two-limb decomposition | [r58](round-index.md#r58), corrected inventory [r60](round-index.md#r60) | 268,092,468 → 266,961,479. Preserve the mathematical bound; r60 retracts the inferred width-8 cost explanation. |
| Packed Pallas GLV halves | [r92](round-index.md#r92) | 182,338,026 → 179,452,923; related fixed-table/scalar storage work also appears in the IPA/SRS record. |
| Remove digit zero-fill | [r98](round-index.md#r98) | 157,442,577 → 157,095,318, with every consumed slot written before use. |
| Whole-MSM boundaries | [r101](round-index.md#r101) | 157,095,318 → 153,727,101. Keep the combined outlined configuration; separate outline gains are not additive. |
| Copy only known-finite coordinate words | [r111](round-index.md#r111) | 151,858,777 → 142,692,781. Copy eight words and set the finite flag; this is distinct from r97's rejected identity-copy hypothesis. |

These are historical SP1 6.1 receipt-ABI results. Current retained arithmetic
call counts are Pallas add/double 697,988 / 369, Vesta add/double 1,310,683 / 112;
they remain identical after the proof-systems 0.7 migration.

## Implementation and correctness

- [Shared point/MSM kernel](../../vendor/proof-systems/curves/src/pasta/point_spikes.rs).
- [Pallas GLV](../../vendor/proof-systems/curves/src/pasta/pallas_glv.rs) and
  [canonical Pallas adapter](../../vendor/proof-systems/curves/src/pasta/canonical_pallas.rs).
- [Vesta GLV](../../crates/pickles-verifier/src/vesta_glv.rs),
  [Vesta adapter](../../crates/pickles-verifier/src/vesta_spikes.rs), and
  [canonical MSM](../../crates/pickles-verifier/src/canonical_msm.rs).
- [Arkworks MSM](../../vendor/algebra/ec/src/scalar_mul/variable_base/mod.rs) and
  [GLV support](../../vendor/algebra/ec/src/scalar_mul/glv.rs).
- Tests: `point_spikes`, `pallas_spikes`, `pallas_msm_spikes`, `msm_spikes`,
  `positional_recoding`, `fixed_pallas_prefix` and `pasta_input_contract`.

Keep zero/identity/equal/opposite points, signed carry boundaries, shorter-slice
semantics and differential scalar reconstruction covered. For uninitialized
storage, every read must remain dominated by the corresponding write.

## Alternatives and upgrade boundary

Window retuning in r17/r50/r51/r59/r60/r73/r112 established configuration-specific
tradeoffs. Lower stored cells at a smaller window are not evidence of faster
proofs. Epoch buckets (r99/r99b), four-lane Vesta (r100) and sorted chain grouping
(r108) regressed. Their records remain in the [experiment register](08-experiments-and-corrections.md).
Re-measure compiler/layout changes on the combined selected backend, and keep
recoder geometry equivalent before attributing a window comparison.

## Retained controls and commit ownership

| Setting | Current value |
| --- | --- |
| `MIZU_PALLAS_AFFINE` | `1` |
| `MIZU_PALLAS_GLV` | `1` |
| `MIZU_PALLAS_POINT_PRECOMPILE` | `1` |
| `MIZU_PALLAS_WINDOW` | `13` |
| `MIZU_VESTA_AFFINE` | `1` |
| `MIZU_VESTA_GLV` | `1` |
| `MIZU_VESTA_POINT_PRECOMPILE` | `1` |
| `MIZU_VESTA_WINDOW` | `16` |
| `MIZU_MSM_FILTER_ZERO` | `1` |
| `MIZU_INLINE_FINITE_COPY` | `words` |
| `MIZU_UNINIT_MSM_DIGITS` | `1` |
| `MIZU_OUTLINE_PALLAS_BUCKETS` | `1` |
| `MIZU_OUTLINE_VESTA_BUCKETS` | `1` |
| `MIZU_PACKED_PALLAS_HALVES` | `1` |

These controls can compose or select fallback paths. Their grouping assigns a
primary documentation area, not independent additive performance credit. Exact
source references are in [the file appendix](../file-changes.md).

Pinned topic commits (in their respective repositories):

- algebra `712a6ae79` — Add GLV and fixed/variable-base MSM optimizations.
- proof-systems `9f37ee019` — Add canonical Pasta arithmetic, GLV and SP1 point operations.
- proof-systems `49f2237b4` — Add zero-copy finite Pasta MSMs, coupled w=15/14/13 recoders, in-place b_poly, and soft doubling.

Root-specific code stays in the o1js-to-zkvm module series. Current pins and
ordered commits are in [the dependency lock](../../dependencies/lock.json).
Historical evidence and current implementation are connected by this record,
not by preserving the original experimental commit history.

## mizu-finite-srs-coupled-msm: zero-copy finite SRS tables, coupled w=15/14/13 recoders, cold-outlined equal-x, in-place `b_poly`, and soft doubling

- **Status:** candidate measured on CPU execution (zero-copy finite tables, cold-outlined `equal-x`, unrolled single-slice recoders, coupled Pallas `w=13`, in-place `b_poly`, and `MIZU_SOFT_PASTA_DOUBLE=1` all win; intermediate `u64` occupancy bitset + `#[inline(always)]` cold path + stack `planes` fat-pointer array **rejected**).
- **Area and owner:** root `crates/pickles-verifier` (`fixed_srs.rs`, `fixed_pallas.rs`, `canonical_msm.rs`, `canonical_coefficients.rs`, `accumulator_batch.rs`, `serialize.rs`, `lib.rs`), `crates/o1-verifier/build-env.rs`, and `vendor/proof-systems` (`curves/src/pasta/{point_spikes.rs,canonical_pallas.rs,canonical_words.rs}`, `poly-commitment/src/canonical_ipa.rs`).
- **Implementation identity:** root `68e4cb5` (pin bump `f97cbbf`), `proof-systems` `49f2237b4` (base `b1d9829fa`), `algebra` `f01c749` (base `7ad88c46e`), `sp1` `91c73fdd9` (base `d83ed91f9`).
- **Predecessors/corrections:** extends `r92`/`r98`/`r101`/`r111` and `mizu-pasta-poseidon-precompile` (130,367,324 cycles). Rejects the initial `u64` occupancy bitset bucket layout (measured 161,704,216 cycles at `w=15` because >99.97% of buckets are non-empty at `w=15`/`w=13`, so bit-shifting/masking/bounds checks plus `#[inline(always)]` on the cold `equal-x` path and 16–20-element `planes` slice arrays regressed by +31.3M cycles).
- **Mechanism:**
  1. Pack all-finite fixed Vesta/Pallas SRS tables as 64-byte `[u64; 8]` coordinates in `.rodata` (encoding `all_finite` + `lane_shift` in header word `16`, keeping `HEADER == 88`), borrowing `&[[u64; 8]]` directly with zero heap copy of `points` and skipping unused Montgomery `VestaSrs` / Lagrange tail scanning in `serialize.rs`.
  2. Add coupled finite-borrowed Vesta MSM (`msm_vesta_finite_borrowed`) supporting `(w=16, shift=128)`, `(w=15, shift=120)`, and `(w=14, shift=126)`, and zero-copy finite-prefix Pallas MSM (`msm_pallas_finite_prefix` / `msm_bigint_fixed_prefix_coords`) supporting coupled `(w=13, shift=130)` (`MIZU_FIXED_PALLAS_COUPLED=1`) and uncoupled `shift=128`.
  3. Cold-outline `add_finite_coords_equal_x` (`#[cold] #[inline(never)]`) so the hot bucket-addition loop contains only a single 64-bit `lhs[0] == rhs[0]` compare plus `ecall`, keep single-pointer `[Point; BUCKETS]` with sequential reverse reduction, outline Pallas finite bucket accumulation (`#[inline(never)]`), and unroll single-slice recoders with branchless bitwise-OR `w_nonzero` tracking.
  4. Expand `b_poly` in-place in `canonical_coefficients.rs` and `canonical_ipa.rs`, and add opt-in software affine doubling (`MIZU_SOFT_PASTA_DOUBLE=1`) to eliminate the 481 `VestaDouble`/`PallasDouble` syscalls and their `WeierstrassDoubleAssign` AIR shards.
- **Selection:** `benchmarks/dev/pasta-poseidon.json` optimization env plus `MIZU_FIXED_PALLAS_COUPLED=1`, `MIZU_SOFT_PASTA_DOUBLE=1`, and `MIZU_VESTA_WINDOW=16` (or `15` / `14`). Tables containing infinity (synthetic tests) automatically fall back to the 72-byte `General` layout with `lane_shift = 128`.
- **Control/candidate:** CPU execution only (`cycle-runner`, SP1 `6.10.0`, `fixtures/mainnet-blockchain-snark`, boolean ABI, output SHA-256 `4bf5122f344554c53bde2ebb8cd2b7e3d1600ad631c385a5d7cce23c7785459a`, no proof generated). Control: `benchmarks/runs/pasta-poseidon-full/candidate.json` (`130,367,324` cycles).
- **Result:**
  - **`w=16` (`lane_shift=128`, `MIZU_FIXED_PALLAS_COUPLED=1`, `MIZU_SOFT_PASTA_DOUBLE=1`):** `130,367,324 → 102,661,283` cycles (**-27,706,041 / -21.25%**). Stage breakdown: `pickles-accumulator` `75,046,840 → 58,314,372` (-22.30%), `pickles-wrap-kimchi` `48,960,261 → 37,823,150` (-22.75%), `pickles-wrap-prepare` `2,642,383 → 2,641,832`, `setup` `3,540,439 → 3,704,582` (+164,143 from `PodPallas.infinity <= 1` validation). Syscalls: `vesta_add` `1,310,683`, `pallas_add` `699,945`, `vesta_double` `0` (was 112), `pallas_double` `0` (was 369).
  - **`w=15` (`lane_shift=120`, `MIZU_FIXED_PALLAS_COUPLED=1`, `MIZU_SOFT_PASTA_DOUBLE=1`):** `130,367,324 → 105,862,509` cycles (**-24,504,815 / -18.80%**). Stage breakdown: `pickles-accumulator` `61,515,596` (-18.03%), `pickles-wrap-kimchi` `37,823,150` (-22.75%). Syscalls: `vesta_add` `1,261,505` (**-49,178 / -3.75%**), `pallas_add` `699,945`, `vesta_double` `0`, `pallas_double` `0`, with half the Vesta bucket memory (1.18 MB vs 2.36 MB).
  - **Rejected intermediate bitset draft (`w=15`):** `161,704,216` cycles (+31.34M vs baseline) — replaced by single-pointer `[Point; BUCKETS]` + cold-outlined `equal-x` + unrolled single-slice recoders (-55.84M recovery).
- **Correctness evidence:** Full flag-enabled `pickles-verifier --lib` suite (114/114 passed at `w=15` and `w=14` on real Pickles fixtures), `canonical_words_fuzz` (6/6 passed including coupled finite Vesta/Pallas MSMs and `double_software`), `verifier-fuzz` (`V1b`+`V2`+`V3`+`V4` differential against `EmbeddedVerifier`), `no_std` build check, and honest guest CPU execution (`accepted: true`, matching `expected_public_values_sha256`).
- **Validation scope:** Native unit/fuzz tests, `no_std` check, RV64IM guest compilation, and CPU execution verified. Proving/GPU wall-time impact (especially `w=16` vs `w=15` cell/shard tradeoff) remains to be measured on a proving host.
- **Decision and limits:** Both `w=16` (lowest RV64IM cycles) and `w=15` (lowest `VestaAddAssign` count and half Vesta bucket RAM) are kept selectable via `MIZU_VESTA_WINDOW`; `benchmarks/dev/pasta-poseidon.json` has not been overwritten yet pending GPU/proving-host confirmation.

## mizu-prebake-negatives-and-unchecked-msm: pre-baked `-G` in `.rodata`, unchecked recoder/bucket writes, and dead `p_eval0_chunks` removal

- **Status:** candidate measured on CPU execution (`84.99M` cycles at `w=16`, `85.16M` at `w=15`).
- **Area and owner:** root `crates/pickles-verifier` (`fixed_srs.rs`, `fixed_pallas.rs`, `canonical_msm.rs`, `types.rs`, `convert.rs`, `wire.rs`), `crates/o1-verifier/build-env.rs`, `crates/cycle-runner/src/bin/verifier-fuzz.rs`, and `vendor/proof-systems` (`curves/src/pasta/{point_spikes.rs,canonical_pallas.rs}`).
- **Predecessors/corrections:** extends `mizu-finite-srs-coupled-msm` (`102,664,876` cycles at `w=16` / `105,862,509` at `w=15`).
- **Mechanism:**
  1. `MIZU_PREBAKE_NEGATIVES=1` encodes `[plane0, plane1, -plane0, -plane1]` (`flags = 3` in header word `16`) in `.rodata` for all-finite fixed Vesta/Pallas SRS tables, borrowing `finite_negatives()` directly to eliminate 12.6 MB of heap `negatives` allocation/writes (`1.57M` heap stores) and `196,608` runtime `negate_coords_with` calls.
  2. Eliminates `buckets[bucket]` bounds checks inside the private `accumulate_finite_slice<const BUCKETS: usize>` (digits guaranteed in `1..=BUCKETS` by audited recoders) and `digits[slot]` bounds checks in `recode_vesta_canonical`, `recode_vesta_w15_coupled`, `recode_vesta_w14_coupled`, and `recode_pallas_w13_coupled` after upfront length assertions (`1.70M+` bounds checks removed).
  3. Drops dead `p_eval0_chunks` from `VerifiableProof` and `OcamlProof` (ABI cleanup; `IGNORED_FIELDS` in `verifier-fuzz` is now empty).
- **Selection:** `benchmarks/dev/prebake-negatives-w16.json` or `prebake-negatives-w15.json` (`MIZU_PREBAKE_NEGATIVES=1`, `MIZU_FIXED_PALLAS_COUPLED=1`, `MIZU_SOFT_PASTA_DOUBLE=1`, `MIZU_VESTA_WINDOW=16|15`).
- **Control/candidate:** CPU execution (`cycle-runner`, SP1 `6.10.0`, `fixtures/mainnet-blockchain-snark`, boolean ABI, `public_values_hex: "01"`, no proof generated).
- **Result:**
  - **`w=16` (`MIZU_PREBAKE_NEGATIVES=1`):** `130,367,324 → 84,985,703` cycles (**-45,381,621 / -34.81%** vs baseline; **-17,679,173 / -17.22%** vs `102.66M`). Stage breakdown: `pickles-accumulator` `48,745,649` (-35.05%), `pickles-wrap-kimchi` `29,714,257` (-39.31%), `pickles-wrap-prepare` `2,645,381`, `setup` `3,703,223`. Syscalls: `vesta_add` `1,310,683`, `pallas_add` `699,945`, `vesta_double` `0`, `pallas_double` `0`.
  - **`w=15` (`MIZU_PREBAKE_NEGATIVES=1`):** `130,367,324 → 85,156,453` cycles (**-45,210,871 / -34.68%** vs baseline; only `+170,750` / `+0.20%` over `w=16` while cutting **-49,178 `vesta_add`** calls and halving Vesta bucket RAM).
- **Correctness evidence:** `cycle-runner` honest execution (`accepted: true` on both `w=16` and `w=15`), 33/33 `pickles-verifier` native tests (`fixed_srs`, `fixed_pallas`, `wire::tests`, `verify_accepts_fixtures`) with `MIZU_PREBAKE_NEGATIVES=1`, `no_std` check, and `verifier-fuzz` build check.
- **Decision and limits:** Adding 12.6 MB of pre-baked `-G` grows the guest ELF from 24 MB to 36 MB (crossing `2^22` words = 32 MiB) because the embedded blob still carries ~5.9 MB of unused legacy Montgomery `VestaSrs` (4.72 MB) + Lagrange tail (1.18 MB). Stripping those dead sections is a clean follow-up to stay `< 32 MiB` (`block3.bin` height). Full CPU gate and GPU proving runs remain next.
