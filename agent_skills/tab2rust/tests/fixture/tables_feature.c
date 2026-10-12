/* Not compiled into the fixture binary. All code is under two #if: the DEAD reason names both. */
#ifdef FX_FEATURE
#if FX_LEVEL > 1
static const unsigned char feature_tab[32] = { 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32 };
int fx_feature(int i) { return feature_tab[i & 31]; }
#endif
#endif
