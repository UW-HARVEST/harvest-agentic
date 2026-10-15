//! Safety: the number of operations that require an `unsafe` context —
//! DARPA TRACTOR's safety metric — measured with the stock compiler.
//!
//! DARPA patches rustc to print every call of its unsafety checker's
//! `requires_unsafe`. Stock rustc reaches the same function and stays silent
//! only for operations inside an `unsafe { … }` block. So the package is
//! copied, every `unsafe {` keyword is blanked ([`super::strip`]), and the
//! copy is compiled: each operation then reports itself, as an E0133 error in
//! safe code or as a forced `unsafe_op_in_unsafe_fn` lint in an `unsafe fn`
//! body. Validated op-by-op against the patched rustc (see the
//! `unsafe_ops_oracle` integration test).
//!
//! The patched rustc reports every target `cargo check` compiles — the
//! library, each binary and the build script — so a file that two targets
//! compile counts once per target. Each target is measured in its own pass
//! over a copy whose compiled files are all rewritten; the targets the pass
//! does not measure (the build script, and the library under a binary)
//! compile from the untouched sources, so they cannot fail, and only the
//! measured target's diagnostics count.
//!
//! What makes the count exact rather than approximate:
//! * the extra flags reach only the measured crate (`cargo rustc`), so
//!   dependencies contribute nothing; the build script is reached through
//!   `RUSTFLAGS`, whose diagnostics in dependencies are filtered out;
//! * `-Zdeduplicate-diagnostics=no`: a macro that repeats a fragment produces
//!   identical diagnostics, which rustc would otherwise merge;
//! * `deprecated_safe_2024` is forced and counted: before edition 2024,
//!   calls such as `std::env::set_var` report that lint instead of E0133;
//! * only operations whose span lies in the package are counted, matching
//!   DARPA's filter on standard-library and registry spans;
//! * any error other than E0133 invalidates the run: a type error makes
//!   rustc skip the unsafety check of the whole function, silently.
//!
//! Operations inside compiler-generated unsafe blocks cannot be exposed this
//! way; they are estimated from source separately
//! ([`super::source::CompilerGenerated`]).

use super::scratch::{crate_name, Package, ScratchArea};
use super::source::{AnalyzedFile, CompilerGenerated, FnIndex};
use super::strip::Stripped;
use super::MeasureError;
use cargo_metadata::diagnostic::{Diagnostic, DiagnosticLevel, DiagnosticSpan};
use cargo_metadata::Target;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::process::Command;

/// How many of the most-called unsafe functions the report lists.
const TOP_CALLEES: usize = 15;

/// Diagnostic codes that each mark one unsafe operation.
const OP_CODES: &[&str] = &["E0133", "unsafe_op_in_unsafe_fn", "deprecated_safe_2024"];

/// Flags passed to the measured crate only.
const STRIP_RUSTC_FLAGS: &[&str] = &[
    "-A",
    "warnings",
    "--force-warn",
    "unsafe_op_in_unsafe_fn",
    "--force-warn",
    "deprecated_safe_2024",
    "-Zdeduplicate-diagnostics=no",
];

/// The kind of an unsafe operation, named after rustc's `UnsafeOpKind`
/// (the names DARPA reports).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum OpKind {
    CallToUnsafeFunction,
    DerefOfRawPointer,
    UseOfMutableStatic,
    UseOfExternStatic,
    AccessToUnionField,
    UseOfInlineAssembly,
    CallToFunctionWith,
    Other,
}

