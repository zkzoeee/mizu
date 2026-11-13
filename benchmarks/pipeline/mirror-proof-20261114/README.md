# The first Groth16 proof of the mirror guest

`benchmarks/mirror/RESULTS.md` measured eleven mirror configurations on CPU with
`proof_generated: false` in every one — cycles and permutations, no prover, no
GPU. This is the first proof of that guest.

| | |
| --- | --- |
| guest | `crates/poseidon-bench/src/mirror.rs` (`mirror-operation-program`), 131,008 bytes |
| input | 12 bytes, `(mode=1, accounts=1, depth=35)` as LE u32 — keccak-mirror mode |
| map | 5 shards, 5 distinct shapes, **11 keys** (7.3 s build, 2.3 s core) |
| `vk_root` | **`0x00dcb7878d00c79553bb5c3b7a98aea41df70817439d3c6ee5a79335b85306a9`** |
| proof | verified, 324 raw bytes, Succinct's released `VERIFIER_HASH` |
| wall clock | **71 s** — node_build 17.8, setup 0.2, prove 48.2, verify 0.4 |
| peak VRAM | 11,679 MiB |

Verified on an EVM too: `contracts-check/test/TwoRoots.t.sol`, 226,157 gas.

Proved with the **CUDA gnark prover** (succinctlabs/sp1#3017, fork branch
`codex/mizu-cuda-gnark`) — a second independent check of that carried code on a
completely different workload from the Pickles proof it was measured on.

## Block 1, and why that forces a second verifier

The mirror guest is 200x smaller than the Pickles verifier, which embeds a
26 MB verifier blob:

| | Pickles guest | mirror guest |
| --- | ---: | ---: |
| ELF | 26,382,072 B | **131,008 B** |
| raw preprocessed area | 4,722,688 cells | **915,456 cells** |
| preprocessed block | 3 | **1** |
| clusters reached | 10 of 43 | 5 of 43 |

Two hundred times less code gives only five times less area, because the range
and byte tables are a fixed floor of roughly 900k cells that every guest pays.
Nothing can be below block 1.

Block 1 means **the block-3 map cannot cover it**, so this guest has its own map,
its own root, and its own deployment. `contracts-check/test/TwoRoots.t.sol`
demonstrates it: two verifiers, two roots, each refusing a *real verifying proof*
made under the other's map. Only the 3.6 KB wrapper differs between them — both
import the same 26.7 KB of pairing code.

That is the answer to "is it better to have one verifier per proof" in
executable form: share while the root is shared, split exactly when it diverges.
Not per proof, and not per VK.

## The cycle count is NOT comparable to RESULTS.md

Our ELF runs **129,517 cycles**; `opt-n1-mode1.json` recorded **4,642,756** for
what is nominally the same guest and the same input — and the input really is
the same, `sha256 1dcfcf46..` matches the recorded `input_sha256` exactly.

The ELFs differ: ours is `f6300e05..`, the benchmarked one `931dc022..`. 36x
fewer cycles means ours is the *more* optimised build — its shard shapes reach
clusters 35 and 37, the Pasta/retained ones, so it is evidently using
precompiles where the benchmarked `opt-` build used canonical Poseidon at 48,075
cycles per permutation.

So this proof is real and verified, but **its cycle count is not a mirror cost
figure**. It came from an ELF at a path with no receipt — the same provenance gap
that cost an afternoon on the Pickles guest, recurring because this guest has no
`make build-guest` config. Comparing to RESULTS.md needs a rebuild through the
harness with the recorded recipe.

## What this is not

The mirror *benchmark* guest is a cost model, not the diff proof. Its own doc
comment is explicit: it counts permutations for the hash schedule, "no root here
is claimed to equal a real Zeko root", and keccak is software rather than SP1's
`KECCAK_PERMUTE`, so every keccak figure is an upper bound.

The production guest would start from the Pickles verifier and add the tree
logic — `docs/diff-proof-plan.md` changes 1 and 2 — which keeps it in **block 3**
with 1,568,768 cells of headroom, about 150x the 10,240 cells that adding
`statement-digest` cost. That guest does not exist yet, and when it does the
existing block-3 map and root already cover it.

## The public values, and a test that was missing

`verifyProof` recomputes `sha256(publicValues) & ((1<<253)-1)` and compares it
against public input 1, so calling the contract needs the **raw** public values
— and every driver mode recorded only the hashed inputs. A proof could be made
and then not be checkable on chain for want of 76 bytes.

`--mode execute` now dumps them (no proving, no GPU, about a second, since public
values are a function of the guest and its input alone):

```
0x 01000000 01000000 23000000                     mode=1, accounts=1, depth=35
   78e5d634835f9916088795f293bcc447727750dbb2490cbbf5aeb5cfa6489006   poseidon root
   3316f90d43ec63afc0ea49cd4d5da38456d9e421f0bc39b7b6baaa63ba5d2085   keccak root
```

and it checks itself: `hash_public_values` is
`0xfe07758907d52c9763c7b5aca403c732831d5b147a0f1f616f0b703698ae4a3`, which is
the proof's public input 1 exactly, and the execution reports the same 129,517
cycles as the proof.

**The gap this closed.** `TwoRoots.t.sol` originally asserted only that each
verifier *rejects* the other's proof — two negatives and no positive, which would
have passed just as happily for a garbage proof. It now also asserts that the
mirror verifier accepts the mirror proof (226,157 gas) and that one flipped byte
of public values is refused.
