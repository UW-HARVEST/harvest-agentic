#!/usr/bin/env python3
"""Regression test of tab2rust on tests/fixture/tables.c. For maintainers.

    python3 tests/run_tests.py

Builds the fixture with "tab2rust.py build" (tests/fixture/CMakeLists.txt), then checks: the detect
classes, reasons, notes and report columns, the static copies of stack arrays, the --min rule (text
items), detect --only and --exclude, the stop on a path mismatch, the translate output (it compiles and
verify_values.py finds the same values as the C binary) and its summary, the float literals, struct
renames (also of anonymous structs), the refusals, the keys of header copies, and probe (JSON, --rust,
skip messages, --span FILE --match).
"""
import json
import os
import shutil
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
TOOL = os.path.join(HERE, '..', 'scripts', 'tab2rust.py')
FIXTURE = os.path.join(HERE, 'fixture')

# CMakeLists.txt compiles tables.c and tables_unit2.c. tables_unused.c, tables_feature.c, tables_guard.h are not compiled.
COPY_KEYS = ('tables_copy.h:copy_tab@tables.c', 'tables_copy.h:copy_tab@tables_unit2.c')
PURE = {  # key -> note tags
    'perm_tab': (), 'gear_tab': (), 'rows_tab': (), 'grid_tab': (), 'dbl_tab': (), 'xform_tab': (), 'span_tab': (),
    'name_tab': ('writable',), 'ptr_tab': ('writable',), 'link_tab': ('writable',), 'local_tab': (),
    'nested_tab': (), 'hdr_tab': (),
    'anon_a_tab': (), 'anon_b_tab': (), 'anon_c_tab': (), 'anon_d_tab': (), 'struct_tab': (), 'fn_struct_tab': (),
    'live_stack': ('stack copy',), 'copy_rows': ('stack copy',), 'str_stack': ('stack copy',),
    'gptr_tab': ('exported',), 'bptr_tab': ('exported',), 'mixed_tab': ('exported',), 'addr_tab': ('exported',),
    'line_tab': (), 'zero_tab': ('zero fill',), 'cond_tab': ('#if',), 'macro_tab': ('macro items',),
    'incl_tab': ('body',), 'writable_tab': ('writable',), 'macro_init_tab': ('not in text',),
    COPY_KEYS[0]: (), COPY_KEYS[1]: (),
}
SMALL = ('text_tab', 'short_tab')  # fewer than 30 text items (more binary values): not findings, but PURE
# the "writable" note says "pointers can change" only for these
POINTER_WRITABLE = {'name_tab': True, 'ptr_tab': True, 'link_tab': True, 'writable_tab': False}
TAINTED = {  # key -> reason tags
    'union_tab': ('type',), 'fn_tab': ('type',), 'excess_tab': ('count',), 'stack_fn': ('type',),
    'param_stack': ('stack',), 'extern_stack': ('stack',), 'hdr_stack': ('stack',),
}
# why a stack array has no static copy
STACK_TEXT = {'param_stack': 'initializer element is not constant', 'extern_stack': 'stack (pointers into other files)',
              'hdr_stack': 'stack (not in a compiled .c file)'}
# translate refuses these PURE arrays (the emitter cannot write them): key -> message part
EMIT_REFUSED = {'mixed_tab': 'string literals and pointers into arrays', 'addr_tab': 'a number holds an address'}
# values of the stack copies and of pointers, as Rust {:?} prints them (from the C text)
STACK_VALUES = {'c::FX_STACK_LIVE_STACK[31]': '32', 'c::FX_LOCAL_COPY_ROWS[0].b': '225', 'c::FX_LOCAL_COPY_ROWS[2].b': '7',
                'c::FX_LOCAL_COPY_ROWS[15].b': '2', 'c::FX_LOCAL_STR_STACK[30]': 'Some("s30")',
                'c::FX_LOCAL_STR_STACK[31]': 'None', '*c::GPTR_TAB[3]': '14', '*c::GPTR_TAB[31]': '18',
                'std::ptr::eq(c::GPTR_TAB[9], &c::FX_GDATA[1])': 'true', '*c::BPTR_TAB[3]': '3',
                'std::ptr::eq(c::BPTR_TAB[5], &c::FX_BYTES[5])': 'true'}
