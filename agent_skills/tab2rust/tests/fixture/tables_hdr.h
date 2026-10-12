/* A table in a header: detect shows the compiled file (tables.c). */
static const uint16_t hdr_tab[32] = {
  100, 101, 102, 103, 104, 105, 106, 107, 108, 109, 110, 111, 112, 113, 114, 115,
  116, 117, 118, 119, 120, 121, 122, 123, 124, 125, 126, 127, 128, 129, 130, 131
};
/* An array on the stack in a header function: no static copy (only .c files get copies). */
static inline int fx_hdr_local(int i)
{
  const uint8_t hdr_stack[32] = { 60, 61, 62, 63, 64, 65, 66, 67, 68, 69, 70, 71, 72, 73, 74, 75, 76, 77, 78, 79, 80, 81, 82, 83, 84, 85, 86, 87, 88, 89, 90, 91 };
  return hdr_stack[i & 31];
}

/* probe: macros that are not values. gcc puts the error of their expansion in this header. */
#define FX_HDR_VERSION -FX_HDR_UNDECLARED
#define FX_HDR_DECL extern int
