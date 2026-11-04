# Cursors: buffers as views, pointers as positions

This file teaches the cursor model for one root cause of unsafe code: buffers that the
translation walks with pointer cursors (`roots` sections 2 and 3a). The model comes from the
CRAFT cursor model tutorial (`/home/ubuntu/craft/user-reports/2026-11-03-1339-lead-cursor-model-tutorial-en.md`).
Only the parts that help you rewrite code are kept here.

The idea in one paragraph: each stretch of memory that the code walks is a **region**. The
function at the C boundary makes one slice of the region, the **view**, for each call. Every
pointer into the region becomes an integer **position**. A dereference becomes an index into the
view. Internal functions get the view and positions as parameters, and become safe. The unsafe
code that is left is the construction of the views at the boundary.

The file uses one small function, `sum`, that grows step by step. Each step adds one idea and
shows the code before and after the rewrite. The examples assume these lines in the module:

```rust
use core::cell::Cell;
use core::mem::MaybeUninit;
```

## The terms

| Term | Meaning |
|---|---|
| Region | One stretch of elements inside one allocation (one C object). Two allocations are always two regions. |
| View | The Rust slice of a region. A boundary function makes it, and it lives for one call. |
| Position | A `usize` (or `isize`) that replaces a pointer into a region: the distance from the origin, counted in elements. |
| Origin | The address of position 0. Usually the start of the region. It can lie before the region, also outside the allocation. |
| Null position | The value that replaces a null pointer: `None` of an `Option<usize>`. Needed only when the code stores or tests null. |
| Boundary function | The function that knows the bounds of the region (usually from the C API contract) and makes the view. |
| Permission | Read-only, exclusive write, or shared write. It decides the Rust type of the view. |
| Initialization | For an output buffer: all elements, a prefix, or none are initialized when the view is made. |

Permission and initialization give the Rust type of the view:

| Permission | All initialized | Prefix or none initialized |
|---|---|---|
| Read-only: nothing writes the elements while the view lives | `&[E]` | not possible |
| Exclusive write: only this view reads or writes them | `&mut [E]` | `trusted::OutView<E>` (step 4) |
| Shared write: other views or pointers can also read and write them | `&[Cell<E>]` | `trusted::SharedOutView<E>` (step 8) |

---

## Step 1: One pointer and a length become a view

**Contract.** The callers of `sum` pass `p` valid for `n` initialized bytes. Nothing writes these
bytes during the call.

**Cursor table**

| Region | Element | Permission | Initialization | Bounds per function | Origin | Positions |
|---|---|---|---|---|---|---|
| input | `u8` | read-only | all | `sum`: `[p, p + n)` | `p` | `i` |

**Before**

```rust
unsafe fn sum(p: *const u8, n: usize) -> i32 {
    let mut s: i32 = 0;
    let mut i: usize = 0;
    while i < n {
        s = s.wrapping_add(*p.add(i) as i32);
        i += 1;
    }
    s
}
```

**After**

```rust
unsafe fn sum(p_raw: *const u8, n: usize) -> i32 {
    let src: &[u8] = if n == 0 {
        &[]
    } else {
        // SAFETY: the callers pass `p_raw` valid for reads of `n` initialized bytes, and nothing
        // writes these bytes during the call.
        unsafe { core::slice::from_raw_parts(p_raw, n) }
    };
    let mut s: i32 = 0;
    let mut i: usize = 0;
    while i < n {
        s = s.wrapping_add(src[i] as i32);
        i += 1;
    }
    s
}
```

The view `src` is made once, at the top. Every read is a checked index. The signature does not
change, so no caller changes. This is recipe R4 in its in-body form.

Rules:

- **The view covers exactly the elements that the C contract guarantees.** A longer view is
  undefined behavior, even if the code never reads the extra elements. A shorter view panics where
  the C code is correct. Take the length from the contract, not from the loop that you see.
- **Length 0 gives an empty view.** `from_raw_parts` needs a non-null, aligned pointer, also for
  length 0. C callers often pass null with length 0. Test the length (or the pointer) first, and
  use `&[]`.
- **The element type is the pointer's element type.** Do not make a `&[u32]` of bytes. Read wider
  values from a byte view (see "Pattern table").
- When the contract fixes the length (for example "`p` points to 4 bytes"), the view can be an
  array reference: `let src: &[u8; 4] = unsafe { &*p.cast::<[u8; 4]>() };`.

---

## Step 2: A moving cursor becomes a position

**Contract.** As in step 1. The code now walks the buffer with a pointer cursor.

**Cursor table**

| Region | Element | Permission | Initialization | Bounds per function | Origin | Positions |
|---|---|---|---|---|---|---|
| input | `u8` | read-only | all | `sum`: `[p, p + n)` | `p` | locals `p`, `end` |

**Before**

```rust
unsafe fn sum(mut p: *const u8, n: usize) -> i32 {
    let end = p.add(n);
    let mut s: i32 = 0;
    while p < end {
        s = s.wrapping_add(*p as i32);
        p = p.add(1);
    }
    s
}
```

**After**

```rust
unsafe fn sum(p_raw: *const u8, n: usize) -> i32 {
    let src: &[u8] = if n == 0 {
        &[]
    } else {
        // SAFETY: the callers pass `p_raw` valid for reads of `n` initialized bytes, and nothing
        // writes these bytes during the call.
        unsafe { core::slice::from_raw_parts(p_raw, n) }
    };
    let mut p: usize = 0; // position of p_raw
    let end: usize = p + n; // position of p_raw + n: one past the end, never read
    let mut s: i32 = 0;
    while p < end {
        s = s.wrapping_add(src[p] as i32);
        p += 1;
    }
    s
}
```

