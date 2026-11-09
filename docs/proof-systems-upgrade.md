# Proof-systems 0.7 upgrade — 2026-11-09

The consumer now uses official proof-systems `b1d9829fa0b2bb8e15494df1e5e0c6741394075b`
(workspace version **0.7.0**) plus four optimization commits. This is the exact
proof-systems revision selected through Mina by our o1js-to-zkvm baseline:

```text
o1js-to-zkvm 095a5c0 -> Mina e8c2137 -> proof-systems b1d9829
```

SP1 remains **6.10.0** with the same Pasta fork. Arkworks remains **0.5.0** with
the same algebra fork. The previous optimized proof-systems pin was `9a47c4973`
on official 0.4.1 base `3abb66b`. The new pin is `e7f63a64d8dff5d9c9bc6809dbf1e706f61fef6a`.
The measured root revision is `d0bd6cb32b3cc0c9450a6b2e58df4384419a299d`; subsequent changes
only document the measurement and refresh inventories.

## Separate optimization commits

| Commit | Area |
| --- | --- |
| `9f37ee019` | Add canonical Pasta arithmetic, GLV and SP1 point operations |
| `3d579636d` | Add canonical Poseidon and retained sponge fast paths |
| `af2c000a7` | Add canonical IPA and reusable SRS representations |
| `e7f63a64d` | Optimize verifier challenges, expressions and borrowed commitments |

The root integration commit updates the consuming manifests/lockfiles, SRS cache
seeding, the direct-source test harness and the pinned bundle metadata. The fork
keeps upstream's native layout and authorship. Our local diff is now 28 paths
rather than 33: upstream already supplies the field hooks, deterministic lookup
ordering and chunked lookup-selector fix.

The compatibility adaptations are kept with their responsible optimization:

- Pasta arithmetic: reuse upstream field hooks and pin its SP1 runtime reference
  to 6.10.0; retain canonical arithmetic, checked hints, GLV and point operations.
- Poseidon: retain compile-time constants and prepared canonical arithmetic; use
  upstream's no-std-compatible `LazyLock` for prepared-table initialization.
- IPA/SRS: retain borrowed immutable generators, exact-capacity/canonical IPA and
  fixed-prefix support. Follow upstream's Arc-backed host / Rc-backed no-std
  caches, with distinct full-domain and prefix caches. Seed prefixes through a
  public setter; expose the SRS path helper for build-cache input fingerprints.
- Kimchi: retain authenticated precomputed index digests, trusted opening-verifier
  callbacks, optimized commitments and setup. Reuse upstream's post-digest
  oracle replay rather than carrying another local implementation.

The guest dependencies disable default features. Host-side verifier builds enable
upstream's parallel/std features explicitly; local validation uses one Rayon
thread. Root and receipt lockfiles now resolve seven proof-systems packages to
0.7.0 and add upstream's `spin` dependency at 0.10.1. The independent Pasta tools
do not depend on proof-systems. All four consuming lockfiles retain only SP1
6.10.0. This audit validates the consuming workspaces, not every unrelated
package in the upstream proof-systems development workspace.

## Cycle comparison

| Metric | Previous optimized 0.4.1 | Upgraded optimized 0.7.0 |
| --- | ---: | ---: |
| Guest cycles | 142,660,047 | 142,650,295 |
| Public output | `01` | `01` |
| Pallas additions | 697,988 | 697,988 |
| Vesta additions | 1,310,683 | 1,310,683 |
| UInt256 multiplications | 533,878 | 533,878 |

The change is **-9,752 cycles (-0.006836%)**:
effectively unchanged performance. Fixture hashes, serialized proof input,
all 40 retained settings, compiler/builder identity, output ABI and 2 GiB guest
memory limit match. Pasta add/double counts and UInt256 multiplication counts
are identical. This compares the complete dependency/API/feature migration;
it is not an independent ablation of individual optimizations.

| Instrumented region | Previous | Upgraded | Delta |
| --- | ---: | ---: | ---: |
| `pickles-accumulator` | 75,309,755 | 75,309,755 | +0 |
| `pickles-wrap-kimchi` | 54,852,878 | 54,870,559 | +17,681 |
| `pickles-wrap-prepare` | 5,835,975 | 5,800,067 | -35,908 |
| `setup` | 4,234,120 | 4,233,620 | -500 |
| `verify` | 138,367,029 | 138,357,069 | -9,960 |

Instrumented regions include nested measurements and do not add to the total.

## Validation

| Check | Result |
| --- | --- |
| Core verifier and execution tools | 209 passed |
| Optional receipt example | 20 passed |
| Pasta harness | 20 passed |
| No-std core compilation | Passed |
| Host CLI and core SP1 guest compilation | Passed |
| Optional receipt SP1 guest compilation | Passed |
| Fresh checkout, fresh local forks and bundle restoration | Passed |
| Full locked offline dependency resolution in fresh checkout | Passed |
| Fork pins, trees, ancestry and bundle hashes | Passed |
| Corrupted accumulator challenge, same optimized ELF | Rejected, guest exit 1 |
| Corrupted wrap opening, same optimized ELF | Rejected, guest exit 1 |

No tests failed or were ignored. No fresh SP1 proof generation, network proving,
Linux execution or CUDA run was performed. CPU execution and native verification
do not establish the soundness of newly generated SP1 proofs.

The [measurement and comparison](../benchmarks/core-proof-systems-0.7-sp1-6.10.json)
records immutable pins, tool identities, fixture/output/ELF/runner hashes and
artifact hashes. The [previous measurement](../benchmarks/core-sp1-6.10.json)
remains unchanged. Full logs, frozen binaries, per-file source hashes and both
rejection controls are in the sibling `mizu-proof-systems-0.7-validation-20261109`
directory. The old fork is also preserved in the external recovery bundle
`mizu-history-backup-20261109/proof-systems-pre-0.7.bundle`.

Reproduce the retained core measurement with:

```sh
python3 scripts/benchmark.py run --output benchmarks/runs/proof-systems-0.7
```

For future upgrades, rebase the four topic commits onto an explicit official
base, adapt the consumer, repeat these checks, then update the Gitlink, branch,
bundle and `dependencies/lock.json` atomically. See the
[dependency guide](../dependencies/README.md) and
[current optimization inventory](dependencies-and-optimizations.md).
