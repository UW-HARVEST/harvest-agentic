"""The items of the crate's files, from their syntax trees (syntax.py): functions with their
signature and body facts, structs with raw pointer fields, `static mut` items, `extern`
declarations, crate-level allows, the called names, and the code lines.
"""
import collections
import os
from dataclasses import dataclass

from . import syntax

# Functions of the C memory and allocation runtime and their Rust replacements by name.
C_MEMORY_CALLS = frozenset('''
memcpy memmove memset memcmp strlen strcmp strncmp strcpy strncpy strchr
copy_nonoverlapping write_bytes
'''.split())
ALLOC_CALLS = frozenset('malloc calloc realloc free'.split())
PTR_ARITH_METHODS = frozenset('add sub offset wrapping_offset offset_from byte_add byte_sub byte_offset'.split())
FACTS = ('as_casts', 'unsafe_blocks', 'ptr_arith', 'c_memory', 'alloc', 'zeroed')
# The node kinds that the scan of a file collects.
COLLECTED = frozenset({'function_item', 'function_signature_item', 'struct_item', 'union_item', 'static_item',
                       'type_item', 'inner_attribute_item', 'call_expression', 'generic_function'})


@dataclass
class Param:
    name: str
    type_text: str
    raw_ptr: bool


@dataclass
class Fn:
    file: str
    name: str
    qual: str             # Type::name for a method, else name
    first_line: int       # line of the first attribute or qualifier
    last_line: int        # line of the closing brace
    is_unsafe: bool
    exported: bool        # #[no_mangle] or #[export_name]
    in_trait_impl: bool   # in a trait impl or a trait
    params: list
    facts: dict           # counts in the body, keys FACTS
    uses: dict            # raw pointer parameter -> counts of 'deref', 'arg' and 'block' uses
    raw_in_unsafe: bool   # a raw pointer parameter is named inside an `unsafe {}` block

    @property
    def raw_ptr_params(self):
        return [p.name for p in self.params if p.raw_ptr]

    @property
    def lines(self):
        return self.last_line - self.first_line + 1


@dataclass
class Struct:
    file: str
    line: int
    name: str
    raw_fields: list      # (field name, type text) of the fields with a raw pointer type
    fields: int


@dataclass
class FileItems:
    fns: list
    structs: list         # structs and unions with named fields
    allows: list          # (line, [lint]) of each `#![allow(..)]`
    static_muts: int
    extern_decls: list    # names of the functions declared in `extern` blocks
    calls: set            # names that this file calls
    code_lines: int


def scan_sources(root, rel_paths):
    """Scans the files. Returns ({file: FileItems}, names of the type aliases that hide a raw pointer)."""
    parsed = {}
    aliases = {}
    for rel in rel_paths:
        with open(os.path.join(root, rel), encoding='utf-8', errors='replace') as fh:
            tree = syntax.parse(fh.read().encode())
        nodes = collections.defaultdict(list)
        for node, ctx in syntax.walk(tree):
            if node.type in COLLECTED:
                nodes[node.type].append((node, ctx))
        for node, _ in nodes['type_item']:
            aliases[node.child_by_field_name('name').text.decode()] = node.child_by_field_name('type')
        parsed[rel] = tree, nodes
    raw = raw_pointer_aliases(aliases)
    return {rel: _file_items(rel, tree, nodes, raw) for rel, (tree, nodes) in parsed.items()}, raw


def raw_pointer_aliases(aliases):
    """Names of the type aliases whose type is (or names) a raw pointer type."""
    names = set()
    changed = True
    while changed:
        changed = False
        for name, type_node in aliases.items():
            if name not in names and is_raw_ptr_type(type_node, names):
                names.add(name)
                changed = True
    return frozenset(names)


def is_raw_ptr_type(node, raw_aliases):
    """Whether a type has a raw pointer in it, also behind a type alias (`Option<*mut T>`, `ctx_ptr`)."""
    stack = [node]
    while stack:
        n = stack.pop()
        if n.type == 'pointer_type' or (n.type == 'type_identifier' and n.text.decode() in raw_aliases):
            return True
        stack.extend(n.children)
    return False


def _file_items(rel, tree, nodes, raw):
    allows = []
    for node, ctx in nodes['inner_attribute_item']:
        attr = node.named_children[0]
        args = attr.child_by_field_name('arguments')
        if attr.named_children[0].text == b'allow' and args is not None:
            words = ''.join(t.text.decode() for t in args.children[1:-1] if 'comment' not in t.type)
            allows.append((syntax.line(node, ctx), [w for w in words.split(',') if w]))
    structs = []
    for node, ctx in nodes['struct_item'] + nodes['union_item']:
        body = node.child_by_field_name('body')
        if ctx.template or body is None or body.type != 'field_declaration_list':
            continue
        fields = [f for f in body.named_children if f.type == 'field_declaration']
        keyword = next(c for c in node.children if c.type in ('struct', 'union'))
        structs.append(Struct(rel, syntax.line(keyword, ctx), node.child_by_field_name('name').text.decode(),
                              [(f.child_by_field_name('name').text.decode(), syntax.text(f.child_by_field_name('type')))
                               for f in fields if is_raw_ptr_type(f.child_by_field_name('type'), raw)],
                              len(fields)))
    structs.sort(key=lambda s: s.line)
    return FileItems(
        fns=[_function(rel, node, ctx, raw) for node, ctx in nodes['function_item'] if not ctx.template],
        structs=structs,
        allows=allows,
        static_muts=sum(any(c.type == 'mutable_specifier' for c in n.children) for n, _ in nodes['static_item']),
        extern_decls=[n.child_by_field_name('name').text.decode() for n, ctx in nodes['function_signature_item']
                      if n.parent.parent.type == 'foreign_mod_item' and not ctx.template],
        calls=_called_names(nodes),
        code_lines=len(syntax.code_lines(tree)))


