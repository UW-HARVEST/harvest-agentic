use serde::Serialize;

/// Statistics for a a single test on a program
#[derive(Debug, Clone, Serialize)]
pub struct TestResult {
    pub filename: String,
    pub passed: bool,
    pub skipped: bool,
}

/// Statistics for running many tests on a single program
#[derive(Debug, Serialize)]
pub struct ProgramEvalStats {
    pub program_name: String,
    pub translation_success: bool,
    pub rust_build_success: bool,
    pub total_tests: usize,
    pub passed_tests: usize,
    pub skipped_tests: usize,
    pub error_message: Option<String>,
    // Store individual test results with filenames and pass/fail status
    pub test_results: Vec<TestResult>,
}

impl ProgramEvalStats {
    pub fn new(program_name: &str) -> Self {
        ProgramEvalStats {
            program_name: program_name.to_string(),
            translation_success: false,
            rust_build_success: false,
            total_tests: 0,
            passed_tests: 0,
            skipped_tests: 0,
            error_message: None,
            test_results: Vec::new(),
        }
    }

    /// Number of tests that were actually evaluated (non-skipped)
    pub fn evaluated_tests(&self) -> usize {
        self.total_tests.saturating_sub(self.skipped_tests)
    }

    /// Number of evaluated tests that failed
    pub fn failed_tests(&self) -> usize {
        self.evaluated_tests().saturating_sub(self.passed_tests)
    }

    /// Calculate success rate as a percentage
    pub fn success_rate(&self) -> f64 {
        let evaluated_tests = self.evaluated_tests();
        if evaluated_tests == 0 {
            return 0.0;
        };

        (self.passed_tests as f64 / evaluated_tests as f64) * 100.0
    }

    /// Whether this program passed its whole suite. A program that
    /// evaluated no tests (e.g. a build failure) does not count.
    pub fn all_tests_passed(&self) -> bool {
        let evaluated = self.evaluated_tests();
        evaluated > 0 && self.passed_tests == evaluated
    }
}

/// Summary statistics across all program runs.
///
/// Test outcomes exist at two levels because suites differ widely in how
/// many tests they expose: [`Self::project_pass_rate`] counts programs
/// that passed their whole suite, while [`Self::overall_success_rate`]
/// pools per-test counts and therefore weights each program by its suite
/// size.
#[derive(Debug, Serialize)]
pub struct SummaryStats {
    pub num_programs: usize,
    pub successful_translations: usize,
    pub successful_rust_builds: usize,
    pub total_tests: usize,
    pub total_skipped_tests: usize,
    pub total_passed_tests: usize,
    pub programs_all_tests_passed: usize,
    pub min_evaluated_tests: usize,
    pub max_evaluated_tests: usize,
}

impl SummaryStats {
    /// Number of tests that were actually evaluated (non-skipped)
    pub fn evaluated_tests(&self) -> usize {
        self.total_tests.saturating_sub(self.total_skipped_tests)
    }

    /// Number of evaluated tests that failed
    pub fn failed_tests(&self) -> usize {
        self.evaluated_tests()
            .saturating_sub(self.total_passed_tests)
    }

    /// Fraction of programs that passed their whole suite, as a percentage.
    pub fn project_pass_rate(&self) -> f64 {
        if self.num_programs == 0 {
            0.0
        } else {
            (self.programs_all_tests_passed as f64 / self.num_programs as f64) * 100.0
        }
    }

    /// Pooled per-test success rate across all programs; weights each
    /// program by its suite size.
    pub fn overall_success_rate(&self) -> f64 {
        let evaluated_tests = self.evaluated_tests();
        if evaluated_tests == 0 {
            0.0
        } else {
            (self.total_passed_tests as f64 / evaluated_tests as f64) * 100.0
        }
    }

    /// Calculate translation success rate as a percentage
    pub fn translation_success_rate(&self) -> f64 {
        if self.num_programs == 0 {
            0.0
        } else {
            (self.successful_translations as f64 / self.num_programs as f64) * 100.0
        }
    }

    /// Calculate Rust build success rate as a percentage
    pub fn rust_build_success_rate(&self) -> f64 {
        if self.num_programs == 0 {
            0.0
        } else {
            (self.successful_rust_builds as f64 / self.num_programs as f64) * 100.0
        }
    }

