//! How safe the translated Rust actually is.
//!
//! A translation can pass every test while being mechanically equivalent to
//! C2Rust output, so test pass rate cannot distinguish a genuine improvement
//! from `unsafe` merely relocated. This module measures the difference, by AST
//! rather than by text.
//!
//! ## The four rules that decide whether the numbers mean anything
//!
//! Each of these was wrong in an earlier draft and produced a plausible but
//! badly incorrect answer, so none of them is incidental:
//!
//! 1. **Type aliases are resolved.** A `*mut T` reached through
//!    `pub type png_bytep = *mut png_byte;` is a [`syn::Type::Path`], not a
//!    [`syn::Type::Ptr`], so a naive visitor never sees it. Measured on real
//!    output, resolving aliases moves libpng's pointer-exposing exports from
//!    47 to 375 of 381 — i.e. from "13% of the API exposes raw pointers" to
//!    96%. Without it, two models producing semantically identical ports differ
//!    8x purely on typedef style.
//!
//! 2. **Both export spellings are matched.** `#[no_mangle]` and
//!    `#[unsafe(no_mangle)]` are both in use, and not mixed: on real output
//!    mujs uses the plain form 226 times and the wrapped form 0, while jansson,
//!    lz4, zstd, libpng and libsodium use the wrapped form 128/143/613/381/817
//!    times and the plain form 0. A matcher that knows only one spelling
//!    reports zero exported functions for six of seven crates.
//!
//! 3. **Never `Spanned::span()` on an item.** It joins the item's outer
//!    attributes, so a function preceded by doc comments and `#[unsafe(no_mangle)]`
//!    reports its unsafe region as starting several lines early. That biases
//!    the metric toward whichever model documents its unsafe functions less —
//!    penalising the better-commented translation. Unsafe regions are taken
//!    from the `unsafe` token and the closing brace specifically.
//!
//! 4. **Agent-written tests are counted separately from the library.** The C
//!    input carries no test harness (the external suite is held out), so
//!    folding agent-written Rust tests into the library figures compares unlike
//!    things and lets a model improve its ratio by writing tests. It is
//!    load-bearing: on real output jansson's `tests/` holds 87 `unsafe {}`
//!    blocks against 1 in `src/`, and libpng's `tests/` is 34% unsafe lines
//!    against 89% in `src/`.
//!
//! ## Integrity
//!
//! An unparseable file is recorded, counted, and excluded from BOTH the
//! numerator and the denominator — never silently treated as safe. This is not
//! hypothetical: a real truncated `pcre2_substitute.rs` in an existing results
//! tree is rejected by the parser, and counting it as zero unsafe while still
//! counting its 2285 lines would make that crate look safer than it is.

use harvest_core::stage_manifest;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use syn::visit::Visit;

/// Which part of the crate a file belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    /// `src/**` and a top-level `build.rs`: the translation itself.
    Library,
    /// `tests/`, `benches/`, `examples/`, `fuzz/`, `xtask/`: agent-written, not
    /// a translation of anything in the C input.
    Harness,
    /// Anything else carrying Rust. Reported so a layout change is visible
    /// rather than silently miscounted.
    Other,
}

/// One raw-pointer bucket. Always split const/mut, and always split pointers
/// seen directly from those reached through a crate-local `type` alias, so a
/// reader can see how much of the answer rests on alias resolution.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PtrCounts {
    pub direct_const: usize,
    pub direct_mut: usize,
    pub alias_const: usize,
    pub alias_mut: usize,
}

impl PtrCounts {
    pub fn total(&self) -> usize {
        self.consts() + self.muts()
    }
    pub fn consts(&self) -> usize {
        self.direct_const + self.alias_const
    }
    pub fn muts(&self) -> usize {
        self.direct_mut + self.alias_mut
    }
    fn add_direct(&mut self, c: usize, m: usize) {
        self.direct_const += c;
        self.direct_mut += m;
    }
    fn add_alias(&mut self, c: usize, m: usize) {
        self.alias_const += c;
        self.alias_mut += m;
    }
}

/// A file the parser could not read or understand.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnparsedFile {
    pub path: String,
    pub error: String,
    pub error_line: Option<usize>,
}

/// Safety measurements for one scope of one crate.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScopeMetrics {
    // ── measurement integrity: read these first ──────────────────────────
    pub files: usize,
    pub files_unparsed: usize,
    pub files_unreadable: usize,
    /// Non-blank lines in files excluded from the measurement. Volume that is
    /// in neither the numerator nor the denominator.
    pub unparsed_code_lines: usize,
    pub unparsed: Vec<UnparsedFile>,
    /// True iff any file was excluded, so every derived fraction below is over
    /// a subset of the crate.
    pub partial: bool,

    // ── unsafety ─────────────────────────────────────────────────────────
    /// Lines carrying at least one token, over parsed files only. The
    /// denominator, sharing one source of truth with the numerator: comments
    /// and blank lines are excluded by construction rather than by a heuristic.
    pub code_lines: usize,
    /// Union of all unsafe regions, intersected with `code_lines`. Set
    /// semantics mean an `unsafe {}` nested inside an `unsafe fn` adds nothing.
    pub unsafe_code_lines: usize,
    /// Sub-union from `unsafe fn` only. NOT a partition of `unsafe_code_lines`.
    pub unsafe_fn_lines: usize,
    /// Sub-union from `unsafe {}` only. NOT a partition of `unsafe_code_lines`.
    pub unsafe_block_lines: usize,
    pub unsafe_fns: usize,
    pub unsafe_blocks: usize,
    pub unsafe_impls: usize,
    pub unsafe_traits: usize,

    // ── the exported surface: the right denominator for a cdylib ─────────
    // Every harvest-bench output is crate-type = ["cdylib"], where a `pub fn`
    // with no export attribute exports nothing at all.
    pub ffi_exported_fns: usize,
    pub ffi_exported_unsafe: usize,
    /// Alias-resolved. THE metric that separates "safe API over unsafe
    /// internals" from "unsafe API".
    pub ffi_exported_raw_ptr: usize,
    /// Same count before alias resolution, so its contribution is auditable.
    pub ffi_exported_raw_ptr_direct: usize,
    pub ffi_exported_statics: usize,
    /// An exported `static mut` is API-visible unsoundness.
    pub ffi_exported_statics_mut: usize,
    pub export_attr_plain: usize,
    pub export_attr_unsafe_wrapped: usize,

    // ── raw pointers by type position, never by text ─────────────────────
    /// Declarations: parameters, returns, fields, statics, consts, locals.
    pub ptr_decl: PtrCounts,
    /// `expr as *mut T`. Kept separate because casts would otherwise swallow
    /// the declaration metric (on real output zstd has 2992 casts to 4138
    /// declarations).
    pub ptr_cast: PtrCounts,
    /// Inside `extern "C" { … }`: C's own declarations, not the model's
    /// choices, so not evidence about the translation.
    pub ptr_extern: PtrCounts,
    /// How many crate-local aliases resolved to a pointer type.
    pub ptr_alias_types: usize,

    // ── other places unsoundness hides ───────────────────────────────────
    pub transmutes: usize,
    pub static_mut_items: usize,
    /// From `extern` blocks. Without this libpng reports zero mutable statics
    /// while declaring `static mut STDERR: *mut FILE;`.
    pub extern_static_mut_items: usize,
    pub extern_blocks: usize,
    pub extern_fn_decls: usize,

    // ── the AST blind spot, quantified per result rather than asserted ───
    pub macro_rules_defs: usize,
    pub macro_body_unsafe_tokens: usize,
    pub macro_body_ptr_tokens: usize,
}