DEAD = {  # name -> (reason tag, note tags)
    'dead_tab': ('branch', ()), 'dead_stack': ('branch', ()), 'unemitted_tab': ('function', ()),
    'long_head_tab': ('function', ()),
    'dead_host': ('branch', ('body',)), 'unused_file_tab': ('file', ()), 'feature_tab': ('file', ()),
    'guard_tab': ('file', ()),
}
DEAD_TEXT = {'unused_file_tab': 'file not in the binary (not compiled, or compiles to nothing)',
             'long_head_tab': 'function not emitted: fx_unused_long',
             'feature_tab': 'whole file under #ifdef FX_FEATURE (line 2) and #if FX_LEVEL > 1 (line 3)',
             'guard_tab': 'whole file under #if FX_LEVEL > 2 (line 4)'}
RUST_NAMES = {'local_tab': 'FX_LOOKUP_LOCAL_TAB', 'nested_tab': 'FX_LOOKUP_NESTED_TAB',
              'anon_c_tab': 'FX_ANON_ANON_C_TAB', 'anon_d_tab': 'FX_ANON_ANON_D_TAB',
              'fn_struct_tab': 'FX_STACK_FN_STRUCT_TAB', 'live_stack': 'FX_STACK_LIVE_STACK',
              'copy_rows': 'FX_LOCAL_COPY_ROWS', 'str_stack': 'FX_LOCAL_STR_STACK',
              COPY_KEYS[0]: 'COPY_TAB_1', COPY_KEYS[1]: 'COPY_TAB_2'}


# probe --rust: name -> (Rust type with the default options, value as Rust {:?} prints it)
PROBE = {'FX_STR_Q': ("&'static core::ffi::CStr", '[113, 34, 98, 92, 115, 133, 10, 9, 1]'),
         'FX_U': ('u32', '4000000000'), 'FX_L': ('i64', '-5'), 'FX_BYTE': ('u8', '133'),
         'FX_F': ('f32', '1.5'), 'FX_D': ('f64', '0.1'), 'MODE_NEG': ('i32', '-2'), 'FX_CODE_39': ('i32', '118')}
PROBE_SKIPPED = {'FX_DEAD_STR': 'not defined in this configuration', 'FX_FN': 'function-like macro',
                 'FX_NONCONST': 'not a constant expression', 'FX_CHARS_NAME': 'an address, not a constant value',
                 'FX_HDR_VERSION': 'not a constant expression', 'FX_HDR_DECL': 'not a constant expression'}
ROW_SPEC = {'structs': {'row_t2r': {'rust': 'Row', 'define': True,
                                    'fields': {'a': 'alpha', 'f': 'ratio', 'm': 'type', 'b': 'count', 'nope': 'x'}}}}
XFORM_FIELDS = {'nName': 'n_name', 'zName': 'z_name', 'rLimit': 'r_limit', 'rXform': 'r_xform'}
# float_literal: (value, size) -> literal. The shortest digits, plain form unless much longer.
FLOATS = {(60.0, 4): '60.0', (3600.0, 8): '3600.0', (5373485.0, 4): '5373485.0', (4.6427e14, 8): '4.6427e14',
          (4.6427e14, 4): '4.6427e14', (1e-300, 8): '1.0e-300', (0.001, 4): '0.001', (1e-5, 8): '0.00001',
          (-0.0, 8): '-0.0', (0.30000000000000004, 8): '0.30000000000000004', (1e16, 8): '1.0e16',
          (-7.75, 4): '-7.75', (31536000.0, 4): '31536000.0', (3.4028235e38, 4): '3.4028235e38'}


