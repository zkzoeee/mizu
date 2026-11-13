"""Assert every key of a spot-rebuilt map is present in the merged block map.

Program building is deterministic, so a vk rebuilt for index i must hash to the
same digest the block build produced for i. If a slice had built something other
than what its index says, the structural checks (sorted, tiled, 8,176 distinct)
would all still pass -- this is the check that would not.
"""
import sys, struct
ENTRY = 8*4 + 8
def keys(path):
    b = open(path,'rb').read()
    (n,) = struct.unpack('<Q', b[:8])
    assert len(b) == 8 + n*ENTRY, f'{path}: malformed'
    return {struct.unpack('<8I', b[8+i*ENTRY:8+i*ENTRY+32]) for i in range(n)}
big, small = keys(sys.argv[1]), keys(sys.argv[2])
missing = small - big
print(f'{sys.argv[2]}: {len(small)} keys, {len(small)-len(missing)} present in {len(big)}-key map')
if missing:
    print(f'FAIL: {len(missing)} spot-rebuilt keys are NOT in the block map')
    sys.exit(1)
print('OK: every spot-rebuilt key is in the block map')