impl ScopeMetrics {
    /// Share of code lines inside an unsafe region, as a percentage.
    pub fn unsafe_line_pct(&self) -> f64 {
        if self.code_lines == 0 {
            0.0
        } else {
            self.unsafe_code_lines as f64 / self.code_lines as f64 * 100.0
        }
    }

    /// Share of the exported surface whose signature exposes a raw pointer.
    pub fn exported_raw_ptr_pct(&self) -> f64 {
        if self.ffi_exported_fns == 0 {
            0.0
        } else {
            self.ffi_exported_raw_ptr as f64 / self.ffi_exported_fns as f64 * 100.0
        }
    }

    pub fn raw_ptrs_total(&self) -> usize {
        self.ptr_decl.total() + self.ptr_cast.total() + self.ptr_extern.total()
    }

    fn merge(&mut self, other: &FileMetrics) {
        self.files += 1;
        self.code_lines += other.code_lines;
        self.unsafe_code_lines += other.unsafe_lines.len();
        self.unsafe_fn_lines += other.unsafe_fn_lines.len();
        self.unsafe_block_lines += other.unsafe_block_lines.len();
        self.unsafe_fns += other.m.unsafe_fns;
        self.unsafe_blocks += other.m.unsafe_blocks;
        self.unsafe_impls += other.m.unsafe_impls;
        self.unsafe_traits += other.m.unsafe_traits;
        self.ffi_exported_fns += other.m.ffi_exported_fns;
        self.ffi_exported_unsafe += other.m.ffi_exported_unsafe;
        self.ffi_exported_raw_ptr += other.m.ffi_exported_raw_ptr;
        self.ffi_exported_raw_ptr_direct += other.m.ffi_exported_raw_ptr_direct;
        self.ffi_exported_statics += other.m.ffi_exported_statics;
        self.ffi_exported_statics_mut += other.m.ffi_exported_statics_mut;
        self.export_attr_plain += other.m.export_attr_plain;
        self.export_attr_unsafe_wrapped += other.m.export_attr_unsafe_wrapped;
        add_ptr(&mut self.ptr_decl, &other.m.ptr_decl);
        add_ptr(&mut self.ptr_cast, &other.m.ptr_cast);
        add_ptr(&mut self.ptr_extern, &other.m.ptr_extern);
        self.transmutes += other.m.transmutes;
        self.static_mut_items += other.m.static_mut_items;
        self.extern_static_mut_items += other.m.extern_static_mut_items;
        self.extern_blocks += other.m.extern_blocks;
        self.extern_fn_decls += other.m.extern_fn_decls;
        self.macro_rules_defs += other.m.macro_rules_defs;
        self.macro_body_unsafe_tokens += other.m.macro_body_unsafe_tokens;
        self.macro_body_ptr_tokens += other.m.macro_body_ptr_tokens;
    }
}

fn add_ptr(dst: &mut PtrCounts, src: &PtrCounts) {
    dst.direct_const += src.direct_const;
    dst.direct_mut += src.direct_mut;
    dst.alias_const += src.alias_const;
    dst.alias_mut += src.alias_mut;
}

/// Line counts for one language's sources.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineCounts {
    pub files: usize,
    pub total_lines: usize,
    /// Lines that are neither blank nor pure comment.
    pub code_lines: usize,
}

/// The C side of the comparison.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CSourceMetrics {
    pub c: LineCounts,
    pub h: LineCounts,
    /// Source in other languages, seen and reported but not counted (libsodium
    /// ships `.S` assembly). Reported so its volume is not silently invisible.
    pub other_source: LineCounts,
}

/// Everything measured about one translated program.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SafetyMetrics {
    pub schema_version: u32,
    /// `src/**` + `build.rs`: the translation. The headline scope.
    pub library: ScopeMetrics,
    /// Agent-written tests and benches, measured separately on purpose.
    pub harness: ScopeMetrics,
    /// Rust outside both, if any. A non-empty `files` here means the layout
    /// changed and the scoping rule needs revisiting.
    pub other: ScopeMetrics,
    pub c_source: CSourceMetrics,
}

const SCHEMA_VERSION: u32 = 1;

impl SafetyMetrics {
    /// Rust library code lines per C `.c` code line.
    ///
    /// `.h` is deliberately excluded: headers are 10% of one project's C side
    /// and 38% of another's, so including them deflates projects by different
    /// amounts and breaks the cross-project comparison this ratio exists for.
    /// Every component is stored, so any other rule is recomputable offline.
    pub fn rust_c_ratio(&self) -> Option<f64> {
        if self.c_source.c.code_lines == 0 {
            None
        } else {
            Some(self.library.code_lines as f64 / self.c_source.c.code_lines as f64)
        }
    }
}

// ── Per-file analysis ──────────────────────────────────────────────────

/// Raw per-file counters, before line-set flattening.
#[derive(Debug, Default)]
struct RawCounts {
    unsafe_fns: usize,
    unsafe_blocks: usize,
    unsafe_impls: usize,
    unsafe_traits: usize,
    ffi_exported_fns: usize,
    ffi_exported_unsafe: usize,
    ffi_exported_raw_ptr: usize,
    ffi_exported_raw_ptr_direct: usize,
    ffi_exported_statics: usize,
    ffi_exported_statics_mut: usize,
    export_attr_plain: usize,
    export_attr_unsafe_wrapped: usize,
    ptr_decl: PtrCounts,
    ptr_cast: PtrCounts,
    ptr_extern: PtrCounts,
    transmutes: usize,
    static_mut_items: usize,
    extern_static_mut_items: usize,
    extern_blocks: usize,
    extern_fn_decls: usize,
    macro_rules_defs: usize,
    macro_body_unsafe_tokens: usize,
    macro_body_ptr_tokens: usize,
}

/// One analysed file.
#[derive(Debug, Default)]
struct FileMetrics {
    code_lines: usize,
    unsafe_lines: BTreeSet<usize>,
    unsafe_fn_lines: BTreeSet<usize>,
    unsafe_block_lines: BTreeSet<usize>,
    m: RawCounts,
}

/// Pointer counts a crate-local `type` alias expands to.
type AliasMap = HashMap<String, (usize, usize)>;

/// Does this attribute list export the item to the linker?
///
/// Both spellings must be matched; see rule 2 in the module docs.
fn export_attrs(attrs: &[syn::Attribute]) -> (bool, usize, usize) {
    let mut plain = 0usize;
    let mut wrapped = 0usize;
    for attr in attrs {
        let path = attr.path();
        if path.is_ident("no_mangle") || path.is_ident("export_name") {
            plain += 1;
        } else if path.is_ident("unsafe") {
            // `#[unsafe(no_mangle)]` / `#[unsafe(export_name = "…")]`
            let mut found = false;
            let _ = attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("no_mangle") || meta.path.is_ident("export_name") {
                    found = true;
                    // Consume `= "…"` when present so parsing succeeds.
                    if meta.input.peek(syn::Token![=]) {
                        let _: syn::Result<syn::Expr> = meta.value().and_then(|v| v.parse());
                    }
                }
                Ok(())
            });
            if found {
                wrapped += 1;
            }
        }
    }
    (plain + wrapped > 0, plain, wrapped)
}