def run(*args, ok=True):
    p = subprocess.run([sys.executable, *args], capture_output=True, text=True)
    if ok and p.returncode:
        raise AssertionError(f'{" ".join(args)} failed:\n{p.stdout}\n{p.stderr}')
    return p


def rust_values(td, rs, exprs):
    """Compile a program that uses module rs and prints each expression with {:?}. One line each."""
    main = os.path.join(td, 'show.rs')
    with open(main, 'w') as f:
        f.write(f'#[path = "{rs}"]\nmod c;\nfn main() {{\n'
                + ''.join(f'    println!("{{:?}}", {e});\n' for e in exprs) + '}\n')
    exe = os.path.join(td, 'show')
    subprocess.run(['rustc', '--edition', '2021', '-o', exe, main], check=True, capture_output=True, cwd=td)
    return subprocess.run([exe], capture_output=True, text=True, check=True).stdout.split('\n')[:len(exprs)]


def check_probe(td, check):
    """probe: JSON values, the skip messages, and --rust with the default and other types."""
    tables = os.path.join(FIXTURE, 'tables.c')
    base = [TOOL, 'probe', '--build', td, '--tu', tables]
    p = run(*base, '--span', tables + ':5-44', 'MODE_NEG', 'NOT_A_NAME')
    vals = json.loads(p.stdout)
    check(vals.get('FX_CODE_39') == 118 and vals.get('MODE_NEG') == -2 and 'NOT_A_NAME' not in vals,
          f'probe: wrong values {dict(list(vals.items())[:3])}')
    rs = os.path.join(td, 'consts.rs')
    p = run(*base, '--rust', rs, *PROBE, *PROBE_SKIPPED)
    for name, why in PROBE_SKIPPED.items():
        check(any(l.startswith(f'skipped, {why}:') and name in l for l in p.stderr.split('\n')),
              f'probe: {name} not reported as "{why}":\n{p.stderr}')
    check('CHECK   rustc: the file compiles' in p.stdout, 'probe --rust: rustc check failed:\n' + p.stdout)
    with open(rs) as f:
        text = f.read()
    for name, (rt, _) in PROBE.items():
        check(f'pub const {name}: {rt} = ' in text, f'probe --rust: {name} is not of type {rt}')
    exprs = [f'c::{n}.to_bytes()' if n == 'FX_STR_Q' else f'c::{n}' for n in PROBE]
    for name, want, got in zip(PROBE, [v for _, v in PROBE.values()], rust_values(td, rs, exprs)):
        check(got == want, f'probe --rust: {name} is {got}, expected {want}')
    run(*base, '--rust', rs, '--int-type', 'i64', '--str-type', 'bytes', 'FX_STR_Q', 'FX_BYTE')
    got = rust_values(td, rs, ['c::FX_STR_Q', 'c::FX_BYTE as i64 + i64::MIN'])
    check(got == [PROBE['FX_STR_Q'][1], str(133 - 2 ** 63)], f'probe --rust bytes/i64: {got}')
    run(*base, '--rust', rs, '--str-type', 'str', 'FX_STR_U8')  # &str: UTF-8 only
    got = rust_values(td, rs, ['c::FX_STR_U8'])
    check(got == ['"h\u00e9\\"q\\n\\u{1}"'], f'probe --rust str: {got}')
    p = run(*base, '--rust', rs, '--str-type', 'str', 'FX_STR_Q', ok=False)
    check(p.returncode != 0 and 'is not UTF-8' in p.stderr, 'probe --rust str: FX_STR_Q (not UTF-8) not refused')
    p = run(*base, '--rust', rs, '--int-type', 'u8', 'FX_U', ok=False)
    check(p.returncode != 0 and 'do not fit u8' in p.stderr, 'probe --rust: FX_U fits u8')
    vals = json.loads(run(*base, '--span', tables, '--match', '^FX_CODE_').stdout)
    check(len(vals) == 40 and vals.get('FX_CODE_39') == 118, f'probe --span FILE --match: {len(vals)} values')
    p = run(*base, '--span', tables, ok=False)
    check(p.returncode != 0 and '--match' in p.stderr, 'probe --span FILE without --match: no error')
    p = run(*base, '--span', tables + ':5-44', '--match', '^FX_CODE_0')
    check('--match selected 10 of 40 names. Not selected: FX_CODE_10' in p.stderr, f'probe --match: {p.stderr}')
    p = run(*base, '--span', 'no_such_dir/x.h:1-9', ok=False)
    check(p.returncode != 0 and 'no file no_such_dir/x.h' in p.stderr, f'probe --span missing file: {p.stderr}')


