All analysis is complete. Here is the plan.

---

# C → Rust Migration Plan — LZ4 (`c_src`)

## 1. Overview

**Library.** A customized LZ4 compression suite (fast levels 1–auto, "high compression" levels 3–12, a custom **level-2 "MID"** strategy, the LZ4 *frame* format, a FILE*-based wrapper layer, and a namespaced XXH32/XXH64 implementation). Customizations vs. upstream LZ4 observed in this tree:

- `lz4hc.c` adds an `LZ4MID_*` mid-compression strategy (level 2, dual 4/8-byte hash tables) and a runtime strategy table `k_clTable`.
- `lz4frame.c` adds per-block uncompressed mode (`LZ4F_BlockCompressMode_e`, `LZ4F_uncompressedUpdate`, 3-arg `LZ4F_selectCompression`).
- `lz4file.c` + `lz4file.h` (Xiaomi, 2022) add `LZ4F_readOpen/read/readClose` and `LZ4F_writeOpen/write/writeClose` over `FILE*`.
- `xxhash.c/h` is bundled and compiled with `XXH_NAMESPACE=LZ4_`, so every public xxhash symbol is emitted as `LZ4_XXH*`.

**Source files** (all in `c_src/src/`, headers in `c_src/include/`):

| File | Lines | Unique function definitions | Exports in this build |
|---|---|---|---|
| `lz4.c` | 2829 | 91 (incl. 16 shared-region fns) | 50 |
| `lz4hc.c` | 2192 | 74 (+16 private copies pulled from `lz4.c`) | 35 |
| `lz4frame.c` | 2136 | 57 | 33 |
| `lz4file.c` | 341 | 11 | 6 |
| `xxhash.c` | 1030 | 46 | 19 |
| **Total** | | **279 unique (295 instances)** | **143** |

Counts deduplicate `#if/#elif/#else` variant definitions (e.g. `LZ4_read16` has 3 variants). "143 exports" is the ground truth from `nm -gU` on the built `liblz4.dylib`; every exported symbol is a required `extern "C"` trampoline name for its capsule (see per-capsule boundary tables).

**Call graph.** Generated with the `callgraph` tool (`opt -passes=dot-callgraph` over LLVM IR at `-O0`): **5/5 files compiled OK — no skipped files.** The graph stops at prebuilt system libraries: libc/libsystem (`malloc`, `calloc`, `free`, `fread`, `fwrite`, `memcpy`/`memset` via `__memcpy_chk`/`__memset_chk`, `assert` via `__assert_rtn`). No other prebuilt dependencies.

**Two structural facts the plan depends on:**

1. **Unity-build include (guarded subset).** `lz4hc.c:60-68` does
   ```c
   #ifndef LZ4_SRC_INCLUDED
   #  define LZ4_COMMONDEFS_ONLY
   #  include "lz4.c"   /* LZ4_count, constants, mem */
   #endif
   ```
   i.e. the first ~705 lines of `lz4.c` (types, constants, mem helpers, `LZ4_count`) are compiled into the `lz4hc.c` TU as a **private copy**. `lz4.c:706` opens `#ifndef LZ4_COMMONDEFS_ONLY`, so the rest of `lz4.c` is not included. This is the source of every suffixed `foo.N` name in the call graph (`LZ4_read32.2`, `LZ4_NbCommonBytes.4`, `LZ4_writeLE16.8`, …). It drives capsule C1 below.
2. **`LZ4_FORCE_INLINE` / `FORCE_INLINE` expand to `static inline __attribute__((always_inline))` in this build** (lz4.c:138, xxhash.c:129). Every function marked with them is `static` here, and most vanish from the IR entirely (fully inlined). Static classification below uses the dylib export table as ground truth: *in IR but not exported ⇒ static*.

**Tooling caveat (indirect-call flags).** The task description says the tool flags functions with `[indirect call site — callees unresolved]`. **This installed build of the tool emits no such flags** — verified by re-running the raw `opt -passes=dot-callgraph` pass and grepping its output (zero matches; calls through function pointers simply produce no edge, which is why `LZ4F_compressUpdateImpl` shows no edge to `LZ4F_compressBlock*` and `LZ4MID_compress` shows no edge to `LZ4MID_search*Dict`). I therefore enumerated all function-pointer call sites manually; see Section 3. Treat that section as the recovered-edge record.

---

## 2. Capsule dependency graph and migration order

Edges are the real cross-file call edges recovered from the call graph (plus the indirect edges of Section 3, all of which resolve *within* a single file):

```
        C1 lz4-common
        /            \
       v              v
C2 xxhash        C3 lz4core
     \              /  \
      \             v   v
       v          C4 lz4hc
       v             |   \
      v              v   v
      +----------> C5 lz4frame
                     |
                     v
                  C6 lz4file
```

- `lz4hc → lz4core`: 1 symbol (`LZ4_compressBound`, 7 call sites in the deprecated wrappers, lz4hc.c:2133-2185, and in `LZ4_compress_HC`).
- `lz4frame → lz4core`: 9 symbols; `lz4frame → lz4hc`: 10; `lz4frame → xxhash`: 4 (`LZ4_XXH32`, `LZ4_XXH32_reset`, `LZ4_XXH32_update`, `LZ4_XXH32_digest`).
- `lz4file → lz4frame`: 11 symbols.
- `lz4core → lz4hc`: **none** (dependency is one-way).
- `xxhash`, `lz4-common`: no dependencies.

**Topological (migration) order:** **C1 lz4-common → C2 xxhash (parallel with C1) → C3 lz4core → C4 lz4hc → C5 lz4frame → C6 lz4file.** C1 and C2 are independent and can be migrated in either order or in parallel; every later capsule is blocked on the ones above it.

Build flags to carry into the Rust build: `LZ4_HEAPMODE=0`, `LZ4F_HEAPMODE=0` (both set by CMake); `LZ4HC_HEAPMODE` **unset → defaults to 1** (heap) in lz4hc.c; `XXH_NAMESPACE=LZ4_`; `LZ4_MEMORY_USAGE` default 14.

---

## 3. Capsules (in migration order)

### C1 — `lz4-common`: shared primitives (the `LZ4_COMMONDEFS_ONLY` region)

**What it does.** The compile-time-shared core of `lz4.c` (lines 116–705): scalar types, constants, little-endian read/write helpers, unaligned memory access, fast byte-copy primitives, common-byte counting, and `LZ4_count`. Today it physically exists **twice** (once per including TU: `lz4.c` and `lz4hc.c`). This is the **shared-primitives capsule** — see Section 4 for the gateway-and-delete note.

**Members (16 functions + types/constants/macros).** All `static` in the C source; no linker symbol exists for any of them.

- `LZ4_isAligned`, `LZ4_isLittleEndian`
- `LZ4_read16`, `LZ4_read32`, `LZ4_read_ARCH`, `LZ4_write16`, `LZ4_write32` (each with 3 `#if` variant definitions; only one active per platform)
- `LZ4_readLE16`, `LZ4_readLE32` (defined but unused in this build), `LZ4_writeLE16`
- `LZ4_wildCopy8`, `LZ4_wildCopy32`, `LZ4_memcpy_using_offset_base`, `LZ4_memcpy_using_offset`
- `LZ4_NbCommonBytes`, `LZ4_count`
- Types: `BYTE`, `U16`, `U32`, `S32`, `U64`, `uptrval`, `reg_t`, `LZ4_unalign16/32/ST`, `tableType_t`
- Constants: `MINMATCH(4)`, `WILDCOPYLENGTH(8)`, `LASTLITERALS(5)`, `MFLIMIT(12)`, `KB/MB/GB`
- Macros: `LZ4_FORCE_INLINE`, `ALLOC/ALLOC_AND_ZERO/FREEMEM`, `LZ4_memset/MEM_INIT`, `DEBUGLOG`, `likely/unlikely`

**Dependencies.** None (libc `memcpy`/`memset` only).

**External boundary / required trampolines.** **None.** Nothing outside `lz4.c`/`lz4hc.c` references these by symbol — they are `static`. In Rust this becomes a `pub(crate)` module; the "internal entry surface" is ordinary intra-crate calls from C3 and C4.

**ABI-fixed data.** None — every type here is private to the two TUs.

**Suggested chunking (2 chunks).**
1. Types/constants/macros + `LZ4_isAligned`, `LZ4_isLittleEndian`, `LZ4_read16`, `LZ4_read32`, `LZ4_read_ARCH`, `LZ4_write16`, `LZ4_write32`, `LZ4_writeLE16` (8 fns)
2. `LZ4_readLE16`, `LZ4_readLE32`, `LZ4_NbCommonBytes`, `LZ4_wildCopy8`, `LZ4_wildCopy32`, `LZ4_memcpy_using_offset_base`, `LZ4_memcpy_using_offset`, `LZ4_count` (8 fns)

---

### C2 — `xxhash`: XXH32/XXH64 (namespaced `LZ4_XXH*`)

**What it does.** One-shot and streaming XXH32/XXH64 plus canonical-hash conversion. Inside the project it is used only by C5 (`lz4frame.c`) for header checksums, block checksums, and content checksums; it is also part of the dylib's public API surface.

