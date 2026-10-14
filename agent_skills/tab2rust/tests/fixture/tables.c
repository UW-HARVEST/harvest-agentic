/* tab2rust regression fixture: one example of each table form. */
#include <stdint.h>
#include <stddef.h>

#define FX_CODE_00 1
#define FX_CODE_01 4
#define FX_CODE_02 7
#define FX_CODE_03 10
#define FX_CODE_04 13
#define FX_CODE_05 16
#define FX_CODE_06 19
#define FX_CODE_07 22
#define FX_CODE_08 25
#define FX_CODE_09 28
#define FX_CODE_10 31
#define FX_CODE_11 34
#define FX_CODE_12 37
#define FX_CODE_13 40
#define FX_CODE_14 43
#define FX_CODE_15 46
#define FX_CODE_16 49
#define FX_CODE_17 52
#define FX_CODE_18 55
#define FX_CODE_19 58
#define FX_CODE_20 61
#define FX_CODE_21 64
#define FX_CODE_22 67
#define FX_CODE_23 70
#define FX_CODE_24 73
#define FX_CODE_25 76
#define FX_CODE_26 79
#define FX_CODE_27 82
#define FX_CODE_28 85
#define FX_CODE_29 88
#define FX_CODE_30 91
#define FX_CODE_31 94
#define FX_CODE_32 97
#define FX_CODE_33 100
#define FX_CODE_34 103
#define FX_CODE_35 106
#define FX_CODE_36 109
#define FX_CODE_37 112
#define FX_CODE_38 115
#define FX_CODE_39 118

/* PURE: 1-D numeric tables */
static const uint8_t perm_tab[64] = {
  63, 9, 6, 22, 61, 30, 57, 12, 0, 16, 41, 56, 51, 25, 40, 54,
  14, 55, 46, 53, 50, 42, 17, 2, 38, 1, 59, 45, 35, 60, 24, 10,
  26, 32, 7, 4, 18, 28, 21, 58, 33, 48, 62, 5, 47, 44, 11, 15,
  19, 23, 52, 37, 29, 20, 49, 27, 34, 43, 31, 13, 39, 3, 36, 8
};
static const uint64_t gear_tab[32] = {
  0xf2a74de452e6b438, 0x6513270e269e0d37, 0x0c5c7fd0a6a3a450, 0xd23f0824128b2f33,
  0x1818e811892f902b, 0x9531985d5d9dc9f8, 0xe8e25d940ed90475, 0x36f675cc81e74ef5,
  0x1600a35a099950d8, 0x6b0d549b6f03675a, 0x3d9c172411e20b8f, 0x8d116ece1738f7d9,
  0x0f21ddb66cad4a26, 0x90c192cfd3ac94af, 0xf28c105d1fb17c23, 0xa170b33839263059,
  0x953f48f1a09f76b5, 0x0fd630f1f29d0da9, 0x95e60af593bd04cf, 0x0cb1e29c658cda14,
  0x3898d190f9ebdacc, 0x8e81973e0becd7b0, 0x2217beaddbc496cb, 0x6b4cb2424a23d596,
  0x8a6a63ec24ede6a4, 0x922766581e27a1c0, 0x8f6d05584ef8aa38, 0xae97ba94d0eda82f,
  0x1a61dbe22e44158b, 0x923a736994e3bf91, 0x301850c5a38fd547, 0x18f135d25f557203
};

