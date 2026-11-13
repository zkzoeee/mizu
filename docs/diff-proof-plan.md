# Diff-proof plan: three changes, two owners

Plan for reducing the dual-tree diff proof — a Poseidon ledger walk plus a
keccak mirror — from its measured baseline. Written against the measurements on
branch `codex/mirror-root-cycles` (`benchmarks/mirror/RESULTS.md`, commits
`cc8e3cd` → `d8530be`).

## The target

| change | lives in | cells/account | saving |
| --- | --- | ---: | ---: |
| baseline | — | 26,671,788 | — |
| `H_p` as the keccak leaf | **mizu's SP1 guest** | 26,118,893 | 2.1% |
| + sorted-prefix sharing, N=100 | **mizu's SP1 guest** | 22,436,733 | 15.9% |
| + diff-set commitment | **miza's transition circuit** | 9,696,716 | 63.6% |

The first two need nothing from miza. They are changes to the guest that walks
the tree, so they can land independently of anything on the chain side.

**Only the baseline is measured.** 26,671,788 cells and 111,592 cycles per
account are marginal figures from generated records at depth 35 with both
precompiles, taken as `(N=16 − N=1)/15`. The other three rows are **projections**
from measured per-permutation costs — Poseidon 187,986 cells/permutation, keccak
138,224 — applied to permutation counts each change implies. Treat them as
sizing, not results, and re-measure each one as it lands. The harness for that
is already on the branch and described under *Validation* below.

## Decision 2026-11-14: the full keccak Merkle mirror

**Mode 1, the full keccak Merkle mirror, is the chosen construction** — the one
that lets Ethereum prove inclusion of any account.

### Correction: this plan's baseline was already the right build

An earlier version of this section claimed the target table was measured on the
canonical build and that the precompiled costs reordered the plan. **That was
wrong.** The baseline is the fully precompiled build, and the marginal figures
reproduce exactly from the committed records:

```
records-pp-default-{n1,n16}-mode1.json, ELF f6300e05 (the one we prove with)
  (437,445,632 - 37,368,816) / 15 = 26,671,788 cells/account
  (  1,803,397 -    129,517) / 15 =    111,592 cycles/account
```

which are the baseline row's numbers to the digit. The plan's ordering of its
three changes stands as written. The error came from reading `RESULTS.md`'s
per-permutation table — which spans four builds — without checking which build
the plan had taken.

### What the decomposition does add

`RESULTS.md` and the baseline give totals; the stage split on the chosen build
makes the balance explicit. One account, depth 35, mode 1, `pp-n1-mode1.json`:

| stage | cycles | share |
| --- | ---: | ---: |
| poseidon-ledger | 34,187 | 26% |
| keccak-mirror | **85,792** | **74%** |

Per permutation on this build, Poseidon is **397** cycles and keccak **1,129**,
so **a keccak node costs 2.8 Poseidon nodes** — the reverse of the canonical
build, where Poseidon cost 7.3x a software keccak. The mirror is the dominant
term, which is worth stating plainly next to a decision to keep the most
expensive mirror construction.

It also sharpens where effort goes, without changing the plan's order. Change 2
scales down the ~70 node hashes per account per tree that are most of that 74%,
and applies to both trees. Change 1's two saved permutations are ~2.6% of the
mirror, so take it for the structural leaf-swap property its section argues
rather than for cycles. Change 3 remains the largest number on the table and
remains in tension with this decision, since a diff-set commitment removes the
per-account walks the full mirror exists to provide.

### Build choice is worth 7.7x

The same records show what the configuration is worth, marginal per account:

| build | cells/account | cycles/account |
| --- | ---: | ---: |
| `pp` (Poseidon + keccak precompiles) | **26,671,788** | **111,592** |
| `pc-retained` (keccak precompile, canonical Poseidon) | 205,668,094 | 4,210,680 |

So the plan's choice of build is not incidental: it is 7.7x the cells and 37.7x
the cycles.

### Proved, not only executed

`benchmarks/pipeline/mirror-proof-20261114/` — this exact configuration now has
a Groth16 proof: 5 shards, preprocessed block 1, an 11-key map, 71 s end to end,
verified on an EVM. Every one of the eleven runs behind `RESULTS.md` has
`proof_generated: false`.

