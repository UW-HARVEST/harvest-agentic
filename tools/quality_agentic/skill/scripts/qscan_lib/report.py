"""Write a scan to a directory, read it back, and print the views that agents use."""
import csv
import json
import os

from .recipes import TITLES

FN_COLUMNS = ('file', 'first_line', 'last_line', 'fn', 'vis', 'unsafe_fn', 'trait_impl', 'raw_params',
              'ops', 'deref', 'call', 'static_mut', 'other_ops', 'unsafe_blocks', 'unused_unsafe',
              'lints', 'extra_lints', 'cast_lints', 'as_casts', 'ptr_arith', 'c_memory', 'alloc',
              'zeroed', 'pseudo_safe', 'callers_elsewhere', 'r3_vars', 'blocked_vars', 'recipes', 'score')
INT_COLUMNS = frozenset({'first_line', 'last_line', 'raw_params', 'ops', 'deref', 'call', 'static_mut',
                         'other_ops', 'unsafe_blocks', 'unused_unsafe', 'lints', 'extra_lints',
                         'cast_lints', 'as_casts', 'ptr_arith', 'c_memory', 'alloc', 'zeroed'})
OP_COLUMNS = ('file', 'line', 'col', 'fn', 'kind', 'detail', 'var', 'var_type')
LINT_COLUMNS = ('file', 'line', 'fn', 'group', 'name', 'message')


def _write_tsv(path, columns, records):
    with open(path, 'w', encoding='utf-8', newline='') as fh:
        w = csv.writer(fh, delimiter='\t', lineterminator='\n')
        w.writerow(columns)
        for r in records:
            w.writerow([str(r.get(c, '')).replace('\t', ' ').replace('\n', ' ') for c in columns])


def _read_tsv(path):
    with open(path, encoding='utf-8', newline='') as fh:
        rows = list(csv.DictReader(fh, delimiter='\t'))
    for r in rows:
        for c in INT_COLUMNS | {'line', 'col'}:
            if c in r and r[c] != '':
                r[c] = int(r[c])
        if 'score' in r:
            r['score'] = float(r['score'])
    return rows


def write(scan, out_dir):
    os.makedirs(out_dir, exist_ok=True)
    rows = sorted(scan['rows'], key=lambda r: (r['file'], r['first_line']))
    _write_tsv(os.path.join(out_dir, 'functions.tsv'), FN_COLUMNS, rows)
    _write_tsv(os.path.join(out_dir, 'ops.tsv'), OP_COLUMNS, sorted(scan['ops'], key=lambda o: (o['file'], o['line'], o['col'])))
    _write_tsv(os.path.join(out_dir, 'lints.tsv'), LINT_COLUMNS, sorted(scan['lints'], key=lambda l: (l['file'], l['line'])))
    with open(os.path.join(out_dir, 'summary.json'), 'w', encoding='utf-8') as fh:
        json.dump(scan['totals'], fh, indent=1)
    with open(os.path.join(out_dir, 'summary.md'), 'w', encoding='utf-8') as fh:
        fh.write(summary_text(scan['totals'], rows))


def load(scan_dir):
    if not os.path.isfile(os.path.join(scan_dir, 'summary.json')):
        raise SystemExit(f'qscan: no scan in {scan_dir} (run `qscan.py scan --out {scan_dir}` first)')
    with open(os.path.join(scan_dir, 'summary.json'), encoding='utf-8') as fh:
        totals = json.load(fh)
    return dict(totals=totals,
                rows=_read_tsv(os.path.join(scan_dir, 'functions.tsv')),
                ops=_read_tsv(os.path.join(scan_dir, 'ops.tsv')),
                lints=_read_tsv(os.path.join(scan_dir, 'lints.tsv')))