/* PURE: struct rows with an enum and a float, and a 2-D table */
typedef enum { MODE_FAST = 1, MODE_SLOW = 7, MODE_NEG = -2 } mode_t2r;
typedef struct { int16_t a; float f; mode_t2r m; uint32_t b; } row_t2r;
static const row_t2r rows_tab[8] = {
  { -1, 0.1f, MODE_FAST, 10 }, { -32768, -0.0f, MODE_SLOW, 20 }, { 3, 1e-20f, MODE_NEG, 30 }, { 4, 2.5f, MODE_FAST, 40 },
  { 5, 3.25f, MODE_SLOW, 50 }, { 6, -7.75f, MODE_NEG, 60 }, { 7, 100.0f, MODE_FAST, 70 }, { 32767, 0.3f, MODE_SLOW, 4294967295u }
};
static const int16_t grid_tab[4][8] = {
  { 1, -2, 3, -4, 5, -6, 7, -8 }, { 9, -10, 11, -12, 13, -14, 15, -16 },
  { 17, -18, 19, -20, 21, -22, 23, -24 }, { 25, -26, 27, -28, 29, -30, 31, -32 }
};
static const double dbl_tab[32] = {
  1e-300, 0.30000000000000004, -0.0, 1.0, 60.0, 3600.0, 5373485.0, 4.6427e14, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0, 13.0,
  14.0, 15.0, 16.0, 17.0, 18.0, 19.0, 20.0, 21.0, 22.0, 23.0, 24.0, 25.0, 26.0, 27.0, 28.0, 29.0
};

/* PURE: string pointer table with NULL and escapes, and a text array */
static const char *name_tab[32] = {
  "alpha", "be\"ta", "", "x'\\y", "tab\there", "nl\n", "\x7f\x80", NULL,
  "i8", "i9", "i10", "i11", "i12", "i13", "i14", "i15", "i16", "i17", "i18", "i19",
  "i20", "i21", "i22", "i23", "i24", "i25", "i26", "i27", "i28", "i29", "i30", "i31"
};
static const char text_tab[] = "a text table with a \"quote\" and a tab\t and more text to pass thirty";

/* PURE: pointers into other tables */
static const int16_t *ptr_tab[32] = {
  &grid_tab[0][0], &grid_tab[1][2], &grid_tab[3][7], NULL, &grid_tab[2][1], &grid_tab[0][1], &grid_tab[0][2], &grid_tab[0][3],
  &grid_tab[0][4], &grid_tab[0][5], &grid_tab[0][6], &grid_tab[0][7], &grid_tab[1][0], &grid_tab[1][1], &grid_tab[1][3], &grid_tab[1][4],
  &grid_tab[1][5], &grid_tab[1][6], &grid_tab[1][7], &grid_tab[2][0], &grid_tab[2][2], &grid_tab[2][3], &grid_tab[2][4], &grid_tab[2][5],
  &grid_tab[2][6], &grid_tab[2][7], &grid_tab[3][0], &grid_tab[3][1], &grid_tab[3][2], &grid_tab[3][3], &grid_tab[3][4], &grid_tab[3][5]
};

/* PURE with notes: zero fill, #if inside, macro list, #include of a data file */
static const uint32_t zero_tab[64] = { 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30 };
/* not a finding: 3 text items (the binary has 64 values) */
static const uint32_t short_tab[64] = { 1, 2, 3 };
static const uint16_t cond_tab[32] = {
#if 1
  1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16,
#else
  0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
#endif
  17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32
};
#define FX_TWO(a) (a), (a) + 100
static const uint8_t macro_tab[60] = {
  FX_TWO(0), FX_TWO(1), FX_TWO(2), FX_TWO(3), FX_TWO(4), FX_TWO(5), FX_TWO(6), FX_TWO(7), FX_TWO(8), FX_TWO(9),
  FX_TWO(10), FX_TWO(11), FX_TWO(12), FX_TWO(13), FX_TWO(14), FX_TWO(15), FX_TWO(16), FX_TWO(17), FX_TWO(18), FX_TWO(19),
  FX_TWO(20), FX_TWO(21), FX_TWO(22), FX_TWO(23), FX_TWO(24), FX_TWO(25), FX_TWO(26), FX_TWO(27), FX_TWO(28), FX_TWO(29)
};
static const uint8_t incl_tab[4][8] = {
#include "tables_body.inc"
};
#include "tables_hdr.h"

/* DEAD: not compiled */
#if 0
static const uint8_t dead_tab[40] = { 1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31,32,33,34,35,36,37,38,39,40 };
#endif

/* TAINTED: union and function pointer tables */
typedef union { uint32_t u; float f; } un_t2r;
static const un_t2r union_tab[32] = { {1}, {2}, {3}, {4}, {5}, {6}, {7}, {8}, {9}, {10}, {11}, {12}, {13}, {14}, {15}, {16},
  {17}, {18}, {19}, {20}, {21}, {22}, {23}, {24}, {25}, {26}, {27}, {28}, {29}, {30}, {31}, {32} };
