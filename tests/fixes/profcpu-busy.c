// For profcpu.dme: call_ext("./profcpu-busy.so", "busy")(ms) starts 3 threads that burn CPU for ms milliseconds.
// Built by profcpu-gen.sh.
#include <pthread.h>
#include <stdlib.h>
#include <time.h>

static void *spin(void *arg) {
    long ms = (long)arg;
    struct timespec s, n;
    clock_gettime(CLOCK_MONOTONIC, &s);
    volatile unsigned long x = 0;
    for (;;) {
        for (int i = 0; i < 100000; i++) x += i;
        clock_gettime(CLOCK_MONOTONIC, &n);
        if ((n.tv_sec - s.tv_sec) * 1000 + (n.tv_nsec - s.tv_nsec) / 1000000 >= ms) break;
    }
    return 0;
}

const char *busy(int n, const char **v) {
    long ms = n > 0 ? atol(v[0]) : 500;
    for (int k = 0; k < 3; k++) {
        pthread_t t;
        if (pthread_create(&t, 0, spin, (void *)ms) == 0) pthread_detach(t);
    }
    return "";
}