/// Counts every raw pointer reachable in `ty`, including through crate-local
/// aliases. Returns `(direct_const, direct_mut, alias_const, alias_mut)`.
///
/// Recursion covers the nesting the issue requires: `Option<*mut T>`,
/// `[*const u8; 4]`, `*mut *mut c_char`, `fn(*mut c_void) -> *const c_char`,
/// tuples, and references to any of those.
fn count_ptrs_in_type(ty: &syn::Type, aliases: &AliasMap) -> (usize, usize, usize, usize) {
    let mut d = (0usize, 0usize);
    let mut a = (0usize, 0usize);
    walk_type(ty, aliases, &mut d, &mut a);
    (d.0, d.1, a.0, a.1)
}

fn walk_type(
    ty: &syn::Type,
    aliases: &AliasMap,
    direct: &mut (usize, usize),
    alias: &mut (usize, usize),
) {
    match ty {
        syn::Type::Ptr(p) => {
            if p.mutability.is_some() {
                direct.1 += 1;
            } else {
                direct.0 += 1;
            }
            walk_type(&p.elem, aliases, direct, alias);
        }
        syn::Type::Path(p) => {
            // Alias resolution: matched on the LAST path segment, so a
            // same-named alias in another module over-counts and an alias
            // imported from another crate stays unresolved. `ptr_alias_types`
            // and the `_direct` counters make that visible.
            if let Some(seg) = p.path.segments.last() {
                let name = seg.ident.to_string();
                if let Some((c, m)) = aliases.get(&name) {
                    alias.0 += *c;
                    alias.1 += *m;
                }
                if let syn::PathArguments::AngleBracketed(args) = &seg.arguments {
                    for arg in &args.args {
                        if let syn::GenericArgument::Type(t) = arg {
                            walk_type(t, aliases, direct, alias);
                        }
                    }
                }
            }
            if let Some(q) = &p.qself {
                walk_type(&q.ty, aliases, direct, alias);
            }
        }
        syn::Type::Reference(r) => walk_type(&r.elem, aliases, direct, alias),
        syn::Type::Slice(s) => walk_type(&s.elem, aliases, direct, alias),
        syn::Type::Array(a2) => walk_type(&a2.elem, aliases, direct, alias),
        syn::Type::Paren(p) => walk_type(&p.elem, aliases, direct, alias),
        syn::Type::Group(g) => walk_type(&g.elem, aliases, direct, alias),
        syn::Type::Tuple(t) => {
            for e in &t.elems {
                walk_type(e, aliases, direct, alias);
            }
        }
        syn::Type::BareFn(f) => {
            for input in &f.inputs {
                walk_type(&input.ty, aliases, direct, alias);
            }
            if let syn::ReturnType::Type(_, t) = &f.output {
                walk_type(t, aliases, direct, alias);
            }
        }
        _ => {}
    }
}

/// Whether a signature exposes a raw pointer, with and without aliases.
fn sig_exposes_ptr(sig: &syn::Signature, aliases: &AliasMap) -> (bool, bool) {
    let empty = AliasMap::new();
    let mut any_direct = false;
    let mut any_resolved = false;
    let mut check = |ty: &syn::Type| {
        let (dc, dm, ac, am) = count_ptrs_in_type(ty, aliases);
        if dc + dm > 0 {
            any_direct = true;
        }
        if dc + dm + ac + am > 0 {
            any_resolved = true;
        }
        let (nc, nm, _, _) = count_ptrs_in_type(ty, &empty);
        if nc + nm > 0 {
            any_direct = true;
        }
    };
    for input in &sig.inputs {
        if let syn::FnArg::Typed(t) = input {
            check(&t.ty);
        }
    }
    if let syn::ReturnType::Type(_, t) = &sig.output {
        check(t);
    }
    (any_resolved, any_direct)
}