def check_filters(td, lib):
    """detect --only and --exclude match the file of an array and the .c files that compile it:
    --only tables.c keeps the header table hdr_tab (tables.c compiles it) but not the copy of copy_tab
    in tables_unit2.c. --exclude hdr removes hdr_tab."""
    for opt, hdr in ((['--only', r'^tables\.c$'], True), (['--exclude', 'hdr'], False)):
        rep = os.path.join(td, 'filtered.json')
        run(TOOL, 'detect', FIXTURE, '--binary', lib, '--json', rep, *opt)
        with open(rep) as f:
            keys = [x['key'] for x in json.load(f)['findings']]
        if 'perm_tab' not in keys or ('hdr_tab' in keys) != hdr or (opt[0] == '--only' and COPY_KEYS[1] in keys):
            return f'detect {" ".join(opt)}: keys {keys}'
    return None


def check_detect(td, lib, check):
    """Classes, reasons, notes, and the report columns."""
    rep = os.path.join(td, 'report.json')
    run(TOOL, 'detect', FIXTURE, '--binary', lib, '--json', rep)
    with open(rep) as f:
        found = json.load(f)['findings']
    tags = lambda items: tuple(x['tag'] for x in items)
    got = {x['key']: x for x in found if x['key']}
    for k, notes in PURE.items():
        x = got.get(k, {})
        check((x.get('cls'), tags(x.get('notes', []))) == ('PURE', notes),
              f'detect {k}: expected PURE {notes}, got {x.get("cls")} {tags(x.get("notes", []))}')
    for k, reasons in TAINTED.items():
        x = got.get(k, {})
        check((x.get('cls'), tags(x.get('reasons', []))) == ('TAINTED', reasons),
              f'detect {k}: expected TAINTED {reasons}, got {x.get("cls")} {tags(x.get("reasons", []))}')
    dead = {x['name']: x for x in found if x['cls'] == 'DEAD'}
    for k, (reason, notes) in DEAD.items():
        x = dead.get(k, {'reasons': [], 'notes': []})
        check((tags(x['reasons']), tags(x['notes'])) == ((reason,), notes),
              f'detect {k}: expected DEAD {reason} {notes}, got {x.get("cls")} {x["reasons"]} {x["notes"]}')
    for k, text in DEAD_TEXT.items():
        check([x['text'] for x in dead.get(k, {'reasons': []})['reasons']] == [text],
              f'detect {k}: expected the reason "{text}", got {dead.get(k)}')
    for k, ptr in POINTER_WRITABLE.items():
        text = [x['text'] for x in got.get(k, {}).get('notes', []) if x['tag'] == 'writable']
        check(len(text) == 1 and ('pointers can change' in text[0]) == ptr, f'detect {k}: writable note {text}')
    extra = {x['key'] or x['name'] for x in found} - set(PURE) - set(TAINTED) - set(DEAD)
    check(not extra, f'detect: unexpected findings {extra}')
    run(TOOL, 'detect', FIXTURE, '--binary', lib, '--json', rep, '--min', '1')
    with open(rep) as f:
        small = {x['key']: x for x in json.load(f)['findings'] if x['key'] in SMALL}
    check(sorted(small) == sorted(SMALL) and all(x['cls'] == 'PURE' for x in small.values()),
          f'detect --min 1: {SMALL} not PURE findings: {small}')
    check(got.get('hdr_tab', {}).get('units') == ['tables.c'], f'detect: hdr_tab units {got.get("hdr_tab")}')
    check(got.get('fn_struct_tab', {}).get('storage') == 'function', 'detect: fn_struct_tab not function storage')
    check(got.get('struct_tab', {}).get('storage') == 'file', 'detect: struct_tab not file storage')
    p = run(TOOL, 'detect', FIXTURE, '--binary', lib)
    check(any(l.startswith('hdr_tab |') and l.endswith('| in tables.c') for l in p.stdout.split('\n')),
          'detect: no "in tables.c" column for hdr_tab')
    check(any(l.startswith('stack_fn |') and l.endswith('why: type (function pointer) | notes: stack copy')
              for l in p.stdout.split('\n')), 'detect: stack_fn line without its reason and note')
    for k, text in STACK_TEXT.items():
        got_text = [x['text'] for x in got.get(k, {}).get('reasons', [])]
        check(any(text in t for t in got_text), f'detect {k}: expected "{text}", got {got_text}')