    /// Create SummaryStats from a slice of ProgramEvalStats
    pub fn from_results(results: &[ProgramEvalStats]) -> Self {
        // Spread is taken over programs that actually evaluated something;
        // programs that never got to run would otherwise pin the minimum at 0.
        let evaluated: Vec<usize> = results
            .iter()
            .map(|r| r.evaluated_tests())
            .filter(|n| *n > 0)
            .collect();
        SummaryStats {
            num_programs: results.len(),
            successful_translations: results.iter().filter(|r| r.translation_success).count(),
            successful_rust_builds: results.iter().filter(|r| r.rust_build_success).count(),
            total_tests: results.iter().map(|r| r.total_tests).sum(),
            total_skipped_tests: results.iter().map(|r| r.skipped_tests).sum(),
            total_passed_tests: results.iter().map(|r| r.passed_tests).sum(),
            programs_all_tests_passed: results.iter().filter(|r| r.all_tests_passed()).count(),
            min_evaluated_tests: evaluated.iter().copied().min().unwrap_or(0),
            max_evaluated_tests: evaluated.iter().copied().max().unwrap_or(0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A program with `total` tests of which `passed` passed and `skipped` were
    /// skipped, translated and built successfully.
    fn program(name: &str, total: usize, passed: usize, skipped: usize) -> ProgramEvalStats {
        let mut p = ProgramEvalStats::new(name);
        p.translation_success = true;
        p.rust_build_success = true;
        p.total_tests = total;
        p.passed_tests = passed;
        p.skipped_tests = skipped;
        p
    }

    #[test]
    fn all_tests_passed_requires_at_least_one_evaluated_test() {
        assert!(program("a", 10, 10, 0).all_tests_passed());
        assert!(!program("a", 10, 9, 0).all_tests_passed());
        // Skipped tests are not failures: 8 of 10 evaluated, all 8 passed.
        assert!(program("a", 10, 8, 2).all_tests_passed());
        // A translation that fails to build reports zero tests.
        assert!(!program("a", 0, 0, 0).all_tests_passed());
        assert!(!program("a", 5, 0, 5).all_tests_passed());
    }

    #[test]
    fn the_headline_metric_is_unaffected_by_suite_granularity() {
        // Both programs fail exactly one test; only the suite granularity
        // differs between the two runs.
        let coarse = vec![program("a", 4, 3, 0), program("b", 800, 799, 0)];
        let fine = vec![program("a", 800, 799, 0), program("b", 800, 799, 0)];

        let a = SummaryStats::from_results(&coarse);
        let b = SummaryStats::from_results(&fine);

        assert_eq!(a.programs_all_tests_passed, b.programs_all_tests_passed);
        assert_eq!(a.project_pass_rate(), b.project_pass_rate());
        assert_ne!(
            a.overall_success_rate(),
            b.overall_success_rate(),
            "pooled rate should change when only the suite granularity changes"
        );
    }

    #[test]
    fn a_pooled_average_hides_a_completely_broken_coarse_suite() {
        // One project fails every test, but its four tests are drowned out
        // by the other project's eight hundred.
        let results = vec![program("coarse", 4, 0, 0), program("fine", 800, 800, 0)];
        let s = SummaryStats::from_results(&results);

        assert!(
            s.overall_success_rate() > 99.0,
            "pooled = {:.2}%",
            s.overall_success_rate()
        );
        assert_eq!(s.programs_all_tests_passed, 1);
        assert_eq!(s.project_pass_rate(), 50.0);
    }

    #[test]
    fn project_pass_rate_counts_programs_not_tests() {
        // One tiny suite passes entirely, one huge suite fails a single test.
        let results = vec![program("small", 4, 4, 0), program("big", 1000, 999, 0)];
        let s = SummaryStats::from_results(&results);
        assert_eq!(s.programs_all_tests_passed, 1);
        assert_eq!(s.project_pass_rate(), 50.0);
        assert!(s.overall_success_rate() > 99.0);
    }

    #[test]
    fn the_spread_of_suite_sizes_is_recorded() {
        let results = vec![
            program("small", 4, 4, 0),
            program("big", 800, 800, 0),
            program("failed_to_build", 0, 0, 0),
        ];
        let s = SummaryStats::from_results(&results);
        // The program that evaluated nothing must not pin the minimum at 0.
        assert_eq!(s.min_evaluated_tests, 4);
        assert_eq!(s.max_evaluated_tests, 800);
    }

    #[test]
    fn an_empty_run_reports_zero_rather_than_dividing_by_zero() {
        let s = SummaryStats::from_results(&[]);
        assert_eq!(s.num_programs, 0);
        assert_eq!(s.project_pass_rate(), 0.0);
        assert_eq!(s.overall_success_rate(), 0.0);
        assert_eq!(s.min_evaluated_tests, 0);
        assert_eq!(s.max_evaluated_tests, 0);
    }
}
