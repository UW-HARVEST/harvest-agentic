# Recipes

Each recipe is one small refactoring. `qscan file` suggests recipes per function; the suggestion
is a candidate, and the recipe's conditions decide. Check each condition in the code. When all are
true, apply the recipe. When one is false, skip the recipe and name the condition and the line
(for example "R3 condition 4: `src` can point into `dctx.header`, frame_b.rs:212").

An attempt is cheap: the build and the regression gate catch a mistake, and `git checkout -- FILE`
undoes it. A skip without a named condition is not allowed. "Too risky" is not a reason.

Suffixes in the worklist:

- `s` (for example `R4s`): the function is called from files that your unit does not own. Use the
  **shim** form: keep the old function with its old signature as a thin wrapper, so the other
  files do not change. (R3 needs no shim: its in-body form keeps the signature.)
- `x` (for example `R4x`): the function is exported to C. Its signature stays. Do the recipe
  inside the body, or move the body into a new safe inner function.

General steps for every recipe:

1. Read the whole function first: `sed -n 'A,Bp' FILE` (A, B from the worklist).
2. Change only this function (and, for the shim form, add the new function right after it).
   Do not change `use` lines: write full paths (`core::slice::from_raw_parts`,
   `core::sync::atomic::AtomicU32`). Do not change code outside the functions of your unit.
3. `cargo build --release`. An error in a file outside your unit means your change broke a
   caller: undo it (`git checkout -- FILE`) or use the shim form.
4. After each batch of 2 or 3 functions: `python3 inputs/agent_skills/rust_quality/scripts/regress.py check --quick`.
   `REGRESS: FAIL` means the behavior changed: find the function, fix it in 2 attempts at most,
   else undo it.

Order inside one function: R12, R1, R2, R3/R4, R6, R5, R7, R8, R11, R9, R10 last.

---

## R2: remove `unsafe` from a function with no unsafe operation

**When**: `unsafe fn` and `ops 0`.

**Conditions**:
- `qscan fn NAME` lists no unsafe operation.
- The function is not a trait method, and it is not used as a value of an `unsafe fn` pointer
  type (search: `grep -n 'NAME' -r src/` shows only calls `NAME(`).

**Steps**: remove `unsafe` from the signature. Build. Callers keep working; their
`unsafe {}` around the call can become unneeded (R1 later). For an exported function, the C ABI
does not change (the `unsafe` keyword is not part of the ABI).

---

## R3: raw pointer parameter → reference

**When**: the worklist names "R3 candidates" for the function: raw pointer parameters that the body
only dereferences (`(*p).field`, `*p`, `&mut *p`) or passes on to other functions. Other pointer
parameters of the same function (arithmetic, comparisons) do not matter for this parameter.

**Conditions** (all, for this parameter `p: *mut T` or `*const T`):
1. `T` is a struct, a number type, or a pointer type (`*mut *const u8` → `&mut *const u8` is
   fine). Not `c_void`, not `c_char` (a C string), not an element of an array that the function
   walks (that is R4).
2. No pointer arithmetic on `p`: no `p.add`, `p.offset`, `p.sub`, `p.wrapping_*`, `p as usize`,
   no comparison of `p` with another pointer, no `p.is_null()` (for a null check, see 6).
3. `p` is not stored (in a struct field, a global, a returned value).
4. No other parameter, global or field points into `*p` while the function runs. If a second
   pointer parameter can point into `*p` (for example `src` = `dctx.header`, which LZ4F-style
   decoders do), use the reference for both: derive the second pointer from the reference
   (`ctx.header.as_ptr()`) at the place where the C code made it, or do not apply R3 to `p`.
5. If the body writes through `p` (`(*p).x = ...`), use `&mut T`; if it only reads, use `&T`.
6. If the body begins with a null check (`if p.is_null() { return E; }`), keep the check on the
   raw pointer and make the reference after it (in-body form).

**In-body form (the default; the signature stays)**: rename the parameter to `p_raw`, make the
reference once at the top (after the null checks), and use it below. A call that needs the raw
pointer gets the reference: `&mut T` coerces to `*mut T`, so `callee(p)` keeps working and the
callee's accesses derive from the reference. Never use `p_raw` again after the reference exists.

```rust
// before
unsafe fn ctx_update(ctx: *mut Ctx, src: *const u8, n: usize) {
    (*ctx).total += n;
    if (*ctx).pos + n < 64 { copy_in(ctx, src, n); return; }
    (*ctx).pos = 0;
}
// after: in-body form
unsafe fn ctx_update(ctx_raw: *mut Ctx, src: *const u8, n: usize) {
    // SAFETY: the callers pass a valid Ctx that nothing else uses during the call, and `src`
    // never points into it.
    let ctx = unsafe { &mut *ctx_raw };
    ctx.total += n;
    if ctx.pos + n < 64 { copy_in(ctx, src, n); return; }   // &mut Ctx coerces to *mut Ctx
    ctx.pos = 0;
}
```

