"""Build the Rust crate, find its artifact, list its exports, and run `cargo test`."""
import json
import os
import re
import subprocess

SCRUBBED_ENV = ('RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS', 'CARGO_BUILD_RUSTFLAGS', 'CARGO_TARGET_DIR',
                'CARGO_BUILD_TARGET_DIR')


def env():
    return {k: v for k, v in os.environ.items() if k not in SCRUBBED_ENV}


def build(crate_dir, timeout=1800):
    """`cargo build --release`. Returns (ok, last lines of the output)."""
    proc = subprocess.run(['cargo', 'build', '--release'], cwd=crate_dir, capture_output=True, text=True,
                          env=env(), timeout=timeout)
    tail = '\n'.join((proc.stdout + proc.stderr).strip().splitlines()[-25:])
    return proc.returncode == 0, tail


def artifact(crate_dir):
    """Absolute path of the built cdylib (or, without a library, the first binary)."""
    out = subprocess.run(['cargo', 'metadata', '--no-deps', '--format-version', '1'], cwd=crate_dir,
                         capture_output=True, text=True, env=env())
    if out.returncode != 0:
        raise RuntimeError(f'cargo metadata failed: {out.stderr.strip()}')
    data = json.loads(out.stdout)
    manifest = os.path.realpath(os.path.join(crate_dir, 'Cargo.toml'))
    pkg = next(p for p in data['packages'] if os.path.realpath(p['manifest_path']) == manifest)
    release = os.path.join(data['target_directory'], 'release')
    for t in pkg['targets']:
        if 'cdylib' in t['kind'] or 'dylib' in t['kind']:
            path = os.path.join(release, f"lib{t['name'].replace('-', '_')}.so")
            if os.path.isfile(path):
                return path
    for t in pkg['targets']:
        if 'bin' in t['kind']:
            path = os.path.join(release, t['name'])
            if os.path.isfile(path):
                return path
    raise RuntimeError(f'no built cdylib or binary of {pkg["name"]} in {release}')


def exports(path):
    """Defined dynamic symbols of a shared library, sorted."""
    out = subprocess.run(['nm', '-D', '--defined-only', path], capture_output=True, text=True)
    names = set()
    for line in out.stdout.splitlines():
        parts = line.split()
        if len(parts) >= 3:
            names.add(parts[2])
    return sorted(names)


TEST_LINE = re.compile(r'^test (\S+) \.\.\. (ok|FAILED|ignored)')


def has_tests(crate_dir):
    if os.path.isdir(os.path.join(crate_dir, 'tests')):
        return True
    for dirpath, _, files in os.walk(os.path.join(crate_dir, 'src')):
        for name in files:
            if name.endswith('.rs'):
                with open(os.path.join(dirpath, name), encoding='utf-8', errors='replace') as fh:
                    if '#[test]' in fh.read():
                        return True
    return False


def cargo_test(crate_dir, timeout=1800):
    """Runs `cargo test --release`. Returns ({test name: 'PASS'|'FAIL'|'IGNORED'}, note)."""
    try:
        proc = subprocess.run(['cargo', 'test', '--release', '--no-fail-fast'], cwd=crate_dir, capture_output=True,
                              text=True, env=env(), timeout=timeout)
    except subprocess.TimeoutExpired:
        return {}, f'cargo test timed out after {timeout} s'
    status = {}
    for line in (proc.stdout + '\n' + proc.stderr).splitlines():
        m = TEST_LINE.match(line.strip())
        if m:
            status[m.group(1)] = {'ok': 'PASS', 'FAILED': 'FAIL', 'ignored': 'IGNORED'}[m.group(2)]
    note = ''
    if proc.returncode != 0 and not any(v == 'FAIL' for v in status.values()):
        note = 'cargo test failed before running tests: ' + ' | '.join(proc.stderr.strip().splitlines()[-3:])
    return status, note