What changed:

- Each local that only holds pointers into the region becomes a position: the moving `p` and the
  fixed `end`. `p.add(k)` becomes `p + k`, `*p` becomes `src[p]`, and `p < end` stays `p < end`.
- A position can be one past the end. It is never read there, so it is correct.
- A local that sometimes holds a pointer into another region is not a position. Keep it a pointer,
  or split it into two variables (step 10).
- If the code moves a cursor below the origin (for example `ip = ip.sub(1)` before a test
  `ip < start`), use `isize` positions, or move the origin (step 9).
- If the C code computes a pointer difference that can be negative, compute it as
  `q as isize - p as isize`, not as `q - p` on `usize`.

---

## Step 3: An output buffer with exclusive write

**Contract.** `sum` writes the running sum of each byte to `out`, and returns the last byte that it
wrote. `out` is valid for `n` initialized bytes. `out` and `p` never overlap, and nothing else uses
`out` during the call.

**Cursor table**

| Region | Element | Permission | Initialization | Bounds per function | Origin | Positions |
|---|---|---|---|---|---|---|
| input | `u8` | read-only | all | `sum`: `[p, p + n)` | `p` | locals `p`, `end` |
| output | `u8` | exclusive write | all | `sum`: `[out, out + n)` | `out` | locals `out`, `o` |

**Before**

```rust
unsafe fn sum(mut p: *const u8, n: usize, out: *mut u8) -> i32 {
    let end = p.add(n);
    let mut o = out;
    let mut s: i32 = 0;
    while p < end {
        s = s.wrapping_add(*p as i32);
        p = p.add(1);
        *o = s as u8;
        o = o.add(1);
    }
    if o > out { *o.sub(1) as i32 } else { 0 }
}
```

**After**

```rust
unsafe fn sum(p_raw: *const u8, n: usize, out_raw: *mut u8) -> i32 {
    let src: &[u8] = if n == 0 {
        &[]
    } else {
        // SAFETY: the callers pass `p_raw` valid for reads of `n` initialized bytes, and nothing
        // writes these bytes during the call.
        unsafe { core::slice::from_raw_parts(p_raw, n) }
    };
    let dst: &mut [u8] = if n == 0 {
        &mut []
    } else {
        // SAFETY: the callers pass `out_raw` valid for `n` initialized bytes that no other
        // pointer reads or writes during the call (the buffers never overlap).
        unsafe { core::slice::from_raw_parts_mut(out_raw, n) }
    };
    let mut p: usize = 0;
    let end: usize = p + n;
    let out: usize = 0; // position of out_raw
    let mut o: usize = out;
    let mut s: i32 = 0;
    while p < end {
        s = s.wrapping_add(src[p] as i32);
        p += 1;
        dst[o] = s as u8;
        o += 1;
    }
    if o > out { dst[o - 1] as i32 } else { 0 }
}
```

The permission of a view is a promise to the compiler:

- `&[E]` promises that nothing writes the elements while the view lives.
- `&mut [E]` promises that nothing else reads or writes them while the view lives.
- `&[Cell<E>]` promises neither. Other views and pointers can read and write the same elements.

Rule: **if a second path can reach the same bytes while the view lives, a `&[E]` or `&mut [E]`
view is undefined behavior.** The second path can be another argument (an in-place call), a
pointer in a struct, a callback, or another thread. Answer the checklist question "Other access"
before you choose `&mut [E]`. When the answer is "yes" or "not known", use shared write (step 8).

Copies and fills on exclusive views use slice methods (recipe R5):
`dst[b..b + k].copy_from_slice(&src[a..a + k])`, `dst.copy_within(a..a + k, b)` (overlap in one
region), `dst[a..a + k].fill(x)`.

---

## Step 4: An output buffer that is not initialized

**Contract.** As in step 3, but `out` can come straight from `malloc`: its bytes can be
uninitialized. The code is the same. Only the contract changed.

**Cursor table**

| Region | Element | Permission | Initialization | Bounds per function | Origin | Positions |
|---|---|---|---|---|---|---|
| input | `u8` | read-only | all | `sum`: `[p, p + n)` | `p` | locals `p`, `end` |
| output | `u8` | exclusive write | none | `sum`: `[out, out + n)` | `out` | locals `out`, `o` |

Rules:

- **Never make a `&[E]` or `&mut [E]` over elements that can be uninitialized.** That is
  undefined behavior, also when the code writes them before it reads them. Use `MaybeUninit<E>`.
- **Do not fill caller memory to make it initialized.** That writes bytes that the C code does not
  write, and the C caller can see them.

`MaybeUninit::write` is safe. A read needs to know that the element is initialized. Put this type
in the trusted module (`trusted.rs`). It records the **watermark**: the end of the prefix that is
initialized.

