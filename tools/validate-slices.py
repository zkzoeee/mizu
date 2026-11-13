"""Check the 103 slice reports cover block 3 exactly once, then merge."""
import json, sys, glob, subprocess, os
d = sys.argv[1]
reports = sorted(glob.glob(f'{d}/slice-*.json'), key=lambda p: int(p.split('-')[-1].split('.')[0]))
print(f'{len(reports)} slice reports')
total = 0
seen = {}
block_len = None
for r in reports:
    j = json.load(open(r))
    i = j['slice_index']; n = j['slice_count']
    block_len = block_len or j['block_shapes']
    assert j['block_shapes'] == block_len, f'{r}: block size moved'
    assert j['preprocessed_multiple'] == 3, f'{r}: wrong block'
    assert not j['panic_indices'], f'{r}: panics {j["panic_indices"]}'
    # slice 0 carries the 6 fixed programs; every other slice is block shapes only
    shapes = j['programs_built'] - (6 if i == 0 else 0)
    lo, hi = i * block_len // n, (i + 1) * block_len // n
    assert shapes == hi - lo, f'{r}: built {shapes}, slice spans {hi-lo}'
    assert i not in seen, f'duplicate slice {i}'
    seen[i] = (lo, hi)
    total += shapes
missing = [i for i in range(len(reports)) if i not in seen]
assert not missing, f'missing slices: {missing}'
# the slices must tile the block with no gap and no overlap
edges = [seen[i] for i in sorted(seen)]
assert edges[0][0] == 0 and edges[-1][1] == block_len, f'ends {edges[0]} {edges[-1]}'
for (a_lo, a_hi), (b_lo, b_hi) in zip(edges, edges[1:]):
    assert a_hi == b_lo, f'gap or overlap at {a_hi} vs {b_lo}'
print(f'OK: {len(seen)} slices tile {block_len} block shapes exactly, {total} built + 6 fixed')
assert total == block_len, f'{total} != {block_len}'
