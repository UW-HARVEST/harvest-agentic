#!/usr/bin/env python3
"""Tests of cargo_features.py. For maintainers: python3 tests/test_cargo_features.py"""
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

sys.dont_write_bytecode = True  # no __pycache__ in the skill directory (it is embedded and copied to agents)
HERE = Path(__file__).resolve().parent
TOOL = HERE.parent / 'scripts' / 'cargo_features.py'
sys.path.insert(0, str(TOOL.parent))
import cargo_features  # noqa: E402


def run(*args, cwd):
    return subprocess.run([sys.executable, str(TOOL), *args], cwd=cwd, capture_output=True, text=True)


class NamingCases(unittest.TestCase):
    def test_the_shared_cases(self):
        cases = json.loads((HERE / 'naming_cases.json').read_text())['cases']
        for case in cases:
            self.assertEqual(cargo_features.feature(case['option'], case['value']), case['feature'], case)


class Commands(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.root = Path(self.dir.name)
        config = self.root / 'inputs' / 'c_src' / 'configuration.json'
        config.parent.mkdir(parents=True)
        config.write_text(json.dumps({'configurable_variables': {'DEBUG': [True, False], 'OP': ['add', 'mul']}}))

    def tearDown(self):
        self.dir.cleanup()

    def manifest(self, features):
        (self.root / 'Cargo.toml').write_text('[package]\nname = "p"\n\n[features]\n'
                                              + ''.join(f'{f} = []\n' for f in features))

    def test_names_reads_the_configuration_by_default(self):
        out = run('names', cwd=self.root)
        self.assertEqual(out.returncode, 0, out.stderr)
        lines = out.stdout.splitlines()
        for line in ('DEBUG = []', 'OP_add = []', 'OP_mul = []'):
            self.assertIn(line, lines)

    def test_check_finds_a_name_in_the_wrong_case(self):
        self.manifest(['default', 'debug', 'OP_add', 'OP_mul', 'std'])
        out = run('check', cwd=self.root)
        self.assertEqual(out.returncode, 1)
        self.assertIn('missing feature DEBUG (Cargo.toml has debug', out.stdout)

    def test_check_accepts_the_names_and_other_features(self):
        self.manifest(['default', 'DEBUG', 'OP_add', 'OP_mul', 'std'])
        out = run('check', 'Cargo.toml', cwd=self.root)
        self.assertEqual(out.returncode, 0, out.stdout)

    def test_options_from_the_command_line(self):
        (self.root / 'inputs' / 'c_src' / 'configuration.json').unlink()
        self.manifest(['HASH_BACKEND_shake256', 'HASH_BACKEND_sha2'])
        out = run('check', '--option', 'HASH_BACKEND=shake, sha2', '--bool', 'FAST', cwd=self.root)
        self.assertEqual(out.returncode, 1)
        self.assertEqual(out.stdout.splitlines()[:2], ['missing feature HASH_BACKEND_shake', 'missing feature FAST'])

    def test_no_source_of_options_asks_for_them(self):
        (self.root / 'inputs' / 'c_src' / 'configuration.json').unlink()
        out = run('names', cwd=self.root)
        self.assertEqual(out.returncode, 0)
        self.assertIn('no --option or --bool', out.stdout)

    def test_no_options_need_no_features(self):
        (self.root / 'inputs' / 'c_src' / 'configuration.json').write_text('{"configurable_variables": {}}')
        self.manifest([])
        self.assertIn('No build options', run('names', cwd=self.root).stdout)
        self.assertEqual(run('check', cwd=self.root).returncode, 0)


if __name__ == '__main__':
    unittest.main()