```rust
/// An exclusive output view. The elements `[0, init)` are initialized. `init` grows when the
/// code writes the element at `init` (front to back).
pub struct OutView<'a, E: Copy> {
    buf: &'a mut [MaybeUninit<E>],
    init: usize,
}

impl<'a, E: Copy> OutView<'a, E> {
    /// A view with no initialized element.
    pub fn new(buf: &'a mut [MaybeUninit<E>]) -> Self {
        OutView { buf, init: 0 }
    }

    /// A view whose first `init` elements are already initialized.
    ///
    /// # Safety
    /// The elements `buf[..init]` are initialized.
    pub unsafe fn with_prefix(buf: &'a mut [MaybeUninit<E>], init: usize) -> Self {
        assert!(init <= buf.len());
        OutView { buf, init }
    }

    pub fn set(&mut self, i: usize, v: E) {
        self.buf[i].write(v);
        if i == self.init {
            self.init = i + 1;
        }
    }

    pub fn get(&self, i: usize) -> E {
        assert!(i < self.init, "read of an element that is not initialized");
        // SAFETY: the elements below `self.init` are initialized (`with_prefix`, `set`).
        unsafe { self.buf[i].assume_init() }
    }
}
```

A write above the watermark does not move it. Thus a gap that nothing wrote never counts as
initialized. If the C code writes out of order and then reads, `get` can panic on a correct C read.
Check the order of writes before you use this type.

**Before**: as in step 3.

**After** (the changed lines. The rest is as in step 3.)

```rust
    let cells: &mut [MaybeUninit<u8>] = if n == 0 {
        &mut []
    } else {
        // SAFETY: the callers pass `out_raw` valid for writes of `n` bytes that no other pointer
        // reads or writes during the call. The bytes can be uninitialized.
        unsafe { core::slice::from_raw_parts_mut(out_raw.cast::<MaybeUninit<u8>>(), n) }
    };
    let mut dst = crate::trusted::OutView::new(cells);
    // ...
        dst.set(o, s as u8);
    // ...
    if o > out { dst.get(o - 1) as i32 } else { 0 }
```

When the code never reads the output back, the view can stay `&mut [MaybeUninit<E>]`, and the
writes are `dst[o].write(v)`.

---

## Step 5: The boundary function makes the views

**Contract.** A new exported function `checksum(buf, len, out)` returns -1 when `len` is 0. Else it
skips a 1-byte header and calls `sum` on the rest. The C API requires `buf` valid for `len`
initialized bytes and `out` valid for writes of `len - 1` bytes. They do not overlap.

**Cursor table**

| Region | Element | Permission | Initialization | Bounds per function | Origin | Positions |
|---|---|---|---|---|---|---|
| input | `u8` | read-only | all | `checksum`: `[buf, buf + len)` | `buf` | parameter `sum(p)`, locals `p`, `end` |
| output | `u8` | exclusive write | none | `checksum`: `[out, out + len - 1)` | `out` | parameter `sum(out)`, local `o` |

**Before** (`sum` as in step 3)

```rust
#[unsafe(no_mangle)]
pub unsafe extern "C" fn checksum(buf: *const u8, len: usize, out: *mut u8) -> i32 {
    if len == 0 {
        return -1;
    }
    sum(buf.add(1), len - 1, out)
}
```

**After**

```rust
#[unsafe(no_mangle)]
pub unsafe extern "C" fn checksum(buf: *const u8, len: usize, out: *mut u8) -> i32 {
    if len == 0 {
        return -1;
    }
    // SAFETY: the C API requires `buf` valid for reads of `len` initialized bytes that nothing
    // writes during the call.
    let src: &[u8] = unsafe { core::slice::from_raw_parts(buf, len) };
    let cells: &mut [MaybeUninit<u8>] = if len == 1 {
        &mut []
    } else {
        // SAFETY: the C API requires `out` valid for writes of `len - 1` bytes that do not
        // overlap `buf` and that nothing else uses during the call.
        unsafe { core::slice::from_raw_parts_mut(out.cast::<MaybeUninit<u8>>(), len - 1) }
    };
    let mut dst = crate::trusted::OutView::new(cells);
    sum(src, 1, len - 1, &mut dst, 0) // buf + 1 is position 1; out is position 0
}

fn sum(src: &[u8], mut p: usize, n: usize, dst: &mut crate::trusted::OutView<'_, u8>, out: usize) -> i32 {
    let end = p + n;
    let mut o = out;
    let mut s: i32 = 0;
    while p < end {
        s = s.wrapping_add(src[p] as i32);
        p += 1;
        dst.set(o, s as u8);
        o += 1;
    }
    if o > out { dst.get(o - 1) as i32 } else { 0 }
}
```

`sum` is now a safe function. Each pointer parameter became a position parameter, and each region
that `sum` uses became a view parameter. The call site converts the pointer arguments: `buf.add(1)`
is position 1 of the input view.

Rules:

- **The boundary function makes the view for each call, at the place where the C code first uses
  the region.** Keep the C order: the `len == 0` check comes first, and no view exists on that
  path. The bounds are evaluated at that place, with the values that the variables have there.
- **A view lives only during one call of its boundary function.** Never store a view in a struct
  field or a static. Store positions instead (step 12).
- **Pass a view only to the functions that use it**, and to the functions on the call path to
  them. Do not add view parameters to functions that do not need them.
- Put the bounds where the caller knows them. Here `checksum` knows the C contract of both
  buffers.

When a callee uses only `[a, b)` of a view and returns no position, `&src[a..b]` with positions
that start at 0 is the same model, and it is shorter.

---

## Step 6: The callee is also exported

**Contract.** `sum` is now exported too. C callers call it with the contract of step 4.

**Cursor table**: the bounds column gets a second boundary function for each region.

