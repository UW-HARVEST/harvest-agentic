#define _GNU_SOURCE
#include <math.h>
#include <stdarg.h>
#include <stdlib.h>
#include <string.h>

#include "toy.h"

static int sum_list(int count, va_list args) {
    int sum = TOY_BIAS;
    for (int i = 0; i < count; i++) {
        sum += va_arg(args, int);
    }
    return sum;
}

int toy_sum(int count, ...) {
    va_list args;
    va_start(args, count);
    int sum = sum_list(count, args);
    va_end(args);
    return sum;
}

int toy_root(const char *text) {
    const char *colon = strchr(text, ':');
    if (colon == NULL) {
        return -1;
    }
    return (int)sqrt((double)atoi(text));
}

int toy_print(FILE *out, const char *name, int value) {
    return fprintf(out, "%s=%d\n", name, value);
}