def check_plain(td, check):
    """A binary that "tab2rust.py build" did not make has no static copies."""
    plain = os.path.join(td, 'plain', 'libplain.so')
    os.makedirs(os.path.dirname(plain))
    subprocess.run(['gcc', '-g', '-O0', '-shared', '-fPIC', '-w', '-o', plain,
                    *[os.path.join(FIXTURE, u) for u in ('tables.c', 'tables_unit2.c')]], check=True)
    rep = os.path.join(td, 'plain.json')
    run(TOOL, 'detect', FIXTURE, '--binary', plain, '--json', rep)
    with open(rep) as f:
        x = next((x for x in json.load(f)['findings'] if x['key'] == 'live_stack'), {})
    check(x.get('cls') == 'TAINTED' and 'not from "tab2rust.py build"' in x['reasons'][0]['text'],
          f'detect live_stack in a plain binary: {x.get("cls")} {x.get("reasons")}')


def check_mismatch(td, lib, check):
    """detect stops if the binary was built from another copy of the source."""
    other = os.path.join(td, 'other_copy')
    shutil.copytree(FIXTURE, other)
    p = run(TOOL, 'detect', other, '--binary', lib, ok=False)
    check(p.returncode != 0 and 'built from other files' in p.stderr, 'detect: no stop on a path mismatch')