| Region | Element | Permission | Initialization | Bounds per function | Origin | Positions |
|---|---|---|---|---|---|---|
| input | `u8` | read-only | all | `checksum`: `[buf, buf + len)`; `sum`: `[p, p + n)` | start of the bounds | parameter `sum_inner(p)`, locals `p`, `end` |
| output | `u8` | exclusive write | none | `checksum`: `[out, out + len - 1)`; `sum`: `[out, out + n)` | start of the bounds | parameter `sum_inner(out)`, local `o` |

The exported signature cannot change. Split the function in two. The exported shell keeps the C
signature, makes the views and converts the pointer arguments. The inner function has the body of
step 5. `checksum` calls the inner function directly.

**After**

```rust
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sum(p: *const u8, n: usize, out: *mut u8) -> i32 {
    let src: &[u8] = if n == 0 {
        &[]
    } else {
        // SAFETY: the C API requires `p` valid for reads of `n` initialized bytes that nothing
        // writes during the call.
        unsafe { core::slice::from_raw_parts(p, n) }
    };
    let cells: &mut [MaybeUninit<u8>] = if n == 0 {
        &mut []
    } else {
        // SAFETY: the C API requires `out` valid for writes of `n` bytes that do not overlap `p`
        // and that nothing else uses during the call.
        unsafe { core::slice::from_raw_parts_mut(out.cast::<MaybeUninit<u8>>(), n) }
    };
    let mut dst = crate::trusted::OutView::new(cells);
    sum_inner(src, 0, n, &mut dst, 0)
}
```

`sum_inner` is the internal `sum` of step 5, renamed. In `checksum`, the last line becomes `sum_inner(src, 1, len - 1, &mut dst, 0)`. One region can
have many boundary functions. Each one makes its own view from its own contract.

---

## Step 7: A cursor passed by address, and a returned position

**Contract.** Reading one byte moves into a helper `take` that gets the address of the cursor and
moves it. A second helper `skip_zeros` returns a pointer into the input.

**Cursor table**

| Region | Element | Permission | Initialization | Bounds per function | Origin | Positions |
|---|---|---|---|---|---|---|
| input | `u8` | read-only | all | as in step 6 | start of the bounds | parameters `sum_inner(p)`, `take(pp)`, `skip_zeros(p, end)`, return of `skip_zeros`, locals `p`, `end` |
| output | `u8` | exclusive write | none | as in step 6 | start of the bounds | parameter `sum_inner(out)`, local `o` |

**Before**

```rust
unsafe fn take(pp: *mut *const u8) -> u8 {
    let v = **pp;
    *pp = (*pp).add(1);
    v
}

unsafe fn skip_zeros(mut p: *const u8, end: *const u8) -> *const u8 {
    while p < end && *p == 0 {
        p = p.add(1);
    }
    p
}

// in sum:
        s = s.wrapping_add(take(&mut p) as i32);
```

**After**

```rust
fn take(src: &[u8], pp: &mut usize) -> u8 {
    let v = src[*pp];
    *pp += 1;
    v
}

fn skip_zeros(src: &[u8], mut p: usize, end: usize) -> usize {
    while p < end && src[p] == 0 {
        p += 1;
    }
    p
}

// in sum_inner:
        s = s.wrapping_add(take(src, &mut p) as i32);
```

What changed:

- A pointer to a cursor (`*mut *const u8`, or `&mut *const u8` in many translations) becomes
  `&mut usize`. The helper gets the view as one more parameter.
- A function that returns a pointer into the region returns a position. Its callers assign it to a
  position: `p = skip_zeros(src, p, end);`.
- With more levels of calls, the view goes down the call chain as a parameter.

---

## Step 8: The same memory is input and output: shared write

**Contract.** The code does not change. A C caller can now call `sum(b, n, b)` to work in place.
This is correct in C: each loop reads at `p` before it writes at `o`, and `o` never passes `p`.

Now `src` and `dst` can cover the same bytes. A `&[u8]` and a `&mut [MaybeUninit<u8>]` over the
same bytes is undefined behavior (step 3). Both views become shared write.

**Cursor table**

| Region | Element | Permission | Initialization | Bounds per function | Origin | Positions |
|---|---|---|---|---|---|---|
| input | `u8` | shared write | all | as in step 6 | start of the bounds | as in step 7 |
| output | `u8` | shared write | none | as in step 6 | start of the bounds | as in step 7 |

The shared version of the output type goes into the trusted module too. Its writes go through
`Cell`, so other views and pointers can use the same bytes:

```rust
/// A shared-write output view: other views and pointers can read and write the same elements.
pub struct SharedOutView<'a, E: Copy> {
    buf: &'a [Cell<MaybeUninit<E>>],
    init: Cell<usize>,
}

impl<'a, E: Copy> SharedOutView<'a, E> {
    /// A view with no initialized element.
    pub fn new(buf: &'a [Cell<MaybeUninit<E>>]) -> Self {
        SharedOutView { buf, init: Cell::new(0) }
    }

    /// A view whose first `init` elements are already initialized.
    ///
    /// # Safety
    /// The elements `buf[..init]` are initialized, and no other path de-initializes them while
    /// the view lives.
    pub unsafe fn with_prefix(buf: &'a [Cell<MaybeUninit<E>>], init: usize) -> Self {
        assert!(init <= buf.len());
        SharedOutView { buf, init: Cell::new(init) }
    }

    pub fn set(&self, i: usize, v: E) {
        self.buf[i].set(MaybeUninit::new(v));
        if i == self.init.get() {
            self.init.set(i + 1);
        }
    }

    pub fn get(&self, i: usize) -> E {
        assert!(i < self.init.get(), "read of an element that is not initialized");
        // SAFETY: the elements below `self.init` are initialized (`with_prefix`, `set`).
        unsafe { self.buf[i].get().assume_init() }
    }
}
```

