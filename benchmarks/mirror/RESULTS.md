# Mirror-root cycles, measured on CPU

Branch experiment: what a keccak mirror of the Poseidon ledger costs in SP1
guest cycles, for five mirror constructions, at the Zeko ledger's depth 35,
with keccak as software and as SP1's `KECCAK_PERMUTE` precompile.
CPU only — `MinimalExecutorRunner`, no prover, no GPU, no proof generated.

- Guest: `crates/poseidon-bench/src/mirror.rs` (`mirror-operation-program`)
- Runner: `crates/cycle-runner/src/bin/mirror-bench-runner.rs`
- Every run recomputes the identical schedule natively and is rejected unless
  the guest's committed roots match. All eleven runs here were accepted.

## Per-permutation cost, which is the whole story

| | cycles/permutation | source |
| --- | ---: | --- |
| Poseidon, canonical (retained flags) | **48,075** | `opt-n1-mode0.json`, 86 permutations |
| Keccak, `sha3` software | **6,563** | `sw-n1-mode1.json`, 76 permutations |
| Keccak, SP1 `KECCAK_PERMUTE` precompile | **1,129** | `pc-n1-mode1.json`, 76 permutations |

The precompile is **5.8x** cheaper than software keccak, and Poseidon costs
**42.6x** a precompiled keccak permutation (7.3x a software one). The 48,075 figure is within 1.7%
of the 48,921 in [the Poseidon record](../../docs/optimizations/04-poseidon.md)
(1,565,472 cycles / 32 permutations), measured from a different guest, so the
two agree independently.

## What the mirror costs, one account, depth 35

Baseline (mode 0, Poseidon only): **4,143,999** cycles, of which 4,134,419 is
the Poseidon stage.

| Mirror | mirror cycles | keccak perms | on top of Poseidon | what Ethereum can do with it |
| --- | ---: | ---: | ---: | --- |
| none | — | 0 | — | nothing |
| full keccak Merkle mirror | 498,794 | 76 | **+12.04%** | prove inclusion of any account |
| bucketed, top 16 levels | 259,360 | 38 | **+6.26%** | prove inclusion down to a bucket |
| keccak hash chain | 45,043 | 7 | **+1.09%** | continuity only |
| one keccak over the diff list | 45,057 | 7 | **+1.09%** | continuity only |

Linear in the number of accounts, to within a rounding error: at 16 accounts
the full mirror costs 498,712 cycles/account (+12.09%) against 498,794
(+12.04%) at one, and the diff-list mode falls to +1.02% as its final
permutation amortizes.

`uint256_mul_count` is identical across every mode at a given N, so keccak
contributes no field multiplications at all: the 100,046 muls per account are
all Poseidon, about 1,163 per permutation. Pasta syscall counts are zero
throughout — this guest exercises no Pallas/Vesta precompile.

## With the precompile patched in, the full mirror is nearly free

`tiny-keccak` from `sp1-patches` (tag `patch-2.0.2-sp1-6.10.0`, locked at
`8fca2536`), reached through the root `[patch.crates-io]`, behind the guest's
`keccak-precompile` feature. Identical guest source either way: one `kc::H`
wrapper with two `#[cfg]` bodies and `update(impl AsRef<[u8]>)`, so no call
site changes. **Every precompile run was accepted against the host's
independent `sha3` reference**, so the two implementations agree byte for byte
on all four mirror constructions — a cross-implementation check, not just a
cycle count.

| Mirror | software | precompile | speedup | share, software | share, precompile |
| --- | ---: | ---: | ---: | ---: | ---: |
| full keccak Merkle mirror | 498,358 | **85,793** | 5.81x | +12.05% | **+2.08%** |
| bucketed, top 16 levels | 259,695 | 46,922 | 5.53x | +6.28% | +1.13% |
| keccak hash chain | 45,768 | 10,734 | 4.26x | +1.11% | +0.26% |
| one keccak over the diff list | 45,057 | 10,457 | 4.31x | +1.09% | +0.25% |

Still linear: 85,716 cycles/account at 16 accounts against 85,793 at one.

**This changes the design conclusion.** On software keccak the degraded mirrors
looked attractive — +1.1% against +12.1% is worth arguing about. With the
precompile the full Merkle mirror costs **+2.08%**, and the variants that give
up account-level inclusion proofs save only 1.8 percentage points. The mirror
that actually lets Ethereum verify a withdrawal is the one to build; bucketing
and hash-chaining are no longer worth their loss of function.

The speedup shrinks as the construction gets smaller (5.81x on the full tree,
4.31x on the diff list) because the fixed per-call cost around the syscall is a
larger share of a 7-permutation schedule than of a 76-permutation one.

## Optimizing Poseidon raises the mirror's share, measured

The same 498,794 mirror cycles against three Poseidon builds of the same guest:

| Poseidon build | cycles/perm | `UINT256_MUL` | full mirror's share |
| --- | ---: | ---: | ---: |
| no optimization flags | 136,311 | 170,306 | +4.26% |
| the 40 shared recipe flags (`poseidon-reference`) | 69,612 | 170,306 | +8.33% |
| plus the 2 Poseidon flags (`poseidon-canonical`) | **48,075** | 100,046 | **+12.06%** |

`ZEKO_CANONICAL_POSEIDON` and `ZEKO_POSEIDON_PACKED_RHS` are worth **−30.9%**
of the Poseidon stage on their own and **−41.3%** of the multiplications. The
other 40 flags are worth more still, **−48.9%**, and they change the cycle count
without changing the multiplication count at all — their win is in the work
around the multiplications, not in the count of them.

Two consequences. First, making Poseidon faster makes the mirror relatively
more expensive, not less: the identical keccak work goes from +4.26% to +12.06%
as Poseidon improves. Second, **mizu's independently measured 133,294
cycles/permutation matches the no-flags build here (136,311, 2.2% apart), not
the retained one** — that measurement was of an unoptimized Poseidon, which
reconciles the figures the two projects were carrying.

## What this does not establish

- **The `kc::H` refactor is neutral but not exactly free**: the software
  figures moved −0.087% on the full mirror, +0.129% bucketed and +1.61% on the
  hash chain, the last being ~700 cycles on a small schedule. Figures in the
  sections above the precompile one predate the wrapper; the comparison table
  uses post-wrapper software numbers on both sides.
- **The precompile's syscall count is not recorded.** The runner reports
  `uint256_mul_count` and Pasta syscalls, not `KECCAK_PERMUTE` calls, so the
  1,129 cycles/permutation is derived from the schedule's known permutation
  count rather than read from a counter.
- **Cycles are not proving cost.** No trace cells, no shard count, no proving
  time. The wazabi work found precompile memory interaction at 42.7% of trace
  cells, and the `deferred-mds` candidate is +32.6% cycles against −1.20%
  cells, so the two objectives already disagree in this codebase.
- **No root here is a real Zeko root.** The schedule's permutation counts match
  Mina's (`hash_with_prefix(MinaMklTree{height}, [left, right])` is one
  permutation per node, its per-height prefix folded into a precomputed initial
  state) but the bytes are not Mina's. This measures the hash schedule, not the
  ledger.
- **Leaf shape is assumed**: 16 field elements and 320 bytes per account
  record, giving 8 Poseidon and 3 keccak permutations per leaf hash. The path
  work dominates, so the headline percentages move little with it, but they are
  not derived from a real Zeko account.
- The diff-proof soundness argument needs **strictly increasing** indices, not
  merely sorted: a repeated account's second witness carries a path that is
  stale with respect to the first update.
