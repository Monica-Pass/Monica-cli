"""Bundle immutable CLI + MDBX trees and every locked registry dependency."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import tomllib


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--engine-revision', required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    repo = Path(__file__).resolve().parents[1]
    version = tomllib.loads((repo / 'Cargo.toml').read_text())['package']['version']
    cli_revision = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=repo, text=True).strip()
    args.output.mkdir(parents=True, exist_ok=True)
    stem = f'monica-cli-{version}-source'
    with tempfile.TemporaryDirectory(prefix='monica-source-') as temporary:
        root = Path(temporary) / stem
        root.mkdir()
        for name, source, revision in [('monica-pass-cli', repo, cli_revision), ('mdbx', repo.parent / 'mdbx', args.engine_revision)]:
            destination = root / name
            destination.mkdir()
            process = subprocess.Popen(['git', 'archive', revision], cwd=source, stdout=subprocess.PIPE)
            with tarfile.open(fileobj=process.stdout, mode='r|') as archive:
                archive.extractall(destination, filter='data')
            assert process.wait() == 0
        cli = root / 'monica-pass-cli'
        config = subprocess.check_output(['cargo', 'vendor', '--locked', '../vendor'], cwd=cli, text=True)
        (cli / '.cargo').mkdir(exist_ok=True)
        (cli / '.cargo/config.toml').write_text(config)
        subprocess.run(['cargo', 'metadata', '--locked', '--offline', '--format-version', '1'], cwd=cli, stdout=subprocess.DEVNULL, check=True)
        (root / 'SOURCE-REVISIONS.json').write_text(json.dumps({'cli': cli_revision, 'mdbx': args.engine_revision, 'version': version}, indent=2) + '\n')
        (root / 'BUILD.md').write_text('Build from monica-pass-cli/: cargo build --release --locked --offline\nRust 1.97.0 and a native C toolchain are required. Registry dependencies are vendored.\nSee monica-pass-cli/docs/source-checkout.md for platform details.\n')
        archive = args.output / (stem + '.tar.gz')
        with tarfile.open(archive, 'w:gz') as output:
            output.add(root, arcname=stem)
        checksum = hashlib.sha256(archive.read_bytes()).hexdigest()
        archive.with_name(archive.name + '.sha256').write_text(f'{checksum}  {archive.name}\n')
        print(f'Bundled and checked offline metadata: {archive.name}')


if __name__ == '__main__':
    main()