def summary_text(t, rows):
    kinds = ', '.join(f'{k} {v}' for k, v in t['unsafe_ops_by_kind'].items()) or '-'
    groups = ', '.join(f'{k} {v}' for k, v in t['lints_by_group'].items()) or '-'
    lines = [
        '# qscan summary', '',
        f"- Rust code lines: {t['code_lines']}; functions: {t['functions']} "
        f"({t['exported_functions']} exported, {t['unsafe_functions']} `unsafe fn`)",
        f"- Unsafe operations: {t['unsafe_ops']} ({t['unsafe_ops_per_kloc']} per 1000 lines); "
        f"in exported functions {t['unsafe_ops_exported']}, internal {t['unsafe_ops_internal']}, "
        f"outside functions {t['unsafe_ops_outside_fn']}; by kind: {kinds}",
        f"- `unsafe {{}}` blocks: {t['unsafe_blocks']}; share of function lines in fully safe functions: "
        f"{100 * t['safe_function_share']:.1f}%",
        f"- Suspected pseudo-safe functions (safe signature, raw pointer parameter used in an unsafe block): "
        f"{t['pseudo_safe_functions']}",
        f"- Lints (allows ignored): DARPA groups {t['darpa_lints']} ({t['darpa_lints_per_kloc']} per 1000 lines), "
        f"rustc warnings {t['rustc_warnings']}, extra C-style lints {t['extra_lints']}; by group: {groups}",
        f"- `missing_safety_doc` lints (an `unsafe fn` without `# Safety` docs): {t['lints_by_name'].get('missing_safety_doc', 0)}; "
        f"`# Safety` texts shared by more than 3 functions (boilerplate): {sum(n for n, _ in t.get('repeated_safety_docs', []))}",
        f"- Structs with raw pointer fields: {len(t.get('structs_with_raw_fields', []))}",
        f"- `static mut` items: {t['static_mut_items']}; extern declarations of the crate's own exports: "
        f"{t['self_imported_extern_decls']}",
        f"- Blanket crate-level allows: {len(t['blanket_allows'])}"
        + (f" ({'; '.join(t['blanket_allows'][:8])}{' ...' if len(t['blanket_allows']) > 8 else ''})" if t['blanket_allows'] else ''),
    ]
    if t.get('measure_errors'):
        lines.append(f"- WARNING: {len(t['measure_errors'])} compile errors during measurement; the counts can be "
                     f"incomplete. First: {t['measure_errors'][0]}")
    lines += ['', '## Files (by work)', '', '| file | code lines | fns | unsafe ops | lints | extra lints | pseudo-safe |',
              '|---|---|---|---|---|---|---|']
    per_file = sorted(t['per_file'].items(), key=lambda kv: -(kv[1].get('ops', 0) + kv[1].get('lints', 0)))
    for rel, c in per_file:
        lines.append(f"| {rel} | {c.get('code_lines', 0)} | {c.get('fns', 0)} | {c.get('ops', 0)} | "
                     f"{c.get('lints', 0)} | {c.get('extra_lints', 0)} | {c.get('pseudo_safe', 0)} |")
    lines += ['', '## Top lints', '']
    for name, n in list(t['lints_by_name'].items())[:15]:
        lines.append(f'- {name}: {n}')
    top = sorted(rows, key=lambda r: -r['ops'])[:15]
    lines += ['', '## Functions with the most unsafe operations', '']
    for r in top:
        if r['ops']:
            lines.append(f"- {r['file']}:{r['first_line']} {r['fn']} ({r['vis']}): {r['ops']} ops; recipes {r['recipes']}")
    return '\n'.join(lines) + '\n'


def _in_ranges(row, ranges):
    if not ranges:
        return True
    return any(a <= row['first_line'] <= b for a, b in ranges)


def parse_ranges(text):
    if not text:
        return []
    out = []
    for part in text.split(','):
        a, _, b = part.partition('-')
        out.append((int(a), int(b or a)))
    return out