impl OpKind {
    /// From rustc's message text, which has been stable for years.
    fn from_message(message: &str) -> Self {
        const TABLE: &[(&str, OpKind)] = &[
            (
                "call to deprecated safe function",
                OpKind::CallToUnsafeFunction,
            ),
            ("call to unsafe function", OpKind::CallToUnsafeFunction),
            ("dereference of raw pointer", OpKind::DerefOfRawPointer),
            ("use of mutable static", OpKind::UseOfMutableStatic),
            ("use of extern static", OpKind::UseOfExternStatic),
            ("access to union field", OpKind::AccessToUnionField),
            ("use of inline assembly", OpKind::UseOfInlineAssembly),
            ("with `#[target_feature]`", OpKind::CallToFunctionWith),
        ];
        TABLE
            .iter()
            .find(|(needle, _)| message.contains(needle))
            .map_or(OpKind::Other, |(_, kind)| *kind)
    }
}

/// Where an operation sits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OpScope {
    /// In a function exported to C (`#[no_mangle]` / `#[export_name]`).
    Exported,
    /// In any other function.
    Internal,
    /// Outside function bodies: static and const initializers, and items a
    /// macro generates.
    OutsideFunction,
}

/// One unsafe operation, at the position rustc reports (1-based line and
/// character column of its span).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UnsafeOp {
    pub file: String,
    pub line: usize,
    pub column: usize,
    pub end_line: usize,
    pub end_column: usize,
    pub kind: OpKind,
    /// Called function, for calls to unsafe functions.
    pub callee: Option<String>,
    /// True when the operation is in an `unsafe fn` body (it needed no
    /// `unsafe` block), false when it is in safe code.
    pub in_unsafe_fn: bool,
    pub scope: OpScope,
    /// Innermost function containing the operation's call site.
    pub function: Option<String>,
    /// Cargo target compiled, as `kind:name` (`lib:png`, `bin:png`,
    /// `custom-build:build-script-build`): the library and a binary often
    /// share a name.
    pub target: String,
}

/// Safety summary of one package.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SafetyReport {
    /// Operations that require an `unsafe` context.
    pub unsafe_ops: usize,
    pub by_kind: BTreeMap<OpKind, usize>,
    pub in_exported_functions: usize,
    pub in_internal_functions: usize,
    pub outside_functions: usize,
    pub in_unsafe_fn_bodies: usize,
    pub in_safe_code: usize,
    pub top_callees: Vec<CalleeCount>,
    /// Operations in compiler-generated unsafe blocks, which DARPA's count
    /// includes and `unsafe_ops` cannot (an estimate from source).
    pub compiler_generated: CompilerGenerated,
    /// `unsafe { … }` blocks in the package.
    pub unsafe_blocks: usize,
    /// The operations themselves, written to their own file rather than
    /// the report.
    #[serde(skip)]
    pub ops: Vec<UnsafeOp>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CalleeCount {
    pub name: String,
    pub calls: usize,
}

/// Measures `pristine` (left unmodified: each pass rewrites its own copy).
/// `files` are the package's compiled files, analyzed from `pristine`.
pub fn measure(
    scratch: &ScratchArea,
    pristine: &Package,
    files: &[AnalyzedFile],
) -> Result<SafetyReport, MeasureError> {
    let mut rewritten = HashMap::new();
    for f in files {
        let stripped = f
            .stripped
            .as_ref()
            .map_err(|e| MeasureError::NotRun(format!("{} does not lex: {e}", f.rel_path)))?;
        rewritten.insert(f.rel_path.as_str(), stripped);
    }
    let indexes: HashMap<&str, &FnIndex> = files
        .iter()
        .map(|f| (f.rel_path.as_str(), &f.fn_index))
        .collect();

    let mut ops = Vec::new();
    if let Some(lib) = &pristine.lib {
        let copy = rewritten_copy(scratch, pristine, &rewritten, "safety_lib", false)?;
        let cmd = target_command(&copy, lib, &["--lib"]);
        ops.extend(check_target(&copy, lib, cmd, &indexes)?);
    }
    if !pristine.bins.is_empty() {
        let copy = rewritten_copy(scratch, pristine, &rewritten, "safety_bins", true)?;
        for bin in &pristine.bins {
            let cmd = target_command(&copy, bin, &["--bin", &bin.name]);
            ops.extend(check_target(&copy, bin, cmd, &indexes)?);
        }
    }
    if let Some(build) = &pristine.build_script {
        let copy = scratch.copy_package(&pristine.root, "safety_build")?;
        rewrite(&copy, &rewritten)?;
        let cmd = build_script_command(&copy);
        ops.extend(check_target(&copy, build, cmd, &indexes)?);
    }

    let unsafe_blocks = rewritten.values().map(|s| s.blocks).sum();
    let mut compiler_generated = CompilerGenerated::default();
    for f in files {
        compiler_generated.add(&f.compiler_generated);
    }
    Ok(summarize(ops, compiler_generated, unsafe_blocks))
}

