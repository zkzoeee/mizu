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
| + diff-set commitment | **Miza's transition circuit** | 9,142,141 | 65.7% |
| *alternative:* root built in the settlement layer | **no circuit at all** | 16,166,796 | 39.4% |

**Row 3 was corrected 2026-11-14.** It read 9,696,716 / 63.6%, which does not
reconstruct from the permutation counts as a cumulative row: it silently reverts
change 1, pricing the keccak leaf at 3 permutations again. Cumulative with
changes 1 and 2 the figure is keccak at 60.7 permutations plus 4 Poseidon sponge
permutations — 8,390,197 + 751,944 = **9,142,141**, a 65.7% saving. The
reconstruction is exact to 0.02% on every row, which is how the error was found.

Two consequences. Change 3 removes **all** of the Poseidon work, not the 70 path
permutations its section names: after change 2, Poseidon is 74.7 permutations
(16 leaf + 58.7 path) and the sponge adds 4, so the net 70.7 is right but it is
not "the paths" — the leaf hashes go too, because the guest no longer computes a
Poseidon leaf at all. And therefore **change 1 is a prerequisite for change 3,
not an option**: a guest with no `H_p` of its own can only build a keccak leaf
from the committed `H_p` and `evm_fields`, which is change 1's format exactly.
The sequencing below already puts change 1 first; this is the reason it has to be.

The first two need nothing from Miza. They are changes to the guest that walks
the tree, so they can land independently of anything on the chain side.

**Only the baseline is measured.** 26,671,788 cells and 111,592 cycles per
account are marginal figures from generated records at depth 35 with both
precompiles, taken as `(N=16 − N=1)/15`. The other four rows are **projections**
from measured per-permutation costs — Poseidon 187,986 cells/permutation, keccak
138,224 — applied to permutation counts each change implies. Treat them as
sizing, not results, and re-measure each one as it lands. The harness for that
is already on the branch and described under *Validation* below.

## Decision 2026-11-14: the full keccak Merkle mirror

**Mode 1, the full keccak Merkle mirror, is the chosen construction** — the one
that lets Ethereum prove inclusion of any account.

Chosen *among mirror constructions*. The mirror itself is an **option** on
Mizu, not a requirement: it exists so that a proof settled to an EVM is easy to
use there, and nothing else in this repository depends on it. Whether it is
built inside the guest at all is the step-0 question in *Sequencing*.

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
reads as being in tension with this decision, since a diff-set commitment
removes per-account walks the full mirror exists to provide. **Qualified
2026-11-14: the tension is narrower than that.** Change 3 removes the *Poseidon*
walks only; the keccak mirror and its root survive untouched, so Ethereum's
inclusion proofs are unaffected. What genuinely competes with the full mirror is
the alternative below, which moves the keccak tree out of this proof altogether
— and that is a relocation, not a loss of the property.

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

## Change 3 — diff-set commitment (needs Miza)