**Members (46).** Exported (19, namespaced in IR): `LZ4_XXH_versionNumber`, `LZ4_XXH32`, `LZ4_XXH32_createState`, `LZ4_XXH32_freeState`, `LZ4_XXH32_copyState`, `LZ4_XXH32_reset`, `LZ4_XXH32_update`, `LZ4_XXH32_digest`, `LZ4_XXH32_canonicalFromHash`, `LZ4_XXH32_hashFromCanonical`, `LZ4_XXH64`, `LZ4_XXH64_createState`, `LZ4_XXH64_freeState`, `LZ4_XXH64_copyState`, `LZ4_XXH64_reset`, `LZ4_XXH64_update`, `LZ4_XXH64_digest`, `LZ4_XXH64_canonicalFromHash`, `LZ4_XXH64_hashFromCanonical`.
Static (27): `XXH_malloc`, `XXH_free`, `XXH_memcpy`, `XXH_read32`, `XXH_swap32`, `XXH_isLittleEndian`, `XXH_readLE32_align`, `XXH_readLE32`, `XXH_readBE32`, `XXH32_round`, `XXH32_avalanche`, `XXH32_finalize`, `XXH32_endian_align`, `XXH32_update_endian`, `XXH32_digest_endian`, `XXH_read64`, `XXH_swap64`, `XXH_readLE64_align`, `XXH_readLE64`, `XXH_readBE64`, `XXH64_round`, `XXH64_mergeRound`, `XXH64_avalanche`, `XXH64_finalize`, `XXH64_endian_align`, `XXH64_update_endian`, `XXH64_digest_endian`. (`*_endian_align` / `*_update_endian` / `*_digest_endian` / `readLE*_align` are `FORCE_INLINE` templates inlined at call sites; they do not appear as IR symbols but are real translation units of work.)

**Dependencies.** None (libc).

**External boundary / required trampolines (19).** Exact signatures (Rust types: `XXH32_hash_t=u32`, `XXH64_hash_t=u64`, `XXH_errorcode=i32`, states are opaque pointers):

| Symbol | Signature |
|---|---|
| `LZ4_XXH_versionNumber` | `(void)` |
| `LZ4_XXH32` | `(const void* input, size_t len, unsigned int seed) → u32` |
| `LZ4_XXH32_createState` | `(void) → XXH32_state_t*` |
| `LZ4_XXH32_freeState` | `(XXH32_state_t* statePtr) → XXH_errorcode` |
| `LZ4_XXH32_copyState` | `(XXH32_state_t* dstState, const XXH32_state_t* srcState) → void` |
| `LZ4_XXH32_reset` | `(XXH32_state_t* statePtr, unsigned int seed) → XXH_errorcode` |
| `LZ4_XXH32_update` | `(XXH32_state_t* state_in, const void* input, size_t len) → XXH_errorcode` |
| `LZ4_XXH32_digest` | `(const XXH32_state_t* state_in) → u32` |
| `LZ4_XXH32_canonicalFromHash` | `(XXH32_canonical_t* dst, u32 hash) → void` |
| `LZ4_XXH32_hashFromCanonical` | `(const XXH32_canonical_t* src) → u32` |
| `LZ4_XXH64` | `(const void* input, size_t len, unsigned long long seed) → u64` |
| `LZ4_XXH64_createState` | `(void) → XXH64_state_t*` |
| `LZ4_XXH64_freeState` | `(XXH64_state_t* statePtr) → XXH_errorcode` |
| `LZ4_XXH64_copyState` | `(XXH64_state_t* dstState, const XXH64_state_t* srcState) → void` |
| `LZ4_XXH64_reset` | `(XXH64_state_t* statePtr, unsigned long long seed) → XXH_errorcode` |
| `LZ4_XXH64_update` | `(XXH64_state_t* state_in, const void* input, size_t len) → XXH_errorcode` |
| `LZ4_XXH64_digest` | `(const XXH64_state_t* state_in) → u64` |
| `LZ4_XXH64_canonicalFromHash` | `(XXH64_canonical_t* dst, u64 hash) → void` |
| `LZ4_XXH64_hashFromCanonical` | `(const XXH64_canonical_t* src) → u64` |

In-project callers (C5): `LZ4_XXH32`, `LZ4_XXH32_reset`, `LZ4_XXH32_update`, `LZ4_XXH32_digest`. The other 15 have **no in-project caller** — they are pure downstream API surface; keep the trampolines for ABI parity.

**ABI-fixed data.**
- **None of the state structs are caller-visible**: `XXH32_state_t` / `XXH64_state_t` are *incomplete types* in `xxhash.h` (opaque pointers to downstream). Their full definitions exist only under `XXH_STATIC_LINKING_ONLY` (xxhash.h:246-306) and are seen by exactly one other file — `lz4frame.c` (which does `#define XXH_STATIC_LINKING_ONLY` at lz4frame.c:75) — where they are embedded **by value inside the opaque `LZ4F_dctx`**. So the layout is an *inter-capsule* contract (C5 needs it), not a downstream one. Pin it in Rust if C5 embeds it (see Open Question 2):
  `XXH32_state_s { u32 total_len_32, u32 large_len, u32 v1..v4, u32 mem32[4], u32 memsize, u32 reserved }` (48 B); `XXH64_state_s { u64 total_len, u64 v1..v4, u64 mem64[4], u32 memsize, u32 reserved[2] }` (88 B).
- `XXH32_canonical_t { u8 digest[4] }`, `XXH64_canonical_t { u8 digest[8] }` — complete in the public header, passed **by value** → `#[repr(C)]`, trivial.

**Suggested chunking (4 chunks).**
1. Utilities: `XXH_malloc`, `XXH_free`, `XXH_memcpy`, `XXH_isLittleEndian`, `XXH_read32`, `XXH_swap32`, `XXH_readLE32_align`, `XXH_readLE32`, `XXH_readBE32`, `XXH_read64`, `XXH_swap64`, `XXH_readLE64_align` (12)
2. `XXH_readLE64`, `XXH_readBE64`, `XXH32_round`, `XXH32_avalanche`, `XXH32_finalize`, `XXH32_endian_align`, `XXH32_update_endian`, `XXH32_digest_endian`, `XXH64_round`, `XXH64_mergeRound`, `XXH64_avalanche`, `XXH64_finalize` (12)
3. `XXH64_endian_align`, `XXH64_update_endian`, `XXH64_digest_endian`, `LZ4_XXH32`, `LZ4_XXH64`, `LZ4_XXH32_createState`, `LZ4_XXH64_createState`, `LZ4_XXH32_freeState`, `LZ4_XXH64_freeState`, `LZ4_XXH32_copyState`, `LZ4_XXH64_copyState`, `LZ4_XXH_versionNumber` (12)
4. `LZ4_XXH32_reset`, `LZ4_XXH64_reset`, `LZ4_XXH32_update`, `LZ4_XXH64_update`, `LZ4_XXH32_digest`, `LZ4_XXH64_digest`, `LZ4_XXH32_canonicalFromHash`, `LZ4_XXH64_canonicalFromHash`, `LZ4_XXH32_hashFromCanonical`, `LZ4_XXH64_hashFromCanonical` (10)

---

### C3 — `lz4core`: fast compressor + decompressor + streaming/dictionary API (`lz4.c` minus the C1 region)

**What it does.** Levels 1..auto compression (`LZ4_compress_generic` template family), all decompression paths (`safe`, `fast`, `partial`, `withPrefix64k`, `forceExtDict`, `doubleDict`, streaming `continue`), extState one-shot APIs, streaming context management, dictionary load/save/attach, deprecated entry points, and version/bound helpers.

**Members (75 = 50 exported + 25 static).**
Exported: `LZ4_versionNumber`, `LZ4_versionString`, `LZ4_compressBound`, `LZ4_sizeofState`, `LZ4_compress`, `LZ4_compress_default`, `LZ4_compress_limitedOutput`, `LZ4_compress_fast`, `LZ4_compress_fast_continue`, `LZ4_compress_fast_extState`, `LZ4_compress_fast_extState_fastReset`, `LZ4_compress_destSize`, `LZ4_compress_destSize_extState`, `LZ4_compress_withState`, `LZ4_compress_limitedOutput_withState`, `LZ4_compress_continue`, `LZ4_compress_limitedOutput_continue`, `LZ4_compress_forceExtDict`, `LZ4_createStream`, `LZ4_initStream`, `LZ4_freeStream`, `LZ4_resetStream`, `LZ4_resetStream_fast`, `LZ4_loadDict`, `LZ4_loadDictSlow`, `LZ4_loadDict_internal`, `LZ4_attach_dictionary`, `LZ4_saveDict`, `LZ4_createStreamDecode`, `LZ4_freeStreamDecode`, `LZ4_setStreamDecode`, `LZ4_decoderRingBufferSize`, `LZ4_decompress_safe`, `LZ4_decompress_safe_partial`, `LZ4_decompress_safe_continue`, `LZ4_decompress_safe_usingDict`, `LZ4_decompress_safe_partial_usingDict`, `LZ4_decompress_safe_withPrefix64k`, `LZ4_decompress_safe_forceExtDict`, `LZ4_decompress_safe_partial_forceExtDict`, `LZ4_decompress_fast`, `LZ4_decompress_fast_continue`, `LZ4_decompress_fast_usingDict`, `LZ4_decompress_fast_withPrefix64k`, `LZ4_uncompress`, `LZ4_uncompress_unknownOutputSize`, `LZ4_sizeofStreamState`, `LZ4_resetStreamState`, `LZ4_create`, `LZ4_slideInputBuffer`.
Static: `LZ4_hash4`, `LZ4_hash5`, `LZ4_hashPosition`, `LZ4_clearHash`, `LZ4_putIndexOnHash`, `LZ4_putPositionOnHash`, `LZ4_putPosition`, `LZ4_getIndexOnHash`, `LZ4_getPositionOnHash`, `LZ4_getPosition`, `LZ4_prepareTable`, `LZ4_compress_generic_validated`, `LZ4_compress_generic`, `LZ4_compress_destSize_extState_internal`, `LZ4_stream_t_alignment`, `LZ4_renormDictT`, `read_long_length_no_check`, `LZ4_decompress_unsafe_generic`, `read_variable_length`, `LZ4_decompress_generic`, `LZ4_decompress_safe_partial_withPrefix64k`, `LZ4_decompress_safe_withSmallPrefix`, `LZ4_decompress_safe_partial_withSmallPrefix`, `LZ4_decompress_fast_extDict`, `LZ4_decompress_safe_doubleDict`. (The 16 C1 shared functions used by this capsule come from the C1 module.)