/// A copy of `pristine` with every compiled file rewritten, whose build
/// script — and library, when `intact_lib` — compile from the untouched
/// sources.
fn rewritten_copy(
    scratch: &ScratchArea,
    pristine: &Package,
    rewritten: &HashMap<&str, &Stripped>,
    name: &str,
    intact_lib: bool,
) -> Result<Package, MeasureError> {
    let copy = scratch.copy_package(&pristine.root, name)?;
    rewrite(&copy, rewritten)?;
    copy.compile_from_intact(pristine, intact_lib)?;
    Ok(copy)
}

/// Writes the rewritten text of every file into `pkg`.
fn rewrite(pkg: &Package, rewritten: &HashMap<&str, &Stripped>) -> Result<(), MeasureError> {
    for (rel_path, stripped) in rewritten {
        if stripped.blocks > 0 {
            std::fs::write(pkg.root.join(rel_path), &stripped.text)
                .map_err(MeasureError::not_run("write rewritten source"))?;
        }
    }
    Ok(())
}

/// `cargo rustc` for one library or binary target, with the flags passed
/// to that crate alone.
fn target_command(pkg: &Package, target: &Target, selector: &[&str]) -> Command {
    let mut args = vec!["rustc"];
    args.extend_from_slice(selector);
    args.extend_from_slice(&["--profile=check", "--message-format=json", "--"]);
    args.extend_from_slice(STRIP_RUSTC_FLAGS);
    let mut cmd = pkg.cargo(&args);
    // Unstable `-Z` flags on stable rustc, for this crate alone.
    cmd.env("RUSTC_BOOTSTRAP", crate_name(target));
    cmd
}

/// `cargo check` that compiles the build script first. `cargo rustc` passes
/// its flags only to the final target, so they reach the build script
/// through `RUSTFLAGS`, which also reaches dependencies (and so needs
/// `RUSTC_BOOTSTRAP` for all of them); their diagnostics are not this
/// package's and are dropped.
fn build_script_command(pkg: &Package) -> Command {
    let mut cmd = pkg.cargo(&["check", "--message-format=json"]);
    cmd.env("CARGO_ENCODED_RUSTFLAGS", STRIP_RUSTC_FLAGS.join("\x1f"))
        .env("RUSTC_BOOTSTRAP", "1");
    cmd
}

/// Compiles one target of the rewritten copy and collects its operations.
fn check_target(
    pkg: &Package,
    target: &Target,
    cmd: Command,
    indexes: &HashMap<&str, &FnIndex>,
) -> Result<Vec<UnsafeOp>, MeasureError> {
    let run = pkg.run(cmd)?;
    let is_target = |t: &Target| t.name == target.name && t.kind == target.kind;

    let mut ops = Vec::new();
    let mut errors = Vec::new();
    for m in run.messages.iter().filter(|m| is_target(&m.target)) {
        let d = &m.message;
        let code = d.code.as_ref().map(|c| c.code.as_str());
        if code.is_some_and(|c| OP_CODES.contains(&c)) {
            if let Some(op) = to_op(pkg, d, &target_label(target), indexes) {
                ops.push(op);
            }
        } else if d.level == DiagnosticLevel::Error && !d.spans.is_empty() {
            errors.push(d.rendered.clone().unwrap_or_else(|| d.message.clone()));
        }
    }
    if !errors.is_empty() {
        return Err(MeasureError::DoesNotCompile(errors));
    }
    if ops.is_empty() && !run.compiled.iter().any(is_target) {
        // Cargo failed before compiling the target (manifest, dependency).
        return Err(MeasureError::NotRun(tail(&run.stderr)));
    }
    Ok(ops)
}

