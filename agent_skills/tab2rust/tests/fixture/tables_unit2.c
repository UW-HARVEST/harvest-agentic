/* Second unit of the fixture: its copy of copy_tab has other bytes. */
#define FX_UNIT 2
#include "tables_copy.h"
int fx_copy2(int i) { return copy_tab[i & 31]; }
const char fx_unit2_name[] = "unit2";
static const int dup_tab[4] = { 5, 6, 7, 8 };
int fx_dup2(int i) { return dup_tab[i & 3]; }
