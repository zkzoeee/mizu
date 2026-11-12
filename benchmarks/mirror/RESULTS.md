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

## Both precompiles on: Poseidon collapses and the mirror becomes the expensive half

`MIZU_POSEIDON_PRECOMPILE=1` on top of `MIZU_PASTA_FP_PRECOMPILE=1`
(`benchmarks/dev/pasta-poseidon.json`) routes each kimchi permutation to the
fork's `PALLAS_POSEIDON` / `VESTA_POSEIDON` syscall, documented at
[04-poseidon.md](../../docs/optimizations/04-poseidon.md#mizu-pasta-poseidon-precompile-the-kimchi-permutation-as-one-syscall).
Per account, depth 35, 86 Poseidon and 76 keccak permutations:

| build | Poseidon | c/perm | mirror | c/perm | total |
| --- | ---: | ---: | ---: | ---: | ---: |
| canonical Poseidon + `sha3` keccak | 4,134,419 | 48,075 | 498,358 | 6,557 | 4,642,320 |
| canonical Poseidon + `KECCAK_PERMUTE` | 4,134,421 | 48,075 | 85,793 | 1,129 | 4,229,756 |
| `PALLAS_POSEIDON` + `KECCAK_PERMUTE` | **34,187** | **398** | 85,792 | 1,129 | **129,517** |

The Poseidon stage falls **120.9x**, from 48,075 to **398 cycles per
permutation**, and `UINT256_MUL` calls go from 100,046 to **zero**. The whole
per-account diff proof falls 35.8x, 4,642,320 to 129,517.

**The ranking inverts a third time.** With both precompiles the keccak mirror
costs **+251% of the Poseidon ledger it mirrors** — 85,792 against 34,187 — and
a keccak permutation now costs **2.8x** a Poseidon one, where it started at
one-seventh. Every statement of the form "the mirror is cheap" or "Poseidon
dominates" in this document is conditional on which precompiles are enabled,
and nothing else.

Scaling, per the 142,638,298-cycle retained guest: at 129,517 cycles/account a
diff proof is +0.9% of the guest at 10 accounts, +9.1% at 100 and +90.8% at
1000 — against +35%, +346% and +3463% in the software-Poseidon table above.

**These cycle figures are no longer a proxy for cost, and at 398 they are
barely a measurement of work.** A syscall is one `ecall` plus its memory
accesses; the permutation's real cost moved into the AIR. 04-poseidon.md
records the chips at 55 round rows per permutation of width 2,048, about
113,000 main cells each, so the 86 permutations per account carry roughly 9.7 M
main cells that the cycle count does not see. On the real guest the precompile
still won on cells (−5.22%) as well as cycles (−6.62%), so the direction holds
there; but the Poseidon-to-keccak *ratio* in cells is not 398:1,129, and the
"mirror is now the expensive half" conclusion must be re-checked in cells
before anyone designs around it.

The runner reports `pasta_syscall_counts` as zero in this build: that counter
covers the curve syscalls, not `PALLAS_POSEIDON`. The evidence the syscall is
taken is the 120.9x drop, `UINT256_MUL` falling to zero, and the roots still
matching the host's native reference — which they can only do if the
permutations actually ran.

## Trace cells, which disagree with cycles about which half is expensive

Widths read from the fork's own machine by `crates/cycle-runner/src/bin/chip-widths.rs`,
using the same decomposition the project's trace accounting uses
(`main_width = cost() - preprocessed_width()`). Rows per permutation come from
the chips: keccak's trace generator sets `nb_rows = events * NUM_ROUNDS` with
`NUM_ROUNDS = 24`, and the Pasta Poseidon controller anchors 55 round rows.

| chip | main width | rows/perm | main cells/perm |
| --- | ---: | ---: | ---: |
| `KeccakPermute` | 2,640 | 24 | **63,360** |
| `PallasPoseidonPermute` | 2,048 | 55 (+1 controller, 507) | **113,147** |

`KeccakPermute` at 2,640 is the widest chip in the whole machine; the Poseidon
chips at 2,048 match 04-poseidon.md's recorded width exactly, and the formula
reproduces its published total (12,608 x 2,048 + 256 x 507 = 25,950,976 against
"25.95 M main cells"), so the method is the project's own, not a new one.

**The two metrics disagree in direction.**

| per permutation | Poseidon | keccak | |
| --- | ---: | ---: | --- |
| cycles | 398 | 1,129 | keccak **2.84x** worse |
| main cells | 113,147 | 63,360 | Poseidon **1.79x** worse |

A Poseidon syscall does 55 rounds behind one `ecall`: cheap in cycles, wide and
tall in rows. Keccak's 24 rounds are fewer rows but the widest in the machine.
Cycles count the `ecall`; cells count the AIR. Per account at depth 35:

| | Poseidon ledger | keccak mirror | mirror's share |
| --- | ---: | ---: | ---: |
| cycles | 34,187 | 85,792 | +251% |
| main cells | 9,730,642 | 4,815,360 | **+49.5%** |

**In the metric that actually drives proving cost, the mirror is the cheap half
again and Poseidon is still the expensive one.** Shard area, not cycle count,
is what the prover pays for. Against the retained guest's 43,922,703,280 main
cells over 82 shards (535,642,723 per shard), a diff proof adds +0.3% of the
guest's cells at 10 accounts, **+3.3% at 100** (~2.7 extra shards) and +33.1%
at 1000 (~27 shards) — much friendlier than the cycles reading suggested, and
the diff proof is less cell-dense per cycle than the verifier it rides along
with (112 against 308 cells/cycle).

The published Poseidon precompile figures give the software-to-precompile cell
win directly: 229 permutations moved saved 2,212,937,712 main cells, so
**9,663,483 software cells per permutation against 113,147 precompiled, 85.4x
fewer** — the precompile is an 85x cell win as well as a 121x cycle win. There
is no equivalent measured anchor for software keccak's cells, so no comparable
figure is claimed for it.

Caveats that stand. Main cells are prover inputs, not proving time: the run
manifest says so in as many words (`main_cells_are_prover_inputs_not_proving_time`).
Shard count depends on the stacking height and row-count limits, not on a
simple division, so the shard numbers above are indicative. Cells here are
derived from widths and known rows per permutation, not read from generated
records, so they omit the CPU-side chips the schedule still touches around the
syscalls, and they omit interaction events entirely.

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
