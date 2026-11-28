#!/usr/bin/env python3
"""Tests of c2rust_translate (record, commits, replaced packages) and of hand_changes, with a
translation that the test supplies in place of the C build and c2rust.
For maintainers: python3 tests/test_record.py
"""
import contextlib
import io
import json
import os
import shutil
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

sys.dont_write_bytecode = True  # no __pycache__ in the skill directory (it is embedded and copied to agents)
HERE = Path(__file__).resolve().parent
sys.path[:0] = [str(HERE), str(HERE.parent / 'scripts')]
import c2rust_translate  # noqa: E402
import hand_changes  # noqa: E402
import run_dir  # noqa: E402
from c2rust_translate_lib import translate as real  # noqa: E402

PACKAGE = {'Cargo.toml': '[package]\nname = "toy_c2rust"\n', 'lib.rs': 'pub mod src;\n',
           'src/a.rs': 'fn a() {\n    one();\n    two();\n}\n', 'src/build/b.rs': 'fn b() {}\n'}
LOG = 'Transpiling a.c\nerror: Failed to translate f: Unimplemented builtin g\n'
C_FILE = 'int a(void) {\n    return 1;\n}\n'
# What a working directory has after an honest call, and the tool does not give.
ALWAYS = [('.gitignore', 3, 0), ('c2rust_record/calls.jsonl', 1, 0)]


