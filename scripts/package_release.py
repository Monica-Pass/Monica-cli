"""Package only explicit release files, then exercise the extracted executables."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import struct
import subprocess
import tarfile
import tempfile
import tomllib
import zipfile


def command(*args, **kwargs):
    return subprocess.check_output(args, text=True, encoding='utf-8', **kwargs).strip()


def windows_imports(binary):
    data = binary.read_bytes()
    pe = struct.unpack_from('<I', data, 0x3C)[0]
    assert data[pe:pe + 4] == b'PE\0\0'
    sections, opt_size = struct.unpack_from('<H', data, pe + 6)[0], struct.unpack_from('<H', data, pe + 20)[0]
    opt = pe + 24
    assert struct.unpack_from('<H', data, opt)[0] == 0x20B
    assert struct.unpack_from('<Q', data, opt + 72)[0] >= 8 * 1024 * 1024, 'CLI stack reserve is too small'
    table = opt + opt_size

    def offset(rva):
        for index in range(sections):
            virtual_size, va, raw_size, raw = struct.unpack_from('<IIII', data, table + index * 40 + 8)
            if va <= rva < va + max(virtual_size, raw_size):
                return raw + rva - va
        raise ValueError('Invalid PE RVA')

    cursor = offset(struct.unpack_from('<I', data, opt + 120)[0])
    imports = []
    while any(data[cursor:cursor + 20]):
        start = offset(struct.unpack_from('<I', data, cursor + 12)[0])
        imports.append(data[start:data.index(b'\0', start)].decode('ascii'))
        cursor += 20
    forbidden = ('libgcc', 'libstdc++', 'libwinpthread', 'vcruntime', 'msvcp')
    assert not any(name.lower().startswith(forbidden) for name in imports), imports
    return imports


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--target', required=True)
    parser.add_argument('--engine-revision', required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    repo = Path(__file__).resolve().parents[1]
    version = tomllib.loads((repo / 'Cargo.toml').read_text())['package']['version']
    revision = command('git', 'rev-parse', 'HEAD', cwd=repo)
    assert command('git', 'rev-parse', 'HEAD', cwd=repo.parent / 'mdbx') == args.engine_revision
    windows = args.target == 'x86_64-pc-windows-msvc'
    assert windows or args.target == 'x86_64-unknown-linux-gnu'
    suffix = '.exe' if windows else ''
    binary = repo / 'target' / args.target / 'release' / ('monica-pass' + suffix)
    assert command(str(binary), '--version').endswith(' ' + version)
    if windows:
        runtime = {'imports': windows_imports(binary), 'minimum_os': 'Windows 10 x64'}
    else:
        linked = command('ldd', str(binary))
        assert 'not found' not in linked, linked
        versions = re.findall(r'GLIBC_(\d+)\.(\d+)', command('readelf', '-V', str(binary)))
        maximum = max((int(a), int(b)) for a, b in versions)
        assert maximum <= (2, 35), maximum
        runtime = {'glibc_required_by_binary': '.'.join(map(str, maximum)), 'minimum_os': 'Linux x86_64, glibc 2.35+'}
    args.output.mkdir(parents=True, exist_ok=True)
    stem = f'monica-cli-{version}-' + ('windows-x86_64' if windows else 'linux-x86_64')
    with tempfile.TemporaryDirectory(prefix='monica-package-') as temporary:
        root = Path(temporary) / stem
        root.mkdir()
        for name in ['monica-pass', 'monica']:
            target = root / (name + suffix)
            shutil.copy2(binary, target)
            target.chmod(0o755)
        (root / 'monica-pass.portable').touch()
        documents = ['LICENSE', 'THIRD_PARTY_NOTICES.md', 'SECURITY.md', 'README.md', 'README.en.md', 'RELEASE-NOTES.md']
        # Include tracked documentation only, so README links work offline
        # without pulling local previews, caches or test output into the package.
        documents.extend(filter(None, command('git', 'ls-files', '-z', '--', 'docs', cwd=repo).split('\0')))
        for name in documents:
            destination = root / name
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(repo / name, destination)
        shutil.copy2(repo / 'docs/release-install.md', root / 'INSTALL.md')
        if windows:
            (root / 'scripts').mkdir()
            shutil.copy2(repo / 'scripts/install.ps1', root / 'scripts/install.ps1')
        metadata = json.loads(command('cargo', 'metadata', '--locked', '--format-version', '1', cwd=repo))
        dependencies = []
        for package in metadata['packages']:
            dependencies.append({k: package.get(k) for k in ['name', 'version', 'license', 'repository', 'source']})
            folder = Path(package['manifest_path']).parent
            for pattern in ['LICENSE*', 'COPYING*', 'NOTICE*', 'COPYRIGHT*']:
                for source in folder.glob(pattern):
                    if source.is_file():
                        dest = root / 'licenses' / (package['name'] + '-' + package['version']) / source.name
                        dest.parent.mkdir(parents=True, exist_ok=True)
                        shutil.copy2(source, dest)
        (root / 'DEPENDENCIES.json').write_text(json.dumps(dependencies, indent=2) + '\n')
        info = {'version': version, 'cli_revision': revision, 'mdbx_revision': args.engine_revision,
                'target': args.target, 'rustc': command('rustc', '--version'),
                'binary_sha256': hashlib.sha256(binary.read_bytes()).hexdigest(), **runtime}
        (root / 'BUILD-INFO.json').write_text(json.dumps(info, indent=2) + '\n')
        archive = args.output / (stem + ('.zip' if windows else '.tar.gz'))
        if windows:
            # Registry archives can preserve 1970 mtimes for license files.
            # Clamp to ZIP's supported range while retaining their contents.
            with zipfile.ZipFile(archive, 'w', zipfile.ZIP_DEFLATED, compresslevel=9, strict_timestamps=False) as output:
                for file in sorted(root.rglob('*')):
                    if file.is_file():
                        output.write(file, file.relative_to(root.parent))
        else:
            with tarfile.open(archive, 'w:gz') as output:
                output.add(root, arcname=stem)
        extracted = Path(temporary) / 'extracted'
        extracted.mkdir()
        if windows:
            with zipfile.ZipFile(archive) as source:
                source.extractall(extracted)
        else:
            with tarfile.open(archive) as source:
                source.extractall(extracted, filter='data')
        for name in ['monica-pass', 'monica']:
            executable = extracted / stem / (name + suffix)
            assert command(str(executable), '--version').endswith(' ' + version)
            discovery = json.loads(command(str(executable), 'commands', '--json'))
            assert any(c['name'] == 'direct-config' for c in discovery['data']['commands'])
        if windows:
            installed = Path(temporary) / 'installed'
            subprocess.run(['powershell', '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File',
                str(extracted / stem / 'scripts/install.ps1'), '-InstallDir', str(installed),
                '-NoPath', '-NoShortcut'], check=True)
            assert command(str(installed / 'monica.exe'), '--version').endswith(' ' + version)
        # No vault or client settings may enter a release archive.
        assert not (extracted / stem / 'data').exists()
        checksum = hashlib.sha256(archive.read_bytes()).hexdigest()
        archive.with_name(archive.name + '.sha256').write_text(f'{checksum}  {archive.name}\n')
        print(json.dumps(info, indent=2))
        print(f'Packaged and verified {archive.name}')


if __name__ == '__main__':
    main()
