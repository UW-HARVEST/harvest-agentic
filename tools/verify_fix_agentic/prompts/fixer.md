# Fix task

Reader: a sub-agent of the verification stage that fixes the Rust code.

Read these files first:

- `inputs/prompts/subagent.md`
- `inputs/prompts/verify/verify_rules.md`
- `inputs/prompts/cargo_features.md`

You fix one problem of a C-to-Rust translation that the main agent found: a divergence between
the C code and the Rust code, or entries that `SYMBOLS.md` marks as missing. The C code is in
`inputs/c_src`. The Rust translation is in `src/`. The main agent keeps the hypothesis log and
decides, from its tests, whether the problem is fixed.

Your task message gives:

- your checkout,
- the problem: the hypothesis, the test or the inputs that show it, the C result and the Rust
  result,
- the files and functions that the main agent suspects, when it knows them,
- the method prompt files of this run, when you must build or run tests. Read them too.

## Steps

1. Read the C code of the problem. Read only the functions that you need.
2. Find the matching Rust code in `src/`, and find where it does not do what the C code does.
3. Fix the Rust code. The C code is the ground truth: the Rust result must match the C result
   byte-for-byte. Never change the C code.
4. For a missing entry of `SYMBOLS.md`: when the implementation exists in Rust, add the export or
   the wiring. When the C source was never translated, translate it. Never add a stub. A stub
   that claims behavior is worse than a missing entry.
5. Build the crate (`cargo build --release`). When your task names the method prompt files, run
   the test that shows the problem, in the way that they tell, until it passes.
6. When your task gives you a git worktree, commit your work on its branch.

## Reply

Reply with only these items:

- the files and functions that you changed,
- the cause of the problem, in one or two sentences,
- the result of the build, and of each test that you ran,
- the items that are not fixed, each with the reason ("none" when all are fixed).