class Workspace(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        c_src = Path(self.dir.name) / 'c'
        c_src.mkdir()
        (c_src / 'a.c').write_text(C_FILE)
        self.workspace = run_dir.make(Path(self.dir.name) / 'run', c_src)
        self.addCleanup(os.chdir, os.getcwd())
        self.addCleanup(self.dir.cleanup)
        os.chdir(self.workspace)
        Path('scratch').mkdir()  # a real translation makes it

    def translation(self, files, status=0):
        """In place of `translate`: gives the package `files` (None: c2rust wrote no package). The
        check of the options and the changes of the C source are the real ones."""
        def translate(options, c2rust, toolchain, original=real.C_SRC):
            original = Path(original).resolve()
            real.check_options(options, original)
            work = Path(tempfile.mkdtemp(dir=self.dir.name))
            package = None
            if files is not None:
                package = work / 'toy_c2rust'
                for name, text in files.items():
                    (package / name).parent.mkdir(parents=True, exist_ok=True)
                    (package / name).write_text(text)
            edits, edits_file = real.c_edits(Path(options['c_src']).resolve(), original, work / 'c_edits.patch')
            messages = real.transpile.classify(LOG, options['libc_define'])
            return real.Translation(['cmake'], [c2rust, 'transpile'], status, LOG, 'toy_c2rust', [], ['m'], messages,
                                    package, {} if package else None, edits, edits_file)
        return translate

    def patched(self, module, files, status=0):
        stack = contextlib.ExitStack()
        stack.enter_context(mock.patch.object(module, 'translate', self.translation(files, status)))
        stack.enter_context(mock.patch.multiple(module.transpile, find=lambda: '/bin/c2rust',
                                                version=lambda c2rust: 'C2Rust test',
                                                rustfmt_version=lambda toolchain: 'rustfmt test'))
        return stack

    def call(self, files, *argv, status=0):
        """One tool call whose translation gives the package `files`."""
        with self.patched(c2rust_translate, files, status), \
                mock.patch.object(sys, 'argv', ['c2rust_translate.py', *argv]), \
                contextlib.redirect_stdout(io.StringIO()) as output:
            status = c2rust_translate.main()
        return status, output.getvalue()

    def report(self, files=PACKAGE, original='inputs/c_src'):
        """The report, and its full difference, when a new translation gives the package `files`."""
        with self.patched(hand_changes, files):
            result = hand_changes.report(None, original)
        return result, result['hand_changes_diff']

    def calls(self):
        return [json.loads(line) for line in Path('c2rust_record/calls.jsonl').read_text().splitlines()]

    def git(self, *args):
        return run_dir.git(self.workspace, *args)

    def copy_of_c_source(self, text):
        copy = Path('scratch/c')
        copy.mkdir(exist_ok=True)
        (copy / 'a.c').write_text(text)
        return str(copy)


class Record(Workspace):
    def test_a_call_commits_the_package_and_the_record_and_nothing_else(self):
        Path('notes.md').write_text('mine\n')
        hook = Path('.git/hooks/pre-commit')
        hook.parent.mkdir(exist_ok=True)
        hook.write_text('#!/bin/sh\nexit 1\n')  # a hook of the repository does not run
        hook.chmod(0o755)
        status, output = self.call(PACKAGE, '--transpile-arg=--fail-on-error', '--', '-DA=1')
        self.assertEqual(status, 0)
        self.assertIn('functions_not_translated: 1', output)
        entry, = self.calls()
        self.assertEqual((entry['call'], entry['exit_status'], entry['package']), (1, 0, ['Cargo.toml', 'lib.rs', 'src']))
        self.assertEqual((entry['c2rust_path'], entry['c2rust_version'], entry['rustfmt_version']),
                         ('/bin/c2rust', 'C2Rust test', 'rustfmt test'))
        self.assertEqual(entry['command'], ['c2rust_translate.py', '--transpile-arg=--fail-on-error', '--', '-DA=1'])
        self.assertEqual(entry['options'], {'c_src': 'inputs/c_src', 'binary': [], 'library_only': False,
                                            'libc_define': True, 'transpile_args': ['--fail-on-error'],
                                            'clang_args': ['-DA=1']})
        self.assertEqual(entry['messages']['functions_not_translated'], 1)
        self.assertEqual(self.git('log', '-1', '--format=%an|%s').strip(), 'c2rust_translate|c2rust_translate: call 1')
        self.assertIn('command: c2rust_translate.py --transpile-arg=--fail-on-error -- -DA=1',
                      self.git('log', '-1', '--format=%b'))
        committed = self.git('show', '--name-only', '--format=', 'HEAD').split()
        # src/build/b.rs is in the commit although the ignore file of the run has `build*/`
        self.assertEqual(sorted(committed), ['Cargo.toml', 'c2rust_record/calls.jsonl', 'lib.rs', 'src/a.rs',
                                             'src/build/b.rs'])

    def test_a_call_without_a_package_is_recorded_and_keeps_the_package(self):
        self.call(PACKAGE)
        status, output = self.call(None, status=101)
        self.assertEqual(status, 1)
        self.assertIn('c2rust wrote no package', output)
        self.assertEqual([(c['exit_status'], c['package']) for c in self.calls()],
                         [(0, ['Cargo.toml', 'lib.rs', 'src']), (101, None)])
        self.assertEqual(self.git('log', '-1', '--format=%s').strip(), 'c2rust_translate: call 2')
        self.assertEqual(Path('src/a.rs').read_text(), PACKAGE['src/a.rs'])

    def test_a_refused_option_stops_the_call_before_the_record(self):
        with self.assertRaises(c2rust_translate.ToolError):
            self.call(PACKAGE, '--', '-include', '/outside.h')
        self.assertFalse(Path('c2rust_record').exists())

    def test_a_new_call_replaces_the_package_and_saves_the_changes_made_by_hand(self):
        self.call(PACKAGE)
        Path('src/a.rs').write_text(PACKAGE['src/a.rs'].replace('two', 'three'))
        Path('src/mine.rs').write_text('fn mine() {}\n')
        status, output = self.call({'Cargo.toml': PACKAGE['Cargo.toml'], 'src/c.rs': 'fn c() {}\n'})
        self.assertEqual(status, 0)
        self.assertEqual(sorted(p.name for p in Path('src').iterdir()), ['c.rs'])
        self.assertFalse(Path('lib.rs').exists())
        self.assertIn('lib.rs', self.git('show', '--name-only', '--diff-filter=D', '--format=', 'HEAD'))
        patch = Path('scratch/hand_changes_before_call_2.patch')
        self.assertIn(str(patch), output)
        self.assertIn('+    three();', patch.read_text())
        self.assertIn('+fn mine() {}', patch.read_text())

    def test_a_missing_commit_of_the_last_package_is_told(self):
        self.call(PACKAGE)
        self.git('reset', '-q', '--soft', 'HEAD~1')
        self.git('commit', '-q', '-m', 'the same files, committed by hand')
        Path('src/a.rs').write_text('fn a() {}\n')
        status, output = self.call(PACKAGE)
        self.assertEqual(status, 0)
        self.assertIn('no commit of call 1. The changes that the package files got after that call are not saved',
                      output)

    def test_the_changes_of_the_c_source_are_in_the_record_without_host_paths(self):
        self.call(PACKAGE, '--c-src', self.copy_of_c_source(C_FILE.replace('1', '2')))
        self.assertEqual(self.calls()[0]['c_edits'], {'files': 1, 'added': 1, 'removed': 1,
                                                      'files_not_compared_by_lines': 0, 'list': [['a.c', 1, 1]]})
        self.assertEqual(Path('c2rust_record/c_edits.patch').read_text(),
                         '--- a/a.c\n+++ b/a.c\n@@ -1,3 +1,3 @@\n int a(void) {\n-    return 1;\n+    return 2;\n }\n')
        self.call(PACKAGE)
        self.assertIsNone(self.calls()[1]['c_edits'])
        self.assertFalse(Path('c2rust_record/c_edits.patch').exists())


class HandChanges(Workspace):
    def test_an_untouched_package_has_no_change(self):
        self.call(PACKAGE)
        result, difference = self.report()
        self.assertEqual(result['comparison'], {'made': True, 'call': 1, 'not_made_because': None})
        self.assertEqual((result['c_edits'], result['c_edits_diff']), (None, None))
        self.assertEqual(result['package_changes']['list'], [])
        self.assertEqual(result['other_changes']['list'], ALWAYS)
        self.assertEqual(len(result['calls']), 1)
        self.assertNotIn('src/', difference)
        # What the new translation confirms: the options, and what c2rust left out.
        self.assertEqual(result['compared_options'], self.calls()[0]['options'])
        self.assertEqual(result['messages']['functions_not_translated'], 1)
        self.assertEqual(result['left_out'], {'functions_not_translated': [LOG.splitlines()[1]],
                                              'declarations_skipped': []})
        self.assertEqual(result['transpile_log'], LOG)

    def test_every_file_that_the_tool_does_not_give_is_listed_with_its_difference(self):
        self.call(PACKAGE)
        Path('src/a.rs').write_text(PACKAGE['src/a.rs'].replace('    two();\n', '    three();\n    four();\n'))
        Path('src/mine.rs').write_text('fn mine() {}\n')
        Path('lib.rs').write_text(PACKAGE['lib.rs'] + '#[path = "c2rust_record/extra.rs"] mod extra;\n')
        Path('c2rust_record/extra.rs').write_text('pub fn by_hand() {\n' + '    x();\n' * 200 + '}\n')
        Path('REPAIRS.md').write_text('one\ntwo\n')
        for removed_at_the_end in ('.cache/x', 'target/x', 'scratch/y', 'src/.hidden'):
            Path(removed_at_the_end).parent.mkdir(exist_ok=True)
            Path(removed_at_the_end).write_text('x\n')
        Path('build_x').mkdir()
        Path('build_x/ignored_by_git.rs').write_text('x\n')
        self.git('add', 'src/a.rs')
        self.git('commit', '-q', '-m', 'a change of the agent')
        result, difference = self.report()
        self.assertEqual(result['package_changes'],
                         {'files': 3, 'added': 4, 'removed': 1, 'files_not_compared_by_lines': 0,
                          'list': [('lib.rs', 1, 0), ('src/a.rs', 2, 1), ('src/mine.rs', 1, 0)]})
        self.assertEqual(result['other_changes']['list'],
                         sorted(ALWAYS + [('REPAIRS.md', 2, 0), ('build_x/ignored_by_git.rs', 1, 0),
                                          ('c2rust_record/extra.rs', 202, 0)]))
        self.assertIn('--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1,4 +1,5 @@\n fn a() {\n     one();\n-    two();\n'
                      '+    three();\n+    four();\n }\n', difference)
        self.assertIn('+pub fn by_hand() {\n', difference)

    def test_a_filter_of_the_repository_does_not_hide_a_change(self):
        self.call(PACKAGE)
        self.git('config', 'filter.x.clean', 'git show HEAD:%f')
        Path('.git/info').mkdir(exist_ok=True)
        Path('.git/info/attributes').write_text('*.rs filter=x\n')
        Path('src/a.rs').write_text('fn a() { by_hand(); }\n')
        self.assertEqual(self.report()[0]['package_changes']['list'], [('src/a.rs', 1, 4)])

    def test_a_forged_tool_commit_does_not_hide_a_change(self):
        self.call(PACKAGE)
        Path('src/a.rs').write_text('fn a() { by_hand(); }\n')
        self.git('commit', '-q', '--amend', '--no-edit', 'src/a.rs')
        self.assertEqual(self.git('status', '--short', '--untracked-files=no'), '')
        self.assertEqual(self.report()[0]['package_changes']['list'], [('src/a.rs', 1, 4)])
        shutil.rmtree('.git')  # the report needs no repository
        self.assertEqual(self.report()[0]['package_changes']['list'], [('src/a.rs', 1, 4)])

    def test_the_changes_of_the_c_source_are_the_ones_on_disk(self):
        copy = self.copy_of_c_source(C_FILE.replace('1', '2'))
        self.call(PACKAGE, '--c-src', copy)
        result, _ = self.report()
        self.assertEqual(result['c_edits']['list'], [('a.c', 1, 1)])
        self.assertEqual(result['other_changes']['list'], ALWAYS)
        # The copy changes after the call, and the patch in the record is edited: both show.
        self.copy_of_c_source(C_FILE.replace('1', '2') + 'int more;\n')
        result, difference = self.report()
        self.assertEqual(result['c_edits']['list'], [('a.c', 2, 1)])
        self.assertIn('+int more;\n', result['c_edits_diff'])
        self.assertIn(('c2rust_record/c_edits.patch', 1, 2), result['other_changes']['list'])
        self.assertIn('-+int more;\n', difference)

    def test_a_redirected_link_to_the_inputs_does_not_hide_a_c_edit(self):
        original = (Path('inputs') / 'c_src').resolve()
        fake = Path('scratch/fake/c_src')
        fake.mkdir(parents=True)
        (fake / 'a.c').write_text(C_FILE.replace('1', '2'))
        Path('inputs').unlink()
        Path('inputs').symlink_to('scratch/fake')
        self.call(PACKAGE)
        self.assertIsNone(self.calls()[0]['c_edits'])  # the tool followed the link of the agent
        result, _ = self.report(original=str(original))
        self.assertEqual(result['c_edits']['list'], [('a.c', 1, 1)])
        self.assertIn(('c2rust_record/c_edits.patch', 0, 7), result['other_changes']['list'])

    def test_without_a_package_of_a_new_translation_there_is_no_comparison(self):
        everything = sorted(ALWAYS + [('Cargo.toml', 2, 0), ('lib.rs', 1, 0), ('src/a.rs', 4, 0),
                                      ('src/build/b.rs', 1, 0)])
        for name, text in PACKAGE.items():  # a package that no call wrote
            Path(name).parent.mkdir(parents=True, exist_ok=True)
            Path(name).write_text(text)
        result, _ = self.report()
        self.assertEqual((result['comparison']['made'], result['calls']), (False, []))
        self.assertIn('no call in c2rust_record/calls.jsonl wrote a package', result['comparison']['not_made_because'])
        # No count of hand changes without a comparison: the list has every file.
        self.assertEqual((result['package_changes'], result['c_edits'], result['compared_options']), (None, None, None))
        self.assertEqual(result['other_changes']['list'], [change for change in everything if change != ALWAYS[1]])

        self.call(PACKAGE)
        result, _ = self.report(files=None)  # c2rust writes no package now
        self.assertIn('c2rust wrote no package', result['comparison']['not_made_because'])
        self.assertEqual((result['package_changes'], result['other_changes']['list']), (None, everything))
        self.assertEqual(result['transpile_log'], LOG)

        calls = self.calls()  # the record gets an option that the tool refuses
        calls[0]['options']['clang_args'] = ['-include', '/outside.h']
        Path('c2rust_record/calls.jsonl').write_text(json.dumps(calls[0]) + '\n')
        result, _ = self.report()
        self.assertIn('clang argument "-include"', result['comparison']['not_made_because'])
        self.assertEqual(result['other_changes']['list'], everything)

        Path('c2rust_record/calls.jsonl').write_text('not a record\n')
        self.assertIn('is not a record of the tool', self.report()[0]['comparison']['not_made_because'])

    def test_a_file_that_cannot_be_read_is_a_change_and_does_not_stop_the_report(self):
        self.call(PACKAGE)
        Path('src/a.rs').chmod(0)
        self.addCleanup(Path('src/a.rs').chmod, 0o644)
        os.mkfifo('src/pipe')
        result, difference = self.report()
        self.assertEqual(result['package_changes']['list'], [('src/a.rs', None, None), ('src/pipe', None, None)])
        self.assertIn('b/src/a.rs: cannot be read', difference)

    def test_the_json_document_has_the_token_of_its_reader_and_the_full_difference(self):
        self.call(PACKAGE)
        Path('src/a.rs').write_text('fn a() {}\n')
        with self.patched(hand_changes, PACKAGE), mock.patch.object(sys, 'argv', ['hand_changes.py', '--json']), \
                mock.patch.object(sys, 'stdin', io.StringIO('token-of-the-reader\n')), \
                contextlib.redirect_stdout(io.StringIO()) as output:
            status = hand_changes.main()
        self.assertEqual(status, 0)
        document, = output.getvalue().splitlines()
        document = json.loads(document)
        self.assertEqual(document['token'], 'token-of-the-reader')
        self.assertEqual(document['package_changes']['list'], [['src/a.rs', 1, 4]])
        self.assertIn('+fn a() {}\n', document['hand_changes_diff'])

    def test_the_text_report(self):
        self.call(PACKAGE)
        Path('src/a.rs').write_text('fn a() {}\n')
        with self.patched(hand_changes, PACKAGE), mock.patch.object(sys, 'argv', ['hand_changes.py']), \
                contextlib.redirect_stdout(io.StringIO()) as output:
            status = hand_changes.main()
        self.assertEqual(status, 0)
        self.assertIn('Changed package files: 1 file(s), +1 -4 lines\n  src/a.rs: +1 -4\n', output.getvalue())


if __name__ == '__main__':
    unittest.main()
