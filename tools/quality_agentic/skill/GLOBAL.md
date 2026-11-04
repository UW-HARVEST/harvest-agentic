# Global techniques

The recipes in `RECIPES.md` change one function at a time. Most of the unsafe code of a translated
crate does not go away that way: it comes from the data model that the translation copied from C.
A context struct is reached through raw pointers in dozens of functions; buffers are walked with
pointer cursors; raw pointer fields hold buffers and links; small unsafe helpers make every caller
unsafe. This file lists the techniques that change the data model across files. Global scouts
propose them; implementers carry them out.

`qscan.py roots --scan DIR` shows which root causes a crate has:

| `roots` section | Root cause | Technique |
|---|---|---|
| 1. Dereferences by pointee type | a type reached through raw pointers in many functions | G1, G4 |
| 2. Local pointers; 3a. pointer arithmetic | buffers walked with pointer cursors | G2 (`CURSORS.md`) |
| 3b. Calls of unsafe crate functions | unsafe helpers that make callers unsafe | G7, G5 |
| 4. Structs with raw pointer fields | owned buffers, links and back-pointers kept as raw pointers | G3, G6 |
| exported vs internal ops | the C ABI boundary | G4 |

The limits of a global change (what it may do, and what it may not) are in the rules of your task
file.

## G1: thread references through a call chain

**Root cause**: a context type `T` is passed as `*mut T` through many internal functions, which
use `(*ctx).field`.

**Change**: every internal function that receives the context takes `&mut T` (or `&T` when it only
reads). The exported function makes the reference once, after the C code's null checks:

```rust
#[no_mangle]
pub unsafe extern "C" fn LIB_update(ctx: *mut Ctx, src: *const u8, n: usize) -> usize {
    if ctx.is_null() { return ERR_NULL; }
    // SAFETY: the C API requires a valid context that no other thread uses during the call.
    let ctx = unsafe { &mut *ctx };
    update(ctx, /* ... */)
}
fn update(ctx: &mut Ctx, /* ... */) -> usize { /* ctx.field ... */ }
```

**Order**: leaves first (functions that call nothing that takes the context), then their callers,
up to the exported functions. Each step builds.

**Pitfalls**:

- A second pointer that points into the context (for example `src == ctx.header.as_ptr()` when C
  passes the context's own buffer as input): while `&mut T` exists, reading through that other
  pointer is undefined behavior. Make such a buffer an index or a slice derived from the same
  reference (`&ctx.header[..n]`), or keep the function on raw pointers.
- A pointer identity check in C (`if (src == dctx->header)`) must stay a pointer comparison of the
  same addresses (`core::ptr::eq`).
- A callee that keeps the pointer (stores it in a struct) cannot take a reference.

## G2: buffers as views, cursors as positions

**Root cause**: `ip`, `op`, `iend`, `oend` pointers walk input and output buffers with `.add`,
`.offset`, `offset_from` and comparisons.

**Change**: the cursor model of `CURSORS.md`. Each buffer becomes a view (a slice, or a
`Cell` slice when other paths can write it during the call) that the boundary function makes once
per call; each pointer cursor becomes an integer position in that view; `end - p` becomes a
difference of positions. Write the cursor table and answer the checklist of `CURSORS.md` in the
proposal.

**Pitfalls**:

- Back-references inside the output (LZ77 match copies with offset < length): byte-by-byte forward
  copy semantics, not `copy_within` (which is a memmove).
- Data split over two buffers (a dictionary or prefix in another allocation): two views and an
  explicit boundary between them, not pointer subtraction across allocations.
- Do not use `get_unchecked` (it is unsafe).

## G3: owned buffers instead of raw pointer fields

**Root cause**: a struct holds `*mut u8` buffers allocated with `malloc`/`calloc` (or a custom
allocator) and freed by hand.

**Change**: `Vec<u8>` or `Box<[u8]>` fields; the size bookkeeping fields stay if C can see them.

**Pitfalls**:

- A custom allocator in the C API (user-provided alloc/free callbacks): the callbacks must still be
  called the same number of times with the same sizes. Then `Vec` is not allowed for that buffer;
  use a small owner type in `trusted.rs` that calls the user's allocator.
- Allocation failure returns an error code in C; `Vec` aborts. Use `try_reserve_exact` and return
  the same error code.
- If C callers can see the struct (layout pinned), the field type cannot change.

## G4: an API isolation layer

**Root cause**: most unsafe operations sit in exported functions or are reached from them.

**Change**: one module (`abi.rs` or one per area) holds all exported functions. Each one only checks
nulls, makes references and slices, calls a safe core function, and maps the result to the C
return value. The core modules get `#![deny(unsafe_code)]`. Errors in the core are `Result` or
enums, converted to C codes at the boundary.

**Pitfalls**: the boundary must keep the C order of checks (a null check that C does after another
check stays after it). Moving functions between files changes only the file, not the symbol.

## G5: a small trusted module

**Root cause**: a few primitive operations (unaligned reads and writes, a wild copy, a custom
allocator call) are used everywhere and make every caller unsafe.

**Change**: implement them once, on slices where possible (`u32::from_le_bytes(buf[i..i + 4]
.try_into().unwrap())` is safe), and expose safe functions. What really needs `unsafe` stays in
`trusted.rs` with documented invariants. Callers become safe. For buffers and cursors, use the
cursor model (`CURSORS.md`, with its output view types) instead of cursor types of your own in
`trusted.rs`.

## G6: links as references, options or indices

**Root cause**: struct fields that point to other structs (`dictCtx`, back-pointers, linked
lists).

**Change**: `Option<&'a T>` when the lifetime is clear (a dictionary attached during one call),
an index into an owning `Vec`, or keep the raw pointer inside `trusted.rs` behind safe accessors.

## G7: bottom-up de-escalation of unsafe helpers

**Root cause**: an internal `unsafe fn` with few unsafe operations of its own is called from
many places (`roots` section 3b).

**Change**: make the helper safe (references, slices, or `trusted.rs`), then remove `unsafe` from
callers that have no other unsafe operation left (`qscan autofix --lints unused_unsafe` and R2), and
repeat up the call graph.

## G8: types instead of C integers (idiomaticity)

Internal functions return `Result`/`Option`/`bool` and take enums where C used int codes, flags
and sentinels; the C values appear only at the boundary. Only with a G4 boundary in place.