**Dependencies.** C1 only.

**External boundary / required trampolines (50).**

| Symbol | Signature |
|---|---|
| `LZ4_versionNumber` | `(void) → int` |
| `LZ4_versionString` | `(void) → const char*` |
| `LZ4_compressBound` | `(int isize) → int` |
| `LZ4_sizeofState` | `(void) → int` |
| `LZ4_compress` | `(const char* src, char* dest, int srcSize) → int` *(deprecated)* |
| `LZ4_compress_default` | `(const char* src, char* dst, int srcSize, int dstCapacity) → int` |
| `LZ4_compress_limitedOutput` | `(const char* source, char* dest, int inputSize, int maxOutputSize) → int` *(deprecated)* |
| `LZ4_compress_fast` | `(const char* src, char* dest, int srcSize, int dstCapacity, int acceleration) → int` |
| `LZ4_compress_fast_continue` | `(LZ4_stream_t* LZ4_stream, const char* source, char* dest, int inputSize, int maxOutputSize, int acceleration) → int` |
| `LZ4_compress_fast_extState` | `(void* state, const char* source, char* dest, int inputSize, int maxOutputSize, int acceleration) → int` |
| `LZ4_compress_fast_extState_fastReset` | `(void* state, const char* src, char* dst, int srcSize, int dstCapacity, int acceleration) → int` |
| `LZ4_compress_destSize` | `(const char* src, char* dst, int* srcSizePtr, int targetDstSize) → int` |
| `LZ4_compress_destSize_extState` | `(void* state, const char* src, char* dst, int* srcSizePtr, int targetDstSize, int acceleration) → int` |
| `LZ4_compress_withState` | `(void* state, const char* src, char* dst, int srcSize) → int` *(deprecated)* |
| `LZ4_compress_limitedOutput_withState` | `(void* state, const char* src, char* dst, int srcSize, int dstSize) → int` *(deprecated)* |
| `LZ4_compress_continue` | `(LZ4_stream_t* LZ4_stream, const char* source, char* dest, int inputSize) → int` *(deprecated)* |
| `LZ4_compress_limitedOutput_continue` | `(LZ4_stream_t* LZ4_stream, const char* src, char* dst, int srcSize, int dstCapacity) → int` *(deprecated)* |
| `LZ4_compress_forceExtDict` | `(LZ4_stream_t* LZ4_dict, const char* source, char* dest, int srcSize) → int` |
| `LZ4_createStream` | `(void) → LZ4_stream_t*` |
| `LZ4_initStream` | `(void* buffer, size_t size) → LZ4_stream_t*` |
| `LZ4_freeStream` | `(LZ4_stream_t* LZ4_stream) → int` |
| `LZ4_resetStream` | `(LZ4_stream_t* LZ4_stream) → void` |
| `LZ4_resetStream_fast` | `(LZ4_stream_t* ctx) → void` |
| `LZ4_loadDict` | `(LZ4_stream_t* LZ4_dict, const char* dictionary, int dictSize) → int` |
| `LZ4_loadDictSlow` | `(LZ4_stream_t* LZ4_dict, const char* dictionary, int dictSize) → int` |
| `LZ4_loadDict_internal` | `(LZ4_stream_t* LZ4_dict, const char* dictionary, int dictSize, LoadDict_mode_e _ld) → int` — ⚠ exported but declared in **no header**; `_ld` is the private enum `{_ld_fast=0,_ld_slow=1}` (lz4.c:1585); no in-project callers (only `LZ4_loadDict`/`LZ4_loadDictSlow` in the same file). Keep trampoline for ABI parity (Open Question 4). |
| `LZ4_attach_dictionary` | `(LZ4_stream_t* workingStream, const LZ4_stream_t* dictionaryStream) → void` |
| `LZ4_saveDict` | `(LZ4_stream_t* LZ4_dict, char* safeBuffer, int dictSize) → int` |
| `LZ4_createStreamDecode` | `(void) → LZ4_streamDecode_t*` |
| `LZ4_freeStreamDecode` | `(LZ4_streamDecode_t* LZ4_stream) → int` |
| `LZ4_setStreamDecode` | `(LZ4_streamDecode_t* LZ4_streamDecode, const char* dictionary, int dictSize) → int` |
| `LZ4_decoderRingBufferSize` | `(int maxBlockSize) → int` |
| `LZ4_decompress_safe` | `(const char* source, char* dest, int compressedSize, int maxDecompressedSize) → int` |
| `LZ4_decompress_safe_partial` | `(const char* src, char* dst, int compressedSize, int targetOutputSize, int dstCapacity) → int` |
| `LZ4_decompress_safe_continue` | `(LZ4_streamDecode_t* LZ4_streamDecode, const char* src, char* dst, int srcSize, int dstCapacity) → int` |
| `LZ4_decompress_safe_usingDict` | `(const char* source, char* dest, int compressedSize, int maxOutputSize, const char* dictStart, int dictSize) → int` |
| `LZ4_decompress_safe_partial_usingDict` | `(const char* source, char* dest, int compressedSize, int targetOutputSize, int maxOutputSize, const char* dictStart, int dictSize) → int` |
| `LZ4_decompress_safe_withPrefix64k` | `(const char* source, char* dest, int compressedSize, int maxOutputSize) → int` *(deprecated)* |
| `LZ4_decompress_safe_forceExtDict` | `(const char* source, char* dest, int compressedSize, int maxOutputSize, const void* dictStart, size_t dictSize) → int` |
| `LZ4_decompress_safe_partial_forceExtDict` | `(const char* source, char* dest, int compressedSize, int targetOutputSize, int dstCapacity, const void* dictStart, size_t dictSize) → int` |
| `LZ4_decompress_fast` | `(const char* source, char* dest, int originalSize) → int` |
| `LZ4_decompress_fast_continue` | `(LZ4_streamDecode_t* LZ4_streamDecode, const char* source, char* dest, int originalSize) → int` |
| `LZ4_decompress_fast_usingDict` | `(const char* source, char* dest, int originalSize, const char* dictStart, int dictSize) → int` |
| `LZ4_decompress_fast_withPrefix64k` | `(const char* source, char* dest, int originalSize) → int` *(deprecated)* |
| `LZ4_uncompress` | `(const char* source, char* dest, int outputSize) → int` *(deprecated)* |
| `LZ4_uncompress_unknownOutputSize` | `(const char* source, char* dest, int isize, int maxOutputSize) → int` *(deprecated)* |
| `LZ4_sizeofStreamState` | `(void) → int` *(deprecated)* |
| `LZ4_resetStreamState` | `(void* state, char* inputBuffer) → int` *(deprecated)* |
| `LZ4_create` | `(char* inputBuffer) → void*` *(deprecated)* |
| `LZ4_slideInputBuffer` | `(void* state) → char*` *(deprecated)* |

**Cross-capsule callers (the load-bearing edges).**
- From **C4 lz4hc**: `LZ4_compressBound` (7 sites: lz4hc.c:2133-2185 wrappers + `LZ4_compress_HC`).
- From **C5 lz4frame**: `LZ4_initStream` (LZ4F_compressBegin_internal, LZ4F_createCDict_advanced, LZ4F_compressFrame), `LZ4_loadDict` (LZ4F_compressBegin_internal), `LZ4_loadDictSlow` (LZ4F_createCDict_advanced), `LZ4_compress_fast_continue` (LZ4F_compressBlock, LZ4F_compressBlock_continue), `LZ4_compress_fast_extState_fastReset` (LZ4F_compressBlock), `LZ4_decompress_safe_usingDict` (LZ4F_decompress), `LZ4_attach_dictionary` (LZ4F_initStream), `LZ4_resetStream_fast` (LZ4F_initStream), `LZ4_saveDict` (LZ4F_localSaveDict), `LZ4_sizeofState` (ctxTypeID_to_size).

**ABI-fixed data — pin before writing logic.**
- **`LZ4_stream_t`** = `union LZ4_stream_u { char minStateSize[LZ4_STREAM_MINSIZE]; LZ4_stream_t_internal internal_donotuse; }` (lz4.h:719-732). With default `LZ4_MEMORY_USAGE=14`: `LZ4_HASH_SIZE_U32 = 1<<(14-2) = 4096`; internal = `u32 hashTable[4096]; const u8* dictionary; const LZ4_stream_t* dictCtx; u32 currentOffset; u32 tableType; u32 dictSize;` (+ compiler padding to 16416). `LZ4_STREAM_MINSIZE = (1<<14)+32 = 16416`. **Callers embed this by value** — most importantly C5's `LZ4F_compressFrame` (LZ4F_HEAPMODE=0) declares `LZ4_stream_t lz4ctx;` on its stack (lz4frame.c:499). Must be a `#[repr(C)]` union with compile-time `size_of==16416` + alignment asserts.
- **`LZ4_streamDecode_t`** = `union LZ4_streamDecode_u { char minStateSize[32]; struct { const u8* externalDict; const u8* prefixEnd; size_t extDictSize; size_t prefixSize; } internal_donotuse; }` (lz4.h:758-769). Callers may embed it (it is the `LZ4_streamDecode_t*` parameter of the `continue` APIs). `#[repr(C)]`, size 32.
- `LZ4_stream_t_internal` is referenced via `dictCtx` pointers across C3/C5 (`LZ4_attach_dictionary`, `LZ4_loadDict` semantics) — the pointer fields above are the contract.

