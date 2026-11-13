---
name: sll
description: Tells which struct types of a C project are singly linked lists, from a whole-program analysis, and how to represent each one in Rust. Use it when inputs/extra_info/sll/report.json exists.
license: none
metadata:
  version: "0.1"
---

# sll

## The problem

A C singly linked list uses raw pointers. In Rust, a list with one owner can be a `Vec`, a
`VecDeque` or a chain of `Option<Box<Node>>`, and its code needs no `unsafe`. That is correct only
when no function of the program shares a node between two chains or makes a cycle. You cannot see
this from the few functions that you translate.

`inputs/extra_info/sll/report.json` is the result of a static analysis of the whole C project. It
lists the struct types that point to their own type through one field. For each type it tells
whether every function keeps the chain shape: each node has at most one predecessor, and the chain
ends with NULL.

## What the analysis gives

`inputs/agent_skills/sll/scripts/sll_cards.py` reads the report and writes one card for each list
type. A card gives the type, the place of its definition, the link field, the holders that the
analysis found (struct fields that point to a node), and one class:

| Class | Meaning | What you do |
|---|---|---|
| PROVED | The analysis proved that every function that it examined keeps the chain shape. | Accept the shape. Find the facts below. |
| NOT PROVED | The analysis has no proof for the type. The card gives the cause: no result for some functions, no function examined, or an incomplete scan of the source. This does not show that the type is not a list. | Find the facts below. The shape check below can be necessary. |
| BROKEN | The analysis found a function that breaks the chain shape. | Keep the C layout, without views. |

A card can add `with optimistic assumptions` to PROVED. The class and the decisions below do not
change.

The tool writes no card for a type that is not defined in `inputs/c_src`, for example a type of a
system header.

A type without a card is not in the scope of this skill. Translate it as your other instructions
tell. Do not apply the decisions below to it.

## What the analysis does not give

The analysis examines the functions of the library only. It does not know the callers, the owner of
the nodes, or the allocator. Find these facts in the C source for each card, and write them in the
card:

1. The functions that use the type or the link field, the functions that write the link field,
   the read-only functions, and the single-node functions.
   - A function is **read-only** when all of these are true:
     - It has a parameter that points to a node or to the struct that holds the list head. A
       function that reaches the chain only through another struct, for example through the
       cursor of an iterator, is not read-only.
     - It and the functions that it calls write no field of a node, allocate no node, free no
       node, and call no function pointer.
     - It returns no pointer to a node (as its value or through an output parameter).
   - A function is **single-node** when all of these are true:
     - It has exactly one parameter that points to a node.
     - It writes fields of that node, but not the link field.
     - It and the functions that it calls write no field of another node, allocate no node, free
       no node, and call no function pointer.
     - It returns no pointer to a node (as its value or through an output parameter).
2. Where nodes are allocated and where they are freed. An allocator is **plain** when it is
   `malloc`, `calloc` or `free` called directly. It is **replaceable** when the call goes through a
   function pointer, a callback that the application sets, or a wrapper that does one of these.
3. Each exported function signature and each struct of a public header that contains a pointer to
   a node or to the list head. A program without a library has none.
4. Each other pointer into the chain that stays after a function returns: a tail pointer, a cursor
   or iterator, a pointer to a link field (`node **`), a node pointer that a function returns.
   The holders on the card are a start. The list can be incomplete.
5. The node layout: fixed size, or variable size (a flexible array member, or an allocation that
   is larger than the struct).

## Decision

There are two decisions for each type: the representation of the type, and, for a type that keeps
the C layout, whether its read-only functions use views.

### Representation

Use the first row that applies. Write the representation and the reason in the card.

| # | Condition | Representation |
|---|---|---|
| 1 | The class is BROKEN. | C-LAYOUT, views NO |
| 2 | The library never allocates and never frees a node, and no function writes a field of a node. The caller owns the nodes, and the library only reads them. | BORROWED |
| 3 | The library never allocates and never frees a node, and a function writes a field of a node. | C-LAYOUT |
| 4 | An allocator of the nodes is replaceable, or the node layout is variable size. | C-LAYOUT |
| 5 | An exported signature or a public struct contains a pointer to a node or to the list head (fact 3). | C-LAYOUT |
| 6 | The class is NOT PROVED, and more than 20 functions use the type (fact 1) or the shape check below does not pass. | C-LAYOUT, views NO |
| 7 | The only pointers that stay after a call are the head and, if there is one, the tail. The code adds and removes nodes only at the ends, walks the chain, and frees it. | OWNED-SEQUENCE |
| 8 | No pointer other than the head stays after a call. The code also inserts, removes or moves nodes in the middle. | OWNED-CHAIN |
| 9 | All other cases. | C-LAYOUT |

Do rows 1 to 5 first. They need only `grep`, and they decide most types.

Row 2 comes before row 5. A BORROWED type can be in exported signatures: in an `extern "C"`
function, a parameter `Option<&Node>` has the same ABI as the C pointer `const Node *`.

### Views

This skill does not change a C-LAYOUT type: the translators write the type as without this skill,
with the C struct, the C allocator calls and the raw pointer links. With `Views: NO`, this skill
adds no rule for the type or its functions. With `Views: YES`, the read-only functions walk the
chain in safe Rust through a view, and the single-node functions take the node as a reference (see
`inputs/agent_skills/sll/references/rewriting.md`). These reference parameters have the same ABI
as the C pointers. The other functions get no rule.

For a C-LAYOUT type of rows 3, 4, 5 and 9, write `Views: YES` in the card when all of these are
true, and `Views: NO` when one is false:

1. The node layout is fixed size.
2. The type has one read-only function or more (fact 1).
3. The class is PROVED, or the class is NOT PROVED and the shape check below passes.

With `Views: YES`, list the read-only functions and the single-node functions in the card.

## Shape check

Do this check only when it can change a decision: at row 6, and for item 3 of "Views". Do not do
it when more than 20 functions use the type: there the check does not pass. Read each function
that writes the link field. The check passes when all of these are true for each of these
functions:

1. For each write of the link field: the value is NULL, or the node that the value points to has
   exactly one predecessor when the function returns. The predecessor of a node is the list head
   or the link field that points to the node. Do not examine the value alone. Examine the result
   of the function. These are the usual cases that pass:
   - The function allocated the node, and it stores the pointer to the node in one head or link
     field only.
   - The function overwrites or frees the old predecessor of the node. Head insertion,
     `n->next = *head; *head = n;`: the old first node gets the predecessor `n->next`, and the
     function overwrites its old predecessor `*head`. Removal,
     `prev->next = e->next; free(e);`: the node after `e` gets the predecessor `prev->next`, and
     the function frees its old predecessor `e->next` together with `e`.
2. When the function returns, no node is reachable from itself.
3. The function stores no pointer to a node or to a link field in memory that stays after it
   returns, apart from the pointers of fact 4.

Also make sure that each allocation site sets the link field of the new node before another
function can reach the node. When you are not sure about one of these points, the check does not
pass.

## Command

```
python3 inputs/agent_skills/sll/scripts/sll_cards.py inputs/extra_info/sll/report.json inputs/c_src --cards scout/sll/types
```

The command prints one line for each type of the report and writes the cards. It fills the part
"From the analysis" of each card. When no type gets a card, the last line is `No list types.`
The command does not change a card that exists, and it prints one line for each such card.

When the report is not usable, the command prints an error message, writes no card, and has exit
status 1. Do not read the report yourself then.

## Rewriting

`inputs/agent_skills/sll/references/rewriting.md` tells how to write each representation in Rust.