static int fx_one(int x) { return x + 1; }
static int (*const fn_tab[32])(int) = { fx_one, fx_one, fx_one, fx_one, fx_one, fx_one, fx_one, fx_one, fx_one, fx_one, fx_one, fx_one,
  fx_one, fx_one, fx_one, fx_one, fx_one, fx_one, fx_one, fx_one, fx_one, fx_one, fx_one, fx_one, fx_one, fx_one, fx_one, fx_one,
  fx_one, fx_one, fx_one, fx_one };

/* anonymous structs: gdb prints each type as "struct {...}" and each declaration over several lines */
static const struct { const char *name; const char *cols; } anon_a_tab[16] = {
  {"a0", "c0"}, {"a1", "c1"}, {"a2", "c2"}, {"a3", "c3"}, {"a4", "c4"}, {"a5", "c5"}, {"a6", "c6"}, {"a7", "c7"},
  {"a8", "c8"}, {"a9", "c9"}, {"a10", "c10"}, {"a11", "c11"}, {"a12", "c12"}, {"a13", "c13"}, {"a14", "c14"}, {"a15", "c15"}
};
static const struct { int op; int64_t mask; } anon_b_tab[16] = {
  {1, 0x100000001}, {2, -2}, {3, 3}, {4, 4}, {5, 5}, {6, 6}, {7, 7}, {8, 8},
  {9, 9}, {10, 10}, {11, 11}, {12, 12}, {13, 13}, {14, 14}, {15, 15}, {16, INT64_MIN}
};
int fx_anon(int i)
{
  static const struct { const char *key; uint16_t len; } anon_c_tab[16] = {
    {"k0", 0}, {"k1", 1}, {"k2", 2}, {"k3", 3}, {"k4", 4}, {"k5", 5}, {"k6", 6}, {"k7", 7},
    {"k8", 8}, {"k9", 9}, {"k10", 10}, {"k11", 11}, {"k12", 12}, {"k13", 13}, {"k14", 14}, {"k15", 65535}
  };
  static const struct { uint8_t a; uint32_t b; double c; } anon_d_tab[10] = {
    {1, 2, 0.5}, {3, 4, 1.5}, {5, 6, 2.5}, {7, 8, 3.5}, {9, 10, 4.5},
    {11, 12, 5.5}, {13, 14, 6.5}, {15, 16, 7.5}, {17, 18, 8.5}, {255, 4294967295u, -1.0}
  };
  i &= 15;
  return anon_a_tab[i].name[0] + anon_b_tab[i].op + anon_c_tab[i].len + anon_d_tab[i % 10].a;
}
/* anonymous structs: the spec key is "xform_tab[]" or the default name "xform_tab_row" */
static const struct {
  uint8_t nName; char zName[7]; float rLimit; float rXform;
} xform_tab[] = {
  { 6, "second", 4.6427e+14, 1.0 }, { 6, "minute", 7.7379e+12, 60.0 }, { 4, "hour", 1.2897e+11, 3600.0 },
  { 3, "day", 5373485.0, 86400.0 }, { 5, "month", 176546.0, 2592000.0 }, { 4, "year", 14713.0, 31536000.0 },
  { 4, "tiny", 1e-30, 0.001 }, { 3, "neg", -0.0, -1.5 }
};
/* an anonymous struct in a field of an anonymous struct */
static const struct { int id; struct { uint8_t lo, hi; } r; } span_tab[10] = {
  {1, {2, 3}}, {4, {5, 6}}, {7, {8, 9}}, {10, {11, 12}}, {13, {14, 15}},
  {16, {17, 18}}, {19, {20, 21}}, {22, {23, 24}}, {25, {26, 27}}, {28, {29, 30}}
};
int fx_xform(int i) { return xform_tab[i & 7].nName + span_tab[i % 10].r.hi; }