**Suggested chunking (7 chunks, leaf-first from the IR order).**
1. Table micro-ops: `LZ4_hash4`, `LZ4_hash5`, `LZ4_hashPosition`, `LZ4_clearHash`, `LZ4_putIndexOnHash`, `LZ4_putPositionOnHash`, `LZ4_putPosition`, `LZ4_getIndexOnHash`, `LZ4_getPositionOnHash`, `LZ4_getPosition`, `LZ4_prepareTable`, `LZ4_renormDictT` (12)
2. Compression core: `LZ4_compress_generic_validated`, `LZ4_compress_generic`, `LZ4_compress_fast`, `LZ4_compress_default`, `LZ4_compress_fast_extState`, `LZ4_compress_fast_extState_fastReset`, `LZ4_compress_destSize_extState_internal`, `LZ4_compress_destSize_extState`, `LZ4_compress_destSize`, `LZ4_compress_fast_continue`, `LZ4_compress_forceExtDict`, `LZ4_compressBound` (12)
3. Stream state: `LZ4_stream_t_alignment`, `LZ4_initStream`, `LZ4_createStream`, `LZ4_freeStream`, `LZ4_resetStream`, `LZ4_resetStream_fast`, `LZ4_loadDict_internal`, `LZ4_loadDict`, `LZ4_loadDictSlow`, `LZ4_attach_dictionary`, `LZ4_saveDict`, `LZ4_sizeofState` (12)
4. Decompression core: `read_variable_length`, `read_long_length_no_check`, `LZ4_decompress_unsafe_generic`, `LZ4_decompress_generic`, `LZ4_decompress_safe_doubleDict`, `LZ4_decompress_safe`, `LZ4_decompress_safe_partial`, `LZ4_decompress_fast`, `LZ4_decompress_safe_partial_withPrefix64k`, `LZ4_decompress_fast_withPrefix64k`, `LZ4_decompress_safe_withSmallPrefix`, `LZ4_decompress_safe_partial_withSmallPrefix` (12)
5. Decompression variants: `LZ4_decompress_safe_withPrefix64k`, `LZ4_decompress_safe_forceExtDict`, `LZ4_decompress_safe_partial_forceExtDict`, `LZ4_decompress_fast_extDict`, `LZ4_decompress_safe_continue`, `LZ4_decompress_fast_continue`, `LZ4_decompress_safe_usingDict`, `LZ4_decompress_safe_partial_usingDict`, `LZ4_decompress_fast_usingDict`, `LZ4_createStreamDecode`, `LZ4_freeStreamDecode`, `LZ4_setStreamDecode` (12)
6. Legacy/deprecated: `LZ4_compress_limitedOutput`, `LZ4_compress`, `LZ4_compress_limitedOutput_withState`, `LZ4_compress_withState`, `LZ4_compress_limitedOutput_continue`, `LZ4_compress_continue`, `LZ4_uncompress`, `LZ4_uncompress_unknownOutputSize`, `LZ4_decoderRingBufferSize`, `LZ4_sizeofStreamState`, `LZ4_resetStreamState`, `LZ4_create` (12)
7. Trailing: `LZ4_slideInputBuffer`, `LZ4_versionNumber`, `LZ4_versionString` (3)

---

### C4 — `lz4hc`: high-compression engine (incl. MID level 2) + HC API (`lz4hc.c`)

**What it does.** Compression levels 3–12: hash-chain (`lz4hc`), optimal parsing (`lz4opt`), and the custom level-2 `LZ4MID` strategy (dual 4/8-byte hash tables over `cctx->hashTable`, dictionary search dispatched by function pointer). HC streaming context management, HC dictionary load/save/attach, one-shot/extState HC APIs, and the deprecated `LZ4_compressHC*` entry points. Its private copies of the C1 region (via the `#include "lz4.c"`) are **not** members — in Rust they become calls into the C1 module.

**Members (74 = 35 exported + 39 static).**
Exported: `LZ4HC_searchExtDict`, `LZ4_sizeofStateHC`, `LZ4_compress_HC`, `LZ4_compress_HC_continue`, `LZ4_compress_HC_continue_destSize`, `LZ4_compress_HC_destSize`, `LZ4_compress_HC_extStateHC`, `LZ4_compress_HC_extStateHC_fastReset`, `LZ4_createStreamHC`, `LZ4_freeStreamHC`, `LZ4_initStreamHC`, `LZ4_resetStreamHC`, `LZ4_resetStreamHC_fast`, `LZ4_setCompressionLevel`, `LZ4_favorDecompressionSpeed`, `LZ4_loadDictHC`, `LZ4_attach_HC_dictionary`, `LZ4_saveDictHC`, `LZ4_compressHC`, `LZ4_compressHC_limitedOutput`, `LZ4_compressHC2`, `LZ4_compressHC2_limitedOutput`, `LZ4_compressHC_withStateHC`, `LZ4_compressHC_limitedOutput_withStateHC`, `LZ4_compressHC2_withStateHC`, `LZ4_compressHC2_limitedOutput_withStateHC`, `LZ4_compressHC_continue`, `LZ4_compressHC_limitedOutput_continue`, `LZ4_compressHC2_continue`, `LZ4_compressHC2_limitedOutput_continue`, `LZ4_sizeofStreamStateHC`, `LZ4_resetStreamStateHC`, `LZ4_createHC`, `LZ4_freeHC`, `LZ4_slideInputBufferHC`.
Static: `LZ4HC_getCLevelParams`, `LZ4HC_hashPtr`, `LZ4_read64`, `LZ4_readLE64`, `LZ4MID_hash4`, `LZ4MID_hash4Ptr`, `LZ4MID_hash7`, `LZ4MID_hash8Ptr`, `LZ4HC_NbCommonBytes32`, `LZ4HC_countBack`, `LZ4HC_clearTables`, `LZ4HC_init_internal`, `LZ4HC_encodeSequence`, `LZ4MID_searchHCDict`, `LZ4MID_searchExtDict`, `LZ4MID_addPosition`, `LZ4MID_fillHTable`, `select_searchDict_function`, `LZ4MID_compress`, `LZ4HC_Insert`, `LZ4HC_rotatePattern`, `LZ4HC_countPattern`, `LZ4HC_reverseCountPattern`, `LZ4HC_protectDictEnd`, `LZ4HC_InsertAndGetWiderMatch`, `LZ4HC_InsertAndFindBestMatch`, `LZ4HC_compress_hashChain`, `LZ4HC_compress_generic_internal`, `LZ4HC_compress_generic_noDictCtx`, `isStateCompatible`, `LZ4HC_compress_generic_dictCtx`, `LZ4HC_compress_generic`, `LZ4_streamHC_t_alignment`, `LZ4HC_setExternalDict`, `LZ4_compressHC_continue_generic`, `LZ4HC_literalsPrice`, `LZ4HC_sequencePrice`, `LZ4HC_FindLongerMatch`, `LZ4HC_compress_optimal`.

**Dependencies.** C1 (its private shared copies → module calls), **C3** (`LZ4_compressBound` only).

**External boundary / required trampolines (35).**

