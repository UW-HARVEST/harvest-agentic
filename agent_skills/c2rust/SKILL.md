---
name: c2rust
description: Translates the C project of the run to a Rust package with the transpiler c2rust, makes the output build on stable Rust, commits it, and records each call. Use it for each translation with c2rust, and to list what the working directory has that the tool does not give.
license: none
metadata:
  version: "0.1"
---

# c2rust

Two commands. Run them from the root of the working directory.

- `python3 inputs/agent_skills/c2rust/scripts/c2rust_translate.py` translates the C project and
  writes the Rust package. Below, `TOOL` is this command.
- `python3 inputs/agent_skills/c2rust/scripts/hand_changes.py` lists what the working directory
  has that the tool does not give.

## What one call does

`TOOL [--c-src DIR] [--binary UNIT | --library-only] [--transpile-arg=ARG]... [--no-libc-define] [-- CLANG_ARG...]`

1. It makes the directory `scratch/c2rust/` again, and builds the C source with CMake in
   `scratch/c2rust/build`: a Release build with the default options of the project.
2. It runs `c2rust transpile` one time on the compile database of the build, with all
   translation units. The output of c2rust is in `scratch/c2rust/transpile.log`.
3. It adapts the output of c2rust to stable Rust:
   - It removes the toolchain file of c2rust.
   - `Cargo.toml`: the library is a `cdylib` and an `rlib`. A single executable gets the name
     `driver`.
   - It removes each `#![feature(...)]` attribute.
   - It replaces each extern type with an opaque struct.
   - It renames the C-variadic API to the stable names.
   - It adds the system libraries of the C link line to `build.rs`, by name. A library outside
     the standard directories of the linker also needs a `cargo:rustc-link-search` line there.
4. It puts the package in the working directory (`Cargo.toml`, `build.rs`, `lib.rs`, `src/`) in
   place of the package of the last call.
5. It appends the call to `c2rust_record/calls.jsonl`: the versions of c2rust and rustfmt, the
   command lines, the exit status, the count of each message class, and the changes of the C
   source. For an edited copy of the C source, `c2rust_record/c_edits.patch` has its changes.
6. It commits the package and `c2rust_record/` in git.

The tool runs CMake and c2rust with these environment variables only: `PATH`, `HOME`, `TMPDIR`,
`CARGO_HOME`, `RUSTUP_HOME` and `RUSTUP_TOOLCHAIN`. A variable such as `CFLAGS` or `CPATH` does
not reach them. Thus the options of a call and the C source decide the translation. c2rust
formats its output with the rustfmt of the Rust toolchain of the run.

Exit status 0: the package is in the working directory. Exit status 1: c2rust wrote no package,
and the package files did not change. The tool records and commits the call in both cases. Exit
status 2: the tool stopped and recorded nothing. It prints the cause: an option that it does not
accept, a C build that failed, or output of c2rust in a form that it cannot adapt.

## Options

A call without options translates `inputs/c_src` with the defaults of the tool.

- `--c-src DIR`: translate an edited copy of `inputs/c_src`. The copy must not have a symbolic
  link.
- `--binary UNIT`: the translation unit that has the `main` of the executable, as its file name
  without the suffix (`main` for `main.c`). When the C build has exactly one such unit, the tool
  finds it. The other units become the library, and the executable uses the library.
- `--library-only`: translate no executable. Each unit becomes a part of the library.
- `--transpile-arg=ARG`: an option of `c2rust transpile` (see `c2rust transpile --help`). The
  tool accepts `--preserve-unused-functions`, `--no-simplify-structures`,
  `--no-incremental-relooper`, `--ignore-c-loop-info`, `--ignore-c-multiple-info`,
  `--disable-refactoring`, `--reduce-type-annotations`, `--fail-on-error`, `--fail-on-multiple`,
  `--translate-const-macros=VALUE`, `--translate-fn-macros=VALUE` and `--invalid-code=VALUE`.
- `-- CLANG_ARG...`: more arguments for the C compiler front end of c2rust. The tool accepts
  `-DNAME`, `-DNAME=1` and other macro definitions, `-UNAME`, `-std=NAME`, and `-f`, `-W` and
  `-m` options that have no path. The CMake build of step 1 does not get them. The tool refuses
  a macro value with `/`. Such a definition, and a file that is added or changed (for example a
  header), is possible only in a copy of the C source (`--c-src`).
- `--no-libc-define`: do not give clang `-D_LIBC`. The tool gives clang this macro by default.
  With it, the headers of the C library declare `strchr`, `memchr` and the other string
  functions as plain functions. Without it, they are generic selections that c2rust cannot
  translate. With the macro, clang prints the error `Applications may not define the macro _LIBC`
  for each file. This error is expected, and the tool does not count it. Use `--no-libc-define`
  only when the macro causes a problem.

## The summary

The tool prints the count and the first lines of each message class of c2rust.

| Class | Meaning |
|---|---|
| `panics` | c2rust stopped at a construct that it cannot translate. There is no package. |
| `functions_not_translated` | c2rust left a function out. The message gives its name and the cause. |
| `declarations_skipped` | c2rust left a type or a variable out. |
| `asm_clobbers_dropped` | c2rust translated an inline assembly statement without one of its clobbers. The Rust code can change a register that the compiler uses. |
| `generic_selections_unsupported` | The C code has a generic selection. c2rust stops there. |
| `clang_errors` | clang found an error in the C code, for example a header that it did not find. |
| `duplicate_commands_skipped` | The C build compiles a file more than once. c2rust translated it one time, with the arguments of one of the commands. |
| `rustfmt_failures` | c2rust could not format a Rust file. The file probably has a syntax error. |
| `other_errors`, `other_warnings` | Other messages of c2rust. |

A count of zero for each class does not mean that the package builds or that it is correct. A
message does not always mean that the package is wrong.

## A new call replaces the package

The changes that you made to the package files after the last call are lost. The tool saves them
as a patch in `scratch/` and prints its path.

`c2rust transpile` can also run outside the tool, for example on one file, with an output
directory in `scratch/`. It then needs the file `scratch/c2rust/build/compile_commands.json`:
c2rust uses the compiler arguments of a compile database only when the file has this name.

## hand_changes.py

`hand_changes.py` takes the options of the last call that wrote a package, translates again with
these options and the C source that is on disk now, and compares the result with each file of
the working directory. It prints the calls of the record, the changes of the C source against
`inputs/c_src`, each changed package file with its added and removed lines, and each other file
that the tool does not give. The new translation replaces `scratch/c2rust/`. It takes as long as
a call of the tool.
