#!/usr/bin/env python3
"""tab2rust: find large C data tables and repetitive code, and translate the tables to Rust.

Subcommands:
  build      Debug build (-g -O0) of a CMake project. Prints the binaries.
  detect     Report of large arrays (strict) and repetitive regions (loose). Changes nothing.
  translate  Write Rust for named arrays. The values come from the debug binary.
  dump       Print the compiled values of arrays.
  probe      Print the compiled values of macros and enum constants, or write them as Rust constants.

Run `tab2rust.py <subcommand> -h` for the options. Docs: ../SKILL.md and ../references/.
"""
import argparse
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from tab2rust_lib import build, consts, detect, dump, probe, translate  # noqa: E402
from tab2rust_lib.emit import STRING_TYPES  # noqa: E402


def parser():
    ap = argparse.ArgumentParser(prog='tab2rust.py', description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest='cmd', required=True)

    p = sub.add_parser('build', help='debug build of a CMake project (-g -O0)')
    p.add_argument('src', help='directory with CMakeLists.txt')
    p.add_argument('--out', help='build directory (default: ./tab2rust-build-<SRC name>-<hash>)')
    p.add_argument('--jobs', type=int, default=8)
    p.add_argument('-D', dest='define', action='append', metavar='NAME=VALUE',
                   help='CMake option, for example -DFOO=OFF (repeat for more)')
    p.set_defaults(run=build.run)

    p = sub.add_parser('detect', help='report of large arrays and repetitive regions')
    p.add_argument('src', help='C source directory')
    p.add_argument('--binary', required=True, help='debug binary from "build"')
    p.add_argument('--min', type=int, default=30,
                   help='minimum text items of an array, with the items of its body files (default 30).'
                        ' An array that the text scan did not find uses its binary values')
    p.add_argument('--rep-threshold', type=float, default=0.70, help='minimum redundancy (default 0.70)')
    p.add_argument('--rep-min-lines', type=int, default=15, help='minimum lines of a repetitive region (default 15)')
    p.add_argument('--rep-show', type=int, default=25, help='repetitive regions to print (default 25, --json has all)')
    p.add_argument('--exclude', help='regular expression of relative paths to skip')
    p.add_argument('--only', help='regular expression of relative paths to scan (skip all other files)')
    p.add_argument('--json', help='also write the report as JSON to this file')
    p.set_defaults(run=detect.run)

    p = sub.add_parser('translate', help='write Rust for arrays')
    p.add_argument('--src', required=True, help='C source directory (the same as for detect)')
    p.add_argument('--binary', required=True, help='debug binary from "build"')
    p.add_argument('--symbol', action='append', required=True, help='KEY from the detect report (repeat for more)')
    p.add_argument('--out', required=True, help='output .rs file')
    p.add_argument('--name', action='append', help='KEY=RUST_NAME (repeat for more)')
    p.add_argument('--spec', help='JSON file with more choices (see references/tool-usage.md)')
    p.add_argument('--no-check', action='store_true', help='do not compile the output with rustc')
    p.set_defaults(run=translate.run)

    p = sub.add_parser('dump', help='print compiled values of arrays')
    p.add_argument('--binary', required=True)
    p.add_argument('--symbol', action='append', required=True)
    p.add_argument('--flat', action='store_true', help='one value per line')
    p.add_argument('--out')
    p.set_defaults(run=dump.run)

    p = sub.add_parser('probe', help='print compiled values of macros and enum constants')
    p.add_argument('--build', required=True, help='build directory from "build" (has compile_commands.json)')
    p.add_argument('--tu', required=True, help='a .c file of the project that can see the names')
    p.add_argument('--span', action='append',
                   help='FILE:FIRST-LAST: take the #define and enum names in these lines. FILE alone: the whole file'
                        ' (then --match is required)')
    p.add_argument('--match', metavar='REGEX', help='use only the names that match this regular expression')
    p.add_argument('--out', help='write the JSON values to this file')
    p.add_argument('--rust', help='write one Rust "pub const" for each value to this .rs file')
    p.add_argument('--int-type', default='c', choices=consts.INT_CHOICES, metavar='TYPE',
                   help='Rust type of the integers in --rust: "c" (the size and sign of the C type, default),'
                        ' or a Rust integer type such as u8 or i32')
    p.add_argument('--str-type', default='cstr', choices=sorted(STRING_TYPES),
                   help='Rust type of the strings in --rust: cstr (&CStr, default), bytes (&[u8]), bytes_nul, str (&str, UTF-8 only)')
    p.add_argument('names', nargs='*')
    p.set_defaults(run=probe.run)
    return ap


def main():
    a = parser().parse_args()
    a.run(a)


if __name__ == '__main__':
    main()
