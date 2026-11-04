"""One measurement of a crate: items, unsafe operations and lints, attributed to functions."""
import collections
import os
import re
import time

from . import cargo, items, lints, recipes, unsafe_ops

# Crate-level allows that a translation of C may keep: C names in the ABI, and C code that the
# configuration does not use.
NAMING_ALLOWS = frozenset({'non_snake_case', 'non_camel_case_types', 'non_upper_case_globals', 'dead_code'})
CAST_LINTS = frozenset({'cast_lossless', 'ptr_as_ptr', 'ptr_cast_constness', 'borrow_as_ptr', 'unnecessary_cast'})


class FnIndex:
    """Finds the innermost function that contains a line of a file."""

    def __init__(self, fns):
        self.by_file = collections.defaultdict(list)
        for f in fns:
            self.by_file[f.file].append(f)
        for lst in self.by_file.values():
            lst.sort(key=lambda f: (f.first_line, -f.last_line))

    def at(self, file, line):
        best = None
        for f in self.by_file.get(file, ()):
            if f.first_line > line:
                break
            if f.first_line <= line <= f.last_line:
                if best is None or f.lines <= best.lines:
                    best = f
        return best


def run(crate_dir, work_dir, log=print):
    """Measures the crate. Returns a dict with 'rows' (one per function), 'ops', 'lints' and 'totals'."""
    t0 = time.time()
    crate_dir = os.path.abspath(crate_dir)
    os.makedirs(work_dir, exist_ok=True)
    files = cargo.source_files(crate_dir)
    scanned, raw_aliases = items.scan_sources(crate_dir, files)
    fns = [f for it in scanned.values() for f in it.fns]
    index = FnIndex(fns)
    log(f'qscan: {len(files)} files, {len(fns)} functions; measuring unsafe operations ...')
    ops, op_errors, op_note = unsafe_ops.measure(crate_dir, work_dir, files)
    log(f'qscan: {len(ops)} unsafe operations ({op_note}); running clippy ...')
    lint_list, lint_errors = lints.measure(crate_dir, work_dir)

    # Callers by name: which files call each function name.
    callers = collections.defaultdict(set)
    for rel, it in scanned.items():
        for name in it.calls:
            callers[name].add(rel)

    lines_of = {}
    for rel in files:
        with open(os.path.join(crate_dir, rel), encoding='utf-8', errors='replace') as fh:
            lines_of[rel] = fh.read().split('\n')

    per_fn = collections.defaultdict(lambda: collections.Counter())
    for op in ops:
        f = index.at(op['file'], op['line'])
        op['fn'] = f.qual if f else '-'
        op['var'], op['var_type'] = _deref_target(op, f, lines_of) if op['kind'] == 'deref' else ('', '')
        key = (f.file, f.first_line) if f else None
        if key:
            per_fn[key]['ops'] += 1
            per_fn[key][op['kind'] if op['kind'] in ('deref', 'call', 'static_mut') else 'other_ops'] += 1
    for lint in lint_list:
        f = index.at(lint['file'], lint['line'])
        lint['fn'] = f.qual if f else '-'
        if not f:
            continue
        key = (f.file, f.first_line)
        if lint['name'] == 'unused_unsafe':
            per_fn[key]['unused_unsafe'] += 1
        elif lint['group'] == 'extra':
            per_fn[key]['extra_lints'] += 1
        else:
            per_fn[key]['lints'] += 1
        if lint['name'] in CAST_LINTS:
            per_fn[key]['cast_lints'] += 1

    rows = []
    for f in fns:
        c = per_fn[(f.file, f.first_line)]
        pseudo = not f.is_unsafe and bool(f.raw_ptr_params) and not f.in_trait_impl and f.raw_in_unsafe
        elsewhere = sorted(callers.get(f.name, set()) - {f.file})
        r3 = sorted((u['deref'], p) for p, u in f.uses.items() if u['deref'] and not u['block'])
        blocked = sorted(p for p, u in f.uses.items() if u['deref'] and u['block'])
        row = dict(
            file=f.file, first_line=f.first_line, last_line=f.last_line, fn=f.qual,
            vis='exported' if f.exported else 'internal', unsafe_fn='y' if f.is_unsafe else 'n',
            trait_impl='y' if f.in_trait_impl else 'n', raw_params=len(f.raw_ptr_params),
            ops=c['ops'], deref=c['deref'], call=c['call'], static_mut=c['static_mut'],
            other_ops=c['other_ops'], unsafe_blocks=f.facts['unsafe_blocks'],
            unused_unsafe=c['unused_unsafe'], lints=c['lints'], extra_lints=c['extra_lints'],
            cast_lints=c['cast_lints'], as_casts=f.facts['as_casts'], ptr_arith=f.facts['ptr_arith'],
            c_memory=f.facts['c_memory'], alloc=f.facts['alloc'], zeroed=f.facts['zeroed'],
            pseudo_safe='y' if pseudo else 'n',
            callers_elsewhere=','.join(elsewhere[:6]) + (',...' if len(elsewhere) > 6 else '') if elsewhere else '-',
            r3_vars=','.join(f'{p}({n})' for n, p in reversed(r3)) or '-',
            blocked_vars=','.join(blocked) or '-')
        row['recipes'] = ' '.join(recipes.suggest(row)) or '-'
        row['score'] = round(recipes.score(row), 1)
        rows.append(row)

    totals = _totals(crate_dir, scanned, rows, ops, lint_list)
    totals['repeated_safety_docs'] = _repeated_safety_docs(lines_of)
    totals['structs_with_raw_fields'] = [
        dict(file=st.file, line=st.line, name=st.name, fields=st.fields, raw_fields=[f'{n}: {t}' for n, t in st.raw_fields])
        for it in scanned.values() for st in it.structs if st.raw_fields]
    totals['raw_pointer_aliases'] = sorted(raw_aliases)
    totals['measure_errors'] = op_errors[:20] + lint_errors[:20]
    totals['unsafe_ops_target'] = op_note
    totals['seconds'] = round(time.time() - t0, 1)
    return dict(rows=rows, ops=ops, lints=lint_list, totals=totals)


