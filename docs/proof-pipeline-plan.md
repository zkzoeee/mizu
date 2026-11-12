# Proof pipeline plan: core → compressed → PLONK wrap

Tasks for getting from today's core proofs to an EVM-verifiable proof.
**Wrap decision: PLONK.** Rationale and the rejected alternatives are at the end.

## Where this starts

The record is unambiguous: *"No recursion, no wrap, no Groth16, no recursion VK
map."* Today's output is a CUDA core proof — 82 shards, **132,029,932 proof
bytes**, verified by the complete verifier on the pod and again locally. Nothing
downstream of that exists.

## Phase A — make the custom chips recursion-legal

This phase gates everything else.

1. **Enumerate the shard shapes the fork's machine can emit.** The Pasta
   precompiles add AIR ids — the Poseidon record cites **134 to 139** — and each
   has `*Control` and `*User` variants (`benchmarks/mirror/chip-widths.txt` lists
   them). Every shape the chosen routing can produce has to be in the allowed set.
2. **Regenerate the compress shapes**: `vendor/sp1/crates/prover/compress_shape.json`
   and the `_mprotect` variant, via `crates/prover/src/shapes.rs`.
3. **Build the recursion VK map.** The blocker the record names in three places:
   *"That map is not built; building it needs SP1's recursion toolchain and
   GPU-class compute."* Artifacts are `vk_map.bin` (7.4 MB at stock), plus
   `allowed_vk_map.bin` and `dummy_vk_map.bin` per `crates/prover/shapes.sh`.
4. **Decide the routing the map must cover, before building it.** The retained
   arm uses `pasta-uint` routing, placing all 2,542,549 retained calls on CPU
   shards, and the record warns that flipping `SP1CoreOpts::default` *"would also
   change the SDK's recursive proving path"*. Building for both routings doubles
   the compute, so pick one.
5. **Stand up fork-local artifact hosting.** `shapes.sh` uploads to the
   `sp1-circuits` S3 bucket keyed by git ref; a fork needs its own location and
   its own ref discipline.

## Phase B — compressed proving

6. **Add a compress mode to the harness.** `tools/pasta-proof` has
   `manifest-core-gpu` and `manifest-core-verify` only — there is no recursive
   path today.
7. **Budget the recursion tree.** 82 shards compress pairwise; measure wall-clock
   and VRAM per level. Peak VRAM for core proving is 18,991 MiB of 46,068, so
   there is headroom, but recursion nodes are a different shape.
8. **Retest the known GPU hazard.** At `PASTA_PROVE_INFLIGHT=2`, four worker
   threads panic in `sp1-gpu-logup-gkr`'s `generate_first_layer` at
   `main_poly_height(name).unwrap()` — a chip sits in the shard's chip set with no
   main polynomial on the device. The default is 1, which is serial. If that class
   of bug recurs in the recursion prover it blocks this phase.
9. **Verify a compressed proof** with the complete verifier, keeping the
   non-vacuity gate (an empty proof must be rejected) and the committed-digest
   check that core proving already enforces.
10. **Record the size collapse.** 132 MB core against a few hundred KB compressed
    is the whole reason for the phase.

## Phase C — shrink, wrap, EVM verifier

11. **Build the gnark FFI** (`miza-sp1-runtime/Dockerfile.gnark-ffi`).
12. **Generate the PLONK wrap circuit and its keys for this fork.** SP1's
    published artifacts are keyed to stock circuit versions, and extra core chips
    change the compress circuit → its VK → what shrink and wrap verify → the wrap
    circuit. **PLONK's universal SRS means no ceremony**: no 1-of-N honesty
    assumption of our own, no transcript to coordinate, and no redo when a chip
    changes. That is the reason for the decision.
13. **Deploy and gas-profile the generated Solidity verifier.** Needed for real,
    not estimated: the settlement economics key off it.
14. **Re-pin `SP1_CIRCUIT_VERSION`** and record artifact hashes beside the
    existing manifest pins.

