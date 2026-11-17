#!/usr/bin/env python3
"""Sign filesystem Mach-O code inside Java resources, before signing the app.

JARs retain exact upstream bytes/signatures, including loader-extracted native
libraries. Java launchers need JIT and library-loading entitlements for those
upstream libraries. This does not execute any downloaded program.
"""
import argparse
import os
from pathlib import Path
import subprocess

MAGIC = {b'\xfe\xed\xfa\xce', b'\xce\xfa\xed\xfe', b'\xfe\xed\xfa\xcf', b'\xcf\xfa\xed\xfe', b'\xca\xfe\xba\xbe', b'\xbe\xba\xfe\xca', b'\xca\xfe\xba\xbf', b'\xbf\xba\xfe\xca'}


def sign(root, identity):
    entitlements = Path(__file__).with_name('macos-java-entitlements.plist')
    paths = []
    for path in sorted(root.rglob('*')):
        if path.is_symlink():
            raise ValueError(f'Resource symlink: {path}')
        if not path.is_file():
            continue
        with path.open('rb') as stream:
            if stream.read(4) not in MAGIC:
                continue
        kind = subprocess.check_output(['/usr/bin/file', '-b', str(path)], text=True)
        if 'Mach-O' not in kind:
            continue  # CAFEBABE also prefixes ordinary Java class files.
        if 'arm64' not in kind:
            raise ValueError(f'Non-arm64 filesystem Mach-O: {path}')
        args = ['/usr/bin/codesign', '--force', '--sign', identity]
        if identity != '-':
            args += ['--options', 'runtime', '--timestamp']
        if 'executable' in kind:
            args += ['--entitlements', str(entitlements)]
        subprocess.run([*args, str(path)], check=True, capture_output=True)
        subprocess.run(['/usr/bin/codesign', '--verify', '--strict', str(path)], check=True)
        paths.append(path)
    # The original JDK is a code bundle. Seal it after its libraries/launchers.
    runtime = root / 'java/runtime'
    if (runtime / 'Contents/Info.plist').exists():
        args = ['/usr/bin/codesign', '--force', '--sign', identity]
        if identity != '-':
            args += ['--options', 'runtime', '--timestamp']
        subprocess.run([*args, str(runtime)], check=True, capture_output=True)
        subprocess.run(['/usr/bin/codesign', '--verify', '--strict', '--deep', str(runtime)], check=True)
    print(f'Signed and verified {len(paths)} nested Mach-O files ({"ad hoc" if identity == "-" else "configured identity"})')


if __name__ == '__main__':
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('root', type=Path)
    args = p.parse_args()
    sign(args.root, os.environ.get('SIGNING_IDENTITY') or '-')