| Symbol | Signature |
|---|---|
| `LZ4_compress_HC` | `(const char* src, char* dst, int srcSize, int dstCapacity, int compressionLevel) → int` |
| `LZ4_compress_HC_continue` | `(LZ4_streamHC_t* LZ4_streamHCPtr, const char* src, char* dst, int srcSize, int dstCapacity) → int` |
| `LZ4_compress_HC_continue_destSize` | `(LZ4_streamHC_t* LZ4_streamHCPtr, const char* src, char* dst, int* srcSizePtr, int targetDestSize) → int` |
| `LZ4_compress_HC_destSize` | `(void* state, const char* source, char* dest, int* sourceSizePtr, int targetDestSize, int cLevel) → int` |
| `LZ4_compress_HC_extStateHC` | `(void* state, const char* src, char* dst, int srcSize, int dstCapacity, int compressionLevel) → int` |
| `LZ4_compress_HC_extStateHC_fastReset` | `(void* state, const char* src, char* dst, int srcSize, int dstCapacity, int compressionLevel) → int` |
| `LZ4_createStreamHC` | `(void) → LZ4_streamHC_t*` |
| `LZ4_freeStreamHC` | `(LZ4_streamHC_t* LZ4_streamHCPtr) → int` |
| `LZ4_initStreamHC` | `(void* buffer, size_t size) → LZ4_streamHC_t*` |
| `LZ4_resetStreamHC` | `(LZ4_streamHC_t* LZ4_streamHCPtr, int compressionLevel) → void` |
| `LZ4_resetStreamHC_fast` | `(LZ4_streamHC_t* LZ4_streamHCPtr, int compressionLevel) → void` |
| `LZ4_setCompressionLevel` | `(LZ4_streamHC_t* LZ4_streamHCPtr, int compressionLevel) → void` |
| `LZ4_favorDecompressionSpeed` | `(LZ4_streamHC_t* LZ4_streamHCPtr, int favor) → void` |
| `LZ4_loadDictHC` | `(LZ4_streamHC_t* LZ4_streamHCPtr, const char* dictionary, int dictSize) → int` |
| `LZ4_attach_HC_dictionary` | `(LZ4_streamHC_t *working_stream, const LZ4_streamHC_t *dictionary_stream) → void` |
| `LZ4_saveDictHC` | `(LZ4_streamHC_t* LZ4_streamHCPtr, char* safeBuffer, int dictSize) → int` |
| `LZ4_sizeofStateHC` | `(void) → int` |
| `LZ4_sizeofStreamStateHC` | `(void) → int` *(deprecated)* |
| `LZ4_resetStreamStateHC` | `(void* state, char* inputBuffer) → int` *(deprecated)* |
| `LZ4_createHC` | `(const char* inputBuffer) → void*` *(deprecated)* |
| `LZ4_freeHC` | `(void* LZ4HC_Data) → int` *(deprecated)* |
| `LZ4_slideInputBufferHC` | `(void* LZ4HC_Data) → char*` *(deprecated)* |
| `LZ4_compressHC` | `(const char* source, char* dest, int inputSize) → int` *(deprecated)* |
| `LZ4_compressHC_limitedOutput` | `(const char* source, char* dest, int inputSize, int maxOutputSize) → int` *(deprecated)* |
| `LZ4_compressHC2` | `(const char* source, char* dest, int inputSize, int compressionLevel) → int` *(deprecated)* |
| `LZ4_compressHC2_limitedOutput` | `(const char* source, char* dest, int inputSize, int maxOutputSize, int compressionLevel) → int` *(deprecated)* |
| `LZ4_compressHC_withStateHC` | `(void* state, const char* source, char* dest, int inputSize) → int` *(deprecated)* |
| `LZ4_compressHC_limitedOutput_withStateHC` | `(void* state, const char* source, char* dest, int inputSize, int maxOutputSize) → int` *(deprecated)* |
| `LZ4_compressHC2_withStateHC` | `(void* state, const char* source, char* dest, int inputSize, int compressionLevel) → int` *(deprecated)* |
| `LZ4_compressHC2_limitedOutput_withStateHC` | `(void* state, const char* source, char* dest, int inputSize, int maxOutputSize, int compressionLevel) → int` *(deprecated)* |
| `LZ4_compressHC_continue` | `(LZ4_streamHC_t* ctx, const char* source, char* dest, int inputSize) → int` *(deprecated)* |
| `LZ4_compressHC_limitedOutput_continue` | `(LZ4_streamHC_t* ctx, const char* source, char* dest, int inputSize, int maxOutputSize) → int` *(deprecated)* |
| `LZ4_compressHC2_continue` | `(void* LZ4HC_Data, const char* source, char* dest, int inputSize, int compressionLevel) → int` *(deprecated)* |
| `LZ4_compressHC2_limitedOutput_continue` | `(void* LZ4HC_Data, const char* source, char* dest, int inputSize, int maxOutputSize, int compressionLevel) → int` *(deprecated)* |
| `LZ4HC_searchExtDict` | `(const BYTE* ip, U32 ipIndex, const BYTE* const iLowLimit, const BYTE* const iHighLimit, const LZ4HC_CCtx_internal* dictCtx, U32 gDictEndIndex, int currentBestML, int nbAttempts) → LZ4HC_match_t` — ⚠ exported, declared in **no header**, no cross-file caller (only `LZ4MID_searchHCDict`, same file). Its `LZ4HC_CCtx_internal` parameter type is fully defined in `lz4hc.h`, so it is nominally callable. Keep trampoline for ABI parity (Open Question 4). |

**Cross-capsule callers (load-bearing edges, all from C5 lz4frame).**
`LZ4_initStreamHC`, `LZ4_loadDictHC`, `LZ4_setCompressionLevel`, `LZ4_favorDecompressionSpeed` (LZ4F_compressBegin_internal / LZ4F_createCDict_advanced); `LZ4_compress_HC_continue` (LZ4F_compressBlockHC, LZ4F_compressBlockHC_continue); `LZ4_compress_HC_extStateHC_fastReset` (LZ4F_compressBlockHC); `LZ4_attach_HC_dictionary`, `LZ4_resetStreamHC_fast` (LZ4F_initStream); `LZ4_saveDictHC` (LZ4F_localSaveDict); `LZ4_sizeofStateHC` (ctxTypeID_to_size).

**ABI-fixed data — pin before writing logic.**
- **`LZ4_streamHC_t`** = `union LZ4_streamHC_u { char minStateSize[262200]; LZ4HC_CCtx_internal internal_donotuse; }` (lz4hc.h:234-258). Internal: `u32 hashTable[32768]` (LZ4HC_HASH_LOG=15); `u16 chainTable[65536]` (LZ4HC_MAXD); `const u8* end, *prefixStart, *dictStart`; `u32 dictLimit, lowLimit, nextToUpdate`; `short compressionLevel`; `i8 favorDecSpeed; i8 dirty`; `const LZ4HC_CCtx_internal* dictCtx`. Callers may embed it by value (lz4hc.h invites static allocation), and **C5 allocates it as an opaque byte buffer** sized by `LZ4_sizeofStateHC()` / `sizeof(LZ4_streamHC_t)` and hands it to `LZ4_initStreamHC` — so C5 needs only the *size* across the boundary, but any direct C caller of the public header needs the layout. `#[repr(C)]` + size 262200 / alignment asserts.
- **MID invariant (internal, not ABI):** the level-2 engine reinterprets `hashTable` as `hash4Table[0..16384) + hash8Table[16384..32768)` (`LZ4MID_HASHLOG = LZ4HC_HASH_LOG-1`, lz4hc.c:141-143; `LZ4MID_fillHTable` at :488). Preserve the exact table geometry in the Rust port.
- `LZ4HC_CCtx_internal` is shared with C5 only through `LZ4HC_searchExtDict`'s parameter (opaque-pointer-ish usage) and through `dictCtx` fields — no C5 code touches its fields directly.

**Indirect dispatch inside this capsule.** `LZ4MID_compress` calls `searchIntoDict(...)` through `LZ4MID_searchIntoDict_f` chosen by `select_searchDict_function` — see Section 3-of-plan (indirect findings) #2.

**Suggested chunking (7 chunks, leaf-first).**
1. Params/hash/util: `LZ4HC_getCLevelParams`, `LZ4HC_hashPtr`, `LZ4_read64`, `LZ4_readLE64`, `LZ4MID_hash4`, `LZ4MID_hash4Ptr`, `LZ4MID_hash7`, `LZ4MID_hash8Ptr`, `LZ4HC_NbCommonBytes32`, `LZ4HC_countBack`, `LZ4HC_clearTables`, `LZ4HC_init_internal` (12)
2. Match/pattern primitives: `LZ4HC_rotatePattern`, `LZ4HC_countPattern`, `LZ4HC_reverseCountPattern`, `LZ4HC_protectDictEnd`, `LZ4HC_Insert`, `LZ4HC_encodeSequence`, `LZ4HC_FindLongerMatch`, `LZ4HC_literalsPrice`, `LZ4HC_sequencePrice`, `LZ4HC_InsertAndGetWiderMatch`, `LZ4HC_InsertAndFindBestMatch`, `LZ4HC_compress_hashChain` (12)
3. MID + generic engine: `LZ4MID_addPosition`, `LZ4MID_fillHTable`, `select_searchDict_function`, `LZ4MID_searchHCDict`, `LZ4MID_searchExtDict`, `LZ4HC_searchExtDict`, `LZ4MID_compress`, `isStateCompatible`, `LZ4HC_compress_generic_internal`, `LZ4HC_compress_generic_noDictCtx`, `LZ4HC_compress_generic_dictCtx`, `LZ4HC_compress_generic` (12)
4. Optimal + context API: `LZ4HC_compress_optimal`, `LZ4HC_setExternalDict`, `LZ4_compressHC_continue_generic`, `LZ4_sizeofStateHC`, `LZ4_streamHC_t_alignment`, `LZ4_initStreamHC`, `LZ4_createStreamHC`, `LZ4_freeStreamHC`, `LZ4_resetStreamHC`, `LZ4_resetStreamHC_fast`, `LZ4_setCompressionLevel`, `LZ4_favorDecompressionSpeed` (12)
5. Dict + one-shot HC: `LZ4_loadDictHC`, `LZ4_attach_HC_dictionary`, `LZ4_saveDictHC`, `LZ4_compress_HC`, `LZ4_compress_HC_extStateHC`, `LZ4_compress_HC_extStateHC_fastReset`, `LZ4_compress_HC_destSize`, `LZ4_compress_HC_continue`, `LZ4_compress_HC_continue_destSize`, `LZ4_sizeofStreamStateHC`, `LZ4_resetStreamStateHC`, `LZ4_createHC` (12)
6. Deprecated (a): `LZ4_freeHC`, `LZ4_slideInputBufferHC`, `LZ4_compressHC`, `LZ4_compressHC_limitedOutput`, `LZ4_compressHC2`, `LZ4_compressHC2_limitedOutput`, `LZ4_compressHC_withStateHC`, `LZ4_compressHC_limitedOutput_withStateHC`, `LZ4_compressHC2_withStateHC`, `LZ4_compressHC2_limitedOutput_withStateHC`, `LZ4_compressHC_continue`, `LZ4_compressHC_limitedOutput_continue` (12)
7. Deprecated (b): `LZ4_compressHC2_continue`, `LZ4_compressHC2_limitedOutput_continue` (2)

---

### C5 — `lz4frame`: LZ4 frame format (streaming + one-shot + CDict) (`lz4frame.c`)

**What it does.** Frame header encode/decode (magic, FLG/BD/BK/C/HC bits, content size, dictID, header checksum via XXH32), block framing with optional block checksum, content checksum, streaming compression (`LZ4F_cctx`) and decompression (`LZ4F_dctx`), one-shot `LZ4F_compressFrame`, bulk-dictionary `LZ4F_CDict`, custom-allocator plumbing, skippable frames, and the custom per-block **uncompressed** mode (`LZ4F_BlockCompressMode_e` + `LZ4F_uncompressedUpdate`).

