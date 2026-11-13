"""Merge recursion VK maps: union the digests, re-index in BTreeMap order.

The controller does exactly this when it merges chunk outputs
(worker/controller/vk_tree.rs): collect every vk into a BTreeSet, then
`.enumerate()` into a BTreeMap. So a map merged here is byte-identical to one a
single build would have produced over the union -- and `from_map` derives the
root from the map alone, so the root matches too.

Wire format: bincode BTreeMap<[SP1Field; 8], usize> = u64 LE length, then each
entry as 8 x u32 LE digest + u64 LE index.
"""
import struct, sys, hashlib

ENTRY = 8 * 4 + 8

def parse(path):
    b = open(path, 'rb').read()
    (n,) = struct.unpack('<Q', b[:8])
    assert len(b) == 8 + n * ENTRY, f"{path}: {len(b)} bytes, expected {8 + n*ENTRY} for {n} entries"
    out = []
    for i in range(n):
        off = 8 + i * ENTRY
        digest = struct.unpack('<8I', b[off:off+32])
        (idx,) = struct.unpack('<Q', b[off+32:off+40])
        out.append((digest, idx))
    return out, b

def dump(entries):
    b = struct.pack('<Q', len(entries))
    for i, d in enumerate(entries):
        b += struct.pack('<8I', *d) + struct.pack('<Q', i)
    return b

def check(path):
    """The file must already be sorted by our comparator, with values 0..n-1."""
    entries, raw = parse(path)
    digests = [d for d, _ in entries]
    idxs = [i for _, i in entries]
    ok_sorted = digests == sorted(digests)
    ok_idx = idxs == list(range(len(idxs)))
    ok_round = dump(digests) == raw
    print(f"{path}: {len(entries)} keys  sorted={ok_sorted}  values_0..n-1={ok_idx}  roundtrip={ok_round}")
    return ok_sorted and ok_idx and ok_round

if __name__ == '__main__':
    if sys.argv[1] == 'check':
        sys.exit(0 if all(check(p) for p in sys.argv[2:]) else 1)
    out = sys.argv[1]
    union = set()
    for p in sys.argv[2:]:
        entries, _ = parse(p)
        assert check(p), f"{p} failed its invariants"
        union |= {d for d, _ in entries}
    merged = dump(sorted(union))
    open(out, 'wb').write(merged)
    print(f"merged {len(union)} keys -> {out}  sha256={hashlib.sha256(merged).hexdigest()[:16]}")
