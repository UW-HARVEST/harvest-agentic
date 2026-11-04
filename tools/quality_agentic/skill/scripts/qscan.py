#!/usr/bin/env python3
"""qscan: measure the safety and the lints of a Rust crate per function, and suggest recipes.

Subcommands:
  scan     Measure the crate: unsafe operations (as DARPA counts them), clippy and rustc lints
           (crate allows ignored), pseudo-safe functions. Writes a scan directory.
  file     The worklist of one file (or line ranges of it) from a scan: functions with suggested
           recipes, highest score first.
  fn       All measured facts of one function.
  compare  Compare two scans: total and per-file changes, and the problems a change must not create.
  plan     Divide the work of a scan into units and waves for worker sub-agents.
  roots    Root causes of the unsafe operations, grouped for global refactoring: types reached
           through raw pointers, buffer cursors, unsafe callees, structs with raw pointer fields.
  autofix  Apply the syntax-only clippy fixes, one lint at a time, each built and optionally committed.

Run from the root of the crate (your checkout). `qscan.py <subcommand> -h` shows the options.
Docs: ../SKILL.md and ../RECIPES.md.
"""
import argparse
import os
import sys

sys.dont_write_bytecode = True  # no __pycache__ in the skill directory (it is embedded and copied to agents)
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from qscan_lib import cargo, lints, plan, pyenv, report, roots  # noqa: E402

DEFAULT_WORK = 'scratch/qscan-work'


def cmd_scan(a):
    pyenv.ensure(a.crate)  # the parser needs tree-sitter
    from qscan_lib import scan
    data = scan.run(a.crate, a.work, log=lambda m: print(m, file=sys.stderr))
    report.write(data, a.out)
    with open(os.path.join(a.out, 'summary.md'), encoding='utf-8') as fh:
        text = fh.read()
    # The head of the summary; the full text is in the scan directory.
    print('\n'.join(text.splitlines()[:30]))
    print(f'\nqscan: wrote {a.out}/ (summary.md, functions.tsv, ops.tsv, lints.tsv) in {data["totals"]["seconds"]} s')


def cmd_file(a):
    s = report.load(a.scan)
    print(report.file_view(s, os.path.normpath(a.path), report.parse_ranges(a.lines), a.all, a.limit), end='')


def cmd_fn(a):
    print(report.fn_view(report.load(a.scan), a.name), end='')


def cmd_compare(a):
    files = [os.path.normpath(f) for f in a.files.split(',')] if a.files else None
    text, _ = report.compare(report.load(a.before), report.load(a.after), files)
    print(text, end='')


def cmd_plan(a):
    text = plan.render(report.load(a.scan), a.budget_lines, a.parallel, a.min_score)
    if a.out:
        os.makedirs(os.path.dirname(a.out) or '.', exist_ok=True)
        with open(a.out, 'w', encoding='utf-8') as fh:
            fh.write(text)
    print(text, end='')


def cmd_roots(a):
    text = roots.report(report.load(a.scan), a.top)
    if a.out:
        os.makedirs(os.path.dirname(a.out) or '.', exist_ok=True)
        with open(a.out, 'w', encoding='utf-8') as fh:
            fh.write(text)
    print(text, end='')


def cmd_autofix(a):
    chosen = a.lints.split(',') if a.lints else list(lints.SAFE_AUTOFIX)
    unknown = [x for x in chosen if x not in lints.SAFE_AUTOFIX]
    if unknown:
        sys.exit(f'qscan: not in the safe autofix list: {", ".join(unknown)}')
    print(f'qscan autofix: {len(chosen)} lints, one at a time (each one is built) ...')
    applied = lints.autofix(a.crate, a.work, chosen, a.commit)
    print(f'qscan autofix: applied {len(applied)} lints: {", ".join(applied) or "-"}')
    if applied and a.commit:
        print('Each applied lint is one commit ("quality autofix: <lint>"). Run the regression check now; '
              'if it fails, revert the autofix commits one at a time (git revert --no-edit <sha>) to find the bad one.')


def parser():
    ap = argparse.ArgumentParser(prog='qscan.py', description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('--crate', default='.', help='crate root (default: the current directory)')
    ap.add_argument('--work', default=None, help=f'directory for build copies (default: <crate>/{DEFAULT_WORK})')
    sub = ap.add_subparsers(dest='cmd', required=True)

    p = sub.add_parser('scan', help='measure the crate and write a scan directory')
    p.add_argument('--out', required=True, help='scan directory to write, for example scratch/qscan_before')
    p.set_defaults(run=cmd_scan)

    p = sub.add_parser('file', help='worklist of one file from a scan')
    p.add_argument('path', help='file relative to the crate root, for example src/lz4.rs')
    p.add_argument('--scan', required=True, help='scan directory')
    p.add_argument('--lines', help='only functions that start in these line ranges, for example 100-900,1200-1500')
    p.add_argument('--all', action='store_true', help='also functions without suggested recipes')
    p.add_argument('--limit', type=int, default=25, help='maximum functions to print (default 25)')
    p.set_defaults(run=cmd_file)

    p = sub.add_parser('fn', help='all facts of one function')
    p.add_argument('name', help='function name or Type::name')
    p.add_argument('--scan', required=True, help='scan directory')
    p.set_defaults(run=cmd_fn)

    p = sub.add_parser('compare', help='compare two scans')
    p.add_argument('before', help='scan directory before the change')
    p.add_argument('after', help='scan directory after the change')
    p.add_argument('--files', help='comma list of files to show and check (default: all)')
    p.set_defaults(run=cmd_compare)

    p = sub.add_parser('plan', help='divide the work of a scan into units and waves')
    p.add_argument('--scan', required=True, help='scan directory')
    p.add_argument('--budget-lines', type=int, default=1500, help='code lines per unit (default 1500)')
    p.add_argument('--parallel', type=int, default=5, help='units per wave (default 5)')
    p.add_argument('--min-score', type=float, default=3.0, help='skip files with less work (default 3)')
    p.add_argument('--out', help='also write the plan to this file')
    p.set_defaults(run=cmd_plan)

    p = sub.add_parser('roots', help='root causes of the unsafe operations, for global refactoring')
    p.add_argument('--scan', required=True, help='scan directory')
    p.add_argument('--top', type=int, default=15, help='rows per table (default 15)')
    p.add_argument('--out', help='also write the report to this file')
    p.set_defaults(run=cmd_roots)

    p = sub.add_parser('autofix', help='syntax-only clippy fixes, one lint at a time')
    p.add_argument('--lints', help='comma list (default: the whole safe list, see qscan_lib/lints.py)')
    p.add_argument('--commit', action='store_true', help='commit each applied lint (needs a clean git status)')
    p.set_defaults(run=cmd_autofix)
    return ap


def main():
    a = parser().parse_args()
    a.crate = os.path.abspath(a.crate)
    a.work = os.path.abspath(a.work or os.path.join(a.crate, DEFAULT_WORK))
    try:
        a.run(a)
    except cargo.CargoError as e:
        sys.exit(f'qscan: {e}')


if __name__ == '__main__':
    main()
