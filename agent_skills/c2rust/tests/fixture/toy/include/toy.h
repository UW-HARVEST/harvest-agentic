#ifndef TOY_H
#define TOY_H

#include <stdio.h>

/* Sum of `count` int arguments, plus TOY_BIAS. */
int toy_sum(int count, ...);
/* Integer square root of the text before the first ':' of `text`. */
int toy_root(const char *text);
/* Writes the name and the value to `out`. Returns the number of characters. */
int toy_print(FILE *out, const char *name, int value);

#endif
