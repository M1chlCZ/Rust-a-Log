"""Package a native release from the separate source checkout. Standard library only."""
import os
from pathlib import Path
import re
import subprocess
import tarfile
import zipfile

source = Path('source')
target = os.environ['RELEASE_TARGET']
tag = os.environ['RELEASE_REF']
version = re.search(r'^version = "([^"]+)"', (source / 'Cargo.toml').read_text(), re.M)[1]
assert tag == f'v{version}', 'Release tag must match Cargo.toml'
binary = source / 'target' / target / 'release' / ('rual.exe' if 'windows' in target else 'rual')
result = subprocess.run([str(binary), '--version'], capture_output=True, text=True, check=True)
assert result.stdout.strip() == f'rual {version}', result.stdout
members = [(binary, binary.name), (source / 'LICENSE', 'LICENSE'), (source / 'README.md', 'README.md')]
for directory in ('docs', 'adapters'):
    for path in (source / directory).rglob('*'):
        if path.is_file() and not any(part in ('target', 'bin', 'obj', 'node_modules', '__pycache__') for part in path.parts):
            members.append((path, str(path.relative_to(source))))
Path('dist').mkdir(exist_ok=True)
name = f'rual-{version}-{target}'
if 'windows' in target:
    with zipfile.ZipFile(f'dist/{name}.zip', 'w', zipfile.ZIP_DEFLATED) as archive:
        for path, member in members:
            archive.write(path, member)
else:
    with tarfile.open(f'dist/{name}.tar.gz', 'w:gz') as archive:
        for path, member in members:
            archive.add(path, arcname=member)
print(f'Packaged {target} {version}')
