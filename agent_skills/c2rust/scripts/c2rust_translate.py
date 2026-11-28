#!/usr/bin/env python3
"""c2rust_translate: translate the C project of the run to a Rust package with c2rust.

One call builds the C project with CMake, runs `c2rust transpile` on all of its translation units,
makes the output a package that stable Rust builds, puts the package in the working directory,
and commits it. Each call is recorded in c2rust_record/calls.jsonl. See ../SKILL.md.
"""
import argparse
import json
import shlex
import shutil
import sys
import time
from pathlib import Path

sys.dont_write_bytecode = True  # no __pycache__ in the skill directory (it is embedded and copied to agents)
sys.path.insert(0, str(Path(__file__).resolve().parent))

from c2rust_translate_lib import ToolError, rust_toolchain, transpile  # noqa: E402
from c2rust_translate_lib.translate import translate  # noqa: E402
from c2rust_translate_lib.workspace import (  # noqa: E402
    C_EDITS_FILE, C_SRC, CALLS_FILE, RECORD_DIR, WORK_DIR, changes_since, commit_call, commit_of_call, read_calls,
    require_workspace)

# The message lines of one class that the summary prints.
SUMMARY_LINES = 8


def parse_args():
    parser = argparse.ArgumentParser(prog='c2rust_translate.py', description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--c-src', default=str(C_SRC), metavar='DIR',
                        help=f'the C source to translate: {C_SRC} (default) or an edited copy of it')
    executable = parser.add_mutually_exclusive_group()
    executable.add_argument('--binary', action='append', default=[], metavar='UNIT',
                            help='the translation unit with the `main` of the executable, as its file name without '
                                 'suffix. Default: the unit of the executable of the C build, when there is one')
    executable.add_argument('--library-only', action='store_true', help='translate no executable')
    parser.add_argument('--no-libc-define', action='store_true',
                        help=f'do not give clang {transpile.LIBC_DEFINE}')
    parser.add_argument('--transpile-arg', action='append', default=[], metavar='ARG',
                        help='an option of `c2rust transpile`, written as --transpile-arg=ARG (repeat for more)')
    parser.add_argument('clang_args', nargs='*', metavar='CLANG_ARG',
                        help='after `--`: more arguments for clang, for example a macro definition')
    return parser.parse_args()


def main():
    args = parse_args()
    require_workspace()
    options = {'c_src': args.c_src, 'binary': args.binary, 'library_only': args.library_only,
               'libc_define': not args.no_libc_define, 'transpile_args': args.transpile_arg,
               'clang_args': args.clang_args}
    calls = read_calls()
    c2rust = transpile.find()
    toolchain = rust_toolchain()
    entry = {'call': len(calls) + 1, 'time': time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime()),
             'command': sys.argv, 'options': options, 'c2rust_path': c2rust,
             'c2rust_version': transpile.version(c2rust), 'rust_toolchain': toolchain,
             'rustfmt_version': transpile.rustfmt_version(toolchain)}

    result = translate(options, c2rust, toolchain)
    messages = result.messages
    entry |= {'cmake_command': result.cmake_command, 'c2rust_command': result.c2rust_command,
              'exit_status': result.exit_status,
              'messages': {name: len(lines) for name, lines in messages.items()},
              'crate': result.crate, 'binaries': result.binaries, 'link_libraries': result.link_libraries,
              'adaptation': result.adaptation, 'c_edits': result.c_edits, 'package': None}

    previous = next((call for call in reversed(calls) if call.get('package')), None)
    replaced = previous['package'] if previous else []
    if result.package:
        if previous:
            save_hand_changes(previous, entry['call'])
        entry['package'] = install(result, replaced)

    RECORD_DIR.mkdir(exist_ok=True)
    with CALLS_FILE.open('a') as file:
        file.write(json.dumps(entry) + '\n')
    message = (f'command: {shlex.join(sys.argv)}\nc2rust: {entry["c2rust_version"]}\n'
               f'c2rust command: {shlex.join(result.c2rust_command)}\nexit status: {result.exit_status}\n')
    try:
        commit = commit_call(entry['call'], {*replaced, *(entry['package'] or ()), str(RECORD_DIR)}, message)[:12]
    except ToolError as error:
        commit = None
        print(f'The call is recorded, but not committed: {error}')
    print_summary(entry, messages, commit)
    return 0 if result.package else 1


def save_hand_changes(previous, call):
    """Before a new package replaces the files of the call `previous`: saves the changes that these
    files got after the commit of that call, as a patch in the scratch directory."""
    base = commit_of_call(previous['call'])
    if base is None:
        print(f'The history of HEAD has no commit of call {previous["call"]}. The changes that the package files got '
              'after that call are not saved.')
        return
    patch = changes_since(base, previous['package'])
    if patch:
        file = WORK_DIR.parent / f'hand_changes_before_call_{call}.patch'
        file.write_text(patch, errors='surrogateescape')
        print(f'The new package replaces files that were changed after call {previous["call"]}. '
              f'Their changes are in {file}.')


def install(result, replaced):
    """Puts the package of `result` and the patch of its C edits in the working directory, in place
    of the package entries `replaced` of an earlier call. Returns the names of the new entries."""
    entries = sorted(entry.name for entry in result.package.iterdir())
    for name in {*replaced, *entries}:
        path = Path(name)
        if path.is_dir() and not path.is_symlink():
            shutil.rmtree(path)
        else:
            path.unlink(missing_ok=True)
    for name in entries:
        source = result.package / name
        if source.is_dir():
            shutil.copytree(source, name)
        else:
            shutil.copy2(source, name)
    RECORD_DIR.mkdir(exist_ok=True)
    if result.c_edits_file:
        shutil.copy2(result.c_edits_file, C_EDITS_FILE)
    else:
        C_EDITS_FILE.unlink(missing_ok=True)
    return entries


def print_summary(entry, messages, commit):
    print(f'{entry["c2rust_version"]}, exit status {entry["exit_status"]}. Full output: {WORK_DIR / "transpile.log"}')
    for name, lines in messages.items():
        if lines:
            print(f'{name}: {len(lines)}')
            distinct = list(dict.fromkeys(lines))
            for line in distinct[:SUMMARY_LINES]:
                print(f'  {line[:240]}')
            if len(distinct) > SUMMARY_LINES:
                print(f'  ... {len(distinct) - SUMMARY_LINES} more different lines')
    if entry['package']:
        print(f'Package: {", ".join(entry["package"])} (crate {entry["crate"]}'
              + (f', executable `driver` from the unit {entry["binaries"][0]}' if len(entry['binaries']) == 1 else '')
              + ')')
        if entry['link_libraries']:
            print(f'System libraries of the C link line, in build.rs: {", ".join(entry["link_libraries"])}')
    else:
        print('c2rust wrote no package. The package files of the working directory did not change.')
    print(f'Call {entry["call"]} of {CALLS_FILE}' + (f', commit {commit}' if commit else ''))


if __name__ == '__main__':
    try:
        sys.exit(main())
    except ToolError as error:
        print(f'c2rust_translate: {error}', file=sys.stderr)
        sys.exit(2)
