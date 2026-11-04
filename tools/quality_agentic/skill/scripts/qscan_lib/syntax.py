"""Rust syntax trees from tree-sitter, with the bodies of macros parsed as code.

tree-sitter keeps the body of a macro (`assert!(..)`, a `macro_rules!` template) as a flat token
tree. walk() parses each such body again as call arguments, a block or an array, and walks the
result in place, so that a cast or a call inside a macro counts like any other.
"""
from typing import NamedTuple

import tree_sitter
import tree_sitter_rust

_PARSER = tree_sitter.Parser(tree_sitter.Language(tree_sitter_rust.language()))


class Ctx(NamedTuple):
    """Where a node is. A node of a macro body comes from a separate parse of that body: add
    `shift` to its byte offsets and `rows` to its rows to get its place in the file."""
    shift: int = 0
    rows: int = 0
    template: bool = False      # inside a macro_rules! template
    unsafe: bool = False        # inside an `unsafe {}` block
    owner: tuple = None         # (self type or trait name, is a trait impl or trait) of the innermost impl


def parse(src):
    """The root node of the syntax tree of `src` (bytes)."""
    return _PARSER.parse(src).root_node


# Rows are read as point[0]: `point.row` crashes Python 3.14 with tree-sitter 0.26.0 (a reference
# count error that frees the Point too early).
def line(node, ctx):
    """1-based line of the node's first byte in the file."""
    return node.start_point[0] + ctx.rows + 1


def end_line(node, ctx):
    """1-based line of the node's last byte in the file."""
    return node.end_point[0] + ctx.rows + 1


def text(node):
    """The source text of a node with its white space made single spaces."""
    return ' '.join(node.text.decode('utf-8', 'replace').split())


def callee(fn):
    """(name, path, method, turbofish) of the function expression of a call or of a turbofish
    (`f::<T>`); name is None for a computed function. path is the last path segment before the name
    (`ptr` in `std::ptr::copy`)."""
    turbofish = fn.type == 'generic_function'
    if turbofish:
        fn = fn.child_by_field_name('function')
    if fn.type == 'identifier':
        return fn.text.decode(), None, False, turbofish
    if fn.type == 'scoped_identifier':
        path = fn.child_by_field_name('path')
        if path is not None and path.type == 'scoped_identifier':
            path = path.child_by_field_name('name')
        return (fn.child_by_field_name('name').text.decode(), path.text.decode() if path else None,
                False, turbofish)
    if fn.type == 'field_expression':
        return fn.child_by_field_name('field').text.decode(), None, True, turbofish
    return None, None, False, turbofish


def code_lines(root):
    """The lines that hold code: a token that is not a comment (a multi-line literal holds all its lines)."""
    lines = set()
    stack = list(root.children)
    while stack:
        n = stack.pop()
        if n.type in ('line_comment', 'block_comment'):
            continue
        if n.child_count == 0 or n.type in ('string_literal', 'raw_string_literal', 'char_literal'):
            lines.update(range(n.start_point[0] + 1, n.end_point[0] + 2))
        else:
            stack.extend(n.children)
    return lines


def walk(node, ctx=Ctx()):
    """Yields (node, ctx) for `node` and every node below it, in source order. Token trees are
    not entered; the token tree of a macro call or a macro_rules! rule is parsed as code."""
    stack = [(node, ctx)]
    while stack:
        node, ctx = stack.pop()
        yield node, ctx
        kind = node.type
        if kind == 'macro_invocation':
            kids = [_reparse(node.children[-1], ctx)]
        elif kind == 'macro_definition':
            kids = [_reparse(rule.child_by_field_name('right'), ctx._replace(template=True))
                    for rule in node.named_children if rule.type == 'macro_rule']
        elif kind == 'token_tree':
            continue
        else:
            if kind == 'unsafe_block':
                ctx = ctx._replace(unsafe=True)
            elif kind in ('impl_item', 'trait_item'):
                ctx = ctx._replace(owner=_owner(node))
            kids = [(child, ctx) for child in node.children]
        stack.extend(reversed(kids))


# The forms in which a macro body is parsed: the first one without a syntax error is used, else
# the one with the fewest errors.
_WRAPPERS = ((b'_(', b');'), (b'{', b'}'), (b'[', b'];'))


def _reparse(tree, ctx):
    """(root, ctx) of the content of a token tree, parsed in the forms of _WRAPPERS."""
    content = tree.text[1:-1]
    start = tree.start_byte + ctx.shift + 1
    parses = []
    for head, tail in _WRAPPERS:
        root = parse(head + content + tail)
        if not root.has_error:
            parses = [(0, root, head)]
            break
        parses.append((_errors(root), root, head))
    _, root, head = min(parses, key=lambda p: p[0])
    return root, ctx._replace(shift=start - len(head), rows=tree.start_point[0] + ctx.rows)


def _errors(root):
    """The count of syntax errors in a tree."""
    count = 0
    stack = [root]
    while stack:
        n = stack.pop()
        count += n.type == 'ERROR' or n.is_missing
        stack.extend(n.children)
    return count


def _owner(node):
    if node.type == 'trait_item':
        return node.child_by_field_name('name').text.decode(), True
    return _type_name(node.child_by_field_name('type')), node.child_by_field_name('trait') is not None


def _type_name(node):
    """The name of a type: the last name outside generic arguments (`&mut a::Foo<T>` -> `Foo`)."""
    name = '?'
    stack = [node]
    while stack:
        n = stack.pop()
        if n.type in ('type_arguments', 'lifetime'):
            continue
        if n.type in ('type_identifier', 'identifier', 'primitive_type'):
            name = n.text.decode()
        stack.extend(reversed(n.children))
    return name
