#!/usr/bin/env python3
"""Build an explicitly selected local artwork pack matching the committed index."""
import argparse
import hashlib
import json
from pathlib import Path
import struct


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--asar', type=Path, required=True, help='Local app.asar input')
    parser.add_argument('--output-pack', type=Path, required=True, help='Local output outside the repository')
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    output = args.output_pack.resolve()
    if output == root or root in output.parents:
        parser.error('Choose an output outside the repository')
    index = json.loads((root / 'src/assets/emojis/zalo/index.json').read_text())
    pack = bytearray()
    with args.asar.open('rb') as archive:
        fields = struct.unpack('<4I', archive.read(16))
        header = json.loads(archive.read(fields[3]))
        base = 8 + fields[1]
        for sprite in index['sprites']:
            node = header
            for part in sprite['member'].split('/'):
                node = node['files'][part]
            if node.get('unpacked'):
                raise ValueError('Unpacked member is unsupported')
            archive.seek(base + int(node['offset']))
            raw = archive.read(node['size'])
            if (len(raw) != sprite['length'] or len(pack) != sprite['offset']
                    or hashlib.sha256(raw).hexdigest() != sprite['sha256']):
                raise ValueError('Local artwork differs from the committed index')
            pack.extend(raw)
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_bytes(pack)
    if output.read_bytes() != pack:
        raise ValueError('Output readback mismatch')
    print(f'EMOJI PACK: sprites={len(index["sprites"])} bytes={len(pack)} sha256={hashlib.sha256(pack).hexdigest()}')


if __name__ == '__main__':
    main()
