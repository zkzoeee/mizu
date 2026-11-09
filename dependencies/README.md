# Dependency patch stacks and upstream sync

`lock.json` is the source of truth for each official upstream base, our pinned
commit/tree, ordered patch subjects and bundle hash. There are three active forks.
Upstream trees and layouts remain intact; unrelated source, tests and assets are
not deleted. `.gitmodules` names the official source repositories. Bootstrap
restores our unpublished commits into local bare forks and overrides the clone
URLs locally before checking out the immutable pins.

See the [version and optimization inventory](../docs/dependencies-and-optimizations.md)
for modified crates, other dependency sources and coordinated upgrade steps;
the [file appendix](../docs/file-changes.md) lists every changed path and retained
benchmark flag. Exact revisions and lockfile summaries are also available in
[`inventory.json`](inventory.json).

| Fork | Official base | Our modules |
| --- | --- | --- |
| algebra | v0.5.0, `7ad88c4` | Field arithmetic; GLV/MSM; polynomial evaluation |
| proof-systems | 0.7.0, `b1d9829` | Canonical Pasta; Poseidon; IPA/SRS; Kimchi verifier |
| SP1 | 6.10.0, `d83ed91` | Pasta curves; execution/AIRs; registry-consumer manifests; counters |

The proof-systems base matches the exact revision pinned through Mina by our
official o1js-to-zkvm baseline. Its four optimization commits preserve upstream's
no-std feature split and SRS ownership conventions. The fork's SP1 dependency
is aligned to 6.10.0. SP1's four patched
crates use registry dependencies for unchanged SP1/SLOP packages, keeping a single
6.10 type graph when used by the registry SDK. That manifest adapter is its own
commit; the actual source stays at upstream paths.

## Updating upstream

1. Fetch the official remote in the fork you are updating. Keep the old base/tip
   from `lock.json` and choose an explicit new upstream base.
2. Rebase that fork's small series (the branch recorded in `lock.json`) onto the new base. Review with
   `git range-diff OLD_BASE..OLD_TIP NEW_BASE..NEW_TIP`; resolve API changes in the
   responsible module rather than copying a full repository tree.
3. Test from this consumer: native verifier/arithmetic tests, no-std check,
   guest/host compilation and CPU honest/corrupt executions. Test the optional
   receipt workspace separately when its shared dependency changes. For SP1 AIR
   changes, Linux/CUDA and honest/adversarial proof gates remain additional work
   on an explicitly authorized, appropriately sized host.
4. Export the updated branch with `git bundle create <bundle> <branch> ^<base>`.
   Update the Gitlink, base/revision/tree, ordered patch list and bundle hash in
   `lock.json` in one dependency-bump commit. Run bootstrap and verify.py.

For root updates, fetch o1-labs/o1js-to-zkvm and rebase the root module commits
onto the chosen upstream commit. Preserve `crates/o1-verifier` and
`crates/o1-verifier-host` so upstream changes land in their original paths.
Update the root upstream base in `lock.json`. Keep optimization changes,
measurement tools and optional application examples in separate commits.

Forks are local until published. Once repository URLs are known, publish these
same upstream-based branches and update `.gitmodules` to those fork URLs; pins
and implementation do not need another rewrite. No repository URL is invented.

```sh
python3 dependencies/bootstrap.py --offline
python3 dependencies/verify.py
```