**Before**: as in step 7.

**After**

```rust
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sum(p: *const u8, n: usize, out: *mut u8) -> i32 {
    let src: &[Cell<u8>] = if n == 0 {
        &[]
    } else {
        // SAFETY: the C API requires `p` valid for reads of `n` initialized bytes. `out` can
        // point to the same bytes, and `Cell` allows that.
        unsafe { core::slice::from_raw_parts(p.cast::<Cell<u8>>(), n) }
    };
    let cells: &[Cell<MaybeUninit<u8>>] = if n == 0 {
        &[]
    } else {
        // SAFETY: the C API requires `out` valid for writes of `n` bytes. `p` can point to the
        // same bytes, and `Cell` allows that.
        unsafe { core::slice::from_raw_parts(out.cast::<Cell<MaybeUninit<u8>>>(), n) }
    };
    let dst = crate::trusted::SharedOutView::new(cells);
    sum_inner(src, 0, n, &dst, 0)
}

fn take(src: &[Cell<u8>], pp: &mut usize) -> u8 {
    let v = src[*pp].get();
    *pp += 1;
    v
}

fn sum_inner(
    src: &[Cell<u8>],
    mut p: usize,
    n: usize,
    dst: &crate::trusted::SharedOutView<'_, u8>,
    out: usize,
) -> i32 {
    let end = p + n;
    let mut o = out;
    let mut s: i32 = 0;
    while p < end {
        s = s.wrapping_add(take(src, &mut p) as i32);
        dst.set(o, s as u8);
        o += 1;
    }
    if o > out { dst.get(o - 1) as i32 } else { 0 }
}
```

What changed: only the view types and the read `src[*pp].get()`. The positions and the loop stay
the same. `checksum` makes the same `Cell` views.

Shared write needs no promise about overlap, so it is the correct choice when the contract allows
overlap or does not say. Use read-only or exclusive write only when the checklist answer shows that
no other path reaches the bytes (for example, the C API documents that the buffers must not
overlap).

On `Cell` views, copy element by element: `dst[b + j].set(src[a + j].get())`. Keep the C copy
direction. A `memmove` copies backward when the destination is after the source, and an LZ77 match
copy reads bytes that the same loop wrote.

Rule: **a pointer that leaves the model while the view is still used later needs shared write.**
Examples: a pointer that a callback gets, or a pointer that C code stores and uses during the
scope. Restore such a pointer from the raw pointer of the view: `out.wrapping_add(o)`. With
`&[E]` or `&mut [E]`, an access through that pointer makes the later use of the view undefined
behavior. With `Cell` views, it is correct.

---

## Step 9: The origin: read before the start, and a base outside the allocation

**Contract.** Each output byte is now the running sum XOR the previous output byte. The first loop
reads `out[-1]`. The C API requires the byte before `out` to be in the same allocation and
initialized (for example the last byte of the previous call), also when `n` is 0.

**Cursor table**

| Region | Element | Permission | Initialization | Bounds per function | Origin | Positions |
|---|---|---|---|---|---|---|
| input | `u8` | shared write | all | as in step 6 | start of the bounds | as in step 7 |
| output | `u8` | shared write | prefix 1 | `checksum`: `[out - 1, out + len - 1)`; `sum`: `[out - 1, out + n)` | `out - 1` (so `out` is position 1) | parameter `sum_inner(out)`, local `o` |

**Before** (the changed lines of `sum`)

```rust
    while p < end {
        s = s.wrapping_add(take(&mut p) as i32);
        *o = (s ^ *o.sub(1) as i32) as u8;
        o = o.add(1);
    }
```

**After** (the shell, and the changed line of `sum_inner`)

```rust
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sum(p: *const u8, n: usize, out: *mut u8) -> i32 {
    let src: &[Cell<u8>] = if n == 0 {
        &[]
    } else {
        // SAFETY: as in step 8.
        unsafe { core::slice::from_raw_parts(p.cast::<Cell<u8>>(), n) }
    };
    // SAFETY: the C API requires `out - 1 .. out + n` to be one allocation, valid for writes.
    // `p` can point into the same bytes, and `Cell` allows that.
    let cells: &[Cell<MaybeUninit<u8>>] =
        unsafe { core::slice::from_raw_parts(out.sub(1).cast::<Cell<MaybeUninit<u8>>>(), n + 1) };
    // SAFETY: the C API requires the byte at `out - 1` to be initialized.
    let dst = unsafe { crate::trusted::SharedOutView::with_prefix(cells, 1) };
    sum_inner(src, 0, n, &dst, 1) // out is position 1: the origin is out - 1
}

// in sum_inner:
        dst.set(o, (s ^ dst.get(o - 1) as i32) as u8);
```

A position is relative to an origin. The slice index is the position minus the position of the
region start. Choose the origin so that this is simple:

- Origin at the region start (as here): the index is the position, and no position is negative.
  `out` converts to position 1.
- Origin at `out`: `out[-1]` is position -1. Then positions are `isize`, and the index is
  `position + 1`. This is also correct, but every index needs the offset.

`sum_inner` did not change except for the read. It gets `out` as a position, so the shell decides
where the origin is.

