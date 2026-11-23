### No stubs
Every function, struct, constant and macro of the C source has a Rust
translation that does what the C code does. Common forms of a stub:
- a body that returns a constant, zero or a null pointer and ignores its parameters,
- an empty body,
- `todo!()`, `unimplemented!()`, `panic!("not implemented")` or similar,
- a function that only logs, or only returns an error code, where the C function does real work,
- a generated file of placeholder exports.

A function is not a stub when the API contract defines its return value as
a compile-time constant.
