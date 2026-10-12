# Scout task

You explore the C code in `c_src/` for the main agent of a C-to-Rust translation. The main agent
makes the translation plan from your reply. Other sub-agents translate the subtasks.

Your task message gives:

- your scout number `n` and the number of scouts `N`,
- your file group: directories or files of `c_src/`, or "all",
- the context window and the output token limit of the translation sub-agents. They use the same
  model as you.

Rules:

- Do not write Rust code. Do not change `c_src/`.
- Write your files only in `scout/`.
- Read a whole file only when it is short. Use `grep`, `head`, `sed -n 'A,Bp'`, and `wc -l`.
- Do not read the body of a data initializer of more than about 50 lines. Write its file and lines
  in your report.
{TAB2RUST_SCOUT}
## Explore

Find these facts for the files of your group:

1. Inventory: each `.c` and `.h` file, its line count, and its role in one line.
2. Project type: library (exported functions), program (`main()`), or both.
3. Public API: the exported functions and types.
4. Build configurability: the options and cache variables in `c_src/CMakeLists.txt` and
   `c_src/CMakePresets.json`, the source files that they select, and the `#if` switches that they
   control. These become Cargo features.
5. Symbol renames: macros that change linker symbols, for example `#define foo NAMESPACE(foo)`.
6. Call groups: functions that call each other, also across files. Find the call sites with `grep`.

## Propose subtasks

Divide the translation of your group into subtasks. Each subtask:

- has a list of files or functions, and the subtasks that it depends on,
- fits in one context window: C input + Rust output + tool overhead is at most 30% of the window,
- has a Rust output below the output token limit. Rust output is about C lines × 1.2 lines, at about
  10 tokens per line. Split a large file into function groups.

Keep a call group in one subtask when you can. Put the program entry point (`main()`) and a test
harness entry point in early subtasks.

## Report

Write `scout/REPORT_<n>.md`. It contains the sections of your reply, and in full: the inventory, the
public API, the build options, the renames, and the call groups.

## Reply

Reply with only these sections, in at most 80 lines.

### Codebase summary

- Files: <one line per .c/.h with line count + 1-line role>
- Project type: bin / lib / both
- Build configurability: <Cargo features needed, if any>
- Public API surface: <list of public functions/types>

### Proposed subtasks

- [ ] S<n>.1: <name> — files/functions: <list> — estimated output: ~Nk tokens — depends on: <other subtasks>

### Risks

At most 5 lines: renames, feature combinations, very large functions, and other problems for the
translation.
