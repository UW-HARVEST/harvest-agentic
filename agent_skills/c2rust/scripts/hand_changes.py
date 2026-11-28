#!/usr/bin/env python3
"""hand_changes: what the working directory has that c2rust_translate does not give.

    hand_changes.py [--json] [--c2rust PATH] [--original DIR]

Takes the options of the last call in c2rust_record/calls.jsonl that wrote a package, translates
again with these options and the C source that is on disk now, and compares the result, byte for
byte, with every file of the working directory. So this is true, whatever the state of the git
repository: the working directory is the output of the tool for these options and this C source,
plus the changes that the report lists. When there is no such call, or the new translation gives
no package, the comparison is not made, and the report lists every file.

Prints the calls of the record, the changes of the C source against the original, each changed
package file with its added and removed lines, and each other file that the tool does not give.
With --json it prints one JSON document in one line: the report, the full differences, and the
output of c2rust in the new translation. It then reads one line from standard input and gives
it back as `token`, so that its reader knows the document from other text in the stream. The
new translation replaces the files of the tool in scratch/c2rust/.

Run it in the working directory. Exit status 0: the comparison is made. 1: it is not.
"""
import argparse
import json
import shlex
import sys
from pathlib import Path

sys.dont_write_bytecode = True  # no __pycache__ in the skill directory (it is embedded and copied to agents)
sys.path.insert(0, str(Path(__file__).resolve().parent))

from c2rust_translate_lib import ToolError, compare, rust_toolchain, transpile  # noqa: E402
from c2rust_translate_lib.translate import translate  # noqa: E402
from c2rust_translate_lib.workspace import C_EDITS_FILE, C_SRC, CALLS_FILE, read_calls, result_files  # noqa: E402

# The message classes of c2rust whose lines the report gives: each names code that c2rust left out.
LEFT_OUT = ('functions_not_translated', 'declarations_skipped')


def report(c2rust, original):
    """The report on the working directory, with the full differences and the c2rust output."""
    calls, package_call, translation, version, problem = [], None, None, None, None
    try:
        calls = read_calls()
        package_call = next((call for call in reversed(calls) if call.get('package')), None)
        if package_call is None:
            raise ToolError(f'no call in {CALLS_FILE} wrote a package')
        c2rust = c2rust or transpile.find()
        version = transpile.version(c2rust)
        translation = translate(package_call['options'], c2rust, rust_toolchain(), original)
        if translation.package is None:
            raise ToolError(f'c2rust wrote no package (exit status {translation.exit_status})')
    except ToolError as error:
        problem = str(error)
    except Exception as error:  # the comparison must not depend on what the record or the C source holds
        problem = f'the new translation stopped: {type(error).__name__}: {error}'

    expected, entries = {}, set()
    if not problem:
        expected = compare.files(translation.package)
        entries = {path.split('/')[0] for path in expected}
        if translation.c_edits_file:
            expected[C_EDITS_FILE.as_posix()] = translation.c_edits_file
    changes, difference = compare.difference(expected, result_files())
    package = [change for change in changes if change[0].split('/')[0] in entries]
    edits = translation.c_edits_file if translation else None
    return {
        # As the tool recorded them. The agent can write this file.
        'calls': calls,
        'comparison': {'made': not problem, 'call': None if problem else package_call.get('call'),
                       'not_made_because': problem},
        # What the new translation confirms: with these options it gave the package.
        'compared_options': None if problem else package_call['options'],
        'c2rust': {'path': c2rust, 'version': version},
        'messages': translation and {name: len(lines) for name, lines in translation.messages.items()},
        'left_out': translation and {name: list(dict.fromkeys(translation.messages[name])) for name in LEFT_OUT},
        'c_edits': None if problem else translation.c_edits,
        'package_changes': None if problem else compare.totals(package),
        # Without a comparison: every file of the working directory.
        'other_changes': compare.totals([change for change in changes if change not in package]),
        'hand_changes_diff': difference,
        'c_edits_diff': edits.read_text(errors='surrogateescape') if edits and not problem else None,
        'transpile_log': translation and translation.log,
    }


def print_report(result):
    for call in result['calls']:
        print(f'Call {call.get("call")} (exit status {call.get("exit_status")}, '
              f'{"package" if call.get("package") else "no package"}): '
              f'{shlex.join(call.get("command", [])[1:]) or "no argument"}')
    comparison = result['comparison']
    if comparison['made']:
        print(f'Compared with a new translation with the options of call {comparison["call"]} '
              f'({result["c2rust"]["version"]}).')
    else:
        print(f'NO COMPARISON: {comparison["not_made_because"]}. The list has every file of the working directory.')
    for title, key in (('Changes of the C source', 'c_edits'), ('Changed package files', 'package_changes'),
                       ('Other files that the tool does not give', 'other_changes')):
        changes = result[key]
        if changes is not None:
            print(f'{title}: {changes["files"]} file(s), +{changes["added"]} -{changes["removed"]} lines')
            for path, added, removed in changes['list']:
                print(f'  {path}: ' + ('not compared by lines' if added is None else f'+{added} -{removed}'))


def main():
    parser = argparse.ArgumentParser(prog='hand_changes.py', description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--json', action='store_true',
                        help='print the report as one JSON document, with the line of standard input as `token`')
    parser.add_argument('--c2rust', metavar='PATH', help='the c2rust program (default: `c2rust` on PATH)')
    parser.add_argument('--original', metavar='DIR', default=str(C_SRC),
                        help=f'the C source of the run, to which a copy is compared (default: {C_SRC})')
    args = parser.parse_args()
    token = sys.stdin.readline().strip() if args.json else None
    result = report(args.c2rust, args.original)
    if args.json:
        print(json.dumps({'token': token} | result))
    else:
        print_report(result)
    return 0 if result['comparison']['made'] else 1


if __name__ == '__main__':
    sys.exit(main())
