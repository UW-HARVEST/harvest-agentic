//! Code-quality measurements on fixture crates with known answers.
//!
//! The expected unsafe-operation counts are those of DARPA's patched rustc
//! on the same crates (cross-checked op by op in `unsafe_ops_oracle.rs`).
//! These tests compile the fixtures with cargo and clippy.

mod common;

use harvest_benchmark::quality::safety::OpKind;
use harvest_benchmark::quality::{measure, Outcome, QualityReport};

fn measure_fixture(name: &str) -> (tempfile::TempDir, QualityReport) {
    let copy = common::fixtures_copy();
    let m = measure(&copy.path().join(name));
    (copy, m)
}

#[test]
#[cfg_attr(miri, ignore)]
fn every_operation_of_the_known_answer_crate_is_counted() {
    let (_dir, m) = measure_fixture("known_ops");
    let s = m.safety.as_result().unwrap();
    assert_eq!(s.unsafe_ops, 1217);
    // `exported` holds 6 operations; everything else is internal.
    assert_eq!(s.in_exported_functions, 6);
    assert_eq!(s.outside_functions, 0);
    assert_eq!(s.by_kind[&OpKind::UseOfInlineAssembly], 1);
    assert_eq!(s.by_kind[&OpKind::UseOfExternStatic], 1);
    assert_eq!(s.by_kind[&OpKind::AccessToUnionField], 1);
    assert_eq!(s.by_kind[&OpKind::UseOfMutableStatic], 1);
    assert_eq!(s.ops.len(), 1217);
}

#[test]
#[cfg_attr(miri, ignore)]
fn edition_2024_crates_are_counted() {
    let (_dir, m) = measure_fixture("edition2024");
    assert_eq!(m.safety.as_result().unwrap().unsafe_ops, 5);
    let (_dir, m) = measure_fixture("edition2024_extern");
    assert_eq!(m.safety.as_result().unwrap().unsafe_ops, 5);
}

#[test]
#[cfg_attr(miri, ignore)]
fn library_and_binary_are_both_counted_once() {
    // Macro-repeated fragments, asm!, a const, deprecated-safe calls,
    // statics, a union, a range bound, and one operation in the binary.
    let (_dir, m) = measure_fixture("lib_and_bin");
    let s = m.safety.as_result().unwrap();
    assert_eq!(s.unsafe_ops, 25);
    let in_bin = s
        .ops
        .iter()
        .filter(|o| o.target == "bin:lib_and_bin_driver")
        .count();
    assert_eq!(in_bin, 1);
}

#[test]
#[cfg_attr(miri, ignore)]
fn every_target_counts_the_files_it_compiles() {
    // As the patched rustc does: a module of both the library and the binary
    // counts once per target; the binary's own module and the build script
    // are counted too.
    let (_dir, m) = measure_fixture("lib_bin_build");
    let s = m.safety.as_result().unwrap();
    assert_eq!(s.unsafe_ops, 5);
    let at = |file: &str, target: &str| {
        s.ops
            .iter()
            .filter(|o| o.file == file && o.target == target)
            .count()
    };
    assert_eq!(at("src/util.rs", "lib:lib_bin_build"), 1);
    assert_eq!(at("src/util.rs", "bin:lib_bin_build"), 1);
    assert_eq!(at("src/helper.rs", "bin:lib_bin_build"), 1);
    let build = "custom-build:build-script-build";
    assert_eq!(at("build_helper.rs", build), 1);
    assert_eq!(at("build.rs", build), 1);
    // Each file once, the build script's included.
    assert_eq!(m.rust_code_lines, 25);
}

#[test]
#[cfg_attr(miri, ignore)]
fn a_lint_the_library_denies_does_not_hide_the_binary() {
    // Clippy stops at the library; the binary's module is still measured.
    let (_dir, m) = measure_fixture("deny_in_lib");
    let s = m.safety.as_result().unwrap();
    assert_eq!(s.unsafe_ops, 1);
    assert_eq!(s.ops[0].file, "src/helper.rs");
    let lints = &m.idiomaticity.as_result().unwrap().lints;
    assert_eq!(lints.clippy_by_lint["needless_return"], 1);
}

#[test]
#[cfg_attr(miri, ignore)]
fn operations_inside_a_dependency_macro_are_not_counted() {
    let (_dir, m) = measure_fixture("external_macro");
    assert_eq!(m.safety.as_result().unwrap().unsafe_ops, 1);
}

#[test]
#[cfg_attr(miri, ignore)]
fn compiler_generated_operations_are_estimated_separately() {
    let (_dir, m) = measure_fixture("compiler_generated");
    let s = m.safety.as_result().unwrap();
    assert_eq!(s.unsafe_ops, 0);
    // 2 format_args!, 2 .await (2 operations each), 3 derived PartialEq on
    // enums whose variants all carry data.
    assert_eq!(s.compiler_generated.format_args, 2);
    assert_eq!(s.compiler_generated.await_points, 2);
    assert_eq!(s.compiler_generated.derived_partial_eq, 3);
    assert_eq!(s.compiler_generated.total(), 9);
}

