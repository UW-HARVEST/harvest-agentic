#!/usr/bin/env python3
"""Tests of the adaptation of c2rust output. For maintainers: python3 tests/test_adapt.py

fixture/c2rust_output/ is the unchanged output of `c2rust transpile --emit-build-files --binary main`
(c2rust 0.22.1) for fixture/toy/.
"""
import shutil
import sys
import tempfile
import unittest
from pathlib import Path

sys.dont_write_bytecode = True  # no __pycache__ in the skill directory (it is embedded and copied to agents)
HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent / 'scripts'))
from c2rust_translate_lib import ToolError, adapt  # noqa: E402


class AdaptedPackage(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.dir = tempfile.TemporaryDirectory()
        cls.package = Path(cls.dir.name) / 'toy_c2rust'
        shutil.copytree(HERE / 'fixture' / 'c2rust_output', cls.package)
        cls.counts = adapt.adapt_package(cls.package, ['m', 'z'])
        cls.rust = {f.name: f.read_text() for f in cls.package.rglob('*.rs')}

    @classmethod
    def tearDownClass(cls):
        cls.dir.cleanup()

    def test_toolchain_file_is_gone(self):
        self.assertFalse((self.package / 'rust-toolchain.toml').exists())

    def test_manifest(self):
        manifest = (self.package / 'Cargo.toml').read_text()
        self.assertIn('[workspace]\n[package]\nname = "toy_c2rust"', manifest)
        self.assertIn('crate-type = ["cdylib", "rlib"]', manifest)
        self.assertIn('[[bin]]\npath = "src/main.rs"\nname = "driver"', manifest)

    def test_no_feature_attribute_and_other_attributes_stay(self):
        for name in ('lib.rs', 'main.rs'):
            self.assertNotIn('#![feature', self.rust[name])
            self.assertIn('#![allow(', self.rust[name])
        self.assertEqual(self.counts['feature_attributes_removed'], 3)

    def test_extern_types_are_opaque_structs_before_their_block(self):
        text = self.rust['toy.rs']
        self.assertNotIn('pub type _IO_marker;', text)
        self.assertIn('#[repr(C)]\npub struct _IO_marker {\n    _private: [u8; 0],\n}\n', text)
        self.assertLess(text.index('pub struct _IO_marker'), text.index('extern "C" {\n    fn sqrt('))
        self.assertEqual(self.counts['extern_types_replaced'], 6)  # three in each of the two files

    def test_variadic_api_has_the_stable_names(self):
        text = self.rust['toy.rs']
        self.assertIn("let mut args_0: ::core::ffi::VaList<'_>;", text)
        self.assertIn('sum_list(count, args_0.clone())', text)
        self.assertIn('args.next_arg::<::core::ffi::c_int>()', text)
        self.assertEqual(self.counts['variadic_api_renames'], 3)

    def test_link_directives_follow_the_mark_of_the_unix_build_script(self):
        script = (self.package / 'build.rs').read_text()
        self.assertIn(adapt.BUILD_SCRIPT_MARK
                      + '    println!("cargo:rustc-link-lib=m");\n'
                      + '    println!("cargo:rustc-link-lib=z");\n', script)
        self.assertEqual(script.count('rustc-link-lib'), 2)


class Steps(unittest.TestCase):
    def test_a_feature_attribute_over_several_lines(self):
        text, count = adapt.remove_feature_attributes('#![allow(dead_code)]\n#![feature(\n    asm,\n    c_variadic\n)]\nfn f() {}\n')
        self.assertEqual((text, count), ('#![allow(dead_code)]\nfn f() {}\n', 1))

    def test_an_extern_block_with_only_types_goes_away(self):
        text, count = adapt.replace_extern_types('extern "C" {\n    pub type A;\n}\npub type B = *mut A;\n')
        self.assertEqual(count, 1)
        self.assertNotIn('extern', text)
        self.assertIn('pub struct A {', text)
        self.assertIn('pub type B = *mut A;', text)  # a type alias is not an extern type

    def test_several_executables_keep_their_names(self):
        manifest = (HERE / 'fixture' / 'c2rust_output' / 'Cargo.toml').read_text()
        manifest = manifest.replace('[dependencies]', '[[bin]]\npath = "src/other.rs"\nname = "other"\n[dependencies]')
        adapted = adapt.adapt_manifest(manifest)
        self.assertIn('name = "main"', adapted)
        self.assertIn('name = "other"', adapted)

    def test_an_unknown_manifest_form_is_an_error(self):
        with self.assertRaises(ToolError):
            adapt.adapt_manifest('[package]\nname = "x"\n')

    def test_no_library_leaves_the_build_script(self):
        self.assertEqual(adapt.add_link_directives('fn main() {}\n', []), 'fn main() {}\n')
        with self.assertRaises(ToolError):
            adapt.add_link_directives('fn main() {}\n', ['m'])


if __name__ == '__main__':
    unittest.main()