/// Collect `type NAME = …;` definitions that expand to a pointer count.
///
/// Two phases: gather each alias's own direct pointers and the alias names its
/// right-hand side mentions, then iterate to a fixpoint so a chain
/// `type A = B; type B = *mut T;` resolves. Bounded, so a cyclic definition
/// cannot loop forever.
fn collect_pointer_aliases(files: &[syn::File]) -> AliasMap {
    struct Def {
        direct: (usize, usize),
        refs: Vec<String>,
    }
    let mut defs: HashMap<String, Def> = HashMap::new();

    fn gather(ty: &syn::Type, direct: &mut (usize, usize), refs: &mut Vec<String>) {
        match ty {
            syn::Type::Ptr(p) => {
                if p.mutability.is_some() {
                    direct.1 += 1;
                } else {
                    direct.0 += 1;
                }
                gather(&p.elem, direct, refs);
            }
            syn::Type::Path(p) => {
                if let Some(seg) = p.path.segments.last() {
                    refs.push(seg.ident.to_string());
                    if let syn::PathArguments::AngleBracketed(args) = &seg.arguments {
                        for arg in &args.args {
                            if let syn::GenericArgument::Type(t) = arg {
                                gather(t, direct, refs);
                            }
                        }
                    }
                }
            }
            syn::Type::Reference(r) => gather(&r.elem, direct, refs),
            syn::Type::Slice(s) => gather(&s.elem, direct, refs),
            syn::Type::Array(a) => gather(&a.elem, direct, refs),
            syn::Type::Paren(p) => gather(&p.elem, direct, refs),
            syn::Type::Group(g) => gather(&g.elem, direct, refs),
            syn::Type::Tuple(t) => {
                for e in &t.elems {
                    gather(e, direct, refs);
                }
            }
            syn::Type::BareFn(f) => {
                for i in &f.inputs {
                    gather(&i.ty, direct, refs);
                }
                if let syn::ReturnType::Type(_, t) = &f.output {
                    gather(t, direct, refs);
                }
            }
            _ => {}
        }
    }

    struct AliasCollector<'a> {
        defs: &'a mut HashMap<String, Def>,
    }
    impl<'ast> Visit<'ast> for AliasCollector<'_> {
        fn visit_item_type(&mut self, i: &'ast syn::ItemType) {
            let mut direct = (0usize, 0usize);
            let mut refs = Vec::new();
            gather(&i.ty, &mut direct, &mut refs);
            self.defs.insert(i.ident.to_string(), Def { direct, refs });
            syn::visit::visit_item_type(self, i);
        }
    }

    for f in files {
        AliasCollector { defs: &mut defs }.visit_file(f);
    }

    // Fixpoint. Bounded because a cycle would otherwise never settle.
    let mut resolved: AliasMap = defs.iter().map(|(k, d)| (k.clone(), d.direct)).collect();
    for _ in 0..16 {
        let mut changed = false;
        let snapshot = resolved.clone();
        for (name, def) in &defs {
            let mut c = def.direct.0;
            let mut m = def.direct.1;
            for r in &def.refs {
                if r == name {
                    continue; // self-reference: ignore rather than diverge
                }
                if let Some((rc, rm)) = snapshot.get(r) {
                    c += rc;
                    m += rm;
                }
            }
            let entry = resolved.get_mut(name).expect("seeded above");
            if *entry != (c, m) {
                *entry = (c, m);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    // Only aliases that actually reach a pointer are interesting.
    resolved.retain(|_, (c, m)| *c + *m > 0);
    resolved
}

/// The visitor that does the counting.
struct Counter<'a> {
    aliases: &'a AliasMap,
    out: FileMetrics,
    /// Depth of enclosing `extern` blocks, so C's own declarations land in
    /// their own bucket instead of being attributed to the model.
    extern_depth: usize,
}

impl<'a> Counter<'a> {
    fn new(aliases: &'a AliasMap) -> Self {
        Self {
            aliases,
            out: FileMetrics::default(),
            extern_depth: 0,
        }
    }

    fn bucket(&mut self) -> &mut PtrCounts {
        if self.extern_depth > 0 {
            &mut self.out.m.ptr_extern
        } else {
            &mut self.out.m.ptr_decl
        }
    }

    fn count_decl(&mut self, ty: &syn::Type) {
        let (dc, dm, ac, am) = count_ptrs_in_type(ty, self.aliases);
        let b = self.bucket();
        b.add_direct(dc, dm);
        b.add_alias(ac, am);
    }

    fn mark(&mut self, from: usize, to: usize, kind: UnsafeKind) {
        if from == 0 || to < from {
            return;
        }
        for line in from..=to {
            self.out.unsafe_lines.insert(line);
            match kind {
                UnsafeKind::Fn => {
                    self.out.unsafe_fn_lines.insert(line);
                }
                UnsafeKind::Block => {
                    self.out.unsafe_block_lines.insert(line);
                }
                UnsafeKind::Other => {}
            }
        }
    }
}

#[derive(Copy, Clone)]
enum UnsafeKind {
    Fn,
    Block,
    Other,
}

impl<'ast> Visit<'ast> for Counter<'_> {
    fn visit_item_fn(&mut self, i: &'ast syn::ItemFn) {
        let (exported, plain, wrapped) = export_attrs(&i.attrs);
        self.out.m.export_attr_plain += plain;
        self.out.m.export_attr_unsafe_wrapped += wrapped;
        if exported {
            self.out.m.ffi_exported_fns += 1;
            if i.sig.unsafety.is_some() {
                self.out.m.ffi_exported_unsafe += 1;
            }
            let (resolved, direct) = sig_exposes_ptr(&i.sig, self.aliases);
            if resolved {
                self.out.m.ffi_exported_raw_ptr += 1;
            }
            if direct {
                self.out.m.ffi_exported_raw_ptr_direct += 1;
            }
        }
        if let Some(tok) = i.sig.unsafety {
            self.out.m.unsafe_fns += 1;
            // From the `unsafe` token, never the item span: the item span
            // includes outer attributes and doc comments (rule 3).
            self.mark(
                tok.span.start().line,
                i.block.brace_token.span.close().end().line,
                UnsafeKind::Fn,
            );
        }
        syn::visit::visit_item_fn(self, i);
    }

    fn visit_impl_item_fn(&mut self, i: &'ast syn::ImplItemFn) {
        // An `unsafe fn` in an impl is an ImplItemFn, not an ItemFn: without
        // this hook every method would be missed.
        if let Some(tok) = i.sig.unsafety {
            self.out.m.unsafe_fns += 1;
            self.mark(
                tok.span.start().line,
                i.block.brace_token.span.close().end().line,
                UnsafeKind::Fn,
            );
        }
        syn::visit::visit_impl_item_fn(self, i);
    }

    fn visit_trait_item_fn(&mut self, i: &'ast syn::TraitItemFn) {
        if let Some(tok) = i.sig.unsafety {
            self.out.m.unsafe_fns += 1;
            let end = match &i.default {
                Some(b) => b.brace_token.span.close().end().line,
                // No body: the signature is all there is.
                None => tok.span.end().line,
            };
            self.mark(tok.span.start().line, end, UnsafeKind::Fn);
        }
        syn::visit::visit_trait_item_fn(self, i);
    }

    fn visit_expr_unsafe(&mut self, i: &'ast syn::ExprUnsafe) {
        self.out.m.unsafe_blocks += 1;
        self.mark(
            i.unsafe_token.span.start().line,
            i.block.brace_token.span.close().end().line,
            UnsafeKind::Block,
        );
        syn::visit::visit_expr_unsafe(self, i);
    }

    fn visit_item_impl(&mut self, i: &'ast syn::ItemImpl) {
        if let Some(tok) = i.unsafety {
            self.out.m.unsafe_impls += 1;
            self.mark(
                tok.span.start().line,
                i.brace_token.span.close().end().line,
                UnsafeKind::Other,
            );
        }
        syn::visit::visit_item_impl(self, i);
    }

    fn visit_item_trait(&mut self, i: &'ast syn::ItemTrait) {
        if let Some(tok) = i.unsafety {
            self.out.m.unsafe_traits += 1;
            self.mark(
                tok.span.start().line,
                i.brace_token.span.close().end().line,
                UnsafeKind::Other,
            );
        }
        syn::visit::visit_item_trait(self, i);
    }

    fn visit_item_static(&mut self, i: &'ast syn::ItemStatic) {
        let (exported, plain, wrapped) = export_attrs(&i.attrs);
        self.out.m.export_attr_plain += plain;
        self.out.m.export_attr_unsafe_wrapped += wrapped;
        let is_mut = matches!(i.mutability, syn::StaticMutability::Mut(_));
        if is_mut {
            self.out.m.static_mut_items += 1;
        }
        if exported {
            self.out.m.ffi_exported_statics += 1;
            if is_mut {
                self.out.m.ffi_exported_statics_mut += 1;
            }
        }
        self.count_decl(&i.ty);
        syn::visit::visit_item_static(self, i);
    }

    fn visit_item_const(&mut self, i: &'ast syn::ItemConst) {
        self.count_decl(&i.ty);
        syn::visit::visit_item_const(self, i);
    }

    fn visit_item_type(&mut self, i: &'ast syn::ItemType) {
        self.count_decl(&i.ty);
        syn::visit::visit_item_type(self, i);
    }

    fn visit_field(&mut self, i: &'ast syn::Field) {
        self.count_decl(&i.ty);
        syn::visit::visit_field(self, i);
    }

    fn visit_pat_type(&mut self, i: &'ast syn::PatType) {
        // Covers both typed fn parameters and `let x: *mut T`.
        self.count_decl(&i.ty);
        syn::visit::visit_pat_type(self, i);
    }

    fn visit_return_type(&mut self, i: &'ast syn::ReturnType) {
        if let syn::ReturnType::Type(_, t) = i {
            self.count_decl(t);
        }
        syn::visit::visit_return_type(self, i);
    }

    fn visit_expr_cast(&mut self, i: &'ast syn::ExprCast) {
        let (dc, dm, ac, am) = count_ptrs_in_type(&i.ty, self.aliases);
        self.out.m.ptr_cast.add_direct(dc, dm);
        self.out.m.ptr_cast.add_alias(ac, am);
        syn::visit::visit_expr_cast(self, i);
    }

    fn visit_item_foreign_mod(&mut self, i: &'ast syn::ItemForeignMod) {
        self.out.m.extern_blocks += 1;
        self.extern_depth += 1;
        syn::visit::visit_item_foreign_mod(self, i);
        self.extern_depth -= 1;
    }

    fn visit_foreign_item_fn(&mut self, i: &'ast syn::ForeignItemFn) {
        self.out.m.extern_fn_decls += 1;
        syn::visit::visit_foreign_item_fn(self, i);
    }

    fn visit_foreign_item_static(&mut self, i: &'ast syn::ForeignItemStatic) {
        if matches!(i.mutability, syn::StaticMutability::Mut(_)) {
            self.out.m.extern_static_mut_items += 1;
        }
        self.count_decl(&i.ty);
        syn::visit::visit_foreign_item_static(self, i);
    }

    fn visit_expr_call(&mut self, i: &'ast syn::ExprCall) {
        if let syn::Expr::Path(p) = &*i.func {
            if let Some(seg) = p.path.segments.last() {
                if seg.ident.to_string().starts_with("transmute") {
                    self.out.m.transmutes += 1;
                }
            }
        }
        syn::visit::visit_expr_call(self, i);
    }

    fn visit_item_macro(&mut self, i: &'ast syn::ItemMacro) {
        // `macro_rules!` bodies are token soup to the AST, so the blind spot is
        // quantified rather than asserted away: count `unsafe` idents and `*`
        // puncts in the body so a reader can see how much was not analysed.
        if i.mac.path.is_ident("macro_rules") {
            self.out.m.macro_rules_defs += 1;
            for tt in i.mac.tokens.clone() {
                count_macro_tokens(&tt, &mut self.out.m);
            }
        }
        syn::visit::visit_item_macro(self, i);
    }
}

