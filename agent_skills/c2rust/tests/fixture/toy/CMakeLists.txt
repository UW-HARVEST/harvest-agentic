cmake_minimum_required(VERSION 3.14)
project(toy C)

add_library(toy SHARED src/toy.c)
target_include_directories(toy PUBLIC include)
target_compile_definitions(toy PRIVATE TOY_BIAS=1)
target_link_libraries(toy PRIVATE m)

add_executable(toy_cli src/main.c)
target_link_libraries(toy_cli PRIVATE toy)