/* stack arrays, struct-declared statics, a writable table, a macro initializer, excess items */
static const struct fx_entry { uint16_t code; uint8_t flags; } struct_tab[16] = {
  {1, 1}, {2, 0}, {3, 1}, {4, 0}, {5, 1}, {6, 0}, {7, 1}, {8, 0},
  {9, 1}, {10, 0}, {11, 1}, {12, 0}, {13, 1}, {14, 0}, {15, 1}, {65535, 255}
};
static struct fx_link { const char *name; int v; } link_tab[16] = {
  {"l0", 0}, {"l1", 1}, {"l2", 2}, {"l3", 3}, {"l4", 4}, {"l5", 5}, {"l6", 6}, {"l7", 7},
  {"l8", 8}, {"l9", 9}, {"l10", 10}, {"l11", 11}, {"l12", 12}, {"l13", 13}, {"l14", 14}, {NULL, 15}
};
static uint16_t writable_tab[32] = { 1000, 1001, 1002, 1003, 1004, 1005, 1006, 1007, 1008, 1009, 1010, 1011, 1012, 1013, 1014, 1015, 1016, 1017, 1018, 1019, 1020, 1021, 1022, 1023, 1024, 1025, 1026, 1027, 1028, 1029, 1030, 1031 };
#define FX_INIT { 50, 51, 52, 53, 54, 55, 56, 57, 58, 59, 60, 61, 62, 63, 64, 65, 66, 67, 68, 69, 70, 71, 72, 73, 74, 75, 76, 77, 78, 79, 80, 81 }
static const uint8_t macro_init_tab[32] = FX_INIT;
static const int excess_tab[30] = { 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31 };
int fx_stack(int i)
{
#if 0
  int dead_stack[32] = { 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32 };
#endif
  int (*stack_fn[32])(int) = { fx_one, fx_one, fx_one, fx_one, fx_one, fx_one, fx_one, fx_one, fx_one, fx_one, fx_one, fx_one, fx_one, fx_one, fx_one, fx_one, fx_one, fx_one, fx_one, fx_one, fx_one, fx_one, fx_one, fx_one, fx_one, fx_one, fx_one, fx_one, fx_one, fx_one, fx_one, fx_one };
  uint8_t live_stack[32] = { 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32 };
  static const struct fx_pair { int a; int b; } fn_struct_tab[16] = {
    {1, -1}, {2, -2}, {3, -3}, {4, -4}, {5, -5}, {6, -6}, {7, -7}, {8, -8},
    {9, -9}, {10, -10}, {11, -11}, {12, -12}, {13, -13}, {14, -14}, {15, -15}, {16, -16}
  };
  i &= 15;
  writable_tab[i]++;
  link_tab[i].v++;
  return stack_fn[i](i) + live_stack[i] + fn_struct_tab[i].a + struct_tab[i].code + macro_init_tab[i] + excess_tab[i];
}

/* stack arrays and their static copies: a local macro, type and enum; a parameter; a pointer into
   another unit (no copy); strings */
extern const char fx_unit2_name[];
int fx_local(int p)
{
#define FX_HI 0x80
  struct fx_local_row { uint8_t a; uint8_t b; };
  enum { FX_LOCAL_K = 7 };
  const struct fx_local_row copy_rows[16] = {
    {1, 'a' | FX_HI}, {2, 'b'}, {3, FX_LOCAL_K}, {4, 4}, {5, 5}, {6, 6}, {7, 7}, {8, 8},
    {9, 9}, {10, 10}, {11, 11}, {12, 12}, {13, 13}, {14, 14}, {15, 15}, {16, sizeof(struct fx_local_row)}
  };
#undef FX_HI
  int param_stack[32] = { p, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31 };
  const char *extern_stack[32] = { fx_unit2_name, "e1", "e2", "e3", "e4", "e5", "e6", "e7", "e8", "e9", "e10", "e11", "e12", "e13", "e14", "e15", "e16", "e17", "e18", "e19", "e20", "e21", "e22", "e23", "e24", "e25", "e26", "e27", "e28", "e29", "e30", "e31" };
  const char *str_stack[32] = { "s0", "s1", "s2", "s3", "s4", "s5", "s6", "s7", "s8", "s9", "s10", "s11", "s12", "s13", "s14", "s15", "s16", "s17", "s18", "s19", "s20", "s21", "s22", "s23", "s24", "s25", "s26", "s27", "s28", "s29", "s30", NULL };
  p &= 15;
  return copy_rows[p].b + param_stack[p] + extern_stack[p][0] + str_stack[p][0];
}

