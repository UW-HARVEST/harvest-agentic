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

# GoogleTest pinned to a release tag. Offline: pre-populate the source and point
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
# C reference under test, compiled INTO the test binary. The translated Rust
# is a black box loaded via dlopen at run time (see rust_lib.h) — never linked
# here, so the two sides' same-named exports do not collide.
# The C source: inputs/c_src of the checkout. To build elsewhere, pass
# -DHARVEST_C_SRC=<directory> (for example a snapshot's .harvest/c_src).
# import_c_build.py takes the sources, compile settings and libraries from the C
# project's own CMake build, configured with HARVEST_C_OPTIONS (-D flags; build.sh
# passes them).
set(HARVEST_C_SRC "${CMAKE_CURRENT_SOURCE_DIR}/../inputs/c_src" CACHE PATH "C source directory")
set(HARVEST_C_OPTIONS "" CACHE STRING "Options of the C project, as -D flags")
separate_arguments(HARVEST_C_OPTION_LIST UNIX_COMMAND "${HARVEST_C_OPTIONS}")
execute_process(
  COMMAND python3 "${CMAKE_CURRENT_SOURCE_DIR}/import_c_build.py" "${HARVEST_C_SRC}"
          "${CMAKE_BINARY_DIR}/c-build" "${CMAKE_C_COMPILER}" ${HARVEST_C_OPTION_LIST}
  RESULT_VARIABLE C_BUILD_IMPORT_RESULT)
if(NOT C_BUILD_IMPORT_RESULT EQUAL 0)
  message(FATAL_ERROR "import_c_build.py could not describe the C build (see above)")
endif()
include("${CMAKE_BINARY_DIR}/c-build/c_build.cmake")
add_library(c_under_test STATIC ${C_UNDER_TEST_SOURCES})
target_include_directories(c_under_test PUBLIC ${C_UNDER_TEST_INCLUDE_DIRECTORIES})
target_link_libraries(c_under_test PUBLIC ${C_UNDER_TEST_LIBRARIES})
target_compile_options(c_under_test PRIVATE -w)

# The test files: verification_tests.cc and tests/*.cc of HARVEST_TEST_DIR
# (see shared_build.sh).
set(HARVEST_TEST_DIR "${CMAKE_CURRENT_SOURCE_DIR}" CACHE PATH "Directory of the test files")
file(GLOB VERIFY_TEST_SOURCES CONFIGURE_DEPENDS "${HARVEST_TEST_DIR}/tests/*.cc")
add_executable(verification_tests
     "${HARVEST_TEST_DIR}/verification_tests.cc" ${VERIFY_TEST_SOURCES})
target_include_directories(verification_tests PRIVATE
     "${HARVEST_TEST_DIR}" "${HARVEST_C_SRC}/include")
# The test framework links first, so its main() is the one the linker takes,
# also when a C source file defines main() for the program's executable.
{TEST_LINK}
target_link_libraries(verification_tests PRIVATE c_under_test ${CMAKE_DL_LIBS})
gtest_discover_tests(verification_tests)
