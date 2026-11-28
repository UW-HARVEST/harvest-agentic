#!/usr/bin/env python3
"""End-to-end test of c2rust_translate and hand_changes on fixture/toy/: a library with a variadic
function, a `FILE *` parameter, a string function under _GNU_SOURCE and a use of libm, and an
executable. Needs c2rust, CMake, a C compiler, and Cargo with the toolchain of the repository.
Skipped when c2rust or CMake is not installed. For maintainers: python3 tests/test_end_to_end.py
"""
import json
import shutil
import subprocess
import sys
import tempfile
import tomllib
import unittest
from pathlib import Path

sys.dont_write_bytecode = True  # no __pycache__ in the skill directory (it is embedded and copied to agents)
HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import run_dir  # noqa: E402

TOOLCHAIN = tomllib.loads((HERE.parents[2] / 'rust-toolchain.toml').read_text())['toolchain']['channel']


@unittest.skipUnless(shutil.which('c2rust') and shutil.which('cmake'), 'c2rust or cmake is not installed')
class EndToEnd(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.dir = tempfile.TemporaryDirectory()
        cls.addClassCleanup(cls.dir.cleanup)
        cls.workspace = run_dir.make(Path(cls.dir.name) / 'run', HERE / 'fixture' / 'toy')

    def script(self, name, *args, stdin=None):
        # CFLAGS would break the C build: the tool does not pass it on.
        return run_dir.run_script(self.workspace, name, *args, stdin=stdin, RUSTUP_TOOLCHAIN=TOOLCHAIN,
                                  CFLAGS='-DTOY_BIAS=bad')

    def tool(self, *args):
        return self.script('c2rust_translate.py', *args)

    def calls(self):
        return [json.loads(line) for line in (self.workspace / 'c2rust_record/calls.jsonl').read_text().splitlines()]

    def report(self):
        done = self.script('hand_changes.py', '--json', stdin='a token\n')
        document, = done.stdout.splitlines()  # no output of CMake or c2rust is in the stream
        report = json.loads(document)
        self.assertEqual(report['token'], 'a token')
        return done.returncode, report, report['hand_changes_diff']

    def build_and_run_driver(self, *args):
        build = subprocess.run(['cargo', f'+{TOOLCHAIN}', 'build', '--release'], cwd=self.workspace, capture_output=True,
                               text=True)
        self.assertEqual(build.returncode, 0, build.stderr)
        return subprocess.run([self.workspace / 'target/release/driver', *args], capture_output=True, text=True).stdout

    def test_translate_build_change_and_compare(self):
        # 1. A call with no argument gives a package that builds and behaves as the C program. A
        #    new translation gives the same files.
        done = self.tool()
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
        self.assertIn('executable `driver` from the unit main', done.stdout)
        call = self.calls()[0]
        self.assertEqual((call['crate'], call['binaries'], call['link_libraries']), ('toy_c2rust', ['main'], ['m']))
        self.assertEqual(call['messages']['clang_errors'], 0)
        self.assertTrue(call['rustfmt_version'].startswith('rustfmt '))
        self.assertFalse((self.workspace / 'rust-toolchain.toml').exists())
        self.assertEqual(self.build_and_run_driver('81:'), 'sum=33\nroot=9\n')
        symbols = subprocess.run(['nm', '-D', '--defined-only', self.workspace / 'target/release/libtoy_c2rust.so'],
                                 capture_output=True, text=True).stdout
        for name in ('toy_sum', 'toy_root', 'toy_print'):
            self.assertIn(f' T {name}\n', symbols)
        status, report, _ = self.report()
        self.assertEqual((status, report['comparison']['call'], report['package_changes']['files']), (0, 1, 0))
        self.assertEqual(report['compared_options'], call['options'])
        self.assertEqual(report['messages']['functions_not_translated'], 0)
        self.assertEqual([change[0] for change in report['other_changes']['list']],
                         ['.gitignore', 'Cargo.lock', 'c2rust_record/calls.jsonl'])

        # 2. Without the libc macro, c2rust stops at the string function. The call is recorded, and
        #    the package stays.
        done = self.tool('--no-libc-define')
        self.assertEqual(done.returncode, 1, done.stdout + done.stderr)
        call = self.calls()[1]
        self.assertNotEqual(call['exit_status'], 0)
        self.assertIsNone(call['package'])
        self.assertGreater(call['messages']['generic_selections_unsupported'], 0)
        self.assertGreater(call['messages']['panics'], 0)
        self.assertTrue((self.workspace / 'src/toy.rs').is_file())

        # 3. An edited copy of the C source and a clang argument are in the record and in the package.
        copy = self.workspace / 'scratch/c'
        shutil.copytree(self.workspace / 'inputs/c_src', copy)
        source = copy / 'src/toy.c'
        source.write_text(source.read_text().replace('int sum = TOY_BIAS;', 'int sum = TOY_BIAS + TOY_MORE;'))
        (copy / 'CMakeLists.txt').write_text((copy / 'CMakeLists.txt').read_text().replace('TOY_BIAS=1', 'TOY_BIAS=1 TOY_MORE=2'))
        done = self.tool('--c-src', 'scratch/c', '--', '-UTOY_MORE', '-DTOY_MORE=5')
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
        call = self.calls()[2]
        self.assertEqual((call['c_edits']['files'], call['c_edits']['added'], call['c_edits']['removed']), (2, 2, 2))
        self.assertEqual(call['c2rust_command'][-4:], ['--', '-D_LIBC', '-UTOY_MORE', '-DTOY_MORE=5'])
        self.assertEqual(self.build_and_run_driver(), 'sum=37\nroot=12\n')
        status, report, _ = self.report()
        self.assertEqual((status, report['comparison']['call'], report['package_changes']['files']), (0, 3, 0))
        self.assertEqual(report['c_edits']['list'], [['CMakeLists.txt', 1, 1], ['src/toy.c', 1, 1]])

        # 4. A change of the package and a later change of the C copy both show in the comparison
        #    with a new translation.
        rust = self.workspace / 'src/toy.rs'
        rust.write_text(rust.read_text() + '// by hand\n')
        status, report, difference = self.report()
        self.assertEqual(report['package_changes']['list'], [['src/toy.rs', 1, 0]])
        self.assertIn('+// by hand\n', difference)
        source.write_text(source.read_text().replace('return -1;', 'return -2;'))
        status, report, _ = self.report()
        self.assertEqual((status, report['package_changes']['list']), (0, [['src/toy.rs', 2, 1]]))
        self.assertIn('c2rust_record/c_edits.patch', [change[0] for change in report['other_changes']['list']])

        # 5. An option that the tool refuses stops the call before c2rust runs.
        for args in (('--', '-include', str(copy / 'src/toy.c')), ('--transpile-arg=--',), ('--', '-DH="/x.h"')):
            done = self.tool(*args)
            self.assertEqual(done.returncode, 2, args)
        self.assertIn('clang argument "-include"', self.tool('--', '-include', '/x.h').stderr)
        self.assertEqual(len(self.calls()), 3)

        # 6. The library alone.
        done = self.tool('--library-only')
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
        self.assertNotIn('[[bin]]', (self.workspace / 'Cargo.toml').read_text())
        self.assertTrue((self.workspace / 'src/main.rs').is_file())  # the unit is a module of the library


if __name__ == '__main__':
    unittest.main()