def check_translate(td, lib, check):
    """translate output, values, struct rename, refusals, header copies."""
    keys = [k for k in PURE if k not in COPY_KEYS and k not in EMIT_REFUSED] + list(SMALL) + ['fx_gdata', 'fx_bytes']
    out = os.path.join(td, 'tables.rs')
    base = [TOOL, 'translate', '--src', FIXTURE, '--binary', lib]
    p = run(*base, *sum((['--symbol', k] for k in keys), []), '--out', out)
    check('perm_tab -> PERM_TAB: [u8; 64] (' in p.stdout and '= [63, 9, 6, ..., 8]' in p.stdout,
          'translate: no Rust type or preview in the summary:\n' + p.stdout)
    check('notes: macro items (FX_TWO)' in p.stdout, 'translate: no notes for macro_tab:\n' + p.stdout)
    for (e, want), got in zip(STACK_VALUES.items(), rust_values(td, out, list(STACK_VALUES))):
        check(got == want, f'translate stack copy: {e} is {got}, expected {want}')
    with open(out) as f:
        text = f.read()
    for lit in ('1.0e-300, 0.30000000000000004, -0.0, 1.0,', '60.0, 3600.0, 5373485.0, 4.6427e14,',
                'r_limit: 4.6427e14, r_xform: 1.0 }', 'pub struct span_tab_row_r_row {'):
        check(lit in text, f'translate: "{lit}" not in the output')
    pairs = sum((['--pair', f'{k}={RUST_NAMES.get(k, k.upper())}'] for k in keys), [])
    p = run(os.path.join(HERE, 'verify_values.py'), '--binary', lib, '--rs', out, *pairs, ok=False)
    check(p.returncode == 0, 'verify_values:\n' + p.stdout)

    copies = os.path.join(td, 'copies.rs')
    names = sum((['--symbol', k, '--name', f'{k}={RUST_NAMES[k]}'] for k in COPY_KEYS), [])
    run(*base, *names, '--out', copies)
    pairs = sum((['--pair', f'{k}={RUST_NAMES[k]}'] for k in COPY_KEYS), [])
    p = run(os.path.join(HERE, 'verify_values.py'), '--binary', lib, '--rs', copies, *pairs, ok=False)
    check(p.returncode == 0, 'verify_values (header copies):\n' + p.stdout)
    p = run(*base, '--symbol', 'copy_tab', '--out', os.path.join(td, 'x.rs'), ok=False)
    check(p.returncode != 0 and COPY_KEYS[1] in p.stdout, 'translate copy_tab: no choices with @UNIT:\n' + p.stdout)

    spec, rows = os.path.join(td, 'spec.json'), os.path.join(td, 'rows.rs')
    with open(spec, 'w') as f:
        json.dump(ROW_SPEC, f)
    p = run(*base, '--symbol', 'rows_tab', '--spec', spec, '--out', rows)
    check('NOTE    spec "fields" of struct Row: "nope" is not a field' in p.stdout, 'translate: no NOTE for field "nope"')
    with open(rows) as f:
        text = f.read()
    check('pub struct Row {' in text and 'pub r#type: i32,' in text, 'translate: struct Row not defined')
    p = run(os.path.join(HERE, 'verify_values.py'), '--binary', lib, '--rs', rows, '--pair', 'rows_tab=ROWS_TAB', ok=False)
    check(p.returncode == 0, 'verify_values (struct rename):\n' + p.stdout)

    check_anon_spec(td, base, lib, check)

    with open(spec, 'w') as f:  # &str strings: UTF-8 only, else a clear refusal
        json.dump({'strings': 'str'}, f)
    run(*base, '--symbol', 'link_tab', '--spec', spec, '--out', rows)
    got = rust_values(td, rows, ['c::LINK_TAB[3].name', 'c::LINK_TAB[15].name'])
    check(got == ['Some("l3")', 'None'], f'translate strings str: {got}')
    p = run(*base, '--symbol', 'name_tab', '--spec', spec, '--out', rows, ok=False)
    check(p.returncode != 0 and 'is not UTF-8' in p.stdout, 'translate strings str: name_tab (not UTF-8) not refused:\n' + p.stdout)

    for k, reasons in TAINTED.items():
        p = run(*base, '--symbol', k, '--out', os.path.join(td, 'x.rs'), ok=False)
        check(p.returncode != 0 and f'REFUSED {k}: TAINTED: ' in p.stdout
              and all(r in p.stdout for r in reasons), f'translate {k}: not refused with {reasons}:\n{p.stdout}')
    for k, msg in EMIT_REFUSED.items():
        p = run(*base, '--symbol', k, '--out', os.path.join(td, 'x.rs'), ok=False)
        check(p.returncode != 0 and f'REFUSED {k}: ' in p.stdout and msg in p.stdout, f'translate {k}: not refused:\n{p.stdout}')
    p = run(*base, '--symbol', 'dead_tab', '--out', os.path.join(td, 'x.rs'), ok=False)
    check(p.returncode != 0 and 'REFUSED dead_tab: matched 0' in p.stdout, 'translate dead_tab: not refused')
    dup = os.path.join(td, 'dup.rs')  # one C name in two files: the file name goes in front of each Rust name
    p = run(*base, '--symbol', 'tables.c:dup_tab', '--symbol', 'tables_unit2.c:dup_tab', '--out', dup)
    check('file name is added in front: TABLES_DUP_TAB, TABLES_UNIT2_DUP_TAB' in p.stdout,
          'translate: equal default names not prefixed:\n' + p.stdout)
    p = run(os.path.join(HERE, 'verify_values.py'), '--binary', lib, '--rs', dup, '--pair',
            'tables.c:dup_tab=TABLES_DUP_TAB', '--pair', 'tables_unit2.c:dup_tab=TABLES_UNIT2_DUP_TAB', ok=False)
    check(p.returncode == 0, 'verify_values (prefixed names):\n' + p.stdout)


