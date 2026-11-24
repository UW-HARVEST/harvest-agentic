# Surface task

Reader: a sub-agent of the verification stage that writes one surface file, or that does the cfg
completeness check.

Read these files first:

- `inputs/prompts/subagent.md`
- `inputs/prompts/common.md`
- `inputs/prompts/git_subagent.md`
- `inputs/prompts/run.md`
- `inputs/prompts/runtime.md`
- `inputs/prompts/verify/verify_rules.md`

Read each of these files completely, before you start your task. Do not cut
a file with `head`, and do not skip a file.

The four surface files `SYMBOLS.md`, `CONDITIONALS.md`, `ERRORS.md` and `CONFIGS.md` are the test
plan of the verification of a C-to-Rust translation. The tests of the main agent cover their rows.
The C code is in `inputs/c_src`. The Rust translation is in `src/`.

Your task message gives:

- your part: one of the four files, or the cfg completeness check,
- the paths of the built C and Rust artifacts (libraries and executables).

Do only your part. Its section below tells what to do. Read `inputs/c_src` and the built
artifacts. Write a surface file in the working directory. Change no code, unless your part is the
cfg completeness check.

## `SYMBOLS.md`

The entry-point surface. For the library, run `nm -D` on
both `.so` files, list every public symbol of the C `.so`, and mark every
symbol the Rust `.so` does not export. For the executable, list every
option, subcommand, and in-program command from the C argument parser and
help text, and mark every one the Rust executable does not accept. For
each missing entry, record which case holds: the implementation exists in
Rust but is not reachable (not exported, not wired to the parser), or the
C source was never translated. Report the diff. Do not change any code, and
do not stub a missing entry.

## `CONDITIONALS.md`

The conditional-compilation surface. Collect every
macro that `#if`, `#else` and other variants reference in `inputs/c_src`, deduplicated. Skip
macros used only in system headers. Classify each macro:

- feature: the user sets it at build time, with `-D` or a CMake option.
- sys: the environment fixes it. A direct flag comes from the toolchain
  or the operating system. An indirect flag comes from a measurement,
  such as pointer width or byte order.
- constant: an in-source unconditional `#define` fixes it.

| macro | class (feature / sys / constant) | evidence |
|-------|----------------------------------|----------|

List every conditional region with its nesting and its activation premise,
written as an expression over the macros:

| # | location | activation premise |
|---|----------|---------------------|

Every class and every premise names its evidence in the row: a
definition-site grep hit, or a compile probe that prints the macro value
under the project build flags. Example: a source has `#ifndef FOO_ALIGN`
and then `#define FOO_ALIGN 1`. The macro is defined with value 1 by
default, so an `#if FOO_ALIGN` region is active unless the user overrides
the macro at build time. Do not decide that a branch is inactive from
reading alone.

## `ERRORS.md`

The rejection surface of the C code. Grep the C source for
every distinct way an entry point rejects an input: error-return macros,
`return NULL`, negative sentinels, error enums, `assert`, range checks,
null checks, size limits, and the executable's error messages and exit
statuses. Write one row per distinct rejection:

| # | entry point | trigger (the exact invalid input) | expected C result |
|---|-------------|------------------------------------|-------------------|

Derive each row from what the C code checks. Do not invent rows and do not
copy them from documentation alone. Three error branches in one function
are three rows. When a rejection sits behind a conditional, write its
premise from `CONDITIONALS.md` in the trigger column. Example row for a
function that returns NULL on a null argument:
`| 7 | cfg_parse | s == NULL | NULL |`.

## `CONFIGS.md`

The configuration surface. It is the mirror of `ERRORS.md`
for valid inputs. Enumerate the axes the C code branches on. Use the
feature-class macros from `CONDITIONALS.md` as configuration axes. Cover
every option, mode, or flag that the public API or the command line can set. Cover every input
shape the code special-cases: size classes, empty, one, many, boundary
values, element types. Cover every public entry point, including the
lowest-level ones and not only the convenience wrappers. Write one row per
combination the code treats differently:

| # | entry point(s) | configuration (options and input shape) | [ ] |
|---|----------------|------------------------------------------|-----|

## The cfg completeness check

For each region of `CONDITIONALS.md`, confirm one of two
cases in `src/`: a matching conditional exists, or the region is resolved
to one branch with the resolution recorded in `CONDITIONALS.md`. A region
that is reachable under some legal build configuration must not be
dropped. When you find a dropped or mistranslated region, fix
`src/` and rebuild. The Cargo features of the build options are in
`inputs/prompts/cargo_features.md`.

## Reply

Reply with only these items:

- your part, and the file that you wrote or the files that you changed,
- the number of rows (for `CONDITIONALS.md`: the numbers of macros and of regions),
- for `SYMBOLS.md`: each missing entry and its case,
- for the cfg completeness check: each region that you corrected, and the result of the build,
- the rows or regions that you could not settle, each with the reason ("none" when all are
  settled).