fn count_macro_tokens(tt: &proc_macro2::TokenTree, m: &mut RawCounts) {
    match tt {
        proc_macro2::TokenTree::Ident(id) => {
            if id == "unsafe" {
                m.macro_body_unsafe_tokens += 1;
            }
        }
        proc_macro2::TokenTree::Punct(p) => {
            if p.as_char() == '*' {
                m.macro_body_ptr_tokens += 1;
            }
        }
        proc_macro2::TokenTree::Group(g) => {
            for inner in g.stream() {
                count_macro_tokens(&inner, m);
            }
        }
        proc_macro2::TokenTree::Literal(_) => {}
    }
}

/// Lines carrying at least one token.
///
/// This is the denominator, and it comes from the same token stream the AST was
/// parsed from, so numerator and denominator agree on one source map. Blank
/// lines and comments are excluded by construction rather than by a heuristic.
fn token_lines(stream: proc_macro2::TokenStream, out: &mut BTreeSet<usize>) {
    for tt in stream {
        match tt {
            proc_macro2::TokenTree::Group(g) => {
                let open = g.span_open().start().line;
                let close = g.span_close().end().line;
                if open > 0 {
                    out.insert(open);
                }
                if close > 0 {
                    out.insert(close);
                }
                token_lines(g.stream(), out);
            }
            other => {
                let s = other.span();
                let (a, b) = (s.start().line, s.end().line);
                if a > 0 {
                    for line in a..=b.max(a) {
                        out.insert(line);
                    }
                }
            }
        }
    }
}

/// Parse one file's source into a token stream and an AST that share a source map.
fn parse_source(src: &str) -> Result<(proc_macro2::TokenStream, syn::File), String> {
    let stream = proc_macro2::TokenStream::from_str(src).map_err(|e| e.to_string())?;
    let file = syn::parse2::<syn::File>(stream.clone()).map_err(|e| e.to_string())?;
    Ok((stream, file))
}

/// Analyse one already-parsed file against a resolved alias map.
fn analyze_parsed(
    stream: proc_macro2::TokenStream,
    file: &syn::File,
    aliases: &AliasMap,
) -> FileMetrics {
    let mut counter = Counter::new(aliases);
    counter.visit_file(file);
    let mut lines = BTreeSet::new();
    token_lines(stream, &mut lines);
    let mut out = counter.out;
    out.code_lines = lines.len();
    // Intersect: an unsafe span covering a blank or comment-only line must not
    // inflate the numerator past the denominator.
    out.unsafe_lines.retain(|l| lines.contains(l));
    out.unsafe_fn_lines.retain(|l| lines.contains(l));
    out.unsafe_block_lines.retain(|l| lines.contains(l));
    out
}

/// Analyse a single source string, resolving aliases within that string only.
/// Test-only: real measurement resolves aliases across a whole scope, because a
/// `type` alias is routinely declared in a different file from its uses.
#[cfg(test)]
fn analyze_source(src: &str) -> Result<FileMetrics, String> {
    let (stream, file) = parse_source(src)?;
    let aliases = collect_pointer_aliases(std::slice::from_ref(&file));
    Ok(analyze_parsed(stream, &file, &aliases))
}

// ── Crate walking ──────────────────────────────────────────────────────

/// Which scope a path within the crate belongs to, or `None` to skip it.
fn classify(rel: &Path) -> Option<Scope> {
    let mut comps = rel.components().map(|c| c.as_os_str());
    let first = comps.next()?;
    let name = first.to_string_lossy();
    // Framework-owned, build output, and held-out external suite material.
    if name == "target" || name.starts_with('.') || stage_manifest::is_reserved_toplevel(first) {
        return None;
    }
    if rel.components().any(|c| {
        let s = c.as_os_str().to_string_lossy();
        s == "target" || s.starts_with('.')
    }) {
        return None;
    }
    Some(match name.as_ref() {
        "src" => Scope::Library,
        "build.rs" => Scope::Library,
        "tests" | "benches" | "examples" | "fuzz" | "xtask" => Scope::Harness,
        _ => Scope::Other,
    })
}

/// C-side line classifier: is this line anything but blank or comment?
///
/// Tracks block-comment depth across lines, and string/char literals so that
/// `"/* not a comment */"` cannot open a phantom comment. Deterministic, so it
/// is identical for every model and prompt variant.
fn c_code_lines(src: &str) -> usize {
    let mut in_block = false;
    let mut count = 0usize;
    for line in src.lines() {
        let bytes: Vec<char> = line.chars().collect();
        let mut i = 0usize;
        let mut has_code = false;
        let mut in_str: Option<char> = None;
        while i < bytes.len() {
            let c = bytes[i];
            let next = bytes.get(i + 1).copied();
            if in_block {
                if c == '*' && next == Some('/') {
                    in_block = false;
                    i += 2;
                    continue;
                }
                i += 1;
                continue;
            }
            if let Some(q) = in_str {
                if c == '\\' {
                    i += 2;
                    continue;
                }
                if c == q {
                    in_str = None;
                }
                has_code = true;
                i += 1;
                continue;
            }
            if c == '/' && next == Some('*') {
                in_block = true;
                i += 2;
                continue;
            }
            if c == '/' && next == Some('/') {
                break; // rest of line is a comment
            }
            if c == '"' || c == '\'' {
                in_str = Some(c);
                has_code = true;
                i += 1;
                continue;
            }
            if !c.is_whitespace() {
                has_code = true;
            }
            i += 1;
        }
        if has_code {
            count += 1;
        }
    }
    count
}

