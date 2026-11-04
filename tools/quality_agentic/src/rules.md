### Rules for every change (not negotiable)

1. **Behavior stays identical**: the same outputs, return codes and order of checks, the same
   wrapping arithmetic, the same allocator and callback calls (how often, with which arguments),
   and the same writes to caller memory. Do not fix bugs of the C code. Do not add or remove
   checks. A new panic is a behavior change, unless the C code has undefined behavior for that
   input.
2. **The exported C ABI stays identical**: the name, the parameter and return types, `extern "C"`
   and `#[no_mangle]`/`#[export_name]` of every exported function, and the size and layout of
   every type that C can see (a type in the public headers, a type passed by value, a type that
   the caller allocates, a `#[repr(C)]` type used in such a way). `regress.py check` compares the
   exported symbols.
3. **No pseudo-safe functions.** A function that dereferences a raw pointer that its caller gives
   it stays `unsafe fn`. Never move the `unsafe` from a signature into a block to make a function
   look safe. `qscan.py compare` reports new pseudo-safe functions.
4. **No hidden lints**: no new `#[allow]`, `#![allow]`, `#[expect]`, `clippy.toml` or `[lints]`.
   One exception: recipe R9 (`too_many_arguments` on an exported function).
5. **The tests are not ours.** Never edit `verify_env/` or `tests/`, never delete a `#[test]`.
   Never change a test to make it pass.
6. No new dependencies, no `build.rs` changes, no Cargo feature changes.
7. No new `transmute`, `static mut`, `unsafe impl Send/Sync`, `mem::forget` or `Box::leak`.
8. Do not delete code (an unused function stays) and do not replace code with stubs.
9. **No new undefined behavior.** A slice or a reference covers only memory that is valid (and,
   for reading, initialized) for its whole life, and that no other path writes while it lives.
   `add`/`offset`/`sub` on a pointer stay inside one allocation: keep a base that can lie outside
   the buffer as an integer position (`CURSORS.md`).
10. Each `unsafe {}` block covers the smallest expression that needs it. Never wrap a whole
    function body in one block.
11. Write each doc comment and each `// SAFETY:` comment by hand, for its own place. A script may
    edit files, but it must not generate comments or code from a template.

### What a global change may do

- Change the signatures of internal (not exported) functions in all files, and their callers.
- Add types, modules, `impl` blocks and helper functions.
- Change the fields of a struct that C cannot see.
- Add `#![deny(unsafe_code)]` to a module that has no unsafe code left.
- Add one small module of trusted primitives (`trusted.rs`, about 150 lines at most) in which
  each `unsafe` has a `// SAFETY:` comment.

A global change may not do what rules 1 to 11 forbid. In particular, it may not change an exported
signature or a type that C can see.
