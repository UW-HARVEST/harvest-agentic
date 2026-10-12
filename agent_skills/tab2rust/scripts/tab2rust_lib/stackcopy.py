"""Static copies of the arrays on the stack. build calls make(). records.py reads the result.

An array inside a function without 'static' has no values in the binary. make() takes these
arrays from the debug information of the binaries, and finds their declarations with the same
join as detect (findings.py). It copies each compiled .c file that has such arrays and puts
'static' in front of each of their declarations.
The declarations stay in their functions, so local types, macros and constants still apply.
'#line' keeps the original file and line numbers in the debug information. An array whose
initializer is not constant gives a compile error in its lines. make() removes it and compiles
again. One library links all copies. Its records then give the values (records.py).
"""
import concurrent.futures
import json
import os
import re
import subprocess

from . import compdb, die, findings, gdbrun

DIR = 'tab2rust-stack'
MANIFEST = 'stack.json'
ERROR_RE = re.compile(r'^(.*?):(\d+):\d+: error: (.*)$', re.M)


def stack_arrays(src_root, binaries, units):
    """{compiled .c file: [text candidates]} of the arrays on the stack of the binaries."""
    recs = [r for b in binaries for r in gdbrun.inventory(b)['records'] if r['storage'] == 'stack']
    src = findings.Source(src_root)
    src.attach(recs)
    out = {}
    for c in src.cands:
        path = os.path.realpath(c['recs'][0]['file']) if c['recs'] else None
        if path in units:  # not a header or another included file
            out.setdefault(path, []).append(c)
    return out


def copy_text(path, cands):
    """The text of path with 'static ' in front of each declaration of cands."""
    with open(path, encoding='latin-1') as f:
        text = f.read()
    for c in sorted(cands, key=lambda c: -c['pos'][0]):
        at = c['pos'][0] - len(c['head'])  # the first character of the declaration
        text = text[:at] + 'static ' + text[at:]
    return f'#line 1 "{path}"\n' + text


def compile_copy(path, entry, cands, obj):
    """Compile the copy of one file into obj. Returns (compiled, [(candidate, error)])."""
    failed = []
    while cands:
        src = obj[:-2] + '.c'
        with open(src, 'w', encoding='latin-1') as f:
            f.write(copy_text(path, cands))
        p = subprocess.run(['gcc'] + compdb.flags(entry) + ['-iquote', os.path.dirname(path), '-g', '-O0', '-fPIC',
                                                             '-w', '-c', '-o', obj, src],
                           cwd=entry['directory'], capture_output=True, text=True, env=dict(os.environ, LC_ALL='C'))
        if p.returncode == 0:
            return True, failed
        errors = [(int(n), msg) for f, n, msg in ERROR_RE.findall(p.stderr) if f == path]
        bad = {id(c): (c, f'line {n}: {msg[:60]}') for n, msg in errors for c in cands if c['l0'] <= n <= c['l1']}
        if not bad:  # an error outside the arrays: no array of this file gets a copy
            first = ERROR_RE.search(p.stderr)
            msg = first.group(3) if first else (p.stderr.strip().split('\n') or [''])[-1]
            bad = {id(c): (c, 'the copy does not compile: ' + msg[:60]) for c in cands}
        failed += bad.values()
        cands = [c for c in cands if id(c) not in bad]
    return False, failed


def make(src_root, build_dir, binaries, jobs):
    """Write the copies, their library and the manifest to build_dir/tab2rust-stack.
    Returns (arrays with a copy, arrays without)."""
    out = os.path.join(build_dir, DIR)
    os.makedirs(out, exist_ok=True)
    units = compdb.entries(build_dir)
    work = [(path, units[path], cands, os.path.join(out, f'{i:04d}_{os.path.basename(path)[:-2]}.o'))
            for i, (path, cands) in enumerate(sorted(stack_arrays(src_root, binaries, units).items()))]
    with concurrent.futures.ThreadPoolExecutor(jobs) as ex:
        results = list(ex.map(lambda w: compile_copy(*w), work))
    objs = [w[3] for w, (ok, _) in zip(work, results) if ok]
    lib = os.path.join(out, 'stack.so') if objs else None
    if lib:
        p = subprocess.run(['gcc', '-shared', '-Wl,--allow-multiple-definition', '-o', lib] + objs,
                           capture_output=True, text=True)
        if p.returncode:
            die('cannot link the static copies of the stack arrays:\n' + p.stderr[-2000:])
    failed = [dict(file=w[0], l0=c['l0'], l1=c['l1'], error=msg) for w, (_, f) in zip(work, results) for c, msg in f]
    manifest = dict(binaries=sorted({os.path.realpath(b) for b in binaries}), library=lib, units=sorted(units),
                    failed=failed)
    with open(os.path.join(out, MANIFEST), 'w') as f:
        json.dump(manifest, f, indent=1)
    total = sum(len(w[2]) for w in work)
    return total - len(failed), len(failed)


def load(binary):
    """The manifest of the build of binary: in a parent directory of binary, with binary in its list."""
    real = os.path.realpath(binary)
    d = os.path.dirname(real)
    while True:
        path = os.path.join(d, DIR, MANIFEST)
        if os.path.exists(path):
            with open(path) as f:
                m = json.load(f)
            if real in m['binaries']:
                return m
        if d == os.path.dirname(d):
            return None
        d = os.path.dirname(d)


def why_not(m, r, twin):
    """None if twin (the record of the copy) gives the values of the stack record r, else the reason."""
    path = os.path.realpath(r['file'])
    if 'error' in twin or twin['storage'] == 'stack':  # no static copy (a header array stays on the stack)
        if path not in m['units']:
            return 'not in a compiled .c file'
        return next((f['error'] for f in m['failed'] if f['file'] == path and f['l0'] <= r['line'] <= f['l1']),
                    'no copy found')
    if (twin['ctype'], twin['size']) != (r['ctype'], r['size']):
        return 'the copy has another type'
    if None in (twin.get('targets') or {}).values():  # a pointer to a variable that the copies do not define
        return 'pointers into other files'
    return None