**The origin can lie outside the allocation.** C stream code often keeps a base pointer
`base = src - start` and reads `base[idx]` for `idx` in `[start, start + n)`. Then `base` points
before the allocation.

```rust
// before
unsafe fn sum_from(src: *const u8, start: usize, n: usize) -> i32 {
    let base = src.wrapping_sub(start);
    let mut s: i32 = 0;
    let mut idx = start;
    while idx < start + n {
        s = s.wrapping_add(*base.add(idx) as i32);
        idx += 1;
    }
    s
}

// after
fn sum_from(src: &[u8], start: usize, n: usize) -> i32 {
    // The origin is `start` bytes before src[0]: position idx is src[idx - start].
    let mut s: i32 = 0;
    let mut idx = start;
    while idx < start + n {
        s = s.wrapping_add(src[idx - start] as i32);
        idx += 1;
    }
    s
}
```

Rules:

- **Never compute a pointer outside its allocation with `add`, `sub` or `offset`.** That is
  undefined behavior, even when the code never reads through it (`base.add(idx)` above). Keep such
  a base as an integer: the position of the region start relative to the origin.
- **One view per allocation.** Do not put two allocations on one integer axis by pointer
  subtraction. If the C code already uses one integer index space over two buffers (for example a
  dictionary below an index limit and the current block above it), keep the C index. Choose the
  view with the same comparison that the C code makes:
  `if idx < limit { dict[idx - dict_start] } else { cur[idx - cur_start] }`.
- **Compare positions only inside one region.** C code that compares pointers of two regions
  compares addresses: restore both pointers and compare them (`core::ptr::eq` for equality).
- A position may lie outside the bounds for comparison and arithmetic. Only a read or write needs
  it inside the bounds.

---

## Step 10: One variable, two regions: live ranges

**Contract.** At the end, `sum` adds up the bytes it wrote and returns that value. The C code
reuses the variable `p` for this: it now points into `out`.

**Before** (the end of `sum`)

```rust
    p = out as *const u8;
    while p < o as *const u8 {
        c = c.wrapping_add(*p as i32);
        p = p.add(1);
    }
    c
```

**After** (the end of `sum_inner`)

```rust
    let mut q: usize = out; // the second live range of `p`: a position in dst, not in src
    while q < o {
        c = c.wrapping_add(dst.get(q) as i32);
        q += 1;
    }
    c
```

**Cursor table**: the output row gets the local `q`.

`p` holds input positions in the first part and output positions in the second part. The two
parts never mix, so split the variable into one variable per part. A position belongs to exactly
one region.

If the two uses mix (for example, an `if` gives `p` a pointer of either region, and the code after
the `if` uses it), a position alone is not enough. Carry the view with the position (step 11), or
keep this variable a pointer.

Rules on the time a view lives:

- Make the view after the statements that compute its bounds.
- **End the view before the memory is freed or reallocated**, and before a call that can free or
  reallocate it. After `realloc`, make a new view from the new pointer. Positions stay valid
  across a `realloc`: they are offsets, not addresses.

---

## Step 11: One helper, two regions: the view as a parameter

**Contract.** The second loop also reads with `take`. `take` now gets an input position at one call
and an output position at the other call.

**Before** (the second loop of `sum`)

```rust
    p = out as *const u8;
    while p < o as *const u8 {
        c = c.wrapping_add(take(&mut p) as i32);
    }
```

**After**

```rust
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sum(p: *const u8, n: usize, out: *mut u8) -> i32 {
    let src_cells: &[Cell<MaybeUninit<u8>>] = if n == 0 {
        &[]
    } else {
        // SAFETY: the C API requires `p` valid for reads of `n` bytes. `out` can point into the
        // same bytes, and `Cell` allows that.
        unsafe { core::slice::from_raw_parts(p.cast::<Cell<MaybeUninit<u8>>>(), n) }
    };
    // SAFETY: the C API requires the `n` bytes at `p` to be initialized.
    let src = unsafe { crate::trusted::SharedOutView::with_prefix(src_cells, n) };
    // SAFETY: as in step 9.
    let cells: &[Cell<MaybeUninit<u8>>] =
        unsafe { core::slice::from_raw_parts(out.sub(1).cast::<Cell<MaybeUninit<u8>>>(), n + 1) };
    // SAFETY: the C API requires the byte at `out - 1` to be initialized.
    let dst = unsafe { crate::trusted::SharedOutView::with_prefix(cells, 1) };
    sum_inner(&src, 0, n, &dst, 1)
}

fn take(v: &crate::trusted::SharedOutView<'_, u8>, pp: &mut usize) -> u8 {
    let b = v.get(*pp);
    *pp += 1;
    b
}

// in sum_inner: the parameter `src` gets the type `&crate::trusted::SharedOutView<'_, u8>`,
// the first loop stays as in step 9, and the second loop is:
    let mut q: usize = out;
    while q < o {
        c = c.wrapping_add(take(dst, &mut q) as i32);
    }