fn to_op(
    pkg: &Package,
    d: &Diagnostic,
    target: &str,
    indexes: &HashMap<&str, &FnIndex>,
) -> Option<UnsafeOp> {
    let primary = d.spans.iter().find(|s| s.is_primary)?;
    // DARPA counts operations whose own span is in the crate; spans inside
    // the standard library or a registry dependency are dropped.
    if !pkg.owns_file(&primary.file_name) {
        return None;
    }
    // Attribute to where the code was written: the outermost macro call
    // site still in the package (the operation itself when no macro).
    let site = call_site_chain(primary)
        .filter(|s| pkg.owns_file(&s.file_name))
        .last()
        .unwrap_or(primary);
    let body = indexes
        .get(site.file_name.as_str())
        .and_then(|idx| idx.innermost(site.line_start, site.column_start));
    let scope = match body {
        Some(b) if b.exported => OpScope::Exported,
        Some(_) => OpScope::Internal,
        None => OpScope::OutsideFunction,
    };
    let in_unsafe_fn = match d.level {
        DiagnosticLevel::Error => false,
        // The forced lints are warnings in both contexts for
        // `deprecated_safe_2024`; the signature decides.
        _ if d
            .code
            .as_ref()
            .is_some_and(|c| c.code == "deprecated_safe_2024") =>
        {
            body.is_some_and(|b| b.unsafe_sig)
        }
        _ => true,
    };
    let kind = OpKind::from_message(&d.message);
    Some(UnsafeOp {
        file: primary.file_name.clone(),
        line: primary.line_start,
        column: primary.column_start,
        end_line: primary.line_end,
        end_column: primary.column_end,
        kind,
        callee: (kind == OpKind::CallToUnsafeFunction)
            .then(|| backticked(&d.message))
            .flatten(),
        in_unsafe_fn,
        scope,
        function: body.map(|b| b.name.clone()),
        target: target.to_owned(),
    })
}

/// `kind:name` of a target.
fn target_label(target: &Target) -> String {
    let kind = target
        .kind
        .first()
        .map(ToString::to_string)
        .unwrap_or_default();
    format!("{kind}:{}", target.name)
}

/// The span, then each macro call site it was expanded from, outward.
fn call_site_chain(span: &DiagnosticSpan) -> impl Iterator<Item = &DiagnosticSpan> {
    std::iter::successors(Some(span), |s| s.expansion.as_ref().map(|e| &e.span))
}

/// The first `…`-quoted name in a message.
fn backticked(message: &str) -> Option<String> {
    let start = message.find('`')? + 1;
    let len = message[start..].find('`')?;
    Some(message[start..start + len].to_owned())
}

fn tail(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(20)..].join("\n")
}

