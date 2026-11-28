"""Makes the output of `c2rust transpile --emit-build-files` a package that stable Rust builds.

The steps know the shape of c2rust output. They know nothing about the C project. Each step that
changes Rust text returns the new text and the number of changes.
"""
import re
from pathlib import Path

from . import ToolError

# c2rust pins an old nightly toolchain in this file.
TOOLCHAIN_FILE = 'rust-toolchain.toml'
# The line of the build script of c2rust after which link directives go.
BUILD_SCRIPT_MARK = '    // add unix dependencies below\n'

FEATURE_ATTRIBUTE = re.compile(r'^#!\[feature\([^\]]*\)\]\n', re.M)
EXTERN_BLOCK = re.compile(r'^extern "C" \{\n(.*?)^\}\n', re.M | re.S)
EXTERN_TYPE = re.compile(r'^[ \t]*pub type (\w+);\n', re.M)
# The C-variadic API of the nightly that c2rust writes for, and its stable form.
VARIADIC_RENAMES = (
    ('::core::ffi::VaListImpl', "::core::ffi::VaList<'_>"),
    ('.as_va_list()', '.clone()'),
    ('.arg::<', '.next_arg::<'),
)


def adapt_manifest(text):
    """Cargo.toml: the package is its own workspace root, the library is a `cdylib` (for C callers)
    and an `rlib` (for an executable of the package), and a single executable has the name `driver`."""
    text = replace_once(r'\[workspace\]\s*members\s*=\s*\[\s*\]', '[workspace]', text, 'the workspace table')
    text = replace_once(r'crate-type\s*=\s*\[[^\]]*\]', 'crate-type = ["cdylib", "rlib"]', text, 'the crate-type')
    if text.count('[[bin]]') == 1:
        text = replace_once(r'(\[\[bin\]\][^\[]*?name\s*=\s*)"[^"]*"', r'\1"driver"', text, 'the name of the executable')
    return text


def replace_once(pattern, replacement, text, what):
    text, count = re.subn(pattern, replacement, text, flags=re.S)
    if count != 1:
        raise ToolError(f'The Cargo.toml of c2rust does not have the expected form: {what} was found {count} times')
    return text


def remove_feature_attributes(text):
    """Removes each `#![feature(...)]`: stable Rust refuses the attribute."""
    return FEATURE_ATTRIBUTE.subn('', text)


def replace_extern_types(text):
    """Replaces each extern type (`pub type X;` in an `extern "C"` block, an unstable feature) with
    an opaque struct of the same name before the block. The code uses these types only through
    pointers."""
    count = 0

    def block(match):
        nonlocal count
        names = EXTERN_TYPE.findall(match.group(1))
        if not names:
            return match.group(0)
        count += len(names)
        structs = ''.join(f'#[repr(C)]\npub struct {name} {{\n    _private: [u8; 0],\n}}\n' for name in names)
        body = EXTERN_TYPE.sub('', match.group(1))
        return structs + (f'extern "C" {{\n{body}}}\n' if body.strip() else '')

    return EXTERN_BLOCK.sub(block, text), count


def rename_variadic_api(text):
    """Renames the C-variadic API to its stable names. A `va_list` that the code passes to a
    function is passed as a copy, as with `va_copy` in C. C leaves the value of a `va_list` open
    after such a call, so conforming code gives the same results. Code that reads more arguments
    from the same `va_list` after the call, and counts on the reads of the callee, does not."""
    count = 0
    for old, new in VARIADIC_RENAMES:
        count += text.count(old)
        text = text.replace(old, new)
    return text, count


def add_link_directives(build_script, libraries):
    """Adds the system libraries of the C link lines to the build script of c2rust."""
    if not libraries:
        return build_script
    if BUILD_SCRIPT_MARK not in build_script:
        raise ToolError('The build.rs of c2rust does not have the expected form')
    lines = ''.join(f'    println!("cargo:rustc-link-lib={name}");\n' for name in libraries)
    return build_script.replace(BUILD_SCRIPT_MARK, BUILD_SCRIPT_MARK + lines, 1)


def adapt_package(package, libraries):
    """Applies all steps to the c2rust output directory `package`. Returns the number of changes of
    each step on the Rust files."""
    package = Path(package)
    (package / TOOLCHAIN_FILE).unlink(missing_ok=True)
    manifest = package / 'Cargo.toml'
    manifest.write_text(adapt_manifest(manifest.read_text()))
    build_script = package / 'build.rs'
    build_script.write_text(add_link_directives(build_script.read_text(), libraries))
    steps = {'feature_attributes_removed': remove_feature_attributes,
             'extern_types_replaced': replace_extern_types,
             'variadic_api_renames': rename_variadic_api}
    counts = dict.fromkeys(steps, 0)
    for file in sorted(package.rglob('*.rs')):
        text = original = file.read_text()
        for name, step in steps.items():
            text, count = step(text)
            counts[name] += count
        if text != original:
            file.write_text(text)
    return counts