/* pointers: into a global array (the loader writes these addresses, the file has 0), into a byte
   array (references, not strings), strings and references mixed, and an address in a number */
const uint16_t fx_gdata[8] = { 11, 12, 13, 14, 15, 16, 17, 18 };
const uint8_t fx_bytes[8] = { 1, 0, 2, 3, 0, 4, 5, 6 };
const uint16_t *const gptr_tab[32] = { &fx_gdata[0], &fx_gdata[1], &fx_gdata[2], &fx_gdata[3], &fx_gdata[4], &fx_gdata[5], &fx_gdata[6], &fx_gdata[7], &fx_gdata[0], &fx_gdata[1], &fx_gdata[2], &fx_gdata[3], &fx_gdata[4], &fx_gdata[5], &fx_gdata[6], &fx_gdata[7], &fx_gdata[0], &fx_gdata[1], &fx_gdata[2], &fx_gdata[3], &fx_gdata[4], &fx_gdata[5], &fx_gdata[6], &fx_gdata[7], &fx_gdata[0], &fx_gdata[1], &fx_gdata[2], &fx_gdata[3], &fx_gdata[4], &fx_gdata[5], &fx_gdata[6], &fx_gdata[7] };
const uint8_t *const bptr_tab[32] = { &fx_bytes[0], &fx_bytes[1], &fx_bytes[2], &fx_bytes[3], &fx_bytes[4], &fx_bytes[5], &fx_bytes[6], &fx_bytes[7], &fx_bytes[0], &fx_bytes[1], &fx_bytes[2], &fx_bytes[3], &fx_bytes[4], &fx_bytes[5], &fx_bytes[6], &fx_bytes[7], &fx_bytes[0], &fx_bytes[1], &fx_bytes[2], &fx_bytes[3], &fx_bytes[4], &fx_bytes[5], &fx_bytes[6], &fx_bytes[7], &fx_bytes[0], &fx_bytes[1], &fx_bytes[2], &fx_bytes[3], &fx_bytes[4], &fx_bytes[5], &fx_bytes[6], &fx_bytes[7] };
const char *const mixed_tab[32] = { (const char *)&fx_bytes[1], "m1", "m2", "m3", "m4", "m5", "m6", "m7", "m8", "m9", "m10", "m11", "m12", "m13", "m14", "m15", "m16", "m17", "m18", "m19", "m20", "m21", "m22", "m23", "m24", "m25", "m26", "m27", "m28", "m29", "m30", "m31" };
const uintptr_t addr_tab[32] = { (uintptr_t)&fx_gdata[0], 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31 };
const char fx_chars[4] = "abc";
#define FX_CHARS_NAME fx_chars

/* one struct type in several arrays: one Rust definition. Only kv_b_tab has a NULL name, only kv_c_tab
   points into a char array. Struct and field names that are Rust keywords. */
