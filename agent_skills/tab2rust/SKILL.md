---
name: tab2rust
description: Finds data tables and repetitive code in a C project and translates the tables to Rust with exact values. Use when a C-to-Rust translation contains arrays of 30 or more items (lookup tables, Unicode tables, constant tables, struct tables, string tables) or long runs of similar lines (#define lists, enums, X-macros, unrolled code). Use it before you write any table by hand.
license: none
metadata:
  version: "0.3"
---

# tab2rust

## The problem

Large C tables cause two problems in a translation to Rust.

1. **Errors.** An LLM that hand-writes hundreds of numbers often makes mistakes. 
2. **Cost.** Hand-writing a table costs output tokens and time.

This skill gives you a symbolic program that detects large C tables and translates some of them. It also teaches you how to write a symbolic translator for the others.

## The method

1. **Detect.** The tool scans the C source and a debug build of it. It writes
   a short report. It does not translate anything.
2. **Classify.** Each finding is in one of two kinds:
   - **Array.** A C array with an initializer of 30 or more items in the
     source text. The tool gives the array a class (PURE, TAINTED, or DEAD). The PURE arrays can be later mechanically translated.
   - **Repetitive region.** Code or macros that repeat (for example, a long
     list of `#define` lines). The tool only shows where it is, and it shows
     more regions than necessary. You decide. You write the translation,
     with a small script of your own if possible.
3. **Translate.** You give the tool the names of PURE arrays. The tool translates them to Rust with exact values.

The tool reads the values from the compiled program, not from the C text.
Thus macros, `#if` blocks, and `#include` lines inside a table translated in their post-preprocessed form.

## Array classes

The report gives each array one class.

| Class | Meaning | What to do |
|---|---|---|
| PURE | The tool can give the exact values. | Translate it with the tool. |
| TAINTED | The array is in the build, but the tool cannot give the values. The `why` column gives all reasons: `stack` (an array on the stack without a static copy), `type` (union, function pointer, void pointer, bit-field, `long double`), `count` (the text and the binary do not agree). | Translate it manually. |
| DEAD | The array is not in this build. The reason column gives the cause. | Do not translate it. |

An array can also have notes.

| Note | Meaning |
|---|---|
| `#if` | The initializer has preprocessor lines. The values are for this build only. |
| `body in FILE` | The initializer body is in an included file. |
| `macro items` | Macros give the items. |
| `zero fill` | The text has fewer items. The other values are 0. |
| `no braces` | The initializer is a string or macro concatenation. |
| `not in text` | The text scan did not find the initializer. |
| `writable` | The C program can change the array at run time. `pointers can change`: the elements or their fields are pointers. |
| `exported` | The binary exports the array by its name. Other programs and tests can link to it. Give the Rust array the same symbol name with the spec `symbols.KEY.export_name`. |
| `stack copy` | In C, the array is a local variable of its function. The values come from a static copy (see below). If the function writes to the array, copy the table into a local variable in Rust (`let mut a = TABLE;`). |

An array on the stack (in a function, without `static`) has no values in
the binary. Thus `build` makes a static copy of it: it adds `static` to the
declaration in a copy of the file and compiles it. If the copy compiles, the
array is PURE with the note `stack copy`. If the initializer uses a value
that is not constant (for example a parameter), the array stays TAINTED,
and the reason gives the compiler message.

## Recommended workflow

`<SKILL_DIR>` is the directory that contains this SKILL.md file. In each
command, replace `<SKILL_DIR>` with its absolute path.

1. Make a debug build of the C code:
   `python3 <SKILL_DIR>/scripts/tab2rust.py build <dir with CMakeLists.txt> --out <work dir>/c-build`.
   Give CMake options as `-DNAME=VALUE`. Write down the library path that
   the command prints.
2. Run the detector:
   `python3 <SKILL_DIR>/scripts/tab2rust.py detect <C source dir> --binary <library>`.
   To show only some files, add `--only REGEX` or `--exclude REGEX`.
3. Read the report. Do not read the C files of the arrays in full. The
   report counts text items. Thus an array with a short initializer, for
   example `{ MACRO }`, is not in it, even if it has hundreds of values.
   However, `translate` accepts it. `detect --min 1` lists all arrays.
4. Before you translate a PURE array with notes, read a sample of its
   source. Read the first lines, the last lines, and some lines in the
   middle. Use the line numbers from the report.
5. Decide the choices that you need. Usually this is only the Rust name, and
   the Rust struct if your crate already has one. The defaults work for all
   other choices.
6. Run the translator. Write the output directly into your crate:
   `python3 <SKILL_DIR>/scripts/tab2rust.py translate --src <C source dir> --binary <library> --symbol <KEY> --out src/<file>.rs`.
   Give all arrays of one output file to one command: repeat `--symbol`.
   Do not join generated files by hand.
   The command prints the Rust name, the Rust type, and the first and last
   values of each array. Do not read the generated file in full.
7. Connect the file to your crate (`mod`, `use`).
8. Translate the TAINTED arrays by hand.
9. For each repetitive region that you want to translate, follow
   `references/repetitive-structures.md`.

## References

- `references/tool-usage.md`: all commands, the report format, the translate
  options and their defaults. Read it before the first `translate`.
- `references/repetitive-structures.md`: how to translate repetitive regions
  with your own small script.
- `references/internals.md`: how the tool works inside. Read it only if you
  must change the tool.
