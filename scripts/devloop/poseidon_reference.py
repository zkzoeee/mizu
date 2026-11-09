#!/usr/bin/env python3
"""Independent integer oracle for the small exact-Mina-Poseidon operation guest.
Reads published decimal constants; uses Python modular integers, no arkworks,
Montgomery representation, precompile wrapper or guest code.
"""
import argparse
import hashlib
import json
from pathlib import Path
import re

ROOT = Path(__file__).resolve().parents[2]

def reference(iterations):
    if type(iterations) is not int or not 1 <= iterations <= 64:
        raise ValueError('iterations must be in 1..64')
    output = bytearray(iterations.to_bytes(4, 'little'))
    fields = {}
    sources = {}
    for name in ('fp', 'fq'):
        parameters = ROOT / f'vendor/proof-systems/poseidon/src/pasta/{name}_kimchi.rs'
        definition = ROOT / f'vendor/proof-systems/curves/src/pasta/fields/{name}.rs'
        modulus = int(re.search(r'#\[modulus = "(\d+)"\]', definition.read_text())[1])
        values = [int(v) for v in re.findall(r'MontFp!\("(\d+)"\)', parameters.read_text())]
        assert len(values) == 9 + 55 * 3
        assert all(0 <= v < modulus for v in values)
        mds = [values[i:i+3] for i in range(0,9,3)]
        constants = [values[i:i+3] for i in range(9,len(values),3)]
        state = [0,1,modulus-1]
        for _ in range(iterations):
            for round_constants in constants:
                powers = [pow(v,7,modulus) for v in state]
                state = [(sum(a*b for a,b in zip(row,powers)) + c) % modulus
                         for row,c in zip(mds,round_constants)]
        fields[name] = [hex(v) for v in state]
        for value in state:
            output.extend(value.to_bytes(32,'little'))
        for source in (parameters,definition):
            sources[str(source.relative_to(ROOT))] = hashlib.sha256(source.read_bytes()).hexdigest()
    return {'iterations_per_field': iterations, 'public_values_len': len(output),
            'public_values_sha256': hashlib.sha256(output).hexdigest(),
            'public_values_hex': output.hex(), 'fields': fields, 'source_sha256': sources,
            'oracle': 'Python integers: exact x^7, dense 3x3 MDS, then ARK, 55 rounds'}

if __name__ == '__main__':
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--iterations',type=int,default=16)
    parser.add_argument('--output',type=Path,required=True)
    args=parser.parse_args()
    args.output.parent.mkdir(parents=True,exist_ok=True)
    args.output.write_text(json.dumps(reference(args.iterations),indent=2)+'\n')