DEREF_VAR = re.compile(r'[\s(]*\*+[\s(*]*(?:mut\s+)?([A-Za-z_]\w*)')


def _deref_target(op, fn, lines_of):
    """(variable, its type) of a raw pointer dereference: the first identifier of the dereferenced
    expression, typed from the function's parameters or a typed `let` in the function."""
    lines = lines_of.get(op['file'], [])
    if not 0 < op['line'] <= len(lines):
        return '', ''
    m = DEREF_VAR.match(lines[op['line'] - 1][op['col'] - 1:])
    if not m:
        return '', ''
    var = m.group(1)
    if fn is not None:
        for p in fn.params:
            if p.name == var:
                return var, p.type_text
        body = '\n'.join(lines[fn.first_line - 1:fn.last_line])
        let = re.search(r'\blet\s+(?:mut\s+)?' + re.escape(var) + r'\s*:\s*([^=;]+?)\s*[=;]', body)
        if let:
            return var, ' '.join(let.group(1).split())
    return var, 'local'


SAFETY_DOC = re.compile(r'^\s*///\s*#\s*Safety\s*$')


def _repeated_safety_docs(lines_of, limit=3):
    """`# Safety` texts that more than `limit` functions share word for word (boilerplate).
    Returns [(count, first words)] sorted by count."""
    texts = collections.Counter()
    for lines in lines_of.values():
        for i, line in enumerate(lines):
            if not SAFETY_DOC.match(line):
                continue
            body = []
            for nxt in lines[i + 1:i + 12]:
                stripped = nxt.strip()
                if not stripped.startswith('///'):
                    break
                body.append(stripped[3:].strip())
            text = ' '.join(w for w in ' '.join(body).split())
            if text:
                texts[text] += 1
    return [[n, t[:80]] for t, n in texts.most_common() if n > limit]


def _totals(crate_dir, scanned, rows, ops, lint_list):
    code = sum(it.code_lines for it in scanned.values())
    fn_lines = sum(r['last_line'] - r['first_line'] + 1 for r in rows)
    kinds = collections.Counter(o['kind'] for o in ops)
    vis_of = {(r['file'], r['fn']): r['vis'] for r in rows}
    by_vis = collections.Counter(vis_of.get((o['file'], o.get('fn')), 'outside') for o in ops)
    lint_groups = collections.Counter(l['group'] for l in lint_list)
    lint_names = collections.Counter(l['name'] for l in lint_list)
    darpa_lints = sum(n for g, n in lint_groups.items() if g in lints.DARPA_GROUPS or g == 'clippy')
    blanket = []
    naming = []
    for rel, it in scanned.items():
        for line, allowed in it.allows:
            for lint in allowed:
                (naming if lint in NAMING_ALLOWS else blanket).append(f'{rel}:{line} {lint}')
    safe_fns = [r for r in rows if r['unsafe_fn'] == 'n' and r['unsafe_blocks'] == 0]
    pseudo = [r for r in rows if r['pseudo_safe'] == 'y']
    exported_names = {r['fn'] for r in rows if r['vis'] == 'exported'}
    per_file = collections.defaultdict(collections.Counter)
    for r in rows:
        pf = per_file[r['file']]
        pf['fns'] += 1
        pf['ops'] += r['ops']
        pf['lints'] += r['lints']
        pf['extra_lints'] += r['extra_lints']
        pf['unsafe_fns'] += r['unsafe_fn'] == 'y'
        pf['pseudo_safe'] += r['pseudo_safe'] == 'y'
    for rel, it in scanned.items():
        per_file[rel]['code_lines'] = it.code_lines
    return dict(
        code_lines=code,
        functions=len(rows),
        exported_functions=sum(r['vis'] == 'exported' for r in rows),
        unsafe_functions=sum(r['unsafe_fn'] == 'y' for r in rows),
        unsafe_blocks=sum(r['unsafe_blocks'] for r in rows),
        unsafe_ops=len(ops),
        unsafe_ops_per_kloc=round(1000 * len(ops) / code, 1) if code else 0,
        unsafe_ops_by_kind=dict(kinds.most_common()),
        unsafe_ops_exported=by_vis['exported'],
        unsafe_ops_internal=by_vis['internal'],
        unsafe_ops_outside_fn=by_vis['outside'],
        safe_function_share=round(sum(r['last_line'] - r['first_line'] + 1 for r in safe_fns) / fn_lines, 3) if fn_lines else 0,
        pseudo_safe_functions=len(pseudo),
        pseudo_safe_list=[f"{r['file']}:{r['first_line']} {r['fn']}" for r in pseudo][:50],
        darpa_lints=darpa_lints,
        darpa_lints_per_kloc=round(1000 * darpa_lints / code, 1) if code else 0,
        rustc_warnings=lint_groups.get('rustc', 0),
        extra_lints=lint_groups.get('extra', 0),
        lints_by_group=dict(lint_groups.most_common()),
        lints_by_name=dict(lint_names.most_common()),
        blanket_allows=blanket,
        naming_allows=naming,
        static_mut_items=sum(it.static_muts for it in scanned.values()),
        self_imported_extern_decls=sum(1 for it in scanned.values() for d in it.extern_decls
                                       if d in exported_names),
        per_file={rel: dict(c) for rel, c in sorted(per_file.items())},
    )
