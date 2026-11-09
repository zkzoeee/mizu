# Mizu verifier workspace

This repository consolidates Pickles/SP1 iterations on top of o1-labs/o1js-to-zkvm.
Read docs/modules.md and dependencies/README.md before changing module boundaries.

- Core verifier and guest/host paths follow upstream. Receipts are a standalone
  example under examples/settlement; core must not depend on that example.
- Keep cryptographic behavior and strict wire/point validation intact. Preserve
  protocol constants and compatibility environment variables unless deliberately
  migrating their ABI. Keep upstream license notices and authorship.
- Our commits use 0xStruct <0xstruct@gmail.com>; upstream history is unchanged.
- This shared Mac has 16 GB RAM. Use CARGO_BUILD_JOBS=1, RAYON_NUM_THREADS=1 and
  one heavy build/run at a time. Never delete sources or fixtures to free space.
- Native tests, compilation and CPU cycle measurements are appropriate checks.
  Do not generate real proofs, use network proving or rent a GPU without an
  explicit request. Linux/CUDA and honest/adversarial proof gates are separate
  from guest execution validation.
- Dependency changes belong in their upstream-based forks. Update pins, bundles
  and dependency metadata together, then validate the consumer and example.
- For optimization work and dependency ports, update the responsible record in
  docs/optimizations and its evidence in benchmarks/optimization-history.json.
  Follow docs/optimizations/TEMPLATE.md; preserve rejected/superseded outcomes
  and distinguish guest cycles, host/record work and actual proof measurements.
