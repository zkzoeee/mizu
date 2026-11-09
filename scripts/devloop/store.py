"""Content-addressed, fail-closed artifact storage. No cache hit is a new sample."""
import contextlib
import fcntl
import hashlib
import io
import json
import os
import re
from pathlib import Path
import subprocess
import tarfile
import gzip
import time
import uuid

from .compare import read_record

ROOT = Path(__file__).resolve().parents[2]
STORE = ROOT / 'benchmarks/.dev-cache'


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def key(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True, allow_nan=False).encode()).hexdigest()


def write(path, value):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(path.name + '.' + uuid.uuid4().hex + '.tmp')
    temporary.write_text(json.dumps(value, indent=2, sort_keys=True, allow_nan=False) + '\n')
    temporary.replace(path)


@contextlib.contextmanager
def serial():
    path = ROOT / 'target/.mizu-dev.lock'
    path.parent.mkdir(exist_ok=True)
    with path.open('a') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        yield


def environment(settings=None):
    env = {k: v for k, v in os.environ.items() if not k.startswith(('ZEKO_', 'MIZU_'))}
    env.update(settings or {})
    env.update(CARGO_BUILD_JOBS='1', RAYON_NUM_THREADS='1', RUST_TEST_THREADS='1',
               RUSTUP_TOOLCHAIN='1.92.0', CARGO_TARGET_DIR=str(ROOT / 'target'), CARGO_NET_OFFLINE='true')
    return env


def ambient_identity(env, directory=ROOT):
    # Hash compiler overrides/configs without disclosing their values.
    names = ('RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS', 'RUSTC', 'RUSTC_WRAPPER',
             'RUSTC_WORKSPACE_WRAPPER', 'CARGO_INCREMENTAL', 'RUSTC_BOOTSTRAP')
    overrides = {k: hashlib.sha256(v.encode()).hexdigest() for k, v in env.items()
                 if k in names or k.startswith(('CARGO_PROFILE_', 'CARGO_TARGET_'))}
    configs = {}
    for parent in {Path.home(), directory, *directory.parents}:
        folder = parent / '.cargo'
        for name in ('config', 'config.toml'):
            p = folder / name
            if p.is_file(): configs[str(p)] = sha(p)
    return {'compiler_overrides_sha256': overrides, 'cargo_configs_sha256': configs}


def source_snapshot():
    lock = read_record(ROOT / 'dependencies/lock.json')
    result = {}
    for repo, prefix in [(ROOT, '')] + [(ROOT / d['path'], d['path'] + '/') for d in lock['dependencies']]:
        names = subprocess.check_output(['git', '-C', str(repo), 'ls-files', '--cached', '--others', '--exclude-standard', '-z'])
        for raw in names.split(b'\0'):
            if not raw: continue
            name = prefix + os.fsdecode(raw)
            if not (name.startswith(('crates/', 'vendor/', 'fixtures/', 'tools/', 'examples/', 'scripts/devloop/', 'scripts/tests/'))
                    or name in ('Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml', 'dependencies/lock.json', 'scripts/dev.py')):
                continue
            path = ROOT / name
            if path.is_file():
                if path.is_symlink() and not path.resolve().is_relative_to(ROOT.resolve()):
                    raise ValueError(f'source symlink escapes repository: {name}')
                result[name] = sha(path)
    return result


def checked_entry(directory, identity):
    directory = Path(directory)
    receipt = directory / 'entry.json'
    if not receipt.exists():
        if directory.exists() and any(directory.iterdir()):
            raise ValueError(f'incomplete cache entry retained at {directory}; inspect it and use --fresh or a new cache')
        return None
    value = read_record(receipt)
    if key(value.get('identity')) != key(identity):
        raise ValueError(f'cache identity mismatch: {directory}')
    if not isinstance(value.get('artifacts'), dict) or not value['artifacts']:
        raise ValueError('cache receipt has no artifact inventory')
    for name, digest in value['artifacts'].items():
        if not isinstance(name, str) or not isinstance(digest, str) or not re.fullmatch('[0-9a-f]{64}', digest):
            raise ValueError('invalid cached artifact identity')
        path = directory / name
        if not path.resolve().is_relative_to(directory.resolve()) or not path.is_file() or sha(path) != digest:
            raise ValueError(f'corrupt cached artifact: {path}')
    return value


def seal(directory, identity, names, **extra):
    value = {'identity': identity, 'artifacts': {name: sha(Path(directory) / name) for name in names}, **extra}
    write(Path(directory) / 'entry.json', value)
    return value


def archive_source(sources, store=STORE):
    identity = {'sources': sources}
    directory = Path(store) / 'sources' / key(identity)
    if checked_entry(directory, identity):
        return {'path': str(directory / 'sources.tar.gz'), 'sha256': sha(directory / 'sources.tar.gz')}
    directory.mkdir(parents=True, exist_ok=True)
    with (directory / 'sources.tar.gz').open('wb') as stream:
        with gzip.GzipFile(filename='', mode='wb', fileobj=stream, mtime=0) as zipped:
            with tarfile.open(fileobj=zipped, mode='w') as archive:
                for name, digest in sorted(sources.items()):
                    data = (ROOT / name).read_bytes()
                    if hashlib.sha256(data).hexdigest() != digest:
                        raise ValueError(f'source changed while archiving: {name}')
                    info = tarfile.TarInfo(name)
                    info.mode = 0o644
                    info.size = len(data)
                    archive.addfile(info, io.BytesIO(data))
    seal(directory, identity, ['sources.tar.gz'])
    return {'path': str(directory / 'sources.tar.gz'), 'sha256': sha(directory / 'sources.tar.gz')}


def command(argv, env, logfile, cwd=ROOT, allow_failure=False):
    logfile = Path(logfile)
    logfile.parent.mkdir(parents=True, exist_ok=True)
    print('Running ' + logfile.name, flush=True)
    started = time.monotonic()
    with logfile.open('w') as log:
        result = subprocess.run(argv, env=env, cwd=cwd, stdout=log, stderr=subprocess.STDOUT)
    receipt = {'command': argv, 'exit_code': result.returncode, 'seconds': time.monotonic() - started}
    write(logfile.with_suffix('.command.json'), receipt)
    if result.returncode and not allow_failure:
        raise RuntimeError(f'command failed ({result.returncode}); see {logfile}')
    return receipt
