# Translation with c2rust

Read these files first:

- `inputs/agent_skills/c2rust/SKILL.md`

Translate the C project in `inputs/c_src` to a Rust package with the transpiler `c2rust`. The
skill `inputs/agent_skills/c2rust/` has the tool that runs `c2rust`.

The result of this run measures `c2rust`, not your work. Do as little as possible: each change
is the smallest one that removes one problem. A problem that you leave and record is a better
result than a large or uncertain repair.

Where this file and `inputs/prompts/output_spec.md` differ on build options or on a problem that
you leave, this file applies.

## Rules

- The Rust code of the package is the output of the tool. Change it only with the means of
  repair below.
- Do not write a function by hand. Do not rewrite, simplify or clean code, in C or in Rust.
- Keep the unsafe code that `c2rust` writes. Do not make it safe by hand.
- The package links the libraries that the C build links. Do not put a crate or your own code
  in the place of one of them.
- The tool translates one configuration: the default options of the CMake build. Do not write
  Cargo features for the other build options. Record them as not done.
- Do not edit `c2rust_record/`. Do not change or remove the git commits of the tool.

## Means of repair

A problem is one of these:

- `c2rust` stops.
- `c2rust` drops code: a function, a declaration, or a part of a statement.
- The package does not build.
- The Rust code gives a different result than the C code, or it stops where the C code does not.

For each problem, use the first of these means that removes it:

1. **Arguments.** Change the arguments of the tool: an option of `c2rust`, or an argument for
   the C compiler such as a macro definition. An argument must keep the meaning of the C code.
2. **C edit.** Copy `inputs/c_src` to `scratch/`, make the smallest edit in the copy, and
   translate the copy. An edit must keep what the program does: do not remove code or leave a
   file out to make `c2rust` pass. Keep all C edits in one copy.
3. **Rust edit.** Edit the Rust code of the package, a few lines at one site. Make the Rust
   edits last, because a new call of the tool replaces the package.

No means changes the logic of the code. Leave a problem when no means removes it, when an edit
needs more than a few lines at one site (for example a function that is missing), or when you
are not sure that a repair keeps what the code does. Record the problem as not repaired, with
the message of `c2rust` or of the compiler.

## Steps

Each step can show problems. Before the next step, repair or record each problem as the section
above tells. After a repair, do again the step that showed the problem and each step after it.

1. Run the tool. Its summary tells where `c2rust` stopped or dropped code.
2. Build the package with `cargo build --release`.
3. Run a smoke test. Build the C code from `inputs/c_src` in `scratch/`. Make a few calls of the
   library, or a few runs of the program, with the release build of the package, and compare
   the results with the C build. The smoke test is a check that the package works. It is not a
   test suite.
4. Write `REPAIRS.md` in the working directory. It lists:
   - each argument that you gave the tool, with the reason,
   - each C edit (file and line), with the reason,
   - each Rust edit (file and line), with the reason,
   - each problem that you know and did not repair, and each build option that the package does
     not have.

   `python3 inputs/agent_skills/c2rust/scripts/hand_changes.py` prints the arguments, the C edits
   and the changed Rust files, so that the list is complete.