#[test]
#[cfg_attr(miri, ignore)]
fn allow_attributes_hide_lints_as_in_darpas_count() {
    const BODY: &str = r#"
pub fn needless(x: u32) -> u32 { return x + 1; }
fn unused() { let y = 3; }
pub fn pi(x: f64) -> bool { x == 3.14159 }
pub fn branchy(a: u32) -> u32 { if a > 1 { if a > 2 { 3 } else { 4 } } else { 5 } }
"#;
    let dir = tempfile::tempdir().unwrap();
    let plain = common::write_crate(dir.path(), "lints_plain", BODY);
    let silenced = common::write_crate(
        dir.path(),
        "lints_silenced",
        &format!("#![allow(clippy::all, dead_code, unused_variables)]\n{BODY}"),
    );

    let plain = measure(&plain);
    let p = plain.idiomaticity.as_result().unwrap();
    assert_eq!(p.lints.clippy_by_lint["needless_return"], 1);
    assert_eq!(p.lints.clippy_by_lint["approx_constant"], 1);
    assert_eq!(p.lints.clippy_by_group["style"], 1);
    assert_eq!(p.lints.clippy_by_group["correctness"], 1);
    assert_eq!(p.lints.rustc_by_lint["dead_code"], 1);
    assert_eq!(p.lints.rustc_by_lint["unused_variables"], 1);

    let silenced = measure(&silenced);
    let s = silenced.idiomaticity.as_result().unwrap();
    assert_eq!(s.lints.clippy_lints, 0);
    assert_eq!(s.lints.rustc_warnings, 0);

    // Every function is measured; the nested branch scores highest.
    assert_eq!(p.cognitive_complexity.functions, 4);
    assert_eq!(p.cognitive_complexity.max, 3);
    assert_eq!(s.cognitive_complexity, p.cognitive_complexity);
}

#[test]
#[cfg_attr(miri, ignore)]
fn a_crate_that_does_not_compile_is_reported_not_counted() {
    let dir = tempfile::tempdir().unwrap();
    let root = common::write_crate(
        dir.path(),
        "broken",
        "pub fn f(p: *const u8) -> u8 { unsafe { *p } + missing() }",
    );
    let m = measure(&root);
    assert!(
        matches!(&m.safety, Outcome::DoesNotCompile { errors } if !errors.is_empty()),
        "{:?}",
        m.safety
    );
    assert!(matches!(m.idiomaticity, Outcome::DoesNotCompile { .. }));
}

#[test]
#[cfg_attr(miri, ignore)]
fn the_snapshot_is_left_untouched() {
    let (dir, _m) = measure_fixture("lib_and_bin");
    let before =
        std::fs::read_to_string(common::fixtures_root().join("lib_and_bin/src/lib.rs")).unwrap();
    let after = std::fs::read_to_string(dir.path().join("lib_and_bin/src/lib.rs")).unwrap();
    assert_eq!(before, after);
    assert!(
        !dir.path()
            .join("lib_and_bin/target/harvest_quality")
            .exists(),
        "scratch area removed"
    );
}

#[test]
#[cfg_attr(miri, ignore)]
fn only_files_the_crate_compiles_are_measured() {
    // Verify-stage snapshots carry agent tooling next to the crate; its Rust
    // files are not part of the translation.
    let dir = tempfile::tempdir().unwrap();
    let root = common::write_crate(
        dir.path(),
        "with_tooling",
        "mod inner;\npub fn f(p: *const u8) -> u8 { unsafe { *p } }\n",
    );
    std::fs::write(root.join("src/inner.rs"), "pub fn g() -> u8 {\n    1\n}\n").unwrap();
    std::fs::create_dir_all(root.join("verify_env/src")).unwrap();
    std::fs::write(
        root.join("verify_env/src/main.rs"),
        "fn main() {\n    let x = 1;\n    unsafe { h() };\n    print!(\"{}\", format_args!(\"{x}\"));\n}\n",
    )
    .unwrap();
    let m = measure(&root);
    let s = m.safety.as_result().unwrap();
    assert_eq!(s.unsafe_ops, 1);
    assert_eq!(s.unsafe_blocks, 1);
    assert_eq!(s.compiler_generated.total(), 0);
    // `mod inner;` + `pub fn f…` in lib.rs, three lines in inner.rs.
    assert_eq!(m.rust_code_lines, 5);
}

#[test]
#[cfg_attr(miri, ignore)]
fn a_relative_program_path_measures_like_an_absolute_one() {
    // Cargo runs inside the scratch copies, so a relative path handed to it
    // would resolve against the copy: the build records and the clippy
    // configuration would be looked for in the wrong place.
    let dir = tempfile::tempdir_in(".").unwrap();
    let cwd = std::env::current_dir().unwrap();
    let relative = dir.path().strip_prefix(&cwd).unwrap();
    assert!(relative.is_relative());
    let root = common::write_crate(
        relative,
        "relative",
        "pub fn f(x: u32) -> u32 {\n    if x > 1 { x } else { 0 }\n}\n",
    );
    std::fs::create_dir_all(root.join("tools")).unwrap();
    std::fs::write(root.join("tools/script.rs"), "fn a() {}\nfn b() {}\n").unwrap();
    let m = measure(&root);
    assert_eq!(m.rust_code_lines, 3, "only the compiled lib.rs");
    assert_eq!(
        m.idiomaticity
            .as_result()
            .unwrap()
            .cognitive_complexity
            .functions,
        1
    );
    assert_eq!(m.safety.as_result().unwrap().unsafe_ops, 0);
}