## Phase D — end-to-end validation

15. **One full pipeline run**: core → compress → shrink → wrap, with the
    committed digest preserved and checked at every stage.
16. **Verify the wrapped proof on an EVM testnet**, and keep the receipt.
17. **Per-stage cost and latency on GPU**, so the settlement cadence rests on
    measurement. Core proving is 100–110 s and $0.0225–0.0759 per run today;
    compression and wrap are unmeasured.
18. **Negative tests at every stage** — a corrupted accumulator and a corrupted
    opening must fail where they fail for core proving now.

## Two sequencing rules that cost money if ignored

**Freeze the chip set before Phase C.** Shapes, the VK map and the wrap keys are
all downstream of the core machine's chips. Adding a Poseidon commitment check,
changing a precompile or flipping routing *after* the wrap keys exist means
redoing Phase A and Phase C. The diff-proof work ([diff-proof-plan.md](diff-proof-plan.md))
and any precompile change belong **before** this pipeline work.

**Phase A is not a cents-scale job.** Core proofs cost $0.02–0.08 each; a VK map
build needs SP1's recursion toolchain and GPU-class compute, and it is the
prerequisite for everything downstream. Budget it, and settle task 4's routing
question first so it is built once.

## Why PLONK, in numbers

Proof sizes are exact, from `vendor/sp1/crates/verifier/src/constants.rs` and the
two converters at the SP1 6.10 pin. Operation counts are read from
`groth16/verify.rs` and `plonk/kzg.rs`. Only the Solidity overhead above the
precompile floor is estimated.

| | Groth16 | PLONK |
| --- | ---: | ---: |
| raw proof | **256 B** | **864 B** |
| on-chain blob | 356 B | 964 B |
| pairing pairs | 4 | 2 |
| pairing gas | 181,000 | 113,000 |
| ecMul count | 5 | 20–25 |
| ecMul gas | 30,000 | 120,000–150,000 |
| precompile floor | 211,750 | 239,470–269,470 |
| floor + calldata | 217,446 | 254,894–284,894 |
| **with Solidity overhead (est.)** | **~250,000** | **~300,000–350,000** |

PLONK costs about **75,000 gas more per verification, 1.30x** — roughly 10.8M
gas/day at a 10-minute cadence. Against that: a Groth16 phase-2 ceremony, a
1-of-N honesty assumption, a public transcript and beacon, and a **full redo
every time the circuit changes** — which, with custom precompiles in the core
machine, it will.

Note the structure, which is counter-intuitive: pairings favour PLONK by 68,000
gas, and everything else favours Groth16 by 125,720, almost entirely scalar
multiplications. "Groth16 is cheap because it is just pairings" is backwards —
it does *more* pairing work and wins anyway.

## Rejected: Stylus for the verifier

Spike 07 measured this and the answer is settled. Stylus is **3.3x cheaper per
Poseidon hash** (14,873 against the EVM's 48,723) but carries a **2,233,850 gas
fixed overhead per call** against the EVM's 24,983 — 89x — and Test 6 showed
caching removes **0.95%** of it. The crossover stays at about 51 hashes.

For a PLONK verifier it is worse than that, because of *what* Stylus
accelerates. Scalar multiplications and pairings are precompiles, priced
identically on Arbitrum, so Stylus has nothing to improve there — only the
~55,000 gas of field arithmetic and transcript hashing above the floor:

```
PLONK on the EVM      ~325,000   (precompiles 270,000 + solidity 55,000)
PLONK on Stylus     ~2,520,000   (fixed 2,233,850 + precompiles 270,000 + solidity/3.3)
                                 -> 7.8x worse for one verification
```

Stylus saves only ~38,000 gas per verification, so the fixed cost amortizes
after about **58 verifications in a single call** — which no settlement design
here produces. Stylus wins on workloads that are pure field arithmetic in bulk;
PLONK verification is mostly precompile calls, so there is almost nothing for it
to accelerate.
