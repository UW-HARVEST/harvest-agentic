# The fixture as a CMake project, for "tab2rust.py build" (tests/run_tests.py).
cmake_minimum_required(VERSION 3.10)
project(tab2rust_fixture C)
add_library(fixture tables.c tables_unit2.c)