**What.** Miza's transition circuit absorbs `(index, H_p_old, H_p_new,
evm_fields_old, evm_fields_new)` for every changed account, in sorted index
order, into a Poseidon sponge, and exposes the digest as one more element of the
proof's statement. mizu's guest then checks the witness list against that digest
and **never walks the Poseidon tree**.

**Why it is nearly free on the Miza side.** The transition circuit already walks
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

- Mina side: a deployment or upgrade of Miza's zkApp with the new VK.
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

**Unverified precondition.** The 5.7% figure assumes Miza's transition circuit
walks the paths in-circuit. If it instead takes the new root as a witness and
checks something weaker, the commitment is no longer nearly free and this
becomes real work. That circuit is not in this workspace —
`miza-o1js-to-zkvm` is Miza's fork of the *verifier*, not the chain. The chain
is Miza's own L2; whether its transition circuit walks the paths in-circuit is
still unconfirmed, and still decides whether this is a small ask or a project.

## Where the mirror root is produced

Written 2026-11-14. An earlier version of this section asserted that Zeko owns
the chain and gated the plan on Mizu running as a Zeko L2 zkApp. **That was
wrong**, and the correction is in *What this project is* below.

### What this project is

**Mizu** is this repository: making o1js-to-zkvm fast, which it has done —
precompiles at -8.6% of cycles, Pasta retention at -39% of shards, a Groth16
proof end to end, and a wrap verifying key equal to upstream's so no ceremony is
needed. **The mirror tree is an option on top of that**, so that a proof settled
to an EVM is easy to use there. It is not the point of the project and nothing
depends on it.

**Miza** is a Mina L2. It may settle to Mina, or to an EVM; for the EVM path it
would use Mizu, and that is the only path on which the mirror tree matters.
Change 3's ask is addressed to Miza's transition circuit.

**Zeko is not in this repository and is not a dependency.** It is referenced for
its settlement work only. The misreading was of `MIZA.md`'s line that "`zeko`
identifies the source application repository": that names Zeko as an upstream
*source of ported work* in Miza's verifier fork, alongside `nori`. It does not
make Zeko the chain.

### The real choice

If Miza settles to an EVM, something has to produce a keccak root the EVM can
check inclusion against. Two places it can happen:

| | where | cells/account | cycles/account |
| --- | --- | ---: | ---: |
| **in the proof** | Mizu's guest walks a keccak mirror (today) | 10,505,024 | 85,804 |
| **in the settlement layer** | the root is built outside the zk proof | 0 | 0 |

The second is free to this proof and it is where the 39.4% of cells and 71.5% of
cycles go. It is not free overall — the work moves to Miza's settlement, which
has to build the tree and publish the root — but it moves out of a SNARK and
into ordinary code, which is the cheapest place for keccak to live.

This is the decision step 0 in *Sequencing* now asks for, and it is Miza's, not
ours. What this repository should do either way is keep the mirror tree as a
working, measured option, which it now is: proved, verified on an EVM, with its
own `vk_root` and verifier.

### Zeko's encoding, as a reference

`DESIGN-zeko-inner-action-payload.md` (2026-11-04, written against
`zeko-labs/ethereum-settlement` at `fix/pasta-complete-execution-verification`,
every encoding read from source, not inferred) is a worked example of the
settlement-layer option. Zeko's settlement guest commits any inner action it
does not recognise as a withdrawal into a depth-16 keccak root
(`receipt.rs:483-509`), and `innerActionBatch` is a public mapping on
`ZekoSettlement`, so any EVM contract can read the root without permission.

**Riding Zeko's own settlement is not on the table** — that would need Miza to be
a Zeko L2, and it is not. The value is the encoding, and these parts are worth
copying if Miza builds its own settlement tree:

- field bytes **big-endian, right-aligned** (`into_bigint().to_bytes_be()`,
  `receipt.rs:973`). This plan's change 1 already adopts that convention for the
  same reason: endianness silently produces a different root for identical data.
- domain separators **pre-hashed** — the preimage holds `keccak256(DOMAIN)`, not
  the string.
- a `u32` length word inside the fields hash, so a shorter field vector cannot
  be extended into a longer one.
- **context binding**: chain id, bridge address and global index in the leaf.
  Spike-09's encoding has none of this, which makes its leaves replayable across
  chains and deployments. This is the most valuable part to copy.
- fixed depth with a zero-hash ladder, so membership proofs are a fixed-size
  array and an empty tree has a defined root.

Two cautions carried from that doc. The receipt path that builds the tree **has
never been proven** — Zeko's audit figures are execute-mode, its contract tests
use a mock verifier, and the r5 GPU proof took the `receipt_input = None`
branch; the encodings are source-read and unit-tested there, not attested by any
proof. And a raw leaf proves only that an action with a given fields hash sat at
a given index, so the consumer must hold the preimage for it to mean anything.

### A cheaper route to change 3's own property

If the diff-set commitment is wanted, there is a smaller circuit edit than a
sponge plus a statement field: **emit the diffs as Mina actions and use the
action state as the commitment.** Mina maintains the action state at the
protocol level, so soundness stays cryptographic without the circuit computing a
digest, and the edit is an `emitAction` call — no change to the proof's public
interface, no new statement field, no pressure on the eight on-chain state slots.

Two caveats. The action state is a *sequence* commitment in emission order, not
the sorted set this plan wants, so dedup and ordering move into the guest. And
there is no real traffic to size it against: spike 09 stage F found real actions
per checkpoint on mainnet is zero.

### Ruled out by measurement

Computing the keccak root in o1js, inside the Mina-side circuit, does not fit.
The o1js keccak tree was measured at 171x a 2^18 circuit for 1,000 actions.
A measurement, not an estimate.

### None of this is a cost lever

On a 1xA40 at $0.59/hr the whole mirror tree adds **+$0.0024** to a proof at
N=100. Cells and cycles only touch the 156 s re-execution-plus-STARK half, while
the gnark half is a fixed 15,972,262-constraint circuit costing 130 s on CPU or
37 s on CUDA regardless of guest size. Changes 1 and 2 together save $0.0004;
all three save $0.0016. Pod cost cannot justify any of this work.

What these changes buy is **capacity** — cells are what make large batches fit,
and at N=1000 the diff proof is 95.8% of the production base. Which is why the
deciding measurement is still the one this plan already names and nobody has
made: accounts changed per batch. At ~10 accounts none of changes 1-3 is
load-bearing. At ~1000 the Poseidon walk is the binding constraint.

### What the half-and-half split says about priority

| half | cells/account | share | cycles/account | share |
| --- | ---: | ---: | ---: | ---: |
| Poseidon ledger walk | 16,166,796 | **60.6%** | 34,142 | 28.5% |
| keccak mirror | 10,505,024 | 39.4% | 85,804 | **71.5%** |

Poseidon dominates cells; keccak dominates cycles. Change 3 attacks the first,
moving the root out of the proof attacks the second, and they are complementary
rather than competing. If wall clock binds, move the root out. If capacity at
large N binds, change 3 or its action-state variant is the one that matters.

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
3. **Change 3**, in parallel on the Zeko side, since it is a conversation rather
   than a patch. Confirm the in-circuit walk first; it decides whether this is a
   small ask or a project.

**Revised 2026-11-14, and step 0 now comes before all three.** Decide **where
the mirror root is produced** — inside Mizu's guest, as the mirror tree does
today, or in Miza's settlement layer, outside the zk proof. The second removes
39.4% of cells and 71.5% of cycles from this proof, makes change 1 moot, and
halves change 2 because it would then apply to one tree instead of two. Deciding
it first avoids building a leaf format and a shared sweep for a tree that then
moves out. See *Where the mirror root is produced*. Change 3 stays last either
way: it is the only item needing a circuit we do not own, and the measurement
that gates it (accounts per batch) gates everything below.

## The measurement that gates all of it

Accounts changed per batch. Everything here is linear in it, and nothing in this
work has measured it. `mizu/RESULTS-09.md` calls it "the deciding measurement",
records that "nothing in spikes 01-09 has measured" it, and notes the one real
block examined had zero actions. At depth 35 Miza's circuit fits roughly 280
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

- **Depth reduction to 16–20.** Retracted. If Miza runs Mina's ledger code, the
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

## 2026-11-24 benchmark (`feat/mirror-tree-opt`): batched multiproof + dedup + `keccak(H_p ‖ evm_fields)`

Implemented in `crates/poseidon-bench/src/batch_mirror.rs` and wired as Modes 5 (all-unique accounts) and 6 (25% hot-account repeats) in `crates/poseidon-bench/src/mirror.rs`, combining:
1. **Sorted bottom-up Merkle multiproof** (`batch_merkle_update`) — shared ancestors hashed once per level instead of once per account.
2. **Hot-account deduplication** (`dedup_diffs`) — collapses repeated touches to the same leaf index into one net `(first_old → last_new)` transition.
3. **Bound leaves `keccak(H_p ‖ evm_fields)`** (`keccak_leaf_bound`) — 64 bytes (1 keccak permutation) instead of 320 bytes (3 permutations).
4. **Contiguous phase separation** — all Poseidon walks, then all Keccak walks (`--features keccak-precompile`, zero new AIRs / zero wrap-VK impact).

Measured via `mirror-records` (`depth = 35`, `MIZU_POSEIDON_PRECOMPILE=1`, `--features keccak-precompile`):

| Workload (`depth = 35`) | Mode | Cycles | Shards | Main cells | `Global` cells | `KeccakPermute` rows | `PallasPoseidon` rows |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| `N = 64` | 1 (naive full mirror) | `9,077,624` | `7` | `1.964B` | `616.7M` | `121,344` | `302,720` |
| `N = 64` | **5 (opt 1+2+3+5, unique)** | **`7,620,430` (`-16.1%`)** | `7` | **`1.676B` (`-14.7%`)** | `549.1M` (`-11.0%`) | **`92,256` (`-24.0%`)** | **`260,704` (`-13.9%`)** |
| `N = 64` | **6 (opt 1+2+3+5, 25% hot)** | **`5,736,679` (`-36.8%`)** | `7` | **`1.262B` (`-35.8%`)** | `413.4M` (`-33.0%`) | **`69,408` (`-42.8%`)** | **`196,032` (`-35.2%`)** |
| `N = 256` | 1 (naive full mirror) | `36,283,404` | `23` | `7.842B` | `2.457B` | `485,376` | `1,210,880` |
| `N = 256` | **5 (opt 1+2+3+5, unique)** | **`29,374,343` (`-19.0%`)** | **`20` (`-13.0%`)** | **`6.469B` (`-17.5%`)** | `2.126B` (`-13.5%`) | **`354,336` (`-27.0%`)** | **`1,009,056` (`-16.7%`)** |
| `N = 256` | **6 (opt 1+2+3+5, 25% hot)** | **`22,364,196` (`-38.4%`)** | **`16` (`-30.4%`)** | **`4.918B` (`-37.3%`)** | `1.614B` (`-34.3%`) | **`269,952` (`-44.4%`)** | **`766,496` (`-36.7%`)** |

Native unit test `batch_mirror::tests::batch_multiproof_matches_dense_merkle_tree_and_saves_hashes` verifies bit-exact `(old_root, new_root)` equivalence against a dense reference Merkle tree.

### Dual-root commitment (Option 1 `keccakDiffRoot` + Option 3 `keccakGlobalRoot`)
Modes 5 and 6 commit **both** roots in the same Phase 2 pass:
- **`keccakDiffRoot` (Option 1):** compact depth $\lceil \log_2 N \rceil$ binary Keccak Merkle tree over the `N` sorted `keccak(H_p ‖ evm_fields)` leaves (odd trailing nodes paired with `[0u8; 32]`; tested by `keccak_diff_root_and_client_inclusion_proofs_verify`).
- **`keccakGlobalRoot` (Option 3):** depth-35 global Keccak mirror updated via `batch_merkle_update`.

Adding `keccakDiffRoot` alongside `keccakGlobalRoot` adds **+0 shards** and only **+0.53%–0.55% main cells**:

| Workload (`depth = 35`) | Mode | Shards | Cycles | Main cells | Overhead vs Option 3 alone |
| --- | --- | ---: | ---: | ---: | ---: |
| `N = 64` | 5 (unique) | `7` (`+0`) | `7,696,893` | `1.685B` | `+0.53%` |
| `N = 64` | 6 (25% hot) | `7` (`+0`) | `5,795,253` | `1.269B` | `+0.55%` |
| `N = 256` | 5 (unique) | `20` (`+0`) | `29,677,982` | `6.504B` | `+0.55%` |
| `N = 256` | 6 (25% hot) | `16` (`+0`) | `22,593,151` | `4.945B` | `+0.54%` |

### Client & Ethereum verification flow (using `mina-indexer` GraphQL)
1. **Using `keccakDiffRoot` (zero custom infra, ~25k gas):**
   - Client queries any **unmodified `mina-indexer` GraphQL API** for the updated accounts in block/epoch $H$, hashes `keccak256(abi.encodePacked(H_p, evmFields))` locally in JS/TS (`< 1 ms`), builds the depth $\lceil \log_2 N \rceil$ tree, and submits `6–8` sibling hashes to Ethereum.
   - Ideal for bridge deposits, withdrawals, actions, and recent zkApp state transitions.
2. **Using `keccakGlobalRoot` (arbitrary / dormant accounts, ~50k gas):**
   - A thin sparse Keccak-tree sidecar/resolver on `mina-indexer` exposes one GraphQL query `keccakMerklePath(publicKey, tokenId)` returning `(leafIndex, H_p, evmFields, siblings[35])`.
   - The sidecar is **100% untrusted** — Mizu's SP1 proof binds `P_old → P_new` to Mina's verified Pickles proof and `K_old → K_new` to Ethereum's stored `keccakGlobalRoot`.
3. **Solidity leaf & path check (identical for both roots, varying only loop depth):**
   ```solidity
   bytes32 node = keccak256(abi.encodePacked(poseidonLeafHash, evmFields));
   for (uint256 i = 0; i < siblings.length; i++) {
       node = ((index >> i) & 1) == 0
           ? keccak256(abi.encodePacked(node, siblings[i]))
           : keccak256(abi.encodePacked(siblings[i], node));
   }
   require(node == expectedKeccakRoot, "invalid Mina state proof");
   ```

### Combined settlement guest (`o1-verifier-settlement`) — measured end-to-end Groth16 (2026-11-25)

Wired into `crates/pickles-verifier/src/mirror.rs` (exact 20-byte Mina domain prefixes `"MinaMklTree{:03}******"`, `"MinaAccount*********"`, `"MinaProtoState******"` + dual-root Keccak mirror) and `crates/o1-verifier/src/bin/settlement.rs` (`o1-verifier` compact `w=16` + `verify_and_apply_settlement`, committing 196-byte `SettlementPublicValues`).

Measured on the real Mina mainnet Pickles fixture + `N = 64` accounts at `depth = 35` (`benchmarks/pipeline/settlement-groth16-20261125/`):

| Metric | Value | Notes |
| --- | ---: | --- |
| Guest ELF (`o1-verifier-settlement`) | `353bf06b..` | compact `w=16` + `--features settlement` (`KeccakPermute` deferred, zero new AIRs) |
| Cycles (`settlement-bench`) | **`94,643,704`** | `+11.4%` over bare `o1-verifier` (`84.99M`) |
| Main cells (local `settlement-bench`) | **`22.96B`** (`71` shards) | `KeccakPermute`: `1` shard, `70,560` rows (`0.8%` of cells) |
| Committed public values | **`196 B`** | bit-exact match (`pv_sha256 = 9f16c2ea..`) |
| GPU Core STARK + VK-map (1×A40) | **`127.7 s`** core + **`10.4 s`** VK-map | **`66` shards**, `18` distinct shard shapes, `24` VK-map keys |
| **Full Groth16 (1×A40, VK verification ON)** | **`217.7 s` prove (`246.0 s` wall)** | **`324 B`** raw proof, **`verified: true`**, Succinct v6.1.0 artifacts (zero ceremony), **`~$0.040`/proof** |

### Estimated combined latency (`o1-verifier` compact `w=16` + dual-root mirror)
Anchored to measured `mizu/dev` compact `w=16` (`67` shards: `63.8 s` core on RTX 4090, `117.1 s` core on A40) and verified `o1-verifier-settlement` (`N = 64` on A40: `127.7 s` core, `217.7 s` Groth16 prove). Because Mina's block time is ~180 s, single-block cadence (`K = 1`) keeps up in real time on a single RTX 4090:

| Account updates (`N`) | Total shards *(25% hot – unique)* | Core STARK (4090) | Core STARK (A40) | Full Groth16 est. (4090, CUDA gnark) | Full Groth16 (A40) |
| ---: | ---: | ---: | ---: | ---: | ---: |
| `0` *(measured)* | `67` | `63.8 s` | `117.1 s` | `~135 s` | `~215 s` |
| `64` *(A40 measured)* | `66 (GPU)` / `71 (CPU)` | `~68 – 69 s` | **`127.7 s`** | `~141 – 143 s` | **`217.7 s` prove** |
| `128` | `74 – 76` | `~71 – 73 s` | `~129 – 133 s` | `~146 – 149 s` | `~233 – 238 s` |
| `256` | `81 – 85` | `~77 – 81 s` | `~141 – 149 s` | `~155 – 161 s` | `~250 – 260 s` |
| `512` | `93 – 101` | `~88 – 96 s` | `~162 – 176 s` | `~172 – 184 s` | `~280 – 300 s` |
