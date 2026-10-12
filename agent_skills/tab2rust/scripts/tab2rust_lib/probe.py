"""probe: compiled values of macros and enum constants, in the context of one C file.

The probe file includes that C file. Each name is one line of an array initializer, so a
compile error names the line, and the tool drops that name and compiles again. The probe is
linked as a shared library, so string values are real pointers that gdb can follow.
"""
import json
import os
import re
import subprocess
import sys
import tempfile

from . import compdb, consts, die, gdbrun, lexer

KIND = '_Generic(({n}), char*: 3, const char*: 3, float: 2, double: 2, long double: 2, default: 1)'
ARRAYS = {1: ('unsigned long long', 't2r_int', '(unsigned long long)({n})'),
          2: ('double', 't2r_float', '({n})'),
          3: ('const char *', 't2r_str', '({n})')}


def span_names(path, l0=None, l1=None):
    """Object-like #define names and enumerator names in lines l0..l1 (default: the whole file)."""
    with open(path, encoding='latin-1') as f:
        text = lexer.strip('\n'.join(f.read().split('\n')[(l0 or 1) - 1:l1]))
    names = []
    for ln in text.split('\n'):
        m = re.match(r'\s*#\s*define\s+([A-Za-z_]\w*)(?!\()\s+\S', ln)
        if m:
            names.append(m.group(1))
        elif not ln.lstrip().startswith('#'):
            names += [m.group(1) for m in re.finditer(r'(?:^|,|\{)\s*([A-Za-z_]\w*)\s*(?==|,|\}|$)', ln)
                      if m.group(1) not in lexer.C_WORDS]
    return list(dict.fromkeys(names))


def compile_names(td, tu, flags, cwd, arrays):
    """Compile arrays {var: (ctype, [(name, expr)])}. A name whose line fails is dropped from all
    arrays. Returns (so, arrays, {dropped name: first gcc error message})."""
    src, so = os.path.join(td, 'probe.c'), os.path.join(td, 'probe.so')
    dropped = {}
    for _ in range(8):
        lines, where = [f'#include "{tu}"'], {}
        for var, (ctype, items) in arrays.items():
            lines.append(f'{ctype} const {var}[] = {{')
            for name, expr in items:
                where[len(lines) + 1] = name
                lines.append(f'  {expr},')
            lines.append('  0 };')
        with open(src, 'w') as f:
            f.write('\n'.join(lines) + '\n')
        # -ftrack-macro-expansion=0: gcc reports an error in a macro expansion at the probe line,
        # not at the #define in a header, so the error names the line of the name.
        p = subprocess.run(['gcc', '-g', '-O0', '-shared', '-fPIC', '-w', '-ftrack-macro-expansion=0',
                            '-o', so, src] + flags,
                           capture_output=True, text=True, cwd=cwd, env=dict(os.environ, LC_ALL='C'))
        if p.returncode == 0:
            return so, arrays, dropped
        bad = {}
        for m in re.finditer(r'probe\.c:(\d+):\d+: error: (.*)', p.stderr):
            if int(m.group(1)) in where:
                bad.setdefault(where[int(m.group(1))], m.group(2))
        if not bad:
            die('probe compile failed:\n' + p.stderr[-2000:])
        dropped.update(bad)
        arrays = {v: (t, [it for it in items if it[0] not in bad]) for v, (t, items) in arrays.items()}
    die('probe compile failed after several retries')


def function_macros(tu, flags, cwd):
    """Names of the function-like macros that are defined at the end of tu."""
    p = subprocess.run(['gcc', '-E', '-dM', tu] + flags, capture_output=True, text=True, cwd=cwd)
    return set(re.findall(r'^#define ([A-Za-z_]\w*)\(', p.stdout, re.M))


def skip_reasons(dropped, tu, flags, cwd):
    """{reason: [names]} for the names that did not compile."""
    undef = [n for n, msg in dropped.items() if re.match(rf"'{re.escape(n)}' undeclared", msg)]
    fmacros = function_macros(tu, flags, cwd) if undef else set()
    res = {}
    for n, msg in dropped.items():
        if n in undef:
            why = 'function-like macro' if n in fmacros else 'not defined in this configuration'
        else:
            why = 'not a constant expression'
        res.setdefault(why, []).append(n)
    return res


