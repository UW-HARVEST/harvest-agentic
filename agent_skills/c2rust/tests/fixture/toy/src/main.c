#include "toy.h"

int main(int argc, char **argv) {
    toy_print(stdout, "sum", toy_sum(3, 10, 20, argc));
    toy_print(stdout, "root", toy_root(argc > 1 ? argv[1] : "144:"));
    return 0;
}