## Change 1 — `H_p` as the keccak leaf

**What.** Replace the keccak leaf `keccak(account_record)` with
`keccak(H_p ‖ evm_fields)`, where `H_p` is the Poseidon leaf hash the ledger
walk already computed and `evm_fields` are the words the destination contract
pays out against.

**Where.** mizu's guest, at the two keccak leaf-hash sites. In the experiment
these are `keccak_leaf` in `crates/poseidon-bench/src/mirror.rs`; in production
they are wherever the mirror's leaves are built.

**Why it is worth more than 2.1%.** The keccak leaf's preimage *contains* the
Poseidon leaf, so for any keccak leaf there is exactly one Poseidon leaf it can
correspond to. The "leaves cannot be swapped" property stops being an invariant
the guest maintains and becomes a property of the leaf format. The 320-byte
record hash also drops from 3 permutations to 1, which is the 2.1%.

**What it does not remove.** The guest must still ensure `evm_fields` are the
same values that sit in the record `H_p` hashed. That obligation shrinks from
"two full encoders over the whole record, which can drift on field order,
padding, endianness and inclusion" to "a two-or-three-value projection", but it
does not vanish. Add a test that decodes a record, builds both leaves, and
asserts the projection matches.

**Encoding.** Use Zeko's convention for `H_p` — `into_bigint().to_bytes_be()`,
32 bytes big-endian right-aligned, as `receipt.rs:973` does. Do not invent one.
The design doc's own warning applies: little-endian versus big-endian "silently
produces a different root for identical actions". No on-chain range check is
needed, because the contract treats `H_p` as 32 opaque bytes and never
interprets it as a field element.

**Keep the preimage under 136 bytes** so the leaf stays one permutation. `H_p`
plus three 32-byte words is 128; a fourth word costs a second permutation and
half the saving.

**Risk.** Low. Local change, no witness-format change, no protocol change.

## Change 2 — sorted-prefix sharing

**What.** Replace N independent depth-35 walks with one bottom-up sweep over the
subtree the N touched leaves induce. N root-paths share their top `log₂N` levels,
so path work per account falls from `2·35` hashes to about
`2·(35 − log₂N + 1)`.

| N | hashes/account/tree, naive | shared | fewer |
| ---: | ---: | ---: | ---: |
| 10 | 70 | 65.4 | 6.6% |
| 100 | 70 | 58.7 | 16.1% |
| 1000 | 70 | 52.1 | 25.6% |

The saving grows as `log₂N / depth`, so it pays off exactly when batches are
large.

**Algorithm.** Sort diffs by index — required anyway for dedup. Keep a stack of
`(level, index, hash)`. Push each leaf; while the top two entries are siblings
at the same level, combine them, which is the hash that would otherwise be paid
twice; otherwise consume a witness sibling and climb. Carry **two** hash
accumulators per node position so the old-root check and the new-root
computation come out of one traversal: the sibling set is identical for both.

**Witness format changes too**, and shrinks: instead of `N·35` siblings, supply
only those not derivable from another leaf — about 29.4 per account at N=100.
Less input to marshal, less executor memory.

**Soundness, the part to get right.** A multiproof has a shape, and a prover who
controlled the shape could claim two leaves live in different subtrees when they
do not, forcing acceptance of a fabricated sibling where a derived value
belongs. This is closed only if **the shape is a function of the indices alone**.
Drive the sweep off the index bits; never off prover-supplied structure. The
indices are part of the diff list the completeness check already binds.

**Preconditions.** Indices strictly increasing, not merely sorted. A repeated
account's second entry carries a path that is stale with respect to the first
update. Dedup before witnessing, or fold repeated touches into one old→new pair.

**Risk.** Medium. Touches the witness format and the walk at once. Land change 1
first so the leaf invariant is already simplified when this rewrite happens.

## Change 3 — diff-set commitment (needs miza)