fn add_lines(dst: &mut LineCounts, src: &str, code: usize) {
    dst.files += 1;
    dst.total_lines += src.lines().count();
    dst.code_lines += code;
}

/// Measure the C source the translation was made from.
fn measure_c_source(c_root: &Path) -> CSourceMetrics {
    let mut out = CSourceMetrics::default();
    if !c_root.is_dir() {
        return out;
    }
    // Build trees and vendored deps would swamp the real source.
    const SKIP: &[&str] = &[
        "target",
        "CMakeFiles",
        "_deps",
        "gtest_build",
        "verify_env",
        "build",
        "build-test",
        "build-fuzz",
    ];
    for entry in walkdir::WalkDir::new(c_root)
        .into_iter()
        .filter_entry(|e| {
            let n = e.file_name().to_string_lossy();
            !(n.starts_with('.') || SKIP.contains(&n.as_ref()))
        })
        .filter_map(Result::ok)
    {
        if !entry.file_type().is_file() {
            continue;
        }
        let ext = match entry.path().extension().and_then(|e| e.to_str()) {
            Some(e) => e.to_ascii_lowercase(),
            None => continue,
        };
        let Ok(src) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        match ext.as_str() {
            "c" => {
                let n = c_code_lines(&src);
                add_lines(&mut out.c, &src, n);
            }
            "h" => {
                let n = c_code_lines(&src);
                add_lines(&mut out.h, &src, n);
            }
            // Seen and reported, not counted: their volume must not be
            // invisible, but they are not C.
            "s" | "inc" | "cc" | "cpp" | "cxx" | "hpp" | "in" | "am" => {
                add_lines(&mut out.other_source, &src, 0);
            }
            _ => {}
        }
    }
    out
}

/// Measure one translated program: its Rust, split by scope, and its C input.
///
/// Never fails the caller. An unreadable or unparseable file is recorded and
/// excluded from both numerator and denominator; a metrics pass must not lose a
/// translation that cost hours of agent time.
pub fn measure(crate_dir: &Path, c_source_dir: &Path) -> SafetyMetrics {
    let mut out = SafetyMetrics {
        schema_version: SCHEMA_VERSION,
        ..Default::default()
    };

    // Gather sources per scope first: alias resolution needs every file of a
    // scope before any file can be counted.
    let mut by_scope: HashMap<&'static str, Vec<(String, String)>> = HashMap::new();
    let mut unreadable: Vec<(Scope, String)> = Vec::new();
    for entry in walkdir::WalkDir::new(crate_dir)
        .into_iter()
        .filter_entry(|e| {
            let n = e.file_name().to_string_lossy();
            !(n == "target" || (n.starts_with('.') && n != "."))
        })
        .filter_map(Result::ok)
    {
        if !entry.file_type().is_file() {
            continue;
        }
        if entry.path().extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let Ok(rel) = entry.path().strip_prefix(crate_dir) else {
            continue;
        };
        let Some(scope) = classify(rel) else { continue };
        let key = match scope {
            Scope::Library => "library",
            Scope::Harness => "harness",
            Scope::Other => "other",
        };
        match std::fs::read_to_string(entry.path()) {
            Ok(src) => by_scope
                .entry(key)
                .or_default()
                .push((rel.display().to_string(), src)),
            Err(_) => unreadable.push((scope, rel.display().to_string())),
        }
    }

    for (key, files) in &by_scope {
        let metrics = measure_scope(files);
        match *key {
            "library" => out.library = metrics,
            "harness" => out.harness = metrics,
            _ => out.other = metrics,
        }
    }
    for (scope, path) in unreadable {
        let m = match scope {
            Scope::Library => &mut out.library,
            Scope::Harness => &mut out.harness,
            Scope::Other => &mut out.other,
        };
        m.files_unreadable += 1;
        m.partial = true;
        m.unparsed.push(UnparsedFile {
            path,
            error: "file is not valid UTF-8 or could not be read".to_owned(),
            error_line: None,
        });
    }

    out.c_source = measure_c_source(c_source_dir);
    out
}

fn measure_scope(files: &[(String, String)]) -> ScopeMetrics {
    let mut out = ScopeMetrics::default();

    // Parse everything once, keeping the token stream alongside the AST.
    let mut parsed: Vec<(String, proc_macro2::TokenStream, syn::File)> = Vec::new();
    for (path, src) in files {
        match parse_source(src) {
            Ok((stream, file)) => parsed.push((path.clone(), stream, file)),
            Err(error) => {
                // No silent zeros: excluded from BOTH sides, and its volume
                // reported so the exclusion is visible.
                out.files_unparsed += 1;
                out.partial = true;
                out.unparsed_code_lines += src.lines().filter(|l| !l.trim().is_empty()).count();
                out.unparsed.push(UnparsedFile {
                    path: path.clone(),
                    error,
                    error_line: None,
                });
            }
        }
    }

    let asts: Vec<syn::File> = parsed.iter().map(|(_, _, f)| f.clone()).collect();
    let aliases = collect_pointer_aliases(&asts);
    out.ptr_alias_types = aliases.len();

    for (_, stream, file) in parsed {
        let fm = analyze_parsed(stream, &file, &aliases);
        out.merge(&fm);
    }
    out
}