```

**Cursor table**

| Region | Element | Permission | Initialization | Bounds per function | Origin | Positions |
|---|---|---|---|---|---|---|
| input | `u8` | shared write | all | as in step 6 | start of the bounds | parameters `sum_inner(p)`, `take(pp)`, locals `p`, `end` |
| output | `u8` | shared write | prefix 1 | as in step 9 | `out - 1` | parameters `sum_inner(out)`, `take(pp)`, locals `o`, `q` |

Each call site passes its own view with its own position. The function is not duplicated. Both
views must have the same Rust type at that parameter. Use the type that fits both:

- Read-only and shared write: shared write. Make the read-only view as `Cell`.
- All and prefix: the prefix type, with the prefix set to the full length (as `src` above).
- Read-only and exclusive write: no common type. Make the exclusive view shared, or keep two
  helpers.

---

## Step 12: Positions in struct fields, and pointers back to C

**Contract.** `checksum` now gets a `Buf` that holds the data and a read cursor `pos`. `Buf` is
opaque to C: C callers get it from `buf_new` (not shown), which sets `pos` to null, and they never
see its fields. A new exported function `buf_current` returns the cursor to C.

**Cursor table**

| Region | Element | Permission | Initialization | Bounds per function | Origin | Positions |
|---|---|---|---|---|---|---|
| input | `u8` | shared write | all | `checksum`: `[b.data, b.data + b.len)`; `sum`: `[p, p + n)` | `b.data` | field `Buf.pos` (null possible), as in step 11 |
| output | `u8` | shared write | prefix 1 | `checksum`: `[out - 1, out + b.len - 1)`; `sum`: as in step 9 | `out - 1` | as in step 11 |

**Before**

```rust
pub struct Buf {
    pub data: *const u8,
    pub len: usize,
    pub pos: *const u8,
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn checksum(b: *mut Buf, out: *mut u8) -> i32 {
    if (*b).len == 0 {
        return -1;
    }
    (*b).pos = (*b).data.add(1);
    sum((*b).pos, (*b).len - 1, out)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn buf_current(b: *const Buf) -> *const u8 {
    (*b).pos
}
```

**After**

```rust
pub struct Buf {
    pub data: *const u8,
    pub len: usize,
    pub pos: Option<usize>, // a position in [data, data + len); None is the null pointer
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn checksum(b_raw: *mut Buf, out: *mut u8) -> i32 {
    // SAFETY: the C API requires a valid `Buf` that nothing else uses during the call.
    let b = unsafe { &mut *b_raw };
    if b.len == 0 {
        return -1;
    }
    b.pos = Some(1); // data + 1
    let src_cells: &[Cell<MaybeUninit<u8>>] =
        // SAFETY: `b.data` is valid for reads of `b.len` bytes (the contract of `buf_new`).
        // `out` can point into the same bytes, and `Cell` allows that.
        unsafe { core::slice::from_raw_parts(b.data.cast::<Cell<MaybeUninit<u8>>>(), b.len) };
    // SAFETY: the `b.len` bytes at `b.data` are initialized (the contract of `buf_new`).
    let src = unsafe { crate::trusted::SharedOutView::with_prefix(src_cells, b.len) };
    // SAFETY: the C API requires `out - 1 .. out + len - 1` to be one allocation, valid for
    // writes. `b.data` can point into the same bytes, and `Cell` allows that.
    let cells: &[Cell<MaybeUninit<u8>>] =
        unsafe { core::slice::from_raw_parts(out.sub(1).cast::<Cell<MaybeUninit<u8>>>(), b.len) };
    // SAFETY: the C API requires the byte at `out - 1` to be initialized.
    let dst = unsafe { crate::trusted::SharedOutView::with_prefix(cells, 1) };
    sum_inner(&src, 1, b.len - 1, &dst, 1)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn buf_current(b_raw: *const Buf) -> *const u8 {
    // SAFETY: the C API requires a valid `Buf`.
    let b = unsafe { &*b_raw };
    match b.pos {
        None => core::ptr::null(),
        Some(i) => b.data.wrapping_add(i), // restore the pointer that C expects
    }
}
```

What changed:

- The field that pointed into the region holds a position. The struct no longer holds a pointer
  into its own data, and a function that has `&mut Buf` can move the cursor without `unsafe`.
- `buf_new` set the pointer to null, and C can see the difference between null and `data`.
  Thus the field is `Option<usize>`. When no code stores or tests null, a plain `usize` is enough.
- **Restore a pointer from the raw pointer of the region**: `b.data.wrapping_add(i)`. That is safe
  code. Use `wrapping_add`, not `add`, so that a position outside the bounds never makes undefined
  behavior. A null position restores to a null pointer.
- `b` became a reference first (recipe R3). The view is made from `b.data`, which points to another
  allocation, so the view and the reference do not overlap.

Rules for struct fields:

- **A type that C can see keeps its layout** (`GLOBAL.md`). Then the field stays a pointer.
  Convert when you read it, and restore when you write it. Both are safe:

  ```rust
  let p: usize = b.pos.addr().wrapping_sub(b.data.addr()); // convert (divide by size_of::<E>() for wider E)
  b.pos = b.data.wrapping_add(p + k); // restore
  ```

- **Zeroing.** `memset`, `calloc` and `mem::zeroed` give a position field the value 0. Before,
  that was a null pointer. Now it is position 0. Find each zeroing of the struct, and each null
  test of the field. Set `None` (or the C value) explicitly where the C code relied on zero as
  null.
- **A region inside the struct** (an array field `ctx.window`): make the view from the reference,
  `&ctx.window[..]`, not from a raw pointer to the field. A position field next to it lets a
  function use both at the same time, because the borrows of two fields do not conflict:

  ```rust
  pub struct Ctx {
      pub window: [u8; 64],
      pub pos: usize,
      pub total: u32,
  }

  pub fn step(ctx: &mut Ctx) {
      let b = take(&ctx.window, &mut ctx.pos); // two fields: the borrows do not conflict
      ctx.total = ctx.total.wrapping_add(b as u32);
  }
  ```

  (`take` here is the `&[u8]` version of step 7.)

---

## Pattern table

| Translated code | Cursor form |
|---|---|
| `*p`, `*p.add(k)` | `v[p]`, `v[p + k]` (shared write: `v[p].get()`, watermark: `v.get(p)`) |
| `*p = x` | `v[p] = x` (shared write: `v[p].set(x)`, watermark: `v.set(p, x)`) |
| `p = p.add(k)`, `p = p.sub(k)` | `p += k`, `p -= k` |
| `q.offset_from(p) as usize`, `end.offset_from(p)` | `q - p`, `end - p` (`isize` when it can be negative) |
| `(oend as usize).wrapping_sub(op as usize) < k` | `oend - op < k` (both positions in one region) |
| `p < q`, `p == q` in one region | the same comparison on positions |
| `p == q` across regions | restore both pointers, then `core::ptr::eq(p, q)` |
| `p.is_null()` on a stored or tested cursor | `p.is_none()` on `Option<usize>` |
| `pp: *mut *const u8`, `pp: &mut *const u8` | `pp: &mut usize`, plus the view |
| a returned pointer into the region | a returned position |
| `ptr::copy_nonoverlapping(s.add(a), d.add(b), k)` | `d[b..b + k].copy_from_slice(&s[a..a + k])` |
| `ptr::copy(v.add(a), v.add(b), k)` in one region | `v.copy_within(a..a + k, b)` |
| `ptr::write_bytes(v.add(a), x, k)` | `v[a..a + k].fill(x)` |
| a byte-by-byte forward copy that reads its own output (LZ77 match) | a forward loop `v[o + j] = v[m + j]`, never `copy_within` |
| a little-endian 32-bit read at `p.add(i)` | `u32::from_le_bytes([v[i], v[i + 1], v[i + 2], v[i + 3]])` (native order: `from_ne_bytes`) |
| `base.add(idx)` with `base` outside the allocation | `v[idx - start]` |
| a pointer that C needs back | `raw.wrapping_add(p)`, from the raw pointer of the view |

---

## The checklist

Answer these questions for each region, in your notes or in the proposal. Give the evidence
(file:line, the header comment or the C API documentation). A guess is not an answer. When you
cannot answer a question, choose the weaker form: shared write, prefix or none initialized, or no
change for that region.

1. **Range.** Which elements does the C contract guarantee at each boundary function: the start
   and the length in elements? Do they lie in one live allocation for the whole time that the view
   lives (no `free` or `realloc`, also not in a callee)? Does the code read or write past the
   logical end on purpose (a copy in 8- or 32-byte steps), and does the contract cover those bytes?
2. **Other access.** While the view lives, can another path read or write these elements: another
   argument (an in-place or overlapping call), a pointer in a struct, a callback, a custom
   allocator, another thread, a pointer that C got earlier? No path writes: read-only is possible.
   No path reads or writes: exclusive write is possible. Else: shared write.
3. **Initialization.** For an output buffer: are all elements initialized when the view is made, a
   prefix (how many), or none? Does the code read only elements that it wrote before, written
   front to back after the prefix?
4. **Origin.** Where is position 0? Can a position go below the region start or past its end, and
   is it then only compared, never read?
5. **Positions.** Which locals, parameters, return values and fields hold positions of this
   region? Does any of them also hold a pointer into another region (split it by live range, or
   keep it a pointer)?
6. **Exits.** Where does a pointer into the region go back to C (a return value, an
   out-parameter, a field that C reads, a callback argument)? Is the view used again after that
   point?
7. **Null.** Which cursors can be null, and where does the code test them? Which zeroing sets a
   position field?

---

## The cursor model in the quality stage

**Scout.** A proposal that uses the cursor model contains a cursor table and the checklist
answers for each region, in its "Design" section. The table has one row per region:

| Region | Element | Permission | Initialization | Bounds per function | Origin | Positions |
|---|---|---|---|---|---|---|
| input | `u8` | shared write | all | `checksum`: `[b.data, b.data + b.len)`; `sum`: `[p, p + n)` | `b.data`; `p` | `sum_inner(p)`, `take(pp)`, `Buf.pos` (null possible) |
| output | `u8` | shared write | prefix 1 | `checksum`: `[out - 1, out + b.len - 1)`; `sum`: `[out - 1, out + n)` | `out - 1` | `sum_inner(out)`, `take(pp)`, locals `o`, `q` |

The "Contract" section gives the new signatures of the internal functions (view and position
parameters), the changed struct fields, and the trusted types if a row needs `OutView` or
`SharedOutView`.

**Implementer.** Rewrite with the patterns of the steps above, leaves of the call graph first.
Make views only in the boundary functions of the table, with the bounds of the table, and with the
Rust type of its permission and initialization. Keep old pointer entry points as shims until all
callers use the new signatures. When the code shows that a table row is wrong (for example a
second path to the bytes), use the weaker form and say so in the reply.

**Integrator.** Check the merged code against the table:

- Each `from_raw_parts` or `from_raw_parts_mut` is in a boundary function of the table, with its
  bounds and with the type of its permission and initialization.
- No view is stored in a struct field or a static.
- The position fields and parameters are the ones in the table.
- No `add`, `sub` or `offset` computes a pointer outside its allocation.
- Each pointer given back to C is restored from the raw pointer of its region.