**What.** miza's transition circuit absorbs `(index, H_p_old, H_p_new,
evm_fields_old, evm_fields_new)` for every changed account, in sorted index
order, into a Poseidon sponge, and exposes the digest as one more element of the
proof's statement. mizu's guest then checks the witness list against that digest
and **never walks the Poseidon tree**.

**Why it is nearly free on the miza side.** The transition circuit already walks
the Merkle paths in-circuit — that is how it proves `root_old → root_new` — so it
already holds every triple. About 7 fields per account is 4 Poseidon
permutations, 52 rows at 13 rows per permutation:

- 100 accounts → 5,200 rows, 2.0% of a 2^18 circuit
- against the ~91,000 rows (35% of 2^18) the walk already costs

a **5.7% increase on work already being done**, in exchange for removing 70
Poseidon permutations per account from mizu's guest.

**Sorted order matters.** It makes the digest a *set* commitment rather than a
sequence commitment, which is the correct semantics for a diff, and it is
canonical for free given the dedup requirement.

**Commit full records, or records plus projections — not leaf hashes alone.**
Committing only `(index, H_p_old, H_p_new)` is smaller and would hand the guest
`H_p` without a walk, but nothing would then bind the `evm_fields` placed in the
keccak leaf, and a prover could pair a genuine `H_p` with a fabricated amount.
The committed data must cover everything the keccak leaf's preimage draws on.

**The soundness condition is non-negotiable.** The commitment must be computed
inside the circuit that performs the ledger update, not by a sequencer step
afterwards. In-circuit, Mina's rules enforce that the committed set matches the
updates actually applied, so completeness stays cryptographic. Emitted by an
untrusted sequencer, it degrades to honest-majority trust and gives up exactly
the property the Merkle replay was buying. With BFT available it is tempting to
have the quorum sign the commitment instead; since the Pickles proof is being
produced anyway, and the in-circuit cost is 5.7% of work already done, prove it.

**Where the digest goes.** In the **statement**, not Mina's on-chain state. Mina
zkApp state is exactly 8 field elements and may already be fully allocated.
mizu does not read on-chain state: `crates/pickles-verifier/src/statement.rs`
recomputes `hash_messages_for_next_step_proof` over the VK fields, the app state
and the previous-proof commitments, then checks it against
`messages_for_next_step_proof_digest`. Statement fields are host-supplied and
*bound* by that check, which is there to stop "a malicious host from pairing a
valid proof with unrelated application-statement fields". So a statement field
reaches the guest trustlessly and consumes no state slot.

**mizu needs no verifier code change.** `statement.rs` already absorbs
`proof.app_state` at whatever length it is. A longer app state flows through the
existing binding check unmodified.

**But the verification key changes**, and that has a tail:

- Mina side: a deployment or upgrade of miza's zkApp with the new VK.
- mizu side: the VK is pinned. `verification_key_sha256 = eed89d80…`, app
  statement `0x11D6…`, embedded at build time through `fixed_srs.rs` and
  `fixed_pallas.rs`. A new VK means a new embedded setup, new fixture, new
  manifest, new ELF.
- **Every baseline in this plan resets.** Re-measure; do not assume the deltas
  carry across a VK change, since the fixed-table work means a VK change touches
  build-time derived tables and not just a constant.

**That warning is now qualified by a measurement.** A VK rotation was performed
on 2026-11-13: the guest was rebuilt against a different wrap VK
(`fixtures/simplechain/wrap0`) with the same 42 optimization flags, and proved.

| | mainnet VK guest | simplechain VK guest |
| --- | ---: | ---: |
| ELF | `dc8e4b40..` | `51241c13..` |
| cycles | 130,367,324 | **130,307,322** (0.05% apart) |
| raw preprocessed area | 4,722,688 cells | same preprocessed block (3) |
| clusters reached | 10 of 43 | the same 10 |
| `vk_root` | — | **unchanged** — verified under the same block-3 map |
| verifier deployment | — | **the same one** |

So the derived-table rebuild is real but its *consequences* are narrow. A VK
rotation changes the ELF and therefore the `programVKey`; it does not move the
cycle count materially, the preprocessed block, the VK map, `vk_root`, or the
deployed verifier. The only on-chain consequence is a registry entry for the new
`programVKey` — which the allowlist design needs regardless. The rebuild itself
is ~7.5 min marginal through `make build-guest`, with a provenance receipt
(`benchmarks/provenance/`).

Two caveats. The rotation was to a *different circuit's* VK rather than a new VK
for the same circuit after a zkApp upgrade; the latter should be the smaller
change of the two, but that is an inference. And a VK whose blob is much larger
could push the preprocessed area past block 3's 6,291,456-cell ceiling — there is
1,568,768 cells of headroom, and `vk-map-local --shapes-only` settles it in 95 s.

**Also free on our side: the longer statement.** Putting the digest in the
statement lengthens `app_state`. `statement.rs` absorbs it at any length, and the
`statement-digest` ABI hashes `statement.len()` before the fields, so a longer
app state is bound rather than silently truncated at either layer. No code
change in the verifier or the ABI.

**Unverified precondition.** The 5.7% figure assumes miza's transition circuit
walks the paths in-circuit. If it instead takes the new root as a witness and
checks something weaker, the commitment is no longer nearly free and this
becomes real work. That circuit is not in this workspace —
`miza-o1js-to-zkvm` is Miza's fork of the *verifier*, not the chain. Confirm
before scheduling.

## Settled: the production share is 9.58%, not 6.07%

Measured both ways. `mirror-records` now takes `PASTA_RETAIN`, mapped exactly as
the GPU harness maps it (`tools/pasta-proof/src/main.rs:2407`).

**The numerator is routing-invariant.** With both precompiles on, the diff proof
is **identical in every field** — records, cycles, main cells, interaction
events — under default and `pasta-uint` routing, at N=1 and N=16. The reason is
structural: `pasta-uint` retains `UINT256_MUL`, `PALLAS_ADD` and `VESTA_ADD`,
and this guest emits none of them. Its chips are `PallasPoseidonPermute`,
`PallasFpOpAssign` and `KeccakPermute`, and `uint256_mul_count` is zero.

**The denominator is not.** The Pickles verifier emits 2,542,549 retainable
calls, so production is the retained base: 27,839,873,136 cells and 82 records,
against default routing's 43,922,703,280 and 148.

| accounts | diff cells | % retained base | % default base | % cycles |
| ---: | ---: | ---: | ---: | ---: |
| 10 | 266,717,880 | **0.96%** | 0.61% | 0.78% |
| 100 | 2,667,178,800 | **9.58%** | 6.07% | 7.82% |
| 1000 | 26,671,788,000 | 95.80% | 60.72% | 78.23% |

So **9.58% per hundred accounts** is the production figure, and the mirror alone
is 3.77%. Cycle shares are unaffected, as expected.

### The base itself moves with the precompiles, and it is measured now

Rebuilt `o1-verifier` under `benchmarks/dev/pasta-poseidon.json` and ran it on
the pinned mainnet fixture: **130,367,324 cycles**, accepted, output `01`,
`uint256_mul_count` **0**. The guest hashes to
`1f27156b78f95c5ca3937d2e4c90bb717321819b8a4d98979b19b234c810daf6`, which is the
candidate ELF 04-poseidon.md records, and the cycle count matches its figure
exactly — so this is a reproduction of a recorded result, not a new one.
Receipt: `benchmarks/verifier-poseidon-precompile-cycles.json`.

| base configuration | cycles | main cells | routing | diff share at N=100 |
| --- | ---: | ---: | --- | ---: |
| canonical (neither precompile) | 142,638,298 | 43,922,703,280 | default | 6.07% |
| canonical, retained | 142,638,298 | 27,839,873,136 | retained | **9.58%** |
| pasta-fp | 139,610,314 | 42,414,408,432 | default | 6.29% |
| **pasta-poseidon (Fp + Poseidon)** | **130,367,324** | 40,201,470,720 | default | 6.63% |
| pasta-poseidon, retained | 130,367,324 | **unmeasured** | retained | — |

Decomposed: the Fp precompile alone is −2.12% of cycles, Poseidon on top of it
is a further −6.62%, and both together are **−8.60%** against this repo's
canonical base.

**So the 9.58% above is conditional on a base that has neither precompile.** If
production turns Poseidon on — which this plan recommends — the base is the
130,367,324-cycle guest, and the honest answer for its retained-routing cell
count is that nobody has measured it. Note why it is not a simple read-across:
`uint256_mul_count` is **0** in that build, so the `uint` half of
`PASTA_RETAIN=pasta-uint` becomes a no-op for the verifier exactly as it is for
the mirror guest, while the `PallasAdd`/`VestaAdd` half still has 697,988 and
1,310,683 calls to retain. **Open: run the trace accounting on
`1f27156b…` under retention before quoting any production percentage.**

### And a correction to the cost of dropping the Poseidon precompile

Retention is worth far more to the software-Poseidon build than to the verifier,
because that build is almost entirely `UINT256_MUL` work: **−81.6% of cells**
(1,117,574,019 → 205,668,094 marginal cells/account), records 50 → 13 at N=16.

Earlier I put the cost of turning `MIZU_POSEIDON_PRECOMPILE` off at 41.9x on
cells. That compared default routing on both sides, which is not the fair
comparison since retention is free. With retention on, **the honest figure is
7.7x** — 205,668,094 against 26,671,788. Still a bad trade, but an order of
magnitude less bad than stated.

## Validation

Every change is checkable with the harness already on the branch, which is why
the projections above are worth little and the measurements are worth having.

```bash
# cycles, stage-split, and acceptance against a native recomputation
./target/native-test/mirror-bench-runner --elf <guest> --mode 1 --accounts 1 --depth 35 --output <json>
```

```bash
# per-chip rows, main cells and global interaction events, from real records
./target/native-test/mirror-records --elf <guest> --mode 1 --accounts 16 --depth 35 --output <json>
```

Rules that kept the earlier numbers honest and should keep these honest:

- Take marginal costs as `(N=16 − N=1)/15`; the N=1 totals carry fixed startup
  and overstate per-account cost by about 14% in cycles.
- Report cells **and** interaction events. The keccak precompile is a 2.3x cell
  win that costs 8,010 interaction events per account; cycles and cells
  disagreed about which half of the proof was expensive until the fixed overhead
  was removed.
- The mirror's cost must come out identical in a software-Poseidon and a
  precompiled-Poseidon build. It did, to 0.002%, and that cross-check is what
  validates the differencing.
- Every run must be accepted against the runner's native recomputation. A cheap
  number from a guest that computed the wrong root is worse than no number.

## Sequencing

1. **Change 1**, now. Local, low risk, and it simplifies the invariant that
   change 2 would otherwise have to preserve through a rewrite.
2. **Change 2**, next. Guest and witness format together. Measure at N=10, 100
   and 1000 — the saving is a function of batch size, so a single N proves
   nothing.
3. **Change 3**, in parallel on the miza side, since it is a conversation rather
   than a patch. Confirm the in-circuit walk first; it decides whether this is a
   small ask or a project.

## The measurement that gates all of it

Accounts changed per batch. Everything here is linear in it, and nothing in this
work has measured it. `mizu/RESULTS-09.md` calls it "the deciding measurement",
records that "nothing in spikes 01-09 has measured" it, and notes the one real
block examined had zero actions. At depth 35 miza's circuit fits roughly 280
accounts per 2^18 before recursion, so that number bounds the design on both
sides — batch size, settlement cadence, and whether change 2's saving is 6.6% or
25.6%.

## Downstream

The proof pipeline that consumes this work — core → compressed → PLONK wrap —
is planned separately in [proof-pipeline-plan.md](proof-pipeline-plan.md). Its
first sequencing rule matters here: shapes, the recursion VK map and the wrap
keys are all downstream of the core machine's chips, so **the changes in this
plan belong before that pipeline work**, not after.

## Explicitly out of scope

- **Depth reduction to 16–20.** Retracted. If miza runs Mina's ledger code, the
  ledger is depth 35 with accounts placed by hashing `(token_id, public_key)`.
  Re-indexing means forking the ledger and losing the compatibility that makes a
  Mina-shaped settlement proof possible.
- **Carrying account diffs in Zeko inner actions.** An action is emitted by the
  zkApp that runs, about its own updates, so it cannot witness another writer's
  changes; and the 65,536-leaf budget is shared with withdrawals. Viable only for
  accounts Mizu itself writes.
- **Dropping the Poseidon walk without a commitment.** Verifying the Pickles
  proof does not establish completeness: the proof attests `P_old → P_new` and
  says nothing about a prover-supplied witness list. Omit an account and the
  mirror commits to a state that never existed, with nothing to catch it.
