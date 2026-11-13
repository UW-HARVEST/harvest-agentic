# Writing a list type in Rust

The card of a list type (in `scout/sll/types/`) gives its representation. Use that representation
for the type in all functions. Do not choose a different one.

## Scope

This file gives rules for these types and functions only:

- A type with the representation OWNED-SEQUENCE, OWNED-CHAIN or BORROWED.
- The functions of a C-LAYOUT type with `Views: YES` that use a view or a reference parameter.

All other code gets no rule from this file: a type without a card, a C-LAYOUT type with
`Views: NO`, and the other functions of a C-LAYOUT type with `Views: YES`. Translate that code as
your other instructions tell.

## Rules

1. Keep the order of the items. A walk in Rust visits the items in the same order as the walk in C.
2. Keep each count and each size field that C keeps. Do not replace a stored count by `len()` when
   C can make the two different.
3. Do not add a helper function or method that makes a raw pointer access look safe, for example
   `fn next(&self) -> Option<&Node>` on a node with a raw link. The conversion to a view in the
   C-LAYOUT section is the only permitted form.
4. Use only the representations of this file. Do not use `Rc`, `RefCell`, or an index into an
   arena for these types.
5. For OWNED-SEQUENCE, OWNED-CHAIN and BORROWED: when you cannot write a function with the
   representation of the card without `unsafe` or without a copy of a node, stop. Do not work
   around it. Tell the function and the C lines in your report. The main agent then changes the
   card.

## OWNED-SEQUENCE

The list becomes a `Vec<T>` or a `VecDeque<T>` in the struct that holds the head. `T` is the node
without the link field. It is the Rust form of the C node struct. There is no node struct with a
link.

| C code | Rust |
|---|---|
| Insert at the head only, walk from the head | `Vec<T>`: `push`, walk with `.iter().rev()` |
| Insert at the tail only (a tail pointer), walk from the head | `Vec<T>`: `push`, walk with `.iter()` |
| Insert at the head and at the tail | `VecDeque<T>`: `push_front`, `push_back`, walk with `.iter()` |
| Insert at the head only, remove the first node | `Vec<T>`: `pop` (the first node is the last item) |
| Remove the first node, in the other cases | `VecDeque<T>`: `pop_front` |
| Free all nodes | `clear()`, or drop the container |

If C frees a resource of each item when it frees the node, do the same in the same order before
`clear()`.

## OWNED-CHAIN

```rust
struct Node {
    value: T,
    next: Option<Box<Node>>,
}
```

The head is an `Option<Box<Node>>`. A C pointer to the head or to a link field (`Node **pp`) is a
`&mut Option<Box<Node>>` in Rust.

Read walk. C: `for (n = head; n; n = n->next)`.

```rust
let mut cur = head.as_deref();
while let Some(n) = cur {
    // use n
    cur = n.next.as_deref();
}
```

Write walk. C: the same loop with a write to `n->field`.

```rust
let mut cur = head.as_deref_mut();
while let Some(n) = cur {
    // change n
    cur = n.next.as_deref_mut();
}
```

Insert at the head. C: `n->next = *head; *head = n;`

```rust
*head = Some(Box::new(Node { value, next: head.take() }));
```

Insert at the tail. C: `for (pp = head; *pp; pp = &(*pp)->next) {} *pp = n;`

```rust
let mut slot = head;
while slot.is_some() {
    slot = &mut slot.as_mut().unwrap().next;
}
*slot = Some(Box::new(Node { value, next: None }));
```

Remove the first node that matches. C: `if (matches(*pp)) { n = *pp; *pp = n->next; free(n); }`
in the same loop.

```rust
let mut slot = head;
while slot.as_ref().is_some_and(|n| !matches(n)) {
    slot = &mut slot.as_mut().unwrap().next;
}
if let Some(mut n) = slot.take() {
    *slot = n.next.take();
}
```

Write the loops with `slot.as_mut().unwrap()` as shown. A `while let Some(n) = slot` loop that
assigns `slot` in its body does not always compile.

Free all nodes. C: `while (n) { next = n->next; free(n); n = next; }`