def check_anon_spec(td, base, lib, check):
    """An anonymous struct: rename it by "KEY[]" and by its default Rust name. A spec key that
    matches no struct gives a NOTE."""
    for key in ('xform_tab[]', 'xform_tab_row'):
        spec, rs = os.path.join(td, 'xspec.json'), os.path.join(td, 'xform.rs')
        with open(spec, 'w') as f:
            json.dump({'structs': {key: {'rust': 'XformType', 'define': True, 'fields': XFORM_FIELDS},
                                   'nope[]': {'rust': 'Nope'}}}, f)
        p = run(*base, '--symbol', 'xform_tab', '--spec', spec, '--out', rs)
        with open(rs) as f:
            text = f.read()
        check('pub struct XformType {' in text and 'pub z_name: [u8; 7],' in text
              and 'XformType { n_name: 6, z_name: *b"second\\x00", r_limit: 4.6427e14, r_xform: 1.0 }' in text,
              f'translate xform_tab with "{key}": no struct XformType:\n{text[:800]}')
        check('NOTE    spec "structs" key "nope[]": no array of this command has this struct' in p.stdout,
              'translate: no NOTE for "nope[]"')
        p = run(os.path.join(HERE, 'verify_values.py'), '--binary', lib, '--rs', rs, '--pair', 'xform_tab=XFORM_TAB',
                ok=False)
        check(p.returncode == 0, f'verify_values (xform_tab, "{key}"):\n' + p.stdout)


def check_floats(check):
    """float_literal gives the expected text and the same bits."""
    sys.path.insert(0, os.path.join(HERE, '..', 'scripts'))
    from tab2rust_lib.emit import float_bits, float_literal
    for (v, size), want in FLOATS.items():
        got = float_literal(float_bits(repr(v), size), size)
        check(got == want, f'float_literal({v}, {size}): {got}, expected {want}')


def main():
    failures = []
    check = lambda cond, msg: cond or failures.append(msg)
    with tempfile.TemporaryDirectory() as td:
        build = os.path.join(td, 'build')
        run(TOOL, 'build', FIXTURE, '--out', build, '-DFX_OPTION=1')  # -D: a CMake option (unused here)
        lib = os.path.join(build, 'libfixture.so')
        check_detect(td, lib, check)
        check_plain(td, check)
        rep = os.path.join(td, 'report.json')
        with open(rep) as f:
            regions = json.load(f)['repetitive']
        check(any(r['l0'] <= 10 <= r['l1'] and r['line_head'] == '#define' and r['line_head_share'] > 0.9
                  and r['line_head_lines'] >= 40  # FX_CODE_00 .. FX_CODE_39
                  for r in regions), 'detect: #define run not REPETITIVE with first token #define')
        err = check_filters(td, lib)
        check(err is None, err)
        check_mismatch(td, lib, check)
        check_translate(td, lib, check)
        check_floats(check)
        check_probe(build, check)
    for f in failures:
        print('FAIL', f)
    print('all tests passed' if not failures else f'{len(failures)} failures')
    sys.exit(1 if failures else 0)


if __name__ == '__main__':
    main()