def _called_names(nodes):
    """The names that a file calls or names with a turbofish (`f::<T>`, also without a call)."""
    names = {syntax.callee(n.child_by_field_name('function'))[0] for n, _ in nodes['call_expression']}
    names.update(syntax.callee(n)[0] for n, _ in nodes['generic_function'])
    return names - {None, '_'}  # `_(..)` is the form in which a macro body is parsed


def _function(rel, node, ctx, raw):
    name = node.child_by_field_name('name').text.decode()
    first = node
    exported = False
    sib = node.prev_sibling
    while sib is not None and sib.type in ('attribute_item', 'line_comment', 'block_comment'):
        if sib.type == 'attribute_item':
            first = sib
            exported = exported or b'no_mangle' in sib.text or b'export_name' in sib.text
        sib = sib.prev_sibling
    modifiers = next((c for c in node.children if c.type == 'function_modifiers'), None)
    params = []
    for p in node.child_by_field_name('parameters').named_children:
        if p.type != 'parameter':
            continue  # self receiver or a variadic `...`
        pattern, type_node = p.child_by_field_name('pattern'), p.child_by_field_name('type')
        pname = next((n.text.decode() for n, _ in syntax.walk(pattern) if n.type in ('identifier', 'self')), '_')
        params.append(Param(pname, syntax.text(type_node), is_raw_ptr_type(type_node, raw)))
    facts, uses, raw_in_unsafe = _body_facts(node.child_by_field_name('body'), ctx,
                                             {p.name for p in params if p.raw_ptr})
    return Fn(file=rel, name=name, qual=f'{ctx.owner[0]}::{name}' if ctx.owner else name,
              first_line=syntax.line(first, ctx), last_line=syntax.end_line(node, ctx),
              is_unsafe=modifiers is not None and any(c.type == 'unsafe' for c in modifiers.children),
              exported=exported, in_trait_impl=bool(ctx.owner and ctx.owner[1]), params=params,
              facts=facts, uses=uses, raw_in_unsafe=raw_in_unsafe)


def _body_facts(body, ctx, raw):
    """Counts in a function body, the uses of each raw pointer parameter (matched by name), and
    whether one of them is named inside an `unsafe {}` block."""
    facts = dict.fromkeys(FACTS, 0)
    uses = {p: dict(deref=0, arg=0, block=0) for p in raw}
    raw_in_unsafe = False
    for node, c in syntax.walk(body, ctx._replace(unsafe=False)):
        kind = node.type
        if kind == 'type_cast_expression':
            facts['as_casts'] += 1
        elif kind == 'unsafe_block':
            facts['unsafe_blocks'] += 1
        elif kind == 'call_expression':
            name, path, method, turbofish = syntax.callee(node.child_by_field_name('function'))
            if method and name in PTR_ARITH_METHODS:
                facts['ptr_arith'] += 1
            elif name in C_MEMORY_CALLS or (name == 'copy' and path == 'ptr'):
                facts['c_memory'] += 1
            elif name in ALLOC_CALLS and not turbofish:
                facts['alloc'] += 1
            elif name == 'zeroed':
                facts['zeroed'] += 1
        elif kind in ('identifier', 'metavariable') and raw and node.text.decode().lstrip('$') in raw:
            # A macro_rules! template names a parameter as `$p`: matched by name, like an identifier.
            nxt = node.next_sibling
            if nxt is not None and nxt.type == ':':
                continue  # a typed binding `p: T` that shadows the parameter
            uses[node.text.decode().lstrip('$')][_use_kind(node)] += 1
            raw_in_unsafe = raw_in_unsafe or c.unsafe
    return facts, uses, raw_in_unsafe


def _use_kind(node):
    """'deref' for `*p`, `*(p)` and `*(p as *mut T)`; 'arg' for `p` as a call argument (a reference
    coerces to a raw pointer there) or a tuple element; 'block' for every other use: arithmetic,
    comparison, assignment, a method such as a null check, a cast, storing it. A parameter with
    deref uses and no 'block' use is an R3 candidate."""
    up = node.parent
    while up.type in ('parenthesized_expression', 'type_cast_expression'):
        up = up.parent
    if up.type == 'unary_expression' and up.children[0].type == '*':
        return 'deref'
    return 'arg' if node.parent.type in ('arguments', 'tuple_expression') else 'block'
