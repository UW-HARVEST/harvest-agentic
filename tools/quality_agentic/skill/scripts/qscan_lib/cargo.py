"""Cargo helpers: crate metadata, a clean environment, JSON diagnostics, crate copies."""
import json
import os
import shutil
import subprocess

# Environment variables that change lint levels or flags behind our back.
SCRUBBED_ENV = ('RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS', 'CARGO_BUILD_RUSTFLAGS', 'RUSTDOCFLAGS',
                'CLIPPY_CONF_DIR', 'CLIPPY_ARGS', 'CARGO_TARGET_DIR', 'CARGO_BUILD_TARGET_DIR')

# Directories of a checkout that are not part of the crate's sources.
SKIP_DIRS = frozenset({'target', '.git', 'wt', 'scratch', 'inputs', 'shared', 'verify_env', 'cbuild',
                       'fuzz-artifacts', '.opencode', 'quality', 'scout', 'node_modules'})


class CargoError(Exception):
    pass


def env(target_dir):
    e = {k: v for k, v in os.environ.items() if k not in SCRUBBED_ENV}
    e['CARGO_TARGET_DIR'] = str(target_dir)
    e['CARGO_TERM_COLOR'] = 'never'
    return e


def metadata(crate_dir):
    """(package name, lib target name or None, bin target names) of the crate."""
    out = subprocess.run(['cargo', 'metadata', '--no-deps', '--format-version', '1', '--offline'],
                         cwd=crate_dir, capture_output=True, text=True, env=env(os.path.join(crate_dir, 'target')))
    if out.returncode != 0:
        out = subprocess.run(['cargo', 'metadata', '--no-deps', '--format-version', '1'],
                             cwd=crate_dir, capture_output=True, text=True,
                             env=env(os.path.join(crate_dir, 'target')))
    if out.returncode != 0:
        raise CargoError(f'cargo metadata failed:\n{out.stderr.strip()}')
    data = json.loads(out.stdout)
    manifest = os.path.realpath(os.path.join(crate_dir, 'Cargo.toml'))
    pkg = next((p for p in data['packages'] if os.path.realpath(p['manifest_path']) == manifest), None)
    if pkg is None:
        raise CargoError('cargo metadata does not list the package of this directory')
    lib = None
    bins = []
    for t in pkg['targets']:
        kinds = set(t['kind'])
        if kinds & {'lib', 'cdylib', 'staticlib', 'rlib', 'dylib'}:
            lib = t['name']
        elif 'bin' in kinds:
            bins.append(t['name'])
    return pkg['name'], lib, bins


def run_json(args, crate_dir, target_dir, timeout=900):
    """Runs cargo with --message-format=json. Returns (exit code, compiler messages, artifact
    target names, stderr). Messages of other packages are dropped."""
    proc = subprocess.run(args, cwd=crate_dir, capture_output=True, text=True, env=env(target_dir),
                          timeout=timeout)
    root = os.path.realpath(crate_dir)
    messages = []
    artifacts = []
    for line in proc.stdout.splitlines():
        if not line.startswith('{'):
            continue
        try:
            rec = json.loads(line)
        except json.JSONDecodeError:
            continue
        reason = rec.get('reason')
        manifest = rec.get('manifest_path', '')
        if manifest and not os.path.realpath(manifest).startswith(root + os.sep):
            continue
        if reason == 'compiler-message':
            messages.append(rec['message'])
        elif reason == 'compiler-artifact':
            artifacts.append(rec.get('target', {}).get('name', ''))
    return proc.returncode, messages, artifacts, proc.stderr


def crate_rel(path, crate_dir):
    """A diagnostic's file name relative to the crate root, or None when it is outside."""
    full = path if os.path.isabs(path) else os.path.join(crate_dir, path)
    full = os.path.realpath(full)
    root = os.path.realpath(crate_dir)
    if not full.startswith(root + os.sep):
        return None
    return os.path.relpath(full, root)


def primary_location(message, crate_dir):
    """(file relative to the crate, line, column) of a diagnostic. A span inside a macro of
    another crate is followed to its expansion site in this crate."""
    for span in message.get('spans', []):
        if not span.get('is_primary'):
            continue
        s = span
        while s is not None:
            rel = crate_rel(s['file_name'], crate_dir)
            if rel is not None:
                return rel, s['line_start'], s['column_start']
            exp = s.get('expansion')
            s = exp.get('span') if exp else None
    return None, 0, 0


def source_files(crate_dir):
    """The .rs files of the crate that a library or binary build can compile: under src/ and any
    other directory, except tests/, benches/, examples/ and the skipped directories. build.rs is
    listed separately."""
    rels = []
    for dirpath, dirnames, filenames in os.walk(crate_dir):
        rel_dir = os.path.relpath(dirpath, crate_dir)
        top = rel_dir.split(os.sep)[0]
        if rel_dir == '.':
            dirnames[:] = [d for d in dirnames if d not in SKIP_DIRS and d not in ('tests', 'benches', 'examples')
                           and not d.startswith('.') and not d.startswith('build')]
        elif top in SKIP_DIRS:
            dirnames[:] = []
            continue
        for name in filenames:
            if name.endswith('.rs'):
                rel = os.path.normpath(os.path.join(rel_dir, name))
                if rel != 'build.rs':
                    rels.append(rel)
    return sorted(rels)


def copy_crate(crate_dir, dest):
    """Copies the crate without build outputs and agent directories. Symbolic links are skipped."""
    if os.path.exists(dest):
        shutil.rmtree(dest)

    def ignore(dirpath, names):
        rel = os.path.relpath(dirpath, crate_dir)
        skipped = set()
        for name in names:
            full = os.path.join(dirpath, name)
            if os.path.islink(full):
                skipped.add(name)
            elif rel == '.' and (name in SKIP_DIRS or name.startswith('build-') or name == 'wt'):
                skipped.add(name)
        return skipped

    shutil.copytree(crate_dir, dest, ignore=ignore)
    toml = os.path.join(dest, 'Cargo.toml')
    with open(toml, encoding='utf-8') as fh:
        text = fh.read()
    if '[workspace]' not in text:
        with open(toml, 'a', encoding='utf-8') as fh:
            fh.write('\n[workspace]\n')