**Members (57 = 33 exported + 24 static).**
Exported: `LZ4F_isError`, `LZ4F_getErrorName`, `LZ4F_getErrorCode`, `LZ4F_getVersion`, `LZ4F_compressionLevel_max`, `LZ4F_getBlockSize`, `LZ4F_compressFrameBound`, `LZ4F_compressFrame`, `LZ4F_compressFrame_usingCDict`, `LZ4F_createCDict`, `LZ4F_createCDict_advanced`, `LZ4F_freeCDict`, `LZ4F_createCompressionContext`, `LZ4F_createCompressionContext_advanced`, `LZ4F_freeCompressionContext`, `LZ4F_compressBegin_internal`, `LZ4F_compressBegin`, `LZ4F_compressBegin_usingDict`, `LZ4F_compressBegin_usingDictOnce`, `LZ4F_compressBegin_usingCDict`, `LZ4F_compressBound`, `LZ4F_compressUpdate`, `LZ4F_uncompressedUpdate`, `LZ4F_flush`, `LZ4F_compressEnd`, `LZ4F_createDecompressionContext`, `LZ4F_createDecompressionContext_advanced`, `LZ4F_freeDecompressionContext`, `LZ4F_resetDecompressionContext`, `LZ4F_headerSize`, `LZ4F_getFrameInfo`, `LZ4F_decompress`, `LZ4F_decompress_usingDict`.
Static: `LZ4F_calloc`, `LZ4F_malloc`, `LZ4F_free`, `LZ4F_readLE32`, `LZ4F_writeLE32`, `LZ4F_readLE64`, `LZ4F_writeLE64`, `LZ4F_returnErrorCode`, `LZ4F_headerChecksum`, `LZ4F_optimalBSID`, `LZ4F_compressBound_internal`, `LZ4F_initStream`, `ctxTypeID_to_size`, `LZ4F_makeBlock`, `LZ4F_compressBlock`, `LZ4F_compressBlock_continue`, `LZ4F_compressBlockHC`, `LZ4F_compressBlockHC_continue`, `LZ4F_doNotCompressBlock`, `LZ4F_selectCompression`, `LZ4F_localSaveDict`, `LZ4F_compressUpdateImpl`, `LZ4F_decodeHeader`, `LZ4F_updateDict`.

**Dependencies.** C3 (9 symbols), C4 (10 symbols), C2 (4 symbols) — see the edge lists in C3/C4 sections.

**External boundary / required trampolines (33).**

| Symbol | Signature |
|---|---|
| `LZ4F_isError` | `(LZ4F_errorCode_t code) → unsigned` (`LZ4F_errorCode_t = size_t`) |
| `LZ4F_getErrorName` | `(LZ4F_errorCode_t code) → const char*` |
| `LZ4F_getErrorCode` | `(size_t functionResult) → LZ4F_errorCodes` (static-linking-only declaration) |
| `LZ4F_getVersion` | `(void) → unsigned` |
| `LZ4F_compressionLevel_max` | `(void) → int` |
| `LZ4F_getBlockSize` | `(LZ4F_blockSizeID_t blockSizeID) → size_t` (static-linking-only declaration) |
| `LZ4F_compressFrameBound` | `(size_t srcSize, const LZ4F_preferences_t* preferencesPtr) → size_t` |
| `LZ4F_compressFrame` | `(void* dstBuffer, size_t dstCapacity, const void* srcBuffer, size_t srcSize, const LZ4F_preferences_t* preferencesPtr) → size_t` |
| `LZ4F_compressFrame_usingCDict` | `(LZ4F_cctx* cctx, void* dstBuffer, size_t dstCapacity, const void* srcBuffer, size_t srcSize, const LZ4F_CDict* cdict, const LZ4F_preferences_t* preferencesPtr) → size_t` |
| `LZ4F_createCDict` | `(const void* dictBuffer, size_t dictSize) → LZ4F_CDict*` |
| `LZ4F_createCDict_advanced` | `(LZ4F_CustomMem cmem, const void* dictBuffer, size_t dictSize) → LZ4F_CDict*` (static-linking-only) |
| `LZ4F_freeCDict` | `(LZ4F_CDict* cdict) → void` |
| `LZ4F_createCompressionContext` | `(LZ4F_cctx** cctxPtr, unsigned version) → LZ4F_errorCode_t` |
| `LZ4F_createCompressionContext_advanced` | `(LZ4F_CustomMem customMem, unsigned version) → LZ4F_cctx*` (static-linking-only) |
| `LZ4F_freeCompressionContext` | `(LZ4F_cctx* cctx) → LZ4F_errorCode_t` |
| `LZ4F_compressBegin_internal` | `(LZ4F_cctx* cctx, void* dstBuffer, size_t dstCapacity, const void* dictBuffer, size_t dictSize, const LZ4F_CDict* cdict, const LZ4F_preferences_t* preferencesPtr) → size_t` — ⚠ exported, declared in **no header**, no cross-file caller (same-file only). Keep for ABI parity (Open Question 4). |
| `LZ4F_compressBegin` | `(LZ4F_cctx* cctx, void* dstBuffer, size_t dstCapacity, const LZ4F_preferences_t* preferencesPtr) → size_t` |
| `LZ4F_compressBegin_usingDict` | `(LZ4F_cctx* cctx, void* dstBuffer, size_t dstCapacity, const void* dictBuffer, size_t dictSize, const LZ4F_preferences_t* preferencesPtr) → size_t` |
| `LZ4F_compressBegin_usingDictOnce` | `(LZ4F_cctx* cctx, void* dstBuffer, size_t dstCapacity, const void* dictBuffer, size_t dictSize, const LZ4F_preferences_t* preferencesPtr) → size_t` — ⚠ exported, no header declaration, same-file caller only. Keep for ABI parity. |
| `LZ4F_compressBegin_usingCDict` | `(LZ4F_cctx* cctx, void* dstBuffer, size_t dstCapacity, const LZ4F_CDict* cdict, const LZ4F_preferences_t* preferencesPtr) → size_t` |
| `LZ4F_compressBound` | `(size_t srcSize, const LZ4F_preferences_t* preferencesPtr) → size_t` |
| `LZ4F_compressUpdate` | `(LZ4F_cctx* cctxPtr, void* dstBuffer, size_t dstCapacity, const void* srcBuffer, size_t srcSize, const LZ4F_compressOptions_t* compressOptionsPtr) → size_t` |
| `LZ4F_uncompressedUpdate` | `(LZ4F_cctx* cctxPtr, void* dstBuffer, size_t dstCapacity, const void* srcBuffer, size_t srcSize, const LZ4F_compressOptions_t* compressOptionsPtr) → size_t` (static-linking-only declaration; custom feature) |
| `LZ4F_flush` | `(LZ4F_cctx* cctxPtr, void* dstBuffer, size_t dstCapacity, const LZ4F_compressOptions_t* compressOptionsPtr) → size_t` |
| `LZ4F_compressEnd` | `(LZ4F_cctx* cctxPtr, void* dstBuffer, size_t dstCapacity, const LZ4F_compressOptions_t* compressOptionsPtr) → size_t` |
| `LZ4F_createDecompressionContext` | `(LZ4F_dctx** dctxPtr, unsigned version) → LZ4F_errorCode_t` |
| `LZ4F_createDecompressionContext_advanced` | `(LZ4F_CustomMem customMem, unsigned version) → LZ4F_dctx*` (static-linking-only) |
| `LZ4F_freeDecompressionContext` | `(LZ4F_dctx* dctx) → LZ4F_errorCode_t` |
| `LZ4F_resetDecompressionContext` | `(LZ4F_dctx* dctx) → void` |
| `LZ4F_headerSize` | `(const void* src, size_t srcSize) → size_t` |
| `LZ4F_getFrameInfo` | `(LZ4F_dctx* dctx, LZ4F_frameInfo_t* frameInfoPtr, const void* srcBuffer, size_t* srcSizePtr) → size_t` |
| `LZ4F_decompress` | `(LZ4F_dctx* dctx, void* dstBuffer, size_t* dstSizePtr, const void* srcBuffer, size_t* srcSizePtr, const LZ4F_decompressOptions_t* decompressOptionsPtr) → size_t` |
| `LZ4F_decompress_usingDict` | `(LZ4F_dctx* dctx, void* dstBuffer, size_t* dstSizePtr, const void* srcBuffer, size_t* srcSizePtr, const void* dict, size_t dictSize, const LZ4F_decompressOptions_t* decompressOptionsPtr) → size_t` |

**Cross-capsule callers (load-bearing edges, all from C6 lz4file).** `LZ4F_isError`, `LZ4F_createDecompressionContext`, `LZ4F_freeDecompressionContext`, `LZ4F_getFrameInfo`, `LZ4F_decompress` (read path); `LZ4F_createCompressionContext`, `LZ4F_freeCompressionContext`, `LZ4F_compressBound`, `LZ4F_compressBegin`, `LZ4F_compressUpdate`, `LZ4F_compressEnd` (write path). The remaining 22 exports are downstream API surface.