fn summarize(
    ops: Vec<UnsafeOp>,
    compiler_generated: CompilerGenerated,
    unsafe_blocks: usize,
) -> SafetyReport {
    let mut by_kind = BTreeMap::new();
    let mut callees: HashMap<&str, usize> = HashMap::new();
    for op in &ops {
        *by_kind.entry(op.kind).or_default() += 1;
        if let Some(c) = &op.callee {
            *callees.entry(c).or_default() += 1;
        }
    }
    let mut top_callees: Vec<CalleeCount> = callees
        .into_iter()
        .map(|(name, calls)| CalleeCount {
            name: name.to_owned(),
            calls,
        })
        .collect();
    top_callees.sort_by(|a, b| b.calls.cmp(&a.calls).then_with(|| a.name.cmp(&b.name)));
    top_callees.truncate(TOP_CALLEES);
    let count = |scope| ops.iter().filter(|o| o.scope == scope).count();
    let in_unsafe_fn_bodies = ops.iter().filter(|o| o.in_unsafe_fn).count();
    SafetyReport {
        unsafe_ops: ops.len(),
        by_kind,
        in_exported_functions: count(OpScope::Exported),
        in_internal_functions: count(OpScope::Internal),
        outside_functions: count(OpScope::OutsideFunction),
        in_unsafe_fn_bodies,
        in_safe_code: ops.len() - in_unsafe_fn_bodies,
        top_callees,
        compiler_generated,
        unsafe_blocks,
        ops,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_come_from_rustc_messages() {
        let cases = [
            ("call to unsafe function `std::ptr::read` is unsafe and requires unsafe block", OpKind::CallToUnsafeFunction),
            ("call to unsafe function is unsafe and requires unsafe block", OpKind::CallToUnsafeFunction),
            ("call to deprecated safe function `std::env::set_var` is unsafe and requires unsafe block", OpKind::CallToUnsafeFunction),
            ("dereference of raw pointer is unsafe and requires unsafe block", OpKind::DerefOfRawPointer),
            ("use of mutable static is unsafe and requires unsafe block", OpKind::UseOfMutableStatic),
            ("use of extern static is unsafe and requires unsafe block", OpKind::UseOfExternStatic),
            ("access to union field is unsafe and requires unsafe block", OpKind::AccessToUnionField),
            ("use of inline assembly is unsafe and requires unsafe block", OpKind::UseOfInlineAssembly),
            ("call to function `f` with `#[target_feature]` is unsafe and requires unsafe block", OpKind::CallToFunctionWith),
            ("something new", OpKind::Other),
        ];
        for (message, kind) in cases {
            assert_eq!(OpKind::from_message(message), kind, "{message}");
        }
    }

    #[test]
    fn callee_is_the_first_backticked_name() {
        assert_eq!(
            backticked(
                "call to unsafe function `core::ptr::mut_ptr::<impl *mut T>::add` is unsafe"
            )
            .as_deref(),
            Some("core::ptr::mut_ptr::<impl *mut T>::add")
        );
        assert_eq!(backticked("call to unsafe function is unsafe"), None);
    }

    fn op(kind: OpKind, scope: OpScope, in_unsafe_fn: bool, callee: Option<&str>) -> UnsafeOp {
        UnsafeOp {
            file: "src/lib.rs".into(),
            line: 1,
            column: 1,
            end_line: 1,
            end_column: 2,
            kind,
            callee: callee.map(str::to_owned),
            in_unsafe_fn,
            scope,
            function: None,
            target: "x".into(),
        }
    }

    #[test]
    fn summary_partitions_operations() {
        let ops = vec![
            op(OpKind::DerefOfRawPointer, OpScope::Exported, false, None),
            op(
                OpKind::CallToUnsafeFunction,
                OpScope::Internal,
                true,
                Some("a"),
            ),
            op(
                OpKind::CallToUnsafeFunction,
                OpScope::Internal,
                true,
                Some("b"),
            ),
            op(
                OpKind::CallToUnsafeFunction,
                OpScope::OutsideFunction,
                false,
                Some("b"),
            ),
        ];
        let r = summarize(ops, CompilerGenerated::default(), 3);
        assert_eq!(r.unsafe_ops, 4);
        assert_eq!(r.by_kind[&OpKind::CallToUnsafeFunction], 3);
        assert_eq!(
            (
                r.in_exported_functions,
                r.in_internal_functions,
                r.outside_functions
            ),
            (1, 2, 1)
        );
        assert_eq!((r.in_unsafe_fn_bodies, r.in_safe_code), (2, 2));
        assert_eq!(
            r.top_callees[0],
            CalleeCount {
                name: "b".into(),
                calls: 2
            }
        );
        assert_eq!(r.unsafe_blocks, 3);
    }
}
