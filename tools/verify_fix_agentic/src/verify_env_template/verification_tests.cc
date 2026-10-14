// Differential verification: original C reference vs. translated Rust.
//
// The C reference is linked statically into this binary; declare the C
// functions you want to test in the extern "C" block and call them directly.
// The Rust translation is loaded as a black box via harvest::RustLib and called
// through the same C-ABI symbol names. Each test runs both sides on the same
// input and asserts the observable results match.
//
// This is a starting point — replace the placeholder example with tests for the
// actual public API (see inputs/c_src/include for the headers).

#include "gtest/gtest.h"

#include "harvest_diff.h"
#include "rust_lib.h"

// --- C reference (linked statically) ----------------------------------------
// Declare the C functions under test here, matching inputs/c_src/include signatures.
extern "C" {
// Example:
// int my_api(const unsigned char* in, int in_len, unsigned char* out, int cap);
}

namespace {

// Example skeleton — delete and replace with real API tests.
//
// using MyApiFn = int (*)(const unsigned char*, int, unsigned char*, int);
//
// TEST(Differential, MyApiMatchesC) {
//   auto rust = harvest::RustLib::Get().Sym<MyApiFn>("my_api");
//   ASSERT_NE(rust, nullptr);
//   const std::vector<uint8_t> input = {1, 2, 3};
//   auto c_out = harvest::FilledBuffer(64);
//   auto rust_out = harvest::FilledBuffer(64);
//   const int c_rc = my_api(input.data(), input.size(), c_out.data(), 64);
//   const int r_rc = rust(input.data(), input.size(), rust_out.data(), 64);
//   EXPECT_EQ(r_rc, c_rc);
//   // Trim to the meaningful prefix, then compare; Explain() reports the first
//   // differing offset and a hex dump of each side on failure.
//   c_out.resize(c_rc > 0 ? c_rc : 0);
//   rust_out.resize(r_rc > 0 ? r_rc : 0);
//   EXPECT_EQ(rust_out, c_out) << harvest::Explain(c_out, rust_out);
// }

TEST(VerifyEnv, RustLibraryLoads) {
  EXPECT_TRUE(harvest::RustLib::Get().ok())
      << "RUST_LIB_PATH not set or the translated .so failed to load";
}

}  // namespace