**ABI-fixed data — pin before writing logic.** All complete-in-header structs passed by value or filled by callers:
- `LZ4F_frameInfo_t { LZ4F_blockSizeID_t blockSizeID; LZ4F_blockMode_t blockMode; LZ4F_contentChecksum_t contentChecksumFlag; LZ4F_frameType_t frameType; u64 contentSize; u32 dictID; LZ4F_blockChecksum_t blockChecksumFlag; }` (lz4frame.h:175-183) — enums are `int`-sized C enums.
- `LZ4F_preferences_t { LZ4F_frameInfo_t frameInfo; int compressionLevel; unsigned autoFlush; unsigned favorDecSpeed; unsigned reserved[3]; }` (lz4frame.h:192-198).
- `LZ4F_compressOptions_t { unsigned stableSrc; unsigned reserved[3]; }` (lz4frame.h:249-252).
- `LZ4F_decompressOptions_t { unsigned stableDst, skipChecksums, reserved1, reserved0; }` (lz4frame.h:371-379).
- `LZ4F_CustomMem { LZ4F_AllocFunction customAlloc; LZ4F_CallocFunction customCalloc; LZ4F_FreeFunction customFree; void* opaqueState; }` (lz4frame.h:727-735) — **contains three function pointers**; `LZ4F_defaultCMem` is a `static const` in the header (each TU gets its own copy — no issue).
- Error encoding: `LZ4F_errorCode_t = size_t`; errors are the two's-complement negation of the `LZ4F_errorCodes` enum values (`LZ4F_returnErrorCode`, lz4frame.c:311); `LZ4F_isError` compares against the max-code boundary. Preserve exact bit pattern (Open Question 5).
- **Opaque (not ABI-fixed, defined in the .c file):** `struct LZ4F_cctx_s` (lz4frame.c:264-283: cmem, prefs, version, cStage, cdict*, maxBlockSize, maxBufferSize, tmpBuff/tmpIn, tmpInSize, totalInSize, `XXH32_state_t xxh` **by value**, `void* lz4CtxPtr`, U16 lz4CtxAlloc, U16 lz4CtxType, blockCompressMode), `struct LZ4F_dctx_s` (lz4frame.c:1265-1282: …, `XXH32_state_t xxh` + `XXH32_state_t blockChecksum` **by value**, 19-byte header array, dStage_t), `struct LZ4F_CDict_s` (lz4frame.c:531-537: cmem, dictContent, fastCtx `LZ4_stream_t*`, HCCtx `LZ4_streamHC_t*`). Callers only ever hold pointers to these.
- **Cross-capsule layout dependencies (not caller-visible, but real):** `cctx.lz4CtxPtr` is a `LZ4F_malloc`'d buffer of `sizeof(LZ4_stream_t)` or `sizeof(LZ4_streamHC_t)` initialized via `LZ4_initStream`/`LZ4_initStreamHC` (sizes also come from `LZ4_sizeofState()`/`LZ4_sizeofStateHC()` via `ctxTypeID_to_size`) — C5 can treat it as an opaque byte blob sized by C3/C4. But `LZ4F_compressFrame` (HEAPMODE=0) declares **`LZ4_stream_t lz4ctx;` by value on the stack** (lz4frame.c:499) — so C5's Rust port does need C3's `#[repr(C)]` `LZ4_stream_t` (or a same-size `MaybeUninit<[u8;16416]>` + size check, if that is deemed safer — see Open Question 2). C5 also needs the `XXH32_state_t` layout (C2) to embed it in cctx/dctx, unless the design switches to opaque state pointers (Open Question 2).

**Indirect dispatch inside this capsule.** `LZ4F_makeBlock` calls `compress(...)` through `compressFunc_t` chosen by `LZ4F_selectCompression` — see indirect findings #1.

**Suggested chunking (5 chunks, leaf-first).**
1. Utilities/errors: `LZ4F_calloc`, `LZ4F_malloc`, `LZ4F_free`, `LZ4F_readLE32`, `LZ4F_writeLE32`, `LZ4F_readLE64`, `LZ4F_writeLE64`, `LZ4F_returnErrorCode`, `LZ4F_isError`, `LZ4F_getErrorName`, `LZ4F_getErrorCode`, `LZ4F_getVersion` (12)
2. Compression dispatch: `LZ4F_compressBlock`, `LZ4F_compressBlock_continue`, `LZ4F_compressBlockHC`, `LZ4F_compressBlockHC_continue`, `LZ4F_doNotCompressBlock`, `LZ4F_selectCompression`, `LZ4F_makeBlock`, `LZ4F_localSaveDict`, `LZ4F_initStream`, `ctxTypeID_to_size`, `LZ4F_optimalBSID`, `LZ4F_headerChecksum` (12)
3. Begin/bound/update: `LZ4F_getBlockSize`, `LZ4F_compressBound_internal`, `LZ4F_compressBound`, `LZ4F_compressFrameBound`, `LZ4F_compressionLevel_max`, `LZ4F_compressBegin_internal`, `LZ4F_compressBegin`, `LZ4F_compressBegin_usingDictOnce`, `LZ4F_compressBegin_usingDict`, `LZ4F_compressBegin_usingCDict`, `LZ4F_compressUpdateImpl`, `LZ4F_compressUpdate` (12)
4. Frame + contexts: `LZ4F_uncompressedUpdate`, `LZ4F_flush`, `LZ4F_compressEnd`, `LZ4F_compressFrame_usingCDict`, `LZ4F_compressFrame`, `LZ4F_createCDict_advanced`, `LZ4F_createCDict`, `LZ4F_freeCDict`, `LZ4F_createCompressionContext_advanced`, `LZ4F_createCompressionContext`, `LZ4F_freeCompressionContext`, `LZ4F_createDecompressionContext_advanced` (12)
5. Decompression: `LZ4F_createDecompressionContext`, `LZ4F_freeDecompressionContext`, `LZ4F_resetDecompressionContext`, `LZ4F_decodeHeader`, `LZ4F_headerSize`, `LZ4F_getFrameInfo`, `LZ4F_updateDict`, `LZ4F_decompress`, `LZ4F_decompress_usingDict` (9)

---

### C6 — `lz4file`: FILE* streaming wrappers (`lz4file.c`)

**What it does.** Convenience read/write of LZ4 frames over a caller-provided `FILE*`: opens a frame (parses header via C5), buffers IO, and streams `LZ4F_read`/`LZ4F_write`.

**Members (11 = 6 exported + 5 static).** Exported: `LZ4F_readOpen`, `LZ4F_read`, `LZ4F_readClose`, `LZ4F_writeOpen`, `LZ4F_write`, `LZ4F_writeClose`. Static: `returnErrorCode`, `LZ4F_freeReadFile`, `LZ4F_freeAndNullReadFile`, `LZ4F_freeWriteFile`, `LZ4F_freeAndNullWriteFile`.

**Dependencies.** C5 only (11 symbols, listed in C5). Uses libc `fread`/`fwrite`/`calloc`/`malloc`/`free` directly — **no** custom-allocator plumbing (its handles are always stdlib-allocated; all allocation and freeing stays inside this capsule).

**External boundary / required trampolines (6).** No in-project callers — pure API surface (`lz4file.h`, `LZ4FLIB_STATIC_API`, which compiles to a bare declaration here, so default visibility → exported, as confirmed in the dylib).

| Symbol | Signature |
|---|---|
| `LZ4F_readOpen` | `(LZ4_readFile_t** lz4fRead, FILE* fp) → LZ4F_errorCode_t` |
| `LZ4F_read` | `(LZ4_readFile_t* lz4fRead, void* buf, size_t size) → size_t` (error codes returned as `size_t`) |
| `LZ4F_readClose` | `(LZ4_readFile_t* lz4fRead) → LZ4F_errorCode_t` |
| `LZ4F_writeOpen` | `(LZ4_writeFile_t** lz4fWrite, FILE* fp, const LZ4F_preferences_t* prefsPtr) → LZ4F_errorCode_t` |
| `LZ4F_write` | `(LZ4_writeFile_t* lz4fWrite, const void* buf, size_t size) → size_t` |
| `LZ4F_writeClose` | `(LZ4_writeFile_t* lz4fWrite) → LZ4F_errorCode_t` |

**ABI-fixed data.** None. `LZ4_readFile_t` / `LZ4_writeFile_t` are opaque (structs defined in lz4file.c:49 and :193; header only forward-declares them). The caller-visible `LZ4F_preferences_t` is owned by C5's contract. Note `FILE*` crosses into the Rust side — the migration agent must use `std::io::Read`/`Write` over `*mut FILE` (or keep a thin libc `fread`/`fwrite` binding) to match error semantics exactly (fread returns 0 at EOF and sets errno on error — the C code treats `ret==0` as EOF and `ret>0` as success, `ret<0`/error-flag as IO error; preserve that tri-state handling).

**Suggested chunking (1 chunk, 11 fns).** `returnErrorCode`, `LZ4F_freeReadFile`, `LZ4F_freeAndNullReadFile`, `LZ4F_freeWriteFile`, `LZ4F_freeAndNullWriteFile`, `LZ4F_readOpen`, `LZ4F_read`, `LZ4F_readClose`, `LZ4F_writeOpen`, `LZ4F_write`, `LZ4F_writeClose`.

---

## 4. Indirect-call findings (recovered edges)

The callgraph tool as installed flags **no** `[indirect call site]` markers (verified against the raw `opt -passes=dot-callgraph` output), so the three function-pointer dispatch sites below were found by source inspection. All are recorded as real edges; none of them crosses a capsule boundary.

1. **`LZ4F_makeBlock` — lz4frame.c:883-910 (call at :892).** Calls `compress(lz4ctx, src, dst, srcSize, dstSize, level, cdict)` through the `compressFunc_t compress` parameter (`typedef` at lz4frame.c:876: `int (*)(void* ctx, const char* src, char* dst, int srcSize, int dstSize, int level, const LZ4F_CDict* cdict)`). The pointer value is produced by **`LZ4F_selectCompression`** (lz4frame.c:952-963) from `(cctx->prefs.frameInfo.blockMode, cctx->prefs.compressionLevel, cctx->blockCompressMode)`. **Resolved targets (all `static`, same file):**
   - `LZ4B_UNCOMPRESSED` ⇒ `LZ4F_doNotCompressBlock` (:946)
   - level < 3, blockIndependent ⇒ `LZ4F_compressBlock` (:911)
   - level < 3, blockLinked ⇒ `LZ4F_compressBlock_continue` (:923)
   - level ≥ 3, blockIndependent ⇒ `LZ4F_compressBlockHC` (:931)
   - level ≥ 3, blockLinked ⇒ `LZ4F_compressBlockHC_continue` (:940)

   Producers of the pointer: `LZ4F_compressUpdateImpl` (:1001) and `LZ4F_flush` (:1173). Edge consequence: intra-capsule (C5). Targets' own edges (to C3/C4 compressors) are visible in the static graph, so nothing is hidden at the capsule level.
