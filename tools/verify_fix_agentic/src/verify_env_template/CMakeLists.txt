cmake_minimum_required(VERSION 3.19)
project(harvest_verify_env C CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_C_STANDARD 99)

# Use ccache when present.
find_program(CCACHE_PROGRAM ccache)
if(CCACHE_PROGRAM)
  set(CMAKE_C_COMPILER_LAUNCHER "${CCACHE_PROGRAM}")
  set(CMAKE_CXX_COMPILER_LAUNCHER "${CCACHE_PROGRAM}")
endif()

include(FetchContent)

# GoogleTest pinned to a release tag. Declared before FuzzTest so the whole
# build reuses this one copy. Offline: pre-populate the source once and point
# CMake at it with -DFETCHCONTENT_SOURCE_DIR_GOOGLETEST=/path/to/googletest.
FetchContent_Declare(
  googletest
  URL https://github.com/google/googletest/archive/refs/tags/v1.17.0.tar.gz
  DOWNLOAD_EXTRACT_TIMESTAMP ON
)
set(BUILD_GMOCK OFF CACHE BOOL "" FORCE)
set(INSTALL_GTEST OFF CACHE BOOL "" FORCE)
{FUZZTEST_DECLARE}
FetchContent_MakeAvailable(googletest{FUZZTEST_MAKEAVAIL})

enable_testing()
include(GoogleTest)
{FUZZTEST_SETUP_FLAGS}
# C reference under test, compiled INTO the test binary so it carries coverage
# instrumentation under fuzzing mode. The translated Rust is a black box loaded
# via dlopen at run time (see rust_lib.h) — never linked here, so the two sides'
# same-named exports do not collide.
file(GLOB C_UNDER_TEST_SOURCES CONFIGURE_DEPENDS
     "${CMAKE_CURRENT_SOURCE_DIR}/../c_src/src/*.c")
add_library(c_under_test STATIC ${C_UNDER_TEST_SOURCES})
target_include_directories(c_under_test PUBLIC
     "${CMAKE_CURRENT_SOURCE_DIR}/../c_src/include")
# Project-specific C build flags, pre-filled from c_src/CMakeLists.txt where it
# could be parsed. If the C reference needs more (or these are wrong), check
# c_src/CMakeLists.txt `target_compile_definitions` and edit this line.
target_compile_definitions(c_under_test PRIVATE {C_COMPILE_DEFS})
target_compile_options(c_under_test PRIVATE -w)

add_executable(verification_tests verification_tests.cc)
target_include_directories(verification_tests PRIVATE
     "${CMAKE_CURRENT_SOURCE_DIR}/../c_src/include")
target_link_libraries(verification_tests PRIVATE c_under_test ${CMAKE_DL_LIBS})
{TEST_LINK}
gtest_discover_tests(verification_tests)