def file_view(scan, rel, ranges, show_all=False, limit=60):
    """The worklist of one file: functions with work, highest score first."""
    rows = [r for r in scan['rows'] if r['file'] == rel and _in_ranges(r, ranges)]
    if not rows:
        known = sorted({r['file'] for r in scan['rows']})
        return f'qscan: no functions of {rel} in this scan. Files: {", ".join(known[:30])}\n'
    work = [r for r in rows if show_all or r['recipes'] != '-']
    work.sort(key=lambda r: -r['score'])
    ops_by_fn = {}
    for o in scan['ops']:
        if o['file'] == rel:
            ops_by_fn.setdefault(o['fn'], []).append(o)
    lints_by_fn = {}
    for l in scan['lints']:
        if l['file'] == rel:
            lints_by_fn.setdefault(l['fn'], []).append(l)
    total_ops = sum(r['ops'] for r in rows)
    total_lints = sum(r['lints'] for r in rows)
    out = [f'# Worklist of {rel}' + (f' lines {",".join(f"{a}-{b}" for a, b in ranges)}' if ranges else ''),
           f'{len(rows)} functions, {total_ops} unsafe ops, {total_lints} lints. '
           f'{len(work)} functions have suggested recipes. Highest score first.', '']
    for r in work[:limit]:
        flags = []
        if r['vis'] == 'exported':
            flags.append('EXPORTED: keep its C signature')
        if r['unsafe_fn'] == 'y':
            flags.append('unsafe fn')
        if r['pseudo_safe'] == 'y':
            flags.append('PSEUDO-SAFE: make it `unsafe fn` again or remove the raw pointer use')
        if r['callers_elsewhere'] != '-':
            flags.append(f"called from {r['callers_elsewhere']}")
        if r.get('r3_vars', '-') != '-':
            flags.append(f"R3 candidates (dereferenced or passed on only; count of dereferences): {r['r3_vars']}")
        if r.get('blocked_vars', '-') != '-':
            flags.append(f"pointer parameters with arithmetic, comparison, null check or assignment: {r['blocked_vars']}")
        out.append(f"## {r['fn']}  lines {r['first_line']}-{r['last_line']}  score {r['score']}")
        out.append(f"- facts: ops {r['ops']} (deref {r['deref']}, call {r['call']}, static_mut {r['static_mut']}), "
                   f"raw ptr params {r['raw_params']}, unsafe blocks {r['unsafe_blocks']}, unused unsafe {r['unused_unsafe']}, "
                   f"lints {r['lints']}, extra lints {r['extra_lints']}, as casts {r['as_casts']}, "
                   f"ptr arithmetic {r['ptr_arith']}, C memory calls {r['c_memory']}, zeroed {r['zeroed']}")
        if flags:
            out.append(f"- {'; '.join(flags)}")
        rec = [c for c in r['recipes'].split() if c != '-']
        if rec:
            out.append('- recipes: ' + '; '.join(f'{c} ({TITLES.get(c.rstrip("sx"), "?")}'
                                                 f'{", shim variant" if c.endswith("s") else ""}'
                                                 f'{", exported variant" if c.endswith("x") else ""})' for c in rec))
        ops = ops_by_fn.get(r['fn'], [])
        if ops:
            calls = sorted({o['detail'].split('::')[-1] for o in ops if o['kind'] == 'call' and o['detail']})
            lines = sorted({o['line'] for o in ops})
            out.append(f"- unsafe op lines: {', '.join(map(str, lines[:20]))}{' ...' if len(lines) > 20 else ''}"
                       + (f"; unsafe callees: {', '.join(calls[:10])}" if calls else ''))
        ls = lints_by_fn.get(r['fn'], [])
        if ls:
            shown = [f"L{l['line']} {l['name']}" for l in ls if l['group'] != 'extra'][:10]
            extra = sum(1 for l in ls if l['group'] == 'extra')
            out.append(f"- lints: {', '.join(shown) or '-'}" + (f"; extra: {extra}" if extra else ''))
        out.append('')
    if len(work) > limit:
        out.append(f'... {len(work) - limit} more functions (use --limit)')
    out.append('Recipes: inputs/agent_skills/rust_quality/RECIPES.md. Suffix s = keep a shim with the old '
               'signature for callers in other files; suffix x = exported, keep the C signature.')
    return '\n'.join(out) + '\n'


def fn_view(scan, name):
    rows = [r for r in scan['rows'] if r['fn'] == name or r['fn'].endswith('::' + name)]
    if not rows:
        return f'qscan: no function {name}\n'
    out = []
    for r in rows:
        out.append(f"## {r['fn']} in {r['file']} lines {r['first_line']}-{r['last_line']}")
        out.append('- ' + ', '.join(f'{c} {r[c]}' for c in ('vis', 'unsafe_fn', 'raw_params', 'ops', 'deref', 'call',
                                                             'static_mut', 'unsafe_blocks', 'lints', 'pseudo_safe',
                                                             'callers_elsewhere', 'recipes')))
        for o in scan['ops']:
            if o['file'] == r['file'] and o['fn'] == r['fn']:
                out.append(f"  op L{o['line']}:{o['col']} {o['kind']} {o['detail']}")
        for l in scan['lints']:
            if l['file'] == r['file'] and l['fn'] == r['fn']:
                out.append(f"  lint L{l['line']} {l['name']} ({l['group']}): {l['message']}")
    return '\n'.join(out) + '\n'


