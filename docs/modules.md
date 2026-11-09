# Module ownership and consolidation map

The [optimization work records](optimizations/README.md) group the retained
mechanisms, historical rounds, evidence and rejected alternatives by work area.

The root patch stack starts at official o1js-to-zkvm `095a5c0`. Upstream commits
and authors are preserved. Consolidated commits describe the resulting modules;
there are no application snapshots followed by branding or deletion commits.

| Previous area | Clean location / decision | Owner |
| --- | --- | --- |
| Pickles verifier and focused arithmetic tests | `crates/pickles-verifier` | o1js-to-zkvm patch stack |
| Settlement verifier entry point and setup cache | Core `crates/o1-verifier`; receipts isolated below | o1js-to-zkvm patch stack |
| Upstream host CLI | `crates/o1-verifier-host` | o1js-to-zkvm patch stack |
| Cycle/operation runners | `crates/cycle-runner` | Measurement tools |
| Standalone Poseidon screening guest | `crates/poseidon-bench` | Measurement tools; independent of verifier setup |
| Cached native / screening / paired / promotion workflow | `scripts/dev.py`, `scripts/devloop`, `scripts/tests` | Measurement tools; [development guide](development-loop.md) |
| Receipt logic and types | `examples/settlement/{program,types}` | Optional standalone workspace |
| Algebra arithmetic / GLV / MSM | `vendor/algebra` | Arkworks fork, three commits |
| Pasta, Poseidon, IPA, Kimchi fast paths | `vendor/proof-systems` | Official 0.7.0 / `b1d9829`-based fork, four commits |
| Published SP1 crate copies | `vendor/sp1` in native upstream layout | SP1 6.10 fork, four commits |
| Active Pasta harness and micro-guests | `tools/pasta-*` | Focused validation tools |
| Benchmark recipes / selected measurements | `benchmarks` | Performance evidence |
| API, browser apps, Ethereum contracts, deployment/container setup, application SDKs | Archived outside this history | Application scope removed |
| Old fork variants, full run dumps, handoffs, agent/vendor skills and branding/import manifests | Archived outside this history | Recovery / historical evidence |

Core crates do not depend on the receipt example. Proof-systems owns reusable
curve, Poseidon and commitment code; Pickles-specific deferred/statement checks
stay in pickles-verifier. SP1 owns syscall dispatch, AIRs and executor records;
no SP1 source is copied into the root repository's commit stack.

The receipt example preserves its protocol domain strings, wire fields and
versioned outputs. Those identifiers are compatibility data, not project
branding. Upstream copyright and license notices also remain intact.

A source-file inventory and complete Git bundles of the previous project/fork
histories live in the sibling `mizu-history-backup-20261109` recovery archive.
Removed tracked files never enter this clean root patch stack. Untracked local
configuration and old generated assets are retained in that archive too.