def probe(tu, flags, cwd, names):
    """Compiled values. Returns ({name: (kind, value, size, signed)}, {skip reason: [names]}).
    kind is int, float or str. size and signed describe the C type of the expression."""
    with tempfile.TemporaryDirectory() as td:
        # pass 1: kind of each name (integer, float, string); names that do not compile drop out
        so, arr, dropped = compile_names(td, tu, flags, cwd,
                                         {'t2r_kind': ('int', [(n, KIND.format(n=n)) for n in names])})
        kinds = dict(zip([n for n, _ in arr['t2r_kind'][1]], gdbrun.dump(so, [dict(key='k', name='t2r_kind')])[0]['value']))
        # pass 2: values (one array per kind), the sign of each integer, and the size and sign of each C type
        groups = {ARRAYS[k][1]: (ARRAYS[k][0], [(n, ARRAYS[k][2].format(n=n)) for n in kinds if kinds[n] == k])
                  for k in ARRAYS}
        ints = [n for n in kinds if kinds[n] == 1]
        groups['t2r_neg'] = ('signed char', [(n, f'(({n}) < 0)') for n in ints])
        groups['t2r_tsigned'] = ('signed char', [(n, f'((__typeof__({n}))-1 < 0)') for n in ints])
        groups['t2r_size'] = ('int', [(n, f'(int)sizeof({n})') for n in kinds if kinds[n] in (1, 2)])
        so, groups, more = compile_names(td, tu, flags, cwd, {v: g for v, g in groups.items() if g[1]})
        dropped.update(more)
        recs = {r['name']: r for r in gdbrun.dump(so, [dict(key=v, name=v) for v in groups])}
        vals = {var: dict(zip([n for n, _ in items], recs[var]['value'])) for var, (_, items) in groups.items()}
    neg, tsigned, size = (vals.get(v, {}) for v in ('t2r_neg', 't2r_tsigned', 't2r_size'))
    # the name of a char array is an address, not a string constant. (An address of another kind does
    # not compile in the integer arrays: gcc drops it as not constant.)
    addresses = [n for n, v in vals.get('t2r_str', {}).items() if v and 's' not in v]
    out = {}
    for n, v in vals.get('t2r_int', {}).items():
        out[n] = ('int', v - (1 << 64) if neg[n] else v, size[n], bool(tsigned[n]))
    for n, v in vals.get('t2r_float', {}).items():
        out[n] = ('float', v['bits'], size[n], True)
    for n, v in vals.get('t2r_str', {}).items():
        if n not in addresses:
            out[n] = ('str', v and v['s'], None, None)
    unsupported = [n for n, (k, _, sz, _) in out.items() if sz and sz > 8]
    for n in unsupported:  # long double and 128-bit integers: the probe arrays cannot hold them
        del out[n]
    reasons = skip_reasons(dropped, tu, flags, cwd) if dropped else {}
    if unsupported:
        reasons['C type larger than 8 bytes (not supported)'] = unsupported
    if addresses:
        reasons['an address, not a constant value'] = addresses
    return {n: out[n] for n in names if n in out}, reasons


def as_json(vals):
    """The JSON form: integers as numbers, floats as bit patterns of a double, strings as text."""
    res = {}
    for n, (kind, v, size, _) in vals.items():
        if kind == 'int':
            res[n] = v
        elif kind == 'float':
            res[n] = {'float_bits': hex(v)}
        else:
            res[n] = {'str': bytes.fromhex(v).decode('latin-1') if v is not None else None}
    return res


def run(a):
    names = list(a.names or [])
    for sp in a.span or []:
        m = re.match(r'(.+?)(?::(\d+)-(\d+))?$', sp)
        if not m.group(2) and not a.match:
            die(f'--span {sp} takes the whole file. Add --match REGEX to select the names.')
        if not os.path.isfile(m.group(1)):
            die(f'--span {sp}: no file {m.group(1)}. A relative path starts at the current directory '
                f'{os.getcwd()}.')
        names += span_names(m.group(1), *(int(x) for x in m.groups()[1:] if x))
    names = list(dict.fromkeys(names))
    if a.match:
        other = [n for n in names if not re.search(a.match, n)]
        names = [n for n in names if n not in other]
        if other:
            more = ' ...' if len(other) > 40 else ''
            print(f'--match selected {len(names)} of {len(names) + len(other)} names. Not selected: '
                  f'{", ".join(other[:40])}{more}', file=sys.stderr)
    if not names:
        die('no names. Give names, or --span FILE:FIRST-LAST, or --span FILE --match REGEX')
    e = compdb.entry_for(a.build, a.tu)
    vals, reasons = probe(os.path.abspath(a.tu), compdb.flags(e), e['directory'], names)
    if a.out or not a.rust:
        text = json.dumps(as_json(vals), indent=0)
        if a.out:
            with open(a.out, 'w') as f:
                f.write(text + '\n')
            print(f'wrote {a.out}: {len(vals)} values')
        else:
            print(text)
    for why, skipped in reasons.items():
        more = ' ...' if len(skipped) > 40 else ''
        print(f'skipped, {why}: {", ".join(skipped[:40])}{more}', file=sys.stderr)
    if reasons.get('not defined in this configuration'):
        print('  These names are not defined for this build. Do not copy them from other #if branches.',
              file=sys.stderr)
    if a.rust:
        ok = consts.write(a.rust, vals, a.int_type, a.str_type, f'--tu {os.path.basename(a.tu)}')
        if not ok:
            sys.exit(1)