struct fx_kv { const char *name; int v; };
static const struct fx_kv kv_a_tab[16] = {
  {"a0", 0}, {"a1", 1}, {"a2", 2}, {"a3", 3}, {"a4", 4}, {"a5", 5}, {"a6", 6}, {"a7", 7},
  {"a8", 8}, {"a9", 9}, {"a10", 10}, {"a11", 11}, {"a12", 12}, {"a13", 13}, {"a14", 14}, {"a15", 15}
};
static const struct fx_kv kv_b_tab[16] = {
  {"b0", 0}, {"b1", 1}, {"b2", 2}, {"b3", 3}, {"b4", 4}, {"b5", 5}, {"b6", 6}, {"b7", 7},
  {"b8", 8}, {"b9", 9}, {"b10", 10}, {"b11", 11}, {"b12", 12}, {"b13", 13}, {"b14", 14}, {NULL, 15}
};
static const struct fx_kv kv_c_tab[16] = {
  {&fx_chars[0], 0}, {&fx_chars[1], 1}, {&fx_chars[2], 2}, {&fx_chars[3], 3}, {&fx_chars[0], 4}, {&fx_chars[1], 5}, {&fx_chars[2], 6}, {&fx_chars[3], 7},
  {&fx_chars[0], 8}, {&fx_chars[1], 9}, {&fx_chars[2], 10}, {&fx_chars[3], 11}, {&fx_chars[0], 12}, {&fx_chars[1], 13}, {&fx_chars[2], 14}, {&fx_chars[3], 15}
};
static const struct match { int self; int priv; } kw_tab[16] = {
  {0, -1}, {1, -2}, {2, -3}, {3, -4}, {4, -5}, {5, -6}, {6, -7}, {7, -8},
  {8, -9}, {9, -10}, {10, -11}, {11, -12}, {12, -13}, {13, -14}, {14, -15}, {15, -16}
};
/* char pointers made by a cast: into a uint8_t array (a byte string in Rust: kept), into an int8_t array (refused) */
static const int8_t fx_sbytes[8] = { -1, 0, 2, 3, 0, 4, 5, -6 };
static const char *const ucast_tab[32] = { (const char *)&fx_bytes[0], (const char *)&fx_bytes[1], (const char *)&fx_bytes[2], (const char *)&fx_bytes[3], (const char *)&fx_bytes[4], (const char *)&fx_bytes[5], (const char *)&fx_bytes[6], (const char *)&fx_bytes[7], (const char *)&fx_bytes[0], (const char *)&fx_bytes[1], (const char *)&fx_bytes[2], (const char *)&fx_bytes[3], (const char *)&fx_bytes[4], (const char *)&fx_bytes[5], (const char *)&fx_bytes[6], (const char *)&fx_bytes[7], (const char *)&fx_bytes[0], (const char *)&fx_bytes[1], (const char *)&fx_bytes[2], (const char *)&fx_bytes[3], (const char *)&fx_bytes[4], (const char *)&fx_bytes[5], (const char *)&fx_bytes[6], (const char *)&fx_bytes[7], (const char *)&fx_bytes[0], (const char *)&fx_bytes[1], (const char *)&fx_bytes[2], (const char *)&fx_bytes[3], (const char *)&fx_bytes[4], (const char *)&fx_bytes[5], (const char *)&fx_bytes[6], (const char *)&fx_bytes[7] };
static const char *const scast_tab[32] = { (const char *)&fx_sbytes[0], (const char *)&fx_sbytes[1], (const char *)&fx_sbytes[2], (const char *)&fx_sbytes[3], (const char *)&fx_sbytes[4], (const char *)&fx_sbytes[5], (const char *)&fx_sbytes[6], (const char *)&fx_sbytes[7], (const char *)&fx_sbytes[0], (const char *)&fx_sbytes[1], (const char *)&fx_sbytes[2], (const char *)&fx_sbytes[3], (const char *)&fx_sbytes[4], (const char *)&fx_sbytes[5], (const char *)&fx_sbytes[6], (const char *)&fx_sbytes[7], (const char *)&fx_sbytes[0], (const char *)&fx_sbytes[1], (const char *)&fx_sbytes[2], (const char *)&fx_sbytes[3], (const char *)&fx_sbytes[4], (const char *)&fx_sbytes[5], (const char *)&fx_sbytes[6], (const char *)&fx_sbytes[7], (const char *)&fx_sbytes[0], (const char *)&fx_sbytes[1], (const char *)&fx_sbytes[2], (const char *)&fx_sbytes[3], (const char *)&fx_sbytes[4], (const char *)&fx_sbytes[5], (const char *)&fx_sbytes[6], (const char *)&fx_sbytes[7] };
int fx_kv(int i) { i &= 15; return kv_a_tab[i].v + kv_b_tab[i].v + kv_c_tab[i].v + kw_tab[i].self + kw_tab[i].priv + *ucast_tab[i] + *scast_tab[i]; }

/* DEAD in a function with a long head, inside an extern "C" block: the text rule finds the function.
   A compiler-made array (__func__, DW_AT_artificial) is not a record. */
