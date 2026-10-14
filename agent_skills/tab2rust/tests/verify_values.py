#!/usr/bin/env python3
"""Check that Rust tables hold the compiled C values. For maintainers and test runs.

    verify_values.py --binary LIB --rs FILE.rs --pair C_KEY=RUST_NAME [--pair ...]

FILE.rs must compile on its own (no crate:: paths). The script builds a small Rust program
that prints each table with {:?}. It compares the printed scalars, in order, with the values
that gdb reads from the C binary. A pointer item is compared with the value that it points to.
This does not use the text of FILE.rs, so it also checks tables that a const fn computes.
Exit code 0 if all pairs are equal.
"""
import argparse
import os
import re
import struct
import subprocess
import sys
import tempfile

sys.dont_write_bytecode = True  # no __pycache__ in the skill directory (it is embedded and copied to agents)
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', 'scripts'))
from tab2rust_lib import gdbrun, records  # noqa: E402
from tab2rust_lib.emit import index_path  # noqa: E402

TOKEN = re.compile(r'"(?:[^"\\]|\\.)*"|(?<![\w.])-?(?:inf|NaN|\d+(?:\.\d+)?(?:e-?\d+)?)(?![\w.])'
                   r'|\btrue\b|\bfalse\b|\bNone\b')


def unescape(s):
    """Bytes of a Rust Debug string ("a\\x80\\n")."""
    out, i = bytearray(), 0
    simple = {'n': 10, 'r': 13, 't': 9, '0': 0, '\\': 92, "'": 39, '"': 34}
    while i < len(s):
        if s[i] == '\\':
            if s[i + 1] == 'x':
                out.append(int(s[i + 2:i + 4], 16)); i += 4
            elif s[i + 1] == 'u':
                j = s.index('}', i)
                out += chr(int(s[i + 3:j], 16)).encode(); i = j + 1
            else:
                out.append(simple[s[i + 1]]); i += 2
        else:
            out += s[i].encode(); i += 1
    return bytes(out)


def rust_scalars(text):
    """Scalars of one Debug print, in order. Field names and type names are not tokens."""
    res = []
    for t in TOKEN.findall(text):
        if t.startswith('"'):
            res.append(('str', unescape(t[1:-1])))
        elif t in ('true', 'false'):
            res.append(('num', int(t == 'true')))
        elif t == 'None':
            res.append(('null', None))
        else:
            res.append(('num', t))
    return res


def c_scalars(d, v, targets, out):
    """Scalars of a C value, in memory order, with the type needed to compare each one."""
    k = d['kind']
    if k == 'array':
        for x in v:
            c_scalars(d['of'], x, targets, out)
    elif k == 'struct':
        for f, x in zip(d['fields'], v):
            c_scalars(f['type'], x, targets, out)
    elif v is None:
        out.append(('null', None, d))
    elif k == 'str' and 's' in v:
        out.append(('str', bytes.fromhex(v['s']), d))
    elif k in ('ptr', 'str'):
        td, tv = targets[v['sym']]
        path = index_path(td, v['off'], d['to'])
        if path is None:
            raise ValueError(f'pointer {v} does not point at an element')
        for i in path:
            td, tv = td['of'], tv[i]
        c_scalars(td, tv, targets, out)
    else:
        out.append(('num', v, d))


def same(c, r):
    kind, cv, d = c
    if kind != r[0]:
        return False
    if kind == 'null':
        return True
    if kind == 'str':
        return cv == r[1]
    if d['kind'] == 'float':
        fmt, ifmt = ('<f', '<I') if d['size'] == 4 else ('<d', '<Q')
        txt = r[1].replace('inf', 'Infinity')
        return struct.unpack(ifmt, struct.pack(fmt, float(txt)))[0] == cv['bits'] or (txt.lstrip('-') == 'NaN')
    bits = 8 * d['size']
    return (int(r[1]) - int(cv)) % (1 << bits) == 0


def compare(c, r):
    """None if equal, else a short description of the first difference."""
    if len(c) != len(r):
        return f'{len(c)} C values, {len(r)} Rust values'
    for i, (a, b) in enumerate(zip(c, r)):
        if not same(a, b):
            return f'item {i}: C {a[1]!r} Rust {b[1]!r}'
    return None


def rust_prints(rs, names):
    with tempfile.TemporaryDirectory() as td:
        main = os.path.join(td, 'main.rs')
        with open(main, 'w') as f:
            f.write(f'#[path = "{os.path.abspath(rs)}"]\n#[allow(warnings)]\nmod gen;\nfn main() {{\n')
            for n in names:
                f.write(f'    println!("@@{n}");\n    println!("{{:?}}", &gen::{n});\n')
            f.write('}\n')
        exe = os.path.join(td, 'main')
        p = subprocess.run(['rustc', '--edition', '2021', '-o', exe, main], capture_output=True, text=True)
        if p.returncode:
            raise SystemExit('rustc failed:\n' + p.stderr[-2000:])
        out = subprocess.run([exe], capture_output=True, text=True, check=True).stdout
    parts = re.split(r'^@@(\w+)\n', out, flags=re.M)
    return {parts[i]: parts[i + 1] for i in range(1, len(parts), 2)}


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('--binary', required=True)
    ap.add_argument('--rs', required=True)
    ap.add_argument('--pair', action='append', required=True, help='C_KEY=RUST_NAME')
    a = ap.parse_args()
    pairs = [p.split('=', 1) for p in a.pair]
    recs = {r['key']: r for r in records.dump(a.binary, [gdbrun.parse_key(k) for k, _ in pairs])}
    syms = sorted({s for r in recs.values() for s in (r.get('targets') or {})})
    targets = {r['name']: (r['desc'], r['value']) for r in gdbrun.dump(a.binary, [gdbrun.parse_key(s) for s in syms])
               if 'value' in r}
    prints = rust_prints(a.rs, [n for _, n in pairs])
    bad = 0
    for key, name in pairs:
        r = recs[key]
        if 'error' in r:
            print(f'FAIL {key}: {r["error"]}'); bad += 1; continue
        c = []
        c_scalars(r['desc'], r['value'], targets, c)
        rv = rust_scalars(prints[name])
        if len(rv) == 1 and rv[0][0] == 'str' and all(x[0] == 'num' for x in c):
            rv = [('num', str(b)) for b in rv[0][1]]  # a byte string *b"..." prints as one string
        diff = compare(c, rv)
        print(f'{"OK  " if diff is None else "FAIL"} {key} -> {name}: {len(c)} values' + (f'. {diff}' if diff else ''))
        bad += diff is not None
    sys.exit(1 if bad else 0)


if __name__ == '__main__':
    main()