The in-body form removes one unsafe operation per dereference. It works in exported functions
(`R3x`), in functions with callers in other files, and when other pointer parameters walk buffers.

**Signature form (all callers are in your unit)**: change the parameter type, replace `(*p).f` with `p.f`
and `*p` with `*p` of the reference. At each call site pass a reference: `f(&mut *raw)` when the
caller has a raw pointer (inside the caller's unsafe context), or `f(r)` when the caller already
has a reference. When the function has no other unsafe operation left, remove `unsafe` from its
signature (R2).

```rust
// before
unsafe fn ctx_reset(ctx: *mut Ctx, level: c_int) {
    (*ctx).level = level;
    (*ctx).pos = 0;
    if (*ctx).buf_len > 0 { (*ctx).flags |= 1; }
}
// after
fn ctx_reset(ctx: &mut Ctx, level: c_int) {
    ctx.level = level;
    ctx.pos = 0;
    if ctx.buf_len > 0 { ctx.flags |= 1; }
}
// a caller that holds a raw pointer `cctx`:
ctx_reset(&mut *cctx, level);
```

**Exported form (`R3x`)**: the in-body form; the C checks (null checks) stay before the
reference.

---

## R4: pointer + length → slice

**When**: the function walks a buffer that it gets as a pointer and a length (`src: *const u8,
src_len: usize`), with `*src.add(i)`, `src.offset(i)`, or `ptr::copy*`.

**Conditions** (all):
1. You can name the exact length `n` of the buffer from the parameters (or a struct field), and
   every access is inside `0..n`. Check every loop bound. If the code reads before the start
   (`p.offset(-1)`) or at or after the end (`*p.add(n)`), do not apply R4.
2. For a written buffer (`&mut [T]`): no other pointer that the function uses can point into the
   same buffer (`memmove`-like code with source and destination in one buffer: use one
   `&mut [T]` and `copy_within`, or skip).
3. The element type is the pointer's type (`u8` for `*const u8`). Do not reinterpret bytes as
   wider types through a slice.
4. A null pointer is possible only with length 0: handle it as shown below.

**Steps**: make the slice once, at the top of the body, and index it:

```rust
// before
unsafe fn sum(src: *const u8, n: usize) -> u32 {
    let mut s = 0u32;
    let mut i = 0;
    while i < n { s = s.wrapping_add(*src.add(i) as u32); i += 1; }
    s
}
// after (internal function, all callers in your unit): the caller makes the slice
fn sum(src: &[u8]) -> u32 {
    let mut s = 0u32;
    let mut i = 0;
    while i < src.len() { s = s.wrapping_add(u32::from(src[i])); i += 1; }
    s
}
// a caller with a raw pointer and a length:
let s = if n == 0 { &[][..] } else { unsafe { core::slice::from_raw_parts(src, n) } };
let total = sum(s);
```

For the exported form (`R4x`) or the shim form (`R4s`), keep the signature and make the slice at
the top of the body:

```rust
let src: &[u8] = if n == 0 { &[] } else {
    // SAFETY: the C API requires src to be valid for n bytes.
    unsafe { core::slice::from_raw_parts(src, n) }
};
```

Note: indexing checks the bounds. A wrong length panics (the process aborts at the C boundary)
where C would read out of bounds. The regression tests show this. If a test fails, undo.

**R4b: a fixed-length buffer that the caller allocates.** A pointer parameter whose callers all
pass a buffer of a known fixed length (a local array, an array field, an allocation of a constant
size, often behind a macro) becomes `&mut [T; N]` or `&mut [T]`. Initialize memory from
`malloc` before you make a slice of it (`write_bytes` to zero, or the C code's own initialization);
a slice of uninitialized memory is undefined behavior.

---

## R9: fix a clippy lint by hand

**When**: the worklist lists lints for the function. `qscan fn NAME` prints each message.

Common lints in translated code:

| Lint | Fix |
|---|---|
| `needless_range_loop` | `for (i, x) in a.iter().enumerate()` or `for x in &a[..n]` |
| `manual_memcpy` | `dst[a..a + n].copy_from_slice(&src[b..b + n])` |
| `collapsible_if`, `collapsible_else_if` | merge the conditions with `&&` / `else if` |
| `nonminimal_bool` | the simpler expression from the message |
| `identity_op`, `erasing_op` | remove `+ 0`, `* 1`, `<< 0`; keep the C semantics |
| `ptr_eq` | `core::ptr::eq(a, b)` |
| `missing_safety_doc` | recipe R10 |
| `not_unsafe_ptr_arg_deref` | recipe R12 |
| `too_many_arguments` on an **exported** function | the C signature needs it: add `#[allow(clippy::too_many_arguments)] // C ABI signature` on that function. This is the only allowed `allow`. On an internal function: leave it. |
| `upper_case_acronyms`, `non_snake_case` names of C types or exports | leave them (the C names are part of the API) |

Do not change behavior to silence a lint. When the fix is not obvious, leave the lint.

---

## R10: document the remaining unsafe

**When**: always last, for every function that still has unsafe code.

- Each `pub unsafe fn` gets a `/// # Safety` section that names the caller's duties exactly:
  which pointers must be valid, for how many bytes or elements, aligned, not aliased, and other
  preconditions from the C API.
- Each `unsafe {}` block gets a `// SAFETY:` comment on the line before it that says why the
  operation is valid here.

```rust
/// Resets the stream.
///
/// # Safety
/// `ctx` must point to a valid `Ctx` that no other reference uses during the call, and `dst`
/// must be valid for writes of `dst_len` bytes.
pub unsafe fn stream_reset(ctx: *mut Ctx, dst: *mut u8, dst_len: usize) { ... }

// SAFETY: `ctx` was checked for null above; the C API requires an exclusive context.
let ctx = unsafe { &mut *ctx };
```

Write the real reason. Text like "this is safe because the caller guarantees it" is not a reason.

- Write each text by hand for its function. Do not generate docs with a script or a template.
  `qscan compare` reports a `# Safety` text that more than 3 functions share as a PROBLEM.
- Do R10 after R3/R4: document the unsafe that is left, not the unsafe that a recipe removes.
- A parameter that the function does not use has no precondition.

---

## R12: make a pseudo-safe function honest

**When**: the worklist says `PSEUDO-SAFE` (a safe signature, a raw pointer parameter used in an
`unsafe {}` block) or the lint `not_unsafe_ptr_arg_deref`.

Choose the first option that works:

1. **Remove the raw pointer** (internal function): apply R3 or R4 so that the function gets a
   reference or a slice. Then it is really safe.
2. **Make it `unsafe fn`**: add `unsafe` to the signature (for an exported function:
   `pub unsafe extern "C" fn`; the C ABI does not change), and add the `# Safety` doc (R10).
   Callers inside the crate that are safe functions now need `unsafe {}` around the call.
   **Only when the worklist shows no "called from" for the function**: another worker can make
   those callers safe at the same time, and then the merge does not build although both branches
   do. With callers in other files, skip option 2 and write the function in your report. If a
   caller inside your unit breaks, add the `unsafe {}` there (with a `// SAFETY:` comment).

---

## Short recipes: R1, R5, R6, R7, R8, R11

Each line is a whole recipe. Apply it only when its condition is true.

| Recipe | When | Change | Condition |
|---|---|---|---|
| R1: remove an unneeded `unsafe {}` block | rustc warns `unused_unsafe` | `let n = unsafe { len_of(&buf) };` → `let n = len_of(&buf);` (keep the braces when they make a scope) | none |
| R5: C memory calls → slice methods | both sides are slices or arrays already | `copy_nonoverlapping` → `dst[..n].copy_from_slice(&src[..n])`; `copy` in one buffer → `buf.copy_within(a..a + n, b)`; `write_bytes`/`memset` → `arr[..n].fill(v)`; `memcmp(..) == 0` → `a[..n] == b[..n]` | source and destination do not overlap for `copy_from_slice`; only `== 0`/`!= 0` of a `memcmp` result is used |
| R6: local pointer walk → index | `ptr arithmetic` ≥ 3, and the function has a slice of the buffer | `ip = ip.add(1)` → `i += 1`; `*ip` → `src[i]`; `ip.offset_from(src)` → `i`; `end.offset_from(ip)` → `src.len() - i` | the pointer starts at a known index of the slice and moves by known steps inside it; for longer walks across functions see `CURSORS.md` |
| R7: `static mut` → `static`, `const` or atomic | `static_mut` ops > 0 | never written after its initializer: `static` (or `const` for a number); a written number or bool: the atomic type of the same width and sign (`AtomicU32`, `AtomicBool`, ...) with `load`/`store(.., Ordering::Relaxed)` | every use is in your unit; no pointer to it (`&raw mut NAME`, `addr_of_mut!`) is written through or passed on; arrays and structs that are written: skip |
| R8: `mem::zeroed()` → a safe value | `zeroed` > 0 | `0`, `[0u8; 64]`, or `T::default()` | the type is a number, a bool, an array of them, or a type whose `Default` is all zeros; structs with raw pointer fields: skip |
| R11: cast cleanup | `cast_lossless`, `ptr_as_ptr`, `borrow_as_ptr`, `unnecessary_cast` (`qscan autofix` fixes most) | widening `x as u32` → `u32::from(x)`; same-type cast → remove; `p as *mut u8` → `p.cast::<u8>()`; `&x as *const T` → `&raw const x` | keep every narrowing or sign-changing cast: it is C semantics |
