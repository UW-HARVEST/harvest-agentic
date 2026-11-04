"""Clippy and rustc warnings, measured honestly: lint levels are forced from the command line, so a
crate-level `#![allow(clippy::all)]` does not hide anything.

DARPA counts the five default clippy groups and the rustc warnings. qscan reports them, and in a
separate list a few extra lints that point at C-style code (`cast_lossless`, `ptr_as_ptr`,
`undocumented_unsafe_blocks`). It also runs `cargo clippy --fix` for a curated list of lints whose
fix only changes syntax.
"""
import os
import re
import subprocess

from . import cargo

DARPA_GROUPS = ('correctness', 'suspicious', 'complexity', 'perf', 'style')
EXTRA_LINTS = ('cast_lossless', 'ptr_as_ptr', 'ptr_cast_constness', 'borrow_as_ptr', 'undocumented_unsafe_blocks')

# Lints whose machine-applicable fix keeps the behavior: syntax only. Each is applied and built on
# its own by `autofix`, so a bad one can be reverted alone.
SAFE_AUTOFIX = (
    'needless_return', 'let_and_return', 'unused_unit', 'redundant_field_names',
    'redundant_static_lifetimes', 'needless_borrow', 'collapsible_if', 'collapsible_else_if',
    'unnecessary_cast', 'ptr_offset_with_cast', 'zero_ptr', 'char_lit_as_u8', 'bool_comparison',
    'needless_bool', 'assign_op_pattern', 'redundant_closure', 'clone_on_copy',
    'unnecessary_mut_passed', 'len_zero', 'redundant_pattern_matching', 'precedence',
    'needless_late_init', 'match_like_matches_macro', 'manual_range_contains', 'single_match',
    'unused_unsafe', 'unused_parens', 'unused_mut', 'cast_lossless', 'ptr_as_ptr',
    'ptr_cast_constness', 'borrow_as_ptr', 'explicit_auto_deref', 'needless_borrows_for_generic_args',
    'redundant_slicing', 'unnecessary_operation', 'toplevel_ref_arg', 'manual_slice_size_calculation',
)
RUSTC_LINTS = frozenset({'unused_unsafe', 'unused_parens', 'unused_mut'})

def lint_groups():
    """clippy lint name -> group, read from `clippy-driver -W help`. Empty when unavailable."""
    try:
        out = subprocess.run(['clippy-driver', '-W', 'help'], capture_output=True, text=True, timeout=120).stdout
    except (OSError, subprocess.TimeoutExpired):
        return {}
    groups = {}
    for line in out.splitlines():
        parts = line.split(None, 1)
        if len(parts) != 2 or not parts[0].startswith('clippy::'):
            continue
        group = parts[0][len('clippy::'):]
        if group not in DARPA_GROUPS and group not in ('pedantic', 'restriction', 'nursery', 'cargo'):
            continue
        members = parts[1]
        if not members.startswith('clippy::'):
            continue
        for m in members.split(','):
            m = m.strip()
            if m.startswith('clippy::'):
                groups[m[len('clippy::'):].replace('-', '_')] = group
    return groups

def measure(crate_dir, work_dir):
    """Returns (lints, other_errors). lints: list of dict(file, line, name, group, message).
    group is a clippy group, 'extra' or 'rustc'."""
    groups = lint_groups()
    args = ['cargo', 'clippy', '--message-format=json', '--lib', '--bins', '--']
    for g in DARPA_GROUPS:
        args += ['--force-warn', f'clippy::{g}']
    for lint in EXTRA_LINTS:
        args += ['--force-warn', f'clippy::{lint}']
    args += ['--force-warn', 'unused_unsafe']
    _, lib, bins = cargo.metadata(crate_dir)
    if lib is None:
        args.remove('--lib')
    if not bins:
        args.remove('--bins')
    code, messages, _, stderr = cargo.run_json(args, crate_dir, os.path.join(work_dir, 'target-clippy'))
    lints = []
    other = []
    for m in messages:
        level = m.get('level')
        mcode = (m.get('code') or {}).get('code', '')
        rel, line, _ = cargo.primary_location(m, crate_dir)
        if level == 'warning' and mcode:
            if mcode.startswith('clippy::'):
                name = mcode[len('clippy::'):]
                group = 'extra' if name in EXTRA_LINTS else groups.get(name, 'clippy')
            else:
                name, group = mcode, 'rustc'
            lints.append(dict(file=rel or '?', line=line, name=name, group=group,
                              message=m.get('message', '')))
        elif level == 'error':
            if m.get('message', '').startswith('aborting due to'):
                continue
            other.append(f"{rel}:{line}: {m.get('message', '')}")
    if code != 0 and not other:
        other.append((stderr.strip().splitlines() or ['cargo clippy failed'])[-1])
    return lints, other

def _git(crate_dir, *args):
    return subprocess.run(['git', *args], cwd=crate_dir, capture_output=True, text=True)

def autofix(crate_dir, work_dir, lints=SAFE_AUTOFIX, commit=False, log=print):
    """Applies `cargo clippy --fix` one lint at a time. After each lint the crate must still build
    (`cargo build --release`); a lint whose fix does not build is reverted. With commit, each
    applied lint becomes one git commit, so that a lint that breaks a test can be reverted alone."""
    target = os.path.join(work_dir, 'target-fix')
    _, lib, bins = cargo.metadata(crate_dir)
    sel = (['--lib'] if lib else []) + (['--bins'] if bins else [])
    applied = []
    if commit and _git(crate_dir, 'status', '--porcelain', '--untracked-files=no').stdout.strip():
        raise cargo.CargoError('autofix --commit needs a clean git status (commit or stash your edits first)')
    for lint in lints:
        flag = lint if lint in RUSTC_LINTS else f'clippy::{lint}'
        args = ['cargo', 'clippy', '--fix', '--allow-dirty', '--allow-staged', '--allow-no-vcs', *sel,
                '--', '-A', 'clippy::all', '--force-warn', flag]
        proc = subprocess.run(args, cwd=crate_dir, capture_output=True, text=True, env=cargo.env(target), timeout=1800)
        changed = _git(crate_dir, 'diff', '--name-only').stdout.split()
        if not changed:
            continue
        build = subprocess.run(['cargo', 'build', '--release', *sel], cwd=crate_dir, capture_output=True, text=True,
                               env=cargo.env(os.path.join(crate_dir, 'target')), timeout=1800)
        if proc.returncode != 0 or build.returncode != 0:
            _git(crate_dir, 'checkout', '--', *changed)
            log(f'  {lint}: reverted (the fixed crate did not build)')
            continue
        stat = _git(crate_dir, 'diff', '--shortstat').stdout.strip()
        if commit:
            _git(crate_dir, 'commit', '-q', '-am', f'quality autofix: {flag}')
        applied.append(lint)
        log(f'  {lint}: applied ({stat})')
    return applied

ALLOW_RE = re.compile(r'^(clippy::)?[a-z_]+$')
