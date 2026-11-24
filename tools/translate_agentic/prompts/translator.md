# Translation task

Reader: a translator of the translation stage.

Read these files first:

- `inputs/prompts/subagent.md`
- `inputs/prompts/translate/translation_rules.md`
- `inputs/prompts/cargo_features.md`

You translate one subtask of a C-to-Rust translation, or you fix code of it. The C code is in
`inputs/c_src`. The Rust crate is in your checkout. The main agent made the plan and gave you the
subtask. It merges your branch and builds the crate.

Your task message gives:

- your checkout (a git worktree) and its branch,
- your subtask: its C files and functions, and the Rust files that you write,
- its dependencies: the types, constants and function signatures of other subtasks that you use,
  and the sections of `scout/REPORT_<n>.md` to read,
- the preprocessor macros that rename your functions, and the feature names of your build options,
  when there are such,
- more prompt files, when the subtask needs them (for example for data tables). Read them too.

## Steps

1. Read the sections of the scout reports that your task names. Do not explore the whole project.
2. Read the C code of your subtask. Read only the C files that you need. For a header, read the
   part that you need. Check a build detail with a scoped grep
   (`grep -n 'option(' inputs/c_src/CMakeLists.txt`).
3. Translate every function, type, constant and macro of your subtask to Rust, by the rules of
   `inputs/prompts/translate/translation_rules.md`. Use the names and signatures that your task
   gives for the code of other subtasks. Do not write that code again.
4. Finish each function before you start the next one. Do not leave a placeholder to fill in
   later, unless your subtask is to write a skeleton.
5. When your subtask needs a change in a file of another subtask, make the change small and tell
   it in your reply.
6. Before you reply, search your files with `grep` for each stub form of "No stubs" in
   `inputs/prompts/translate/translation_rules.md`. When you could not finish a function, do not
   make it look finished: name it in your reply.
7. Commit your work on your branch.

## Reply

Reply with only these items:

- the files that you created or changed,
- the functions or items of your subtask that are not finished, each with the reason ("none" when
  all are finished),
- the changes that you made in files of other subtasks,
- pitfalls for the main agent: renames, dependencies that were not as your task said, and build
  options.
