# Optimization record template

Copy this entry into the responsible area and add a stable catalog reference.
Use a Mizu ID such as `mizu-ipa-prefix-ownership`; original `rNN` IDs belong to
the historical source workspace.

## <ID>: <concrete mechanism or question>

- **Status:** retained / rejected / superseded / prototype / measurement-only /
  validation-only / pending. Say which parts, if the outcome is mixed.
- **Area and owner:** root module or dependency fork; exact changed files.
- **Implementation identity:** root revision, upstream base, fork pin and topic
  commit(s). Record dirty source hashes if measuring before commit.
- **Predecessors/corrections:** earlier record IDs; what changed or was retracted.
- **Mechanism:** work removed or representation/schedule changed; invariants kept.
- **Selection:** exact flags/features, defaults and fallback conditions. Source
  presence and optional flags do not imply selection.
- **Control/candidate:** full measurement paths/hashes, source/ELF/runner/toolchain
  identities, SP1 version, fixture/VK/input hashes, ABI and exact output hash,
  memory/shard/retention/parallelism options and hardware for host-time claims.
- **Result:** before, after, units, signed delta and denominator. Use null/unknown
  when unmeasured. Keep wall-clock repetitions/spread; do not infer proof time
  from guest cycles or stored cells. Distinguish local-stage and whole-program
  results and instrumented from clean measurements.
- **Correctness evidence:** equivalence/oracle/boundary tests; honest and corrupt
  real-guest runs; applicable setup, ownership, wire and field/point contracts.
- **Validation scope:** separate compilation, CPU execution, row/AIR tests,
  fresh proofs, complete verification, reload/key derivation and Linux/CUDA.
- **Decision and limits:** why retained/rejected; image/memory cost; what remains
  untested; whether this is a full implementation, standalone prototype or model.
- **Source provenance:** original file paths, capture time, SHA-256 and exact
  report field/section. Preserve the corrected and correcting references.
- **Upstream sync:** topic commits to replay; overlaps absorbed upstream; API or
  format changes that need a consumer adaptation and fresh validation.

Add/update the area membership and evidence in
`benchmarks/optimization-history.json`, and the runnable benchmark recipe when a
candidate becomes selected. Keep previous measurements immutable.