2. **`LZ4MID_compress` — lz4hc.c:522 (pointer chosen at :549, called in the main loop).** `searchIntoDict` has type `LZ4MID_searchIntoDict_f` (typedef at lz4hc.c:406: `LZ4HC_match_t (*)(const BYTE* ip, U32 ipIndex, const BYTE* iHighLimit, const LZ4HC_CCtx_internal* dictCtx, U32 gDictEndIndex)`). The pointer is produced by **`select_searchDict_function`** (lz4hc.c:514-520) from `dictCtx`'s stored compression-level strategy. **Resolved targets (all `static`, same file):**
   - `dictCtx == NULL` ⇒ `NULL` (no call made)
   - strategy `lz4mid` (level 2) ⇒ `LZ4MID_searchExtDict` (:420)
   - otherwise ⇒ `LZ4MID_searchHCDict` (:410), which in turn calls `LZ4HC_searchExtDict` (:363, **exported** but same-file) directly.

   Edge consequence: intra-capsule (C4).
3. **`LZ4F_malloc` / `LZ4F_calloc` / `LZ4F_free` — lz4frame.c:101-132.** Call `cmem.customAlloc(cmem.opaqueState, s)`, `cmem.customCalloc(...)`, `cmem.customFree(...)` through the user-supplied `LZ4F_CustomMem` (lz4frame.h:727-735). **Resolved targets: external application code** (or NULL ⇒ stdlib `malloc`/`calloc`/`free`). This is not a library-internal edge, but the Rust port must keep the three optional `extern "C" fn` pointers and the same NULL-fallback semantics; `LZ4F_CustomMem` is ABI-fixed (Section C5). The same file's `LZ4F_calloc` emulates calloc as `customAlloc + memset` when only `customAlloc` is provided — preserve that.
4. **Checked and ruled out.** `ALLOC`/`ALLOC_AND_ZERO`/`FREEMEM` macros (lz4.c:209-227) dispatch to user functions only when `LZ4_USER_MEMORY_FUNCTIONS` is defined — it is **not** in this build (CMake defines only `XXH_NAMESPACE`, `LZ4_HEAPMODE=0`, `LZ4F_HEAPMODE=0`), so these compile to plain `malloc`/`calloc`/`free`. No other function-pointer calls exist in the five sources (exhaustive grep for `(*name)(`-style dispatch; the only remaining `(*...)` uses in the codebase are plain dereferences, e.g. `(*pp)++`).

---

## 5. Shared-primitives capsule: gateway-and-delete note

**C1 `lz4-common`** is the shared-primitives capsule required by the duplication: its 16 static helpers exist today in *two* translation units (the `lz4.c` base definitions and the `lz4hc.c` private copies created by `#include "lz4.c"` under `LZ4_COMMONDEFS_ONLY` — visible in the call graph as `LZ4_read32.2`, `LZ4_read_ARCH.3`, `LZ4_NbCommonBytes.4`, `LZ4_read16.5`, `LZ4_isLittleEndian.6`, `LZ4_writeLE16.8`, `LZ4_write16.9`, etc.).

- **No external trampoline names** — nothing links to these symbols today (all `static`; zero entries in the dylib export table).
- Migrate it first as an internal-only Rust module (`pub(crate)`), with **no** `extern "C"` surface at all — its "gateway" is just Rust function calls from C3 (`lz4core`) and C4 (`lz4hc`).
- **Delete/merge it once both C3 and C4 are migrated**: at that point the module is pure crate-internal code and the unity-include structure (and the `foo.N` duplication) disappears. Keep it as a separate capsule only while C3 and C4 coexist as C.

---

## 6. Open questions / risks (for a human or the migration agent)

1. **Three `#if` variants of `LZ4_read16/32/read_ARCH/write16/32`** (lz4.c:377-424) — only one is active per platform (packed-struct vs `memcpy` vs plain deref, selected by `LZ4_ALIGN_ACCESS`/`LZ4_FORCE_MEMORY_ACCESS`). The Rust port should implement plain unaligned reads and *verify* against the branch active for the target; on x86_64/Apple Silicon all three are equivalent, but `LZ4_read_ARCH`'s width (`reg_t`) matters for `LZ4_NbCommonBytes`/`LZ4_count` performance semantics.
2. **How C5 (Rust) holds `XXH32_state_t` (twice, by value) in cctx/dctx, and `LZ4_stream_t` by value in `LZ4F_compressFrame`.** Two acceptable designs: (a) pin `#[repr(C)]` copies of the `XXH_STATIC_LINKING_ONLY` state structs and the `LZ4_stream_t` union inside C5 (exact C semantics, exact memory layout, more layout to keep honest), or (b) have C5 use C2's opaque `LZ4_XXH32_createState`/`reset` pointers and an opaque `LZ4_sizeofState()`-sized blob for the fast context (fewer pinned layouts, slightly different internal allocation pattern — `LZ4F_freeCompressionContext` frees `lz4CtxPtr` with the *custom allocator*, which must keep working). Recommend (a) for `LZ4_stream_t` (it is cheap and already pinned for C3's ABI) and decide (a)/(b) for the xx states with a size/align compile-time assert either way. **Flagging rather than guessing.**
3. **`LZ4_stream_t` union padding.** The C struct relies on compiler padding ("Implicit padding to ensure structure is aligned", lz4.h:727) to make the internal member exactly `LZ4_STREAM_MINSIZE` (16416 at `LZ4_MEMORY_USAGE=14`). The Rust `#[repr(C)]` union must be asserted at compile time for both size (16416) and alignment (max of member alignments; 16 on aarch64-apple, 8 on x86_64). Same for `LZ4_streamHC_t` (262200) and `LZ4_streamDecode_t` (32).
4. **Exported-but-undeclared symbols.** Four exports have no declaration in any shipped header and no cross-file caller in this project: `LZ4F_compressBegin_internal`, `LZ4F_compressBegin_usingDictOnce`, `LZ4_loadDict_internal` (whose parameter type `LoadDict_mode_e` is a *private* enum), and `LZ4HC_searchExtDict`. They are nonetheless part of the dylib's ABI. Default recommendation: **keep the trampolines** for exact symbol-table parity. Confirm with the team whether any downstream consumer dlsyms them; if not, they could drop — but that is an ABI decision, not a code one.
5. **LZ4F error-code bit pattern.** `returnErrorCode` (lz4frame.c:311, lz4file.c:40) is `(LZ4F_errorCode_t)-(ptrdiff_t)code` — a sign-flip of a small positive enum, observed by callers as a huge `size_t` and detected by `LZ4F_isError` (max-code comparison). Rust must reproduce the exact two's-complement pattern (e.g. `(code as isize).wrapping_neg() as usize`) and the exact `LZ4F_isError` boundary, including `LZ4F_read`/`LZ4F_write` returning error codes *as `size_t` successes' namespace*.
6. **`FILE*` FFI in C6.** `fread`/`fwrite` tri-state semantics (EOF vs error) and the exact `LZ4F_HEADER_SIZE_MAX` (19-byte) stack buffers must be preserved; if the migration agent replaces them with Rust `std::fs::File` IO the ABI of `LZ4F_readOpen(LZ4_readFile_t**, FILE*)` breaks — it must accept a real C `FILE*`.
7. **LZ4HC_HEAPMODE is not pinned by CMake** (defaults to 1 → `LZ4_compress_HC` allocates its workspace on the heap; `LZ4_compressHC*` wrappers likewise). The Rust port should keep heap allocation for that path (or document a deviation); `LZ4_HEAPMODE=0`/`LZ4F_HEAPMODE=0` (stack workspace in `LZ4_compress_default`-style one-shots and in `LZ4F_compressFrame`) must be preserved to keep stack-use behavior identical.
8. **MID geometry (internal, verify in port).** `LZ4MID_HASHTABLESIZE = 1<<(LZ4HC_HASH_LOG-1) = 16384` U32 entries; the 8-byte table occupies the second half of `hashTable[32768]` (lz4hc.c:141-143, :488-491). `LZ4MID_fillHTable` asserts `dict == cctx->prefixStart` and only fills when the dict is large — reproduce exactly.
9. **`k_clTable` strategy mapping (lz4hc.c:94-105).** Levels 0-2 ⇒ `lz4mid`, 3-9 ⇒ `lz4hc`, 10-12 ⇒ `lz4opt` — this table is what makes level 2 route into the MID engine; it is static data inside C4, not a boundary, but a wrong transcription silently changes which algorithm runs.
10. **Thread-safety contract.** The C code is not internally synchronized (contexts are assumed single-threaded; only `LZ4F_CDict` is documented read-only/shared). The migration should add no locking and change no aliasing assumptions — trampolines must forward exactly as-is.
11. **`LZ4F_defaultCMem`** is a `static const` defined in the public header (lz4frame.h:736-740) — every C TU that includes it has its own copy. In the mixed C/Rust phase, C callers pass this by value into `_advanced` constructors; the Rust side just receives a struct of (possibly NULL) function pointers. No action needed, noted to avoid "dedupe it" refactors that would change call semantics.

---

*Plan generated from: `callgraph.py` over CMake `compile_commands.json` (5/5 files compiled, no skips), `nm -gU liblz4.dylib` (143 exports), and source inspection of all five `.c` files and six headers. All trampoline lists and cross-capsule edges above are read off the real call graph / export table, not designed.*