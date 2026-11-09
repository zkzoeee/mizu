# Consolidation validation — 2026-11-09

This records the original consolidation baseline. For the subsequent
proof-systems 0.7 migration and current cycle result, see
[upgrade validation](proof-systems-upgrade.md).

The clean root patch stack starts directly at official o1js-to-zkvm `095a5c0`.
Every new commit excludes the retired application folders; upstream authors are
unchanged and our commits use `0xStruct <0xstruct@gmail.com>`. Three dependency
forks keep their upstream layouts and delete no upstream files. Their ordered
module patches and exact pins are in `dependencies/lock.json`.

| Check | Result |
| --- | --- |
| Core verifier and execution-tool tests | 209 passed |
| Optional receipt example tests | 20 passed |
| Pasta harness tests | 20 passed |
| No-std core check | Passed |
| Upstream-style host and SP1 guest compilation | Passed |
| Standalone Pasta harness compilation | Passed |
| Fresh checkout and fresh local forks, restored from bundles | Passed |
| Locked dependency resolution in the fresh checkout | Passed |
| Preserved dependency implementation files | 627 matched archived bytes |
| Corrupted accumulator challenge in the real core guest | Rejected, exit 1 |

The new optimized core guest measures **142,660,047 cycles**, exits successfully
and commits exactly one byte (`01`). Its Pasta and UInt256 call counts match the
previous implementation. The retained optimization configuration, fixture hashes,
compiler/builder identity, source digest, ELF hash and runner hash are recorded in
[the measurement](../benchmarks/core-sp1-6.10.json).

The historical receipt guest measured 142,692,323 cycles on SP1 6.10 and
142,692,781 on 6.1. Those used a 577-byte receipt output. They remain historical
receipt references, not a direct performance comparison with the boolean guest.
Protocol domains and receipt encodings are preserved in the optional example.

No new proof or CUDA run was performed. Linux CI jobs are configured but were
not executed locally. Real proof soundness/continuity and Linux/CUDA gates remain
separate validation before promoting the new SP1 fork for production proving.

Full logs, frozen binaries and per-file source hashes are saved in the sibling
`mizu-clean-validation-20261109` directory. The previous working tree, complete
project/fork Git bundles and a 4,546-path disposition inventory are in
`mizu-history-backup-20261109`. Old import branches have been removed from the
active repositories; the recovery bundles preserve those histories.