/// Write the metrics beside the crate, under the framework-owned meta dir.
///
/// A per-program file is necessary because `results.csv` is only written once,
/// after every program in a run has finished: a run interrupted midway would
/// otherwise lose the measurements for the programs that did complete.
pub fn write_metrics(program_dir: &Path, metrics: &SafetyMetrics) -> std::io::Result<PathBuf> {
    let dir = stage_manifest::meta_dir(program_dir);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("safety.json");
    let tmp = dir.join(".safety.json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(metrics)?)?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

/// Human-readable summary for the run log.
pub fn log_summary(program: &str, m: &SafetyMetrics) {
    let lib = &m.library;
    log::info!(
        "  safety[{program}]: {:.1}% of {} code lines unsafe ({} unsafe fn, {} unsafe blocks)",
        lib.unsafe_line_pct(),
        lib.code_lines,
        lib.unsafe_fns,
        lib.unsafe_blocks
    );
    log::info!(
        "  safety[{program}]: exported surface {} fns, {} unsafe, {} exposing raw pointers ({:.1}%)",
        lib.ffi_exported_fns,
        lib.ffi_exported_unsafe,
        lib.ffi_exported_raw_ptr,
        lib.exported_raw_ptr_pct()
    );
    log::info!(
        "  safety[{program}]: raw pointers {} total ({} const / {} mut) — \
         {} declared, {} in casts, {} from extern blocks; {} static mut",
        lib.raw_ptrs_total(),
        lib.ptr_decl.consts() + lib.ptr_cast.consts() + lib.ptr_extern.consts(),
        lib.ptr_decl.muts() + lib.ptr_cast.muts() + lib.ptr_extern.muts(),
        lib.ptr_decl.total(),
        lib.ptr_cast.total(),
        lib.ptr_extern.total(),
        lib.static_mut_items + lib.extern_static_mut_items
    );
    if let Some(r) = m.rust_c_ratio() {
        log::info!(
            "  safety[{program}]: {} Rust lines / {} C lines = {r:.2}",
            lib.code_lines,
            m.c_source.c.code_lines
        );
    }
    if m.harness.files > 0 {
        // Reported separately so it is visible that it was excluded, rather
        // than looking like it was forgotten.
        log::info!(
            "  safety[{program}]: excluded from the above — {} harness file(s), {} lines, \
             {} unsafe block(s) (agent-written tests, not a translation of the C input)",
            m.harness.files,
            m.harness.code_lines,
            m.harness.unsafe_blocks
        );
    }
    if lib.partial {
        log::warn!(
            "  safety[{program}]: PARTIAL — {} unparsed, {} unreadable, {} lines excluded; \
             every fraction above is over a subset of the crate",
            lib.files_unparsed,
            lib.files_unreadable,
            lib.unparsed_code_lines
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(src: &str) -> FileMetrics {
        analyze_source(src).expect("fixture parses")
    }

    #[test]
    fn both_export_spellings_are_recognised() {
        // Not academic: on real output mujs uses the plain form 226 times and
        // the wrapped form 0, while five other crates use the wrapped form
        // hundreds of times and the plain form 0. Knowing only one spelling
        // reports zero exported functions for six of seven crates.
        let plain = m("#[no_mangle]\npub extern \"C\" fn a() {}\n");
        assert_eq!(plain.m.ffi_exported_fns, 1);
        assert_eq!(plain.m.export_attr_plain, 1);

        let wrapped = m("#[unsafe(no_mangle)]\npub unsafe extern \"C\" fn b() {}\n");
        assert_eq!(wrapped.m.ffi_exported_fns, 1);
        assert_eq!(wrapped.m.export_attr_unsafe_wrapped, 1);
        assert_eq!(wrapped.m.ffi_exported_unsafe, 1);

        let named = m("#[unsafe(export_name = \"c_name\")]\npub extern \"C\" fn c() {}\n");
        assert_eq!(named.m.ffi_exported_fns, 1);

        // A plain `pub fn` exports nothing from a cdylib.
        let not_exported = m("pub fn d() {}\n");
        assert_eq!(not_exported.m.ffi_exported_fns, 0);
    }

    #[test]
    fn raw_pointers_are_found_through_type_aliases() {
        // THE most important rule. On real output this moves libpng's
        // pointer-exposing exports from 47 to 375 of 381.
        let src = "\
pub type png_byte = u8;
pub type png_bytep = *mut png_byte;
#[unsafe(no_mangle)]
pub extern \"C\" fn f(p: png_bytep) {}
";
        let r = m(src);
        assert_eq!(r.m.ffi_exported_fns, 1);
        assert_eq!(
            r.m.ffi_exported_raw_ptr, 1,
            "alias-resolved signature must count as pointer-exposing"
        );
        assert_eq!(
            r.m.ffi_exported_raw_ptr_direct, 0,
            "and the direct count must show the alias pass is what found it"
        );
        assert!(r.m.ptr_decl.alias_mut >= 1);
    }

    #[test]
    fn alias_chains_resolve_to_a_fixpoint() {
        let src = "\
pub type A = *const u8;
pub type B = A;
pub type C = B;
#[no_mangle]
pub extern \"C\" fn f(p: C) {}
";
        let r = m(src);
        assert_eq!(r.m.ffi_exported_raw_ptr, 1);
    }

    #[test]
    fn a_cyclic_alias_does_not_hang() {
        // Not valid Rust semantically, but it parses, and a fixpoint without a
        // bound would spin forever on it.
        let src = "pub type A = B;\npub type B = A;\npub fn f() {}\n";
        let r = m(src);
        assert_eq!(r.m.ffi_exported_fns, 0);
    }

    #[test]
    fn pointers_are_counted_in_nested_type_positions() {
        // A text search would miss or miscount every one of these.
        let src = "\
pub struct S { pub p: Option<*mut u8>, pub arr: [*const u8; 4] }
pub type Cb = Option<unsafe extern \"C\" fn(*mut core::ffi::c_void) -> *const u8>;
pub fn f(pp: *mut *mut u8) -> (*const u8, u32) { (core::ptr::null(), 0) }
";
        let r = m(src);
        // Option<*mut u8>, [*const u8; 4], fn(*mut c_void) -> *const u8,
        // *mut *mut u8 (2 levels), and the tuple return's *const u8.
        assert!(
            r.m.ptr_decl.total() >= 7,
            "expected nested pointers to be found, got {:?}",
            r.m.ptr_decl
        );
        assert!(r.m.ptr_decl.muts() >= 3);
        assert!(r.m.ptr_decl.consts() >= 3);
    }

    #[test]
    fn casts_and_extern_declarations_are_separate_buckets() {
        // Casts would otherwise swallow the declaration metric (real output:
        // zstd has 2992 casts against 4138 declarations), and an `extern` block
        // is C's own declaration, not a choice the model made.
        let src = "\
unsafe extern \"C\" { pub static mut STDERR: *mut u8; pub fn c_fn(p: *const u8); }
pub fn f(x: usize) -> *mut u8 { x as *mut u8 }
";
        let r = m(src);
        assert!(r.m.ptr_cast.total() >= 1, "cast bucket: {:?}", r.m.ptr_cast);
        assert!(
            r.m.ptr_extern.total() >= 2,
            "extern bucket: {:?}",
            r.m.ptr_extern
        );
        assert_eq!(r.m.extern_blocks, 1);
        assert_eq!(r.m.extern_fn_decls, 1);
        // Without a ForeignItemStatic hook this reads 0 while the code plainly
        // declares a mutable static.
        assert_eq!(r.m.extern_static_mut_items, 1);
    }

    #[test]
    fn unsafe_lines_are_a_union_so_nesting_never_double_counts() {
        let src = "\
pub unsafe fn f() {
    unsafe { let _ = 1; }
    unsafe { let _ = 2; }
}
";
        let r = m(src);
        assert_eq!(r.m.unsafe_fns, 1);
        assert_eq!(r.m.unsafe_blocks, 2);
        // Four lines of function, counted once each despite two nested blocks.
        assert_eq!(r.unsafe_lines.len(), 4);
        assert!(r.unsafe_lines.len() <= r.code_lines);
    }

    #[test]
    fn doc_comments_and_attributes_do_not_inflate_the_unsafe_region() {
        // Using the item span would start the region at the first doc line,
        // biasing the metric against whichever model comments its unsafe code
        // more heavily.
        let src = "\
/// one
/// two
#[unsafe(no_mangle)]
pub unsafe extern \"C\" fn g() { let _ = 1; }
";
        let r = m(src);
        assert_eq!(r.m.unsafe_fns, 1);
        assert_eq!(
            r.unsafe_lines.iter().copied().min(),
            Some(4),
            "region must start at the `unsafe` token, not the doc comment"
        );
        assert_eq!(r.unsafe_lines.len(), 1);
    }

    #[test]
    fn unsafe_methods_in_impls_and_traits_are_counted() {
        // An `unsafe fn` in an impl is an ImplItemFn, not an ItemFn; without
        // those hooks every method is missed.
        let src = "\
pub struct S;
impl S { pub unsafe fn m(&self) {} }
pub trait T { unsafe fn t(&self); }
unsafe impl Send for S {}
";
        let r = m(src);
        assert_eq!(r.m.unsafe_fns, 2, "impl method + trait method");
        assert_eq!(r.m.unsafe_impls, 1);
    }

    #[test]
    fn the_denominator_excludes_comments_and_blank_lines() {
        let src = "\
// a comment

/* block
   comment */
pub fn f() {}

";
        let r = m(src);
        // Only `pub fn f() {}` carries tokens.
        assert_eq!(r.code_lines, 1);
    }

    #[test]
    fn static_mut_and_transmute_are_counted() {
        let src = "\
pub static mut COUNTER: u32 = 0;
pub fn f(x: u32) -> i32 { unsafe { core::mem::transmute(x) } }
";
        let r = m(src);
        assert_eq!(r.m.static_mut_items, 1);
        assert_eq!(r.m.transmutes, 1);
    }

    #[test]
    fn an_exported_mutable_static_is_flagged() {
        let src = "#[unsafe(no_mangle)]\npub static mut G: u32 = 0;\n";
        let r = m(src);
        assert_eq!(r.m.ffi_exported_statics, 1);
        assert_eq!(r.m.ffi_exported_statics_mut, 1);
    }

    #[test]
    fn an_unparseable_file_is_excluded_from_both_sides_not_treated_as_safe() {
        // A real truncated pcre2_substitute.rs exists in an actual results
        // tree. Counting it as zero unsafe while still counting its lines would
        // make the crate look safer than it is.
        let files = vec![
            (
                "src/good.rs".to_owned(),
                "pub unsafe fn a() { let _ = 1; }\n".to_owned(),
            ),
            (
                "src/truncated.rs".to_owned(),
                "pub unsafe fn b() { let _ = 1;\n".to_owned(),
            ),
        ];
        let s = measure_scope(&files);
        assert_eq!(s.files, 1, "only the parseable file is measured");
        assert_eq!(s.files_unparsed, 1);
        assert!(s.partial, "the result must announce it is over a subset");
        assert_eq!(s.unsafe_fns, 1);
        // Its volume is reported but in neither numerator nor denominator.
        assert!(s.unparsed_code_lines > 0);
        assert_eq!(s.code_lines, 1);
        assert_eq!(s.unparsed.len(), 1);
        assert!(s.unparsed[0].path.ends_with("truncated.rs"));
    }

    #[test]
    fn scope_classification_keeps_agent_tests_out_of_the_library() {
        // Load-bearing: on real output jansson's tests/ holds 87 unsafe blocks
        // against 1 in src/, so folding them together would swamp the figure.
        assert_eq!(classify(Path::new("src/lib.rs")), Some(Scope::Library));
        assert_eq!(classify(Path::new("src/a/b.rs")), Some(Scope::Library));
        assert_eq!(classify(Path::new("build.rs")), Some(Scope::Library));
        assert_eq!(classify(Path::new("tests/t.rs")), Some(Scope::Harness));
        assert_eq!(classify(Path::new("benches/b.rs")), Some(Scope::Harness));
        // Framework-owned, build output, and held-out suite material.
        assert_eq!(classify(Path::new("target/debug/x.rs")), None);
        assert_eq!(classify(Path::new(".harvest/c_src/x.rs")), None);
        assert_eq!(classify(Path::new("src/../target/x.rs")), None);
        assert_eq!(classify(Path::new("runner/src/main.rs")), None);
        assert_eq!(classify(Path::new("gtest_suite/x.rs")), None);
    }

    #[test]
    fn c_line_classifier_ignores_comments_but_not_strings_containing_them() {
        let src = "\
int a;            // trailing comment
/* whole line */
/* multi
   line */ int b;
const char *s = \"/* not a comment */\";
char c = '/';

";
        // int a; | `*/ int b;` | the string line | the char line = 4
        assert_eq!(c_code_lines(src), 4);
    }

    #[test]
    fn ratio_is_absent_rather_than_zero_when_there_is_no_c() {
        let mut sm = SafetyMetrics::default();
        assert_eq!(sm.rust_c_ratio(), None);
        sm.library.code_lines = 100;
        sm.c_source.c.code_lines = 200;
        assert_eq!(sm.rust_c_ratio(), Some(0.5));
    }

    /// Offline check against real translated output, for auditing the counting
    /// rules against crates nobody wrote as a fixture. Ignored by default
    /// because it needs a results tree; point it at one and run:
    ///
    /// ```text
    /// SAFETY_BASE=/path/to/results/HarvestBench/claude \
    ///   cargo test -p harvest-benchmark measure_real_output -- --ignored --nocapture
    /// ```
    ///
    /// Prints a table rather than asserting: the numbers are the finding, and
    /// pinning them would just encode one corpus's results as a test.
    #[test]
    #[ignore = "needs a results tree; set SAFETY_BASE"]
    fn measure_real_output() {
        let base = std::env::var("SAFETY_BASE").expect("set SAFETY_BASE");
        println!(
            "{:10} {:>8} {:>8} {:>6} {:>6} {:>6} {:>7} {:>7} {:>7} {:>8} {:>6}",
            "project",
            "rustLOC",
            "unsafe%",
            "ufns",
            "ublk",
            "exp",
            "expUns",
            "expPtr",
            "ptrDir",
            "cLOC",
            "ratio"
        );
        for p in [
            "lz4",
            "libsodium",
            "libpng",
            "jansson",
            "mujs",
            "pcre2",
            "zstd",
        ] {
            let dir = PathBuf::from(&base).join(p).join("verified");
            if !dir.is_dir() {
                println!("{p:10} (absent)");
                continue;
            }
            let m = measure(&dir, &dir.join("c_src"));
            let l = &m.library;
            println!(
                "{:10} {:>8} {:>7.1}% {:>6} {:>6} {:>6} {:>7} {:>7} {:>7} {:>8} {:>6}",
                p,
                l.code_lines,
                l.unsafe_line_pct(),
                l.unsafe_fns,
                l.unsafe_blocks,
                l.ffi_exported_fns,
                l.ffi_exported_unsafe,
                l.ffi_exported_raw_ptr,
                l.ffi_exported_raw_ptr_direct,
                m.c_source.c.code_lines,
                m.rust_c_ratio()
                    .map(|r| format!("{r:.2}"))
                    .unwrap_or_else(|| "-".into()),
            );
            if l.partial {
                println!(
                    "{:10}   PARTIAL: {} unparsed, {} unreadable, {} lines excluded",
                    "", l.files_unparsed, l.files_unreadable, l.unparsed_code_lines
                );
            }
            println!(
                "{:10}   harness scope: {} files, {} lines, {} unsafe blocks (kept out of the above)",
                "", m.harness.files, m.harness.code_lines, m.harness.unsafe_blocks
            );
        }
    }

    #[test]
    fn percentages_are_zero_rather_than_nan_on_empty_input() {
        let s = ScopeMetrics::default();
        assert_eq!(s.unsafe_line_pct(), 0.0);
        assert_eq!(s.exported_raw_ptr_pct(), 0.0);
        assert_eq!(s.raw_ptrs_total(), 0);
    }
}