#ifdef __cplusplus
extern "C" {
#endif
__attribute__((unused)) static inline int fx_unused_long(int argument_number_00, int argument_number_01, int argument_number_02, int argument_number_03, int argument_number_04, int argument_number_05, int argument_number_06, int argument_number_07, int argument_number_08, int argument_number_09, int argument_number_10, int argument_number_11, int argument_number_12, int argument_number_13, int argument_number_14, int argument_number_15, int argument_number_16, int argument_number_17, int argument_number_18, int argument_number_19, int argument_number_20, int argument_number_21, int argument_number_22, int argument_number_23, int argument_number_24, int argument_number_25, int argument_number_26, int argument_number_27, int argument_number_28, int argument_number_29, int argument_number_30, int argument_number_31, int argument_number_32, int argument_number_33, int argument_number_34, int argument_number_35, int argument_number_36, int argument_number_37, int argument_number_38, int argument_number_39)
{
  static const uint8_t long_head_tab[32] = { 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31 };
  return long_head_tab[argument_number_00 & 31];
}
const char *fx_function_with_a_rather_long_name_for_func(void) { return __func__; }
#ifdef __cplusplus
}
#endif

/* DEAD: a function that is not emitted, the body file of a dead host */
static inline int fx_unused(int i)
{
  static const uint8_t unemitted_tab[32] = { 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32 };
  return unemitted_tab[i & 31];
}
#if 0
static const uint8_t dead_host[4][8] = {
#include "tables_body.inc"
};
#endif

/* a header table with other bytes in another unit (tables_unit2.c) */
#define FX_UNIT 1
#include "tables_copy.h"
int fx_copy(int i) { return copy_tab[i & 31]; }

/* function-local and nested-block statics */
int fx_lookup(int i, int j)
{
  static const uint8_t local_tab[40] = { 40,39,38,37,36,35,34,33,32,31,30,29,28,27,26,25,24,23,22,21,20,19,18,17,16,15,14,13,12,11,10,9,8,7,6,5,4,3,2,1 };
  if (j > 0) {
    static const uint8_t nested_tab[32] = { 2,3,5,7,11,13,17,19,23,29,31,37,41,43,47,53,59,61,67,71,73,79,83,89,97,101,103,107,109,113,127,131 };
    return nested_tab[j & 31] + local_tab[i % 40];
  }
  return local_tab[i % 40];
}

/* keep every table referenced */
long fx_use(int i)
{
  i &= 7;
  return perm_tab[i] + (long)gear_tab[i] + rows_tab[i].a + grid_tab[i & 3][i] + (long)dbl_tab[i] + name_tab[i][0]
       + text_tab[i] + *ptr_tab[i] + zero_tab[i] + cond_tab[i] + macro_tab[i] + incl_tab[i & 3][i]
       + (long)union_tab[i].u + fn_tab[i](i) + FX_CODE_05 + hdr_tab[i] + fx_hdr_local(i);
}

/* translate: the same C name in two files (tables_unit2.c) gives equal default Rust names */
static const int dup_tab[4] = { 1, 2, 3, 4 };
int fx_dup(int i) { return dup_tab[i & 3]; }

/* probe cases: escapes, C types, a function-like macro, a non-constant, a dead #if branch */
#define FX_STR_Q "q\"b\\s\x85\n\t\x01"
#define FX_STR_U8 "h\xc3\xa9\"q\n\x01"
#define FX_U 4000000000u
#define FX_L (-5L)
#define FX_BYTE ((unsigned char)0x85)
#define FX_F 1.5f
#define FX_D 0.1
#define FX_FN(x) ((x) + 1)
#define FX_NONCONST fx_lookup
#if 0
#define FX_DEAD_STR "dead"
#endif

/* a table after #line: the debug information has another file name and line */
#line 900 "fx_generated.c"
static const uint8_t line_tab[32] = { 200, 201, 202, 203, 204, 205, 206, 207, 208, 209, 210, 211, 212, 213, 214, 215, 216, 217, 218, 219, 220, 221, 222, 223, 224, 225, 226, 227, 228, 229, 230, 231 };
int fx_line(int i) { return line_tab[i & 31]; }
