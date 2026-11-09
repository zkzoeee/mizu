# Validation lessons from Miza

Reviewed the local `miza-o1js-to-zkvm` checkout at `aab33db009b40d4f92ec77bdd6f4a7628fb0339b`.
The [evidence catalog](../../benchmarks/references/miza-validation-review.json) pins
selected source files and the checks actually executed. Both original repositories
were read only. This is a validation review, not a runtime or optimization import.

Miza has stronger measurement and release-gate structure in several areas. Mizu
already has extensive arithmetic, wire, SRS ownership, fixture and corruption
checks; copying the whole test tree would duplicate useful coverage and inherit
different guest/output contracts.

| Useful lesson | Miza implementation | Mizu application |
| --- | --- | --- |
| Test the comparator | Reject incompatible fixtures, keys, SRS, toolchains, runtime/backend/workloads and outputs; reject bad numeric types, duplicate JSON keys and template records; prevent reusing one measurement as its own control | Strengthen reusable comparison gates around the frozen build/run records; keep changed-output experiments separate |
| Automate guest regression matrices | Six witness mutations include rehashed statements, previous commitments, accumulator, public input and opening, plus a separately baked wrong key | Extend permanent compiled-guest checks beyond the current accumulator/opening controls; adapt failure expectations to Mizu's unsuccessful halt/empty output |
| Check other proof producers in the compiled guest | Mainnet plus seven other retained positive fixtures, with matching family VK/build identities | Keep the existing nine-fixture native matrix and add selected compiled-guest producer coverage before promoting new backends |
| Exercise runtime constraints and interactions | Six Weierstrass curves, two memory-protection modes, changed output/slope/denominator/inverse witnesses, user permissions and cross-chip memory/event checks | Add narrow probes to the existing Pasta harness when changing the SP1 fork; distinguish row checks, lookup interactions and full proof verification |
| Prepare reproducible remote proof work | Exact source, ELF, inputs, worker feature identity, artifact inventory/hashes and resumable complete-verifier cases | Reuse these handoff principles for explicitly requested future proof runs |

Two distinctions matter. `verify_stock_field_parity.py` compares normalized Rust
function bodies against an immutable historical source; it is a source-identity
check, not an independent field-arithmetic oracle. Mizu already has checked
verifier-blob cache identities, so Miza's cache tests are useful design references
rather than evidence that this capability is missing here.

## Executed versus prepared

I ran **13** selected pure-Python comparison/cache gate tests; all passed. They
use synthetic records and temporary cache files. No Miza builds, guest runs or
proofs were requested by this review.

Miza's latest readiness receipt retains a full-guest driver compile and **16**
local orchestration tests using simulated proof artifacts. Its proposed remote
inventory contains eight honest proofs, 24 event/timestamp forgeries, 16 memory
forgeries, 40 arithmetic-witness forgeries, four page-permission forgeries and
eight verifying keys: **100 artifacts**. The source receipt explicitly says
compile-only, with full-guest core/recursive/Groth16 gates still open. Those
counts must not be described as 100 successful cryptographic proof checks.

## Concrete SP1 review follow-up

Miza's Weierstrass doubling AIR adds an explicit nonzero-denominator inverse
check. That check is absent from Mizu's corresponding SP1 fork source. The
runtime README records regenerated double-chip shape costs and witness tests.
This source difference warrants a separate adversarial-witness review of the
current fork and guest input contracts. No exploit of the current Mizu guest
has been demonstrated here, and no runtime patch has been imported.

Keep any resulting correction in its own SP1 topic commit, with updated shape
metadata, native/AIR tests, consumer pins/bundles and the applicable proof gates.
It should not be mixed into a verifier scheduling or guest-cycle experiment.

For the OpenVM-inspired experiments, the current [experiment register](openvm-experiments.md)
separates native checks, compiled-guest execution, changed output contracts,
trace estimates, resource measurements and fresh proof status. The retained
Mizu implementation and the Miza experimental runtime remain separate.