def compare(before, after, files=None):
    """Totals and per-file deltas, and the problems that a change must not create."""
    tb, ta = before['totals'], after['totals']
    keys = ('code_lines', 'unsafe_ops', 'unsafe_ops_exported', 'unsafe_ops_internal', 'unsafe_functions',
            'unsafe_blocks', 'pseudo_safe_functions', 'darpa_lints', 'rustc_warnings', 'extra_lints',
            'static_mut_items', 'exported_functions')
    out = ['# qscan compare', '', '| measure | before | after | change |', '|---|---|---|---|']
    for k in keys:
        out.append(f'| {k} | {tb.get(k, 0)} | {ta.get(k, 0)} | {ta.get(k, 0) - tb.get(k, 0):+d} |')
    out.append(f"| safe_function_share | {tb['safe_function_share']:.3f} | {ta['safe_function_share']:.3f} | "
               f"{ta['safe_function_share'] - tb['safe_function_share']:+.3f} |")
    sel = set(files) if files else set(ta['per_file']) | set(tb['per_file'])
    out += ['', '| file | ops before | ops after | lints before | lints after |', '|---|---|---|---|---|']
    for rel in sorted(sel):
        b = tb['per_file'].get(rel, {})
        a = ta['per_file'].get(rel, {})
        out.append(f"| {rel} | {b.get('ops', 0)} | {a.get('ops', 0)} | {b.get('lints', 0)} | {a.get('lints', 0)} |")
    problems = []
    exp_b = {r['fn'] for r in before['rows'] if r['vis'] == 'exported'}
    exp_a = {r['fn'] for r in after['rows'] if r['vis'] == 'exported'}
    if exp_b - exp_a:
        problems.append(f"exported functions gone: {', '.join(sorted(exp_b - exp_a)[:20])}")
    if exp_a - exp_b:
        problems.append(f"new exported functions: {', '.join(sorted(exp_a - exp_b)[:20])}")
    pseudo_b = {(r['file'], r['fn']) for r in before['rows'] if r['pseudo_safe'] == 'y'}
    new_pseudo = [f'{f}:{n}' for f, n in sorted({(r['file'], r['fn']) for r in after['rows'] if r['pseudo_safe'] == 'y'} - pseudo_b)
                  if not files or f in sel]
    if new_pseudo:
        problems.append(f"new suspected pseudo-safe functions: {', '.join(new_pseudo[:20])}")
    def allow_keys(entries):  # 'file:line lint' -> (file, lint): lines move when a file changes
        return {(e.split(':', 1)[0], e.rsplit(' ', 1)[-1]) for e in entries}
    new_allows = [f'{f} {lint}' for f, lint in sorted(allow_keys(ta['blanket_allows']) - allow_keys(tb['blanket_allows']))]
    if new_allows:
        problems.append(f"new blanket allows: {', '.join(new_allows[:10])}")
    ops_b = {(r['file'], r['fn']): r['ops'] for r in before['rows']}
    worse = [f"{r['file']}:{r['fn']} {ops_b[(r['file'], r['fn'])]}->{r['ops']}" for r in after['rows']
             if (r['file'], r['fn']) in ops_b and r['ops'] > ops_b[(r['file'], r['fn'])] and (not files or r['file'] in sel)]
    rep_b = sum(n for n, _ in tb.get('repeated_safety_docs', []))
    rep_a = sum(n for n, _ in ta.get('repeated_safety_docs', []))
    if rep_a > rep_b:
        problems.append(f"boilerplate `# Safety` docs: {rep_a} functions share a text with more than 3 others "
                        f"(before: {rep_b}); write each function's real preconditions: "
                        + '; '.join(f'{n}x "{t}"' for n, t in ta['repeated_safety_docs'][:3]))
    if ta.get('measure_errors'):
        problems.append(f"the after-scan had compile errors: {ta['measure_errors'][0]}")
    out.append('')
    if worse:
        # Not a problem: a change across functions can move operations, for example into the
        # exported function that converts its raw arguments for a safe core.
        out.append(f"Functions with more unsafe ops than before: {', '.join(worse[:15])}")
        out.append('')
    if problems:
        out.append('PROBLEMS (fix them before you finish):')
        out += [f'- {p}' for p in problems]
    else:
        out.append('No problems: no exported function changed, no new pseudo-safe function, no new blanket allow.')
    return '\n'.join(out) + '\n', problems
