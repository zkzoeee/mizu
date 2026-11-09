# OpenVM-inspired Mizu experiments

These are independent adaptations of the ideas in [the external review](o1-openvm-review.md),
not imports of its dependency forks. SP1 remains 6.10.0; all three dependency pins
remain unchanged. The retained configuration and existing measurements are controls.

## Scope and ownership

| ID | Mechanism | Implementation / selection |
| --- | --- | --- |
| mizu-openvm-allocation | Stack cross-field bytes; iterate CIP evaluation columns without transpose vectors | Root deferred.rs / experimental_cip.rs; MIZU_STACK_CROSS_FIELD=1 and MIZU_CIP_MODE=borrowed |
| mizu-openvm-fused-cip | Two evaluation-point dot products in four-term sum_of_products blocks | Same files; MIZU_CIP_MODE=fused, with stack conversion enabled |
| mizu-openvm-batch-accumulator | Transcript-weighted accumulator coefficient combination; one large MSM and a small commitment MSM | Root accumulator_batch.rs / serialize.rs; bounded batch guest; MIZU_BATCH_ACCUMULATOR=1 |
| mizu-openvm-statement-digest | Keccak digest binding the exact embedded VK JSON identity, statement length and canonical fields | Root attestation.rs; o1-verifier statement-digest feature |
| mizu-openvm-trace-accounting | Bounded replay estimates, event rows and measured runner RSS | cycle-runner --profile-precompiles; no proving APIs |

All variants are opt-in. The normal single-proof guest keeps its boolean ABI.
The ordinary CIP and per-proof accumulator remain reference implementations.
The root owns Pickles-specific schedules and output policy; existing algebra
sum_of_products and SP1 instrumentation are reused without editing their forks.
When syncing upstream, replay these separate root commits and re-run equivalence
and guest validation against the newly pinned dependency implementations.

## Validation and measurement recipe

`scripts/openvm_experiments.py all --output /absolute/new/directory` builds frozen
control/candidate guests and a runner, then executes honest and corrupted inputs.
Phases build/single/batch/profile can be run separately. Completed executions are
reused only with matching source, script, runner, ELF, fixture and result hashes.

The guest batch count is checked before allocating proof storage, bounded to 1..8.
The batch algorithm rejects empty inputs, wrong SRS length and coefficient geometry;
it binds the ordered challenges/commitments, VK digest, count and SRS domain into
a domain-separated Fiat–Shamir challenge, explicitly excluding zero. Native checks
include corruption in every position, opposite cancelling errors, order binding,
truncated SRS, and round-count overflow. This is an experimental construction,
not an independent cryptographic audit.

The cycle batch workload repeats the one available mainnet proof N times. N=1,
2,4,8 compare the same input sequence/output on the same batch guest ABI, using
per-proof versus combined accumulators. These are not diverse production batches.
The batch guest loads the complete embedded wrap setup; its N=1 total must not be
used as an isolated batching comparison with the prefix-trimmed ordinary guest.

The digest wire is Keccak256(ASCII `mizu/verified-statement/v1` || VK-id[32] ||
field-count-u64-LE || canonical-field-u256-LE...). VK-id is SHA256 of the exact
embedded VK JSON. It is versioned Mizu encoding, not Ethereum ABI or OpenVM's
attestation format. The guest emits exactly 32 digest bytes only after successful
verification. The runner computes and checks the expected output, and statement,
accumulator and opening corruptions must halt unsuccessfully before any output.

Compilation, native tests, CPU execution, replay/event estimates and runner RSS
are separate evidence. None establishes fresh proof generation, complete proof
verification, from-disk proof verification, CUDA cost or end-to-end proving time.
Raw /usr/bin/time -l RSS includes the runner and JIT; it is not prover memory.

## Results

Pending matched execution. Prototype source commits: 7f9d257 (allocation/CIP),
4fde221 (batch), 50eaf97 (digest), acbdce5 (measurement tools). The predecessor is
142,650,295 cycles on the retained core checkpoint; fresh same-source controls
will be used for attributing these experiments. No defaults have been changed.