```rust
let mut cur = head.take();
while let Some(mut n) = cur {
    cur = n.next.take();
}
```

Always free a chain with this loop. When a long chain is dropped without it, Rust frees the nodes
by recursion and the stack can overflow.

`Box::new` does not return on an allocation failure. Thus the C branch for `malloc` that returns
NULL has no Rust code. List each such branch in your report.

## BORROWED

The caller owns the nodes and the library only reads them.

```rust
#[repr(C)]
pub struct Node<'a> {
    pub value: c_int,
    pub next: Option<&'a Node<'a>>,
}

#[unsafe(no_mangle)]
pub extern "C" fn smallest<'a>(head: Option<&'a Node<'a>>) -> c_int {
    let Some(first) = head else { return -1 };
    let mut min = first.value;
    let mut cur = first.next;
    while let Some(n) = cur {
        min = min.min(n.value);
        cur = n.next;
    }
    min
}
```

- `Option<&Node>` has the same ABI as `const Node *`: NULL is `None`. The exported symbol and its C
  signature do not change.
- Keep the field order and the field types of the C struct, with `#[repr(C)]`.
- The function contains no `unsafe`. It relies on the C interface contract: each `next` is NULL or
  points to a valid node, and the caller does not change the nodes during the call. Write this in
  a doc comment of the function.
- Never free a node. Never keep a reference to a node after the function returns.

## C-LAYOUT

With `Views: NO` on the card, this file adds no rule for the type (see "Scope").

With `Views: YES`, the type keeps its node struct with a raw pointer link, as in C. The functions
that the card lists as read-only use a view, and the functions that it lists as single-node take
the node as a reference. The two forms are below. The other functions of the type change links,
allocate, free, call a function pointer or return a pointer to a node. They get no rule from this
file.

### Read-only walk through a view

The view struct is a second struct for the same memory. It has the fields of the node in the same
order, and the link field is a reference. The card gives its declaration. The subtask that defines
the node struct defines the view struct next to it:

```rust
#[repr(C)]
pub struct Entry {            // the C layout, for the functions that change the chain
    data: *mut c_void,
    next: *mut Entry,
}

#[repr(C)]
pub struct EntryView<'a> {    // the view, for the read-only functions
    data: *mut c_void,
    next: Option<&'a EntryView<'a>>,
}
```

A read-only function takes the head as `Option<&EntryView>` and walks in safe code:

```rust
#[unsafe(no_mangle)]
pub extern "C" fn list_length<'a>(list: Option<&'a EntryView<'a>>) -> c_uint {
    let mut n = 0;
    let mut cur = list;
    while let Some(e) = cur {
        n += 1;
        cur = e.next;
    }
    n
}
```

- `Option<&EntryView>` has the same ABI as `Entry *`. The exported symbol and its C signature do
  not change.
- A function can return data that it reads through a view, for example `e.data`. It never returns
  a pointer to a node: a function that does this is not read-only, and it does not use a view.
- When C uses the pointer without a NULL check, take the value with `.unwrap()`. A NULL pointer
  then stops the program, as in C. When C checks for NULL, match on the `Option`.
- When the head is a field and not a parameter, convert it one time and then use safe code:

  ```rust
  // SAFETY: each link of the chain is NULL or points to a live node, and nothing changes the
  // chain during this call.
  let head: Option<&EntryView> = unsafe { holder.head.cast::<EntryView>().as_ref() };
  ```

- Never keep a view after the function returns. Never write through a view.
- Memory that the C caller frees comes from the C allocator, also in a read-only function.

### One node as a parameter

A single-node function changes fields of its one node parameter during the call, and does not
change the link. It takes the node as a reference to the node struct:

```rust
#[unsafe(no_mangle)]
pub extern "C" fn list_set_data(entry: Option<&mut Entry>, data: *mut c_void) {
    if let Some(e) = entry {
        e.data = data;
    }
}
```

A read-only function that uses one node takes `Option<&EntryView>`.

## Report

For each list type of your subtask that "Scope" names, add one line to your report:

```
list type <C type>: <representation> — Rust type: <path::Type> — functions with views: <names, or none> — deviations: <none, or the C lines>
```
