# 🌊 Mizu — Trustless Mina → Ethereum State Proofs in SP1

Mizu verifies a real Mina mainnet blockchain SNARK (Pickles/Kimchi) inside the **SP1 zkVM**, mirrors the Mina ledger diff into EVM-native **Keccak-256** roots, wraps the result into a **324-byte Groth16 proof** (with zero custom trusted setup), and verifies it on Ethereum.

Built on [o1-labs/o1js-to-zkvm](https://github.com/o1-labs/o1js-to-zkvm) and SP1 `6.10.0`.

---

## Highlights

- **~51× fewer cycles (~4.34B → < 100M):**
  - Bare Pickles verifier (`mizu/dev` compact `w=16`, zero new AIRs): **`84.99M` cycles** (`67` shards).
  - Full Mina → Ethereum settlement guest (`o1-verifier-settlement`: Pickles SNARK + 64-account depth-35 dual-root ledger mirror): **`94.64M` cycles** (`66` GPU shards).
- **Fast & cheap to prove on commodity GPUs:**
  - Core STARK (`o1-verifier`, RTX 4090): **`63.8 s` wall / `43.0 s` prove loop (~$0.015)**.
  - Full end-to-end **Groth16 settlement proof** (`o1-verifier-settlement`, 1×A40, VK verification ON): **`217.7 s` prove (`246.0 s` wall), ~$0.040/proof**.
  - **No custom trusted setup:** Mizu's wrap VK is byte-identical to upstream SP1, so Succinct's released v6.1.0 Groth16 artifacts apply directly.
- **Cheap on Ethereum:** **`324`-byte** Groth16 proof (`~226k` gas), plus **`~25k–50k` gas** per Mina account/zkApp state read via native `keccak256`.

### Measured Numbers

| Stage / Workload | Cycles | Shards | Time (measured) | Est. Cost |
| --- | ---: | ---: | ---: | ---: |
| Unoptimized Pickles verifier (stock SP1 6.10 baseline) | ~4,340,000,000 | — | — | — |
| Mizu retained config + Pasta precompiles | 130,367,324 | 84 | 294.5 s Groth16 (A40) | ~$0.053 |
| `mizu/dev` compact `w=16` (`o1-verifier`, core STARK) | **84,985,706** | **67** | **63.8 s wall** (RTX 4090) / 117.1 s (A40) | **~$0.015** |
| **Combined settlement Groth16** (`o1-verifier-settlement`, 64 accts, depth 35) | **94,643,704** | **66** | **217.7 s prove** (246.0 s wall, 1×A40) | **~$0.040** |

*Every figure above is a recorded run on rented RunPod GPUs (`benchmarks/pipeline/`), not a projection. The `~4.34B` baseline is from stock SP1 6.10.0 execution (independently corroborated by OpenVM's `4.38B` reference). See [docs/running-a-proof.md](docs/running-a-proof.md), [docs/groth16-runs.md](docs/groth16-runs.md), and [docs/diff-proof-plan.md](docs/diff-proof-plan.md).*

---

## Why Dual Keccak Roots (`keccakDiffRoot` + `keccakGlobalRoot`)?

Mina's ledger is a depth-35 **Poseidon** Merkle tree over the Pasta curves. Verifying a single Poseidon Merkle inclusion proof in Solidity is prohibitively expensive.

Inside the same SP1 settlement proof, Mizu verifies the exact Mina Poseidon state transition (`P_old → P_new`, anchored to the verified Pickles `stateHash`) and simultaneously updates a batched **Keccak-256 mirror** (`+11.4%` cycles, only `0.8%` of main cells for Keccak), committing a fixed 196-byte payload with **two complementary roots**:

1. **`keccakDiffRoot` (compact per-block/epoch diff tree — ~25k gas, zero custom infra):**
   - A small depth-$\lceil \log_2 N \rceil$ Keccak tree over the accounts modified in that block/epoch.
   - Clients query **any unmodified `mina-indexer` GraphQL API**, compute the `6–8` sibling hashes locally in `< 1 ms`, and submit them directly to Ethereum.
   - Ideal for bridge deposits, withdrawals, actions, and recent zkApp state updates.
2. **`keccakGlobalRoot` (full depth-35 global mirror — ~50k gas, any account):**
   - Mirrors Mina's full depth-35 ledger using `keccak256(abi.encodePacked(poseidonLeafHash, evmFields))` (1 Keccak permutation per leaf).
   - Lets any Ethereum contract prove **any arbitrary or dormant Mina account** at any time via a thin, **100% untrusted** sparse-tree GraphQL sidecar.

Both roots verify with the exact same 6-line `keccak256` Merkle loop in Solidity.

---

## What This Unlocks

- **Trustless Mina ↔ Ethereum bridges:** Move MINA, custom tokens, and zkApp messages without multisigs, committees, or optimistic challenge delays.
- **Cross-chain zkApps & intents:** Execute private or succinct logic on Mina and settle/verify the resulting account or zkApp state on Ethereum and EVM L2s at `~25k–50k` gas per read.
- **Real-time Mina light client on EVM:** At `~$0.04`/proof (and `< 150 s` projected on RTX 4090 with CUDA gnark), a single GPU can track Mina's `~180 s` block cadence in real time.

---

## What's Coming Next

1. **`MinaLightClient.sol` & Foundry tests:** On-chain contract verifying the 324-byte Groth16 proof, decoding the 196-byte `SettlementPublicValues`, and testing both `keccakDiffRoot` and `keccakGlobalRoot` inclusion proofs.
2. **Live `mina-indexer` witness pipeline:** GraphQL witness fetcher for `keccakDiffRoot` + thin untrusted sparse-Keccak sidecar for `keccakGlobalRoot`.
3. **Production VK map & RTX 4090 Groth16:** Full preprocessed-block VK map (static `VK_ROOT` across all inputs) + CUDA-gnark Groth16 proving (`< 150 s` end-to-end).
4. **Testnet deployment & relayer:** Continuous Mina → Ethereum Sepolia light-client relayer.

---

## Layout & Setup

```sh
python3 dependencies/bootstrap.py
CARGO_BUILD_JOBS=1 RAYON_NUM_THREADS=1 cargo test --profile native-test --locked -p pickles-verifier -p cycle-runner -- --test-threads=1
```

| Area | Location | Responsibility |
| --- | --- | --- |
| Pickles verifier & Mina mirror | `crates/pickles-verifier` | Wire validation, recursive checks, optimized MSM/SRS, exact Mina Poseidon + dual Keccak mirror |
| SP1 guests | `crates/o1-verifier` | `o1-verifier` (Pickles-only) and `o1-verifier-settlement` (Pickles + dual-root settlement) |
| Proof pipeline | `crates/pipeline-runner`, `tools/pipeline-cuda` | Core → wrap → Groth16, VK maps, shape audits |
| EVM verification | `contracts-check` | Groth16 verifier contracts and tests |
| Mirror benchmarks | `crates/poseidon-bench`, `crates/cycle-runner` | Dual-tree benchmarks, `settlement-bench`, corruption controls |
| Dependency forks | `vendor/algebra`, `vendor/proof-systems`, `vendor/sp1` | Pinned upstream-based patch stacks (`SP1 6.10.0`, `proof-systems 0.7.0`) |

### Documentation
- **Results & cost:** [running-a-proof.md](docs/running-a-proof.md) · [groth16-runs.md](docs/groth16-runs.md) · [gpu-run-on-runpod.md](docs/gpu-run-on-runpod.md)
- **Design & plans:** [diff-proof-plan.md](docs/diff-proof-plan.md) · [proof-pipeline-plan.md](docs/proof-pipeline-plan.md)
- **Architecture & dev:** [build-flags.md](docs/build-flags.md) · [modules.md](docs/modules.md) · [development-loop.md](docs/development-loop.md) · [validation.md](docs/validation.md)

---

## Credits & Inspirations

🌊 Mizu stands on the shoulders of the Mina and zkVM communities:

- **[o1Labs / o1js](https://github.com/o1-labs/o1js-to-zkvm)** — the upstream `o1js-to-zkvm` verifier, Pickles/Kimchi (`proof-systems`), and mainnet fixtures.
- **[Zeko](https://zeko.io)** — zkVM Pickles verification optimization ideas and configuration techniques.
- **[Nori](https://nori.it)** — Mina → Ethereum bridge architecture, state-commitment design, and SP1/Mina interoperability insights.
- **Zakura** — inspirations and ideas around Mina-to-EVM bridging and settlement.
- **Youtpout** — ideas and discussions on Mina state proofs and verification.
- **[Succinct Labs (SP1)](https://github.com/succinctlabs/sp1)** — the SP1 zkVM, precompile architecture, GPU prover, and ceremony-free Groth16 wrap artifacts.
- **[Mina Foundation](https://minaprotocol.com) & [`mina-indexer`](https://github.com/Granola-Team/mina-indexer)** — Mina's succinct blockchain protocol and GraphQL indexing ecosystem.
- **[arkworks](https://github.com/arkworks-rs)** & **[OpenVM](https://github.com/openvm-org/openvm)** — Pasta field/curve arithmetic and public baseline reference.
