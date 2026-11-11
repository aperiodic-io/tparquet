/* The C ABI from plain C, the way C and cgo callers use it: every function, every return code,
 * the argument checks of tparquet.h, and threads (per-thread errors, calls from a pthread key
 * destructor at thread exit). Run by `make -C rust/capi c-test`. */
#include "tparquet.h"
#include <pthread.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static int failures = 0;

#define CHECK(cond)                                                                                \
    do {                                                                                           \
        if (!(cond)) {                                                                             \
            fprintf(stderr, "%s:%d: CHECK failed: %s\n", __FILE__, __LINE__, #cond);              \
            failures++;                                                                            \
        }                                                                                          \
    } while (0)

static int last_error_is(const char *want) {
    char buf[256];
    size_t n = pcp_last_error(buf, sizeof buf);
    return n == strlen(want) && strcmp(buf, want) == 0;
}

/* compresses with a bound-sized buffer; returns the return code and the payload */
static int compress(const void *src, size_t len, int32_t type, int32_t level, int32_t decimal,
                    uint8_t **payload, size_t *written) {
    size_t cap = pcp_compress_bound(len);
    *payload = malloc(cap);
    return pcp_compress((const uint8_t *)src, len, type, level, decimal, *payload, cap, written);
}

static void test_constants(void) {
    CHECK(pcp_codec_id() == 20547);
    CHECK(pcp_abi_version() == PCP_ABI_VERSION);
    CHECK(pcp_compress_bound(0) == 2);
    CHECK(pcp_compress_bound(100) == 102);
    CHECK(pcp_compress_bound(SIZE_MAX) == SIZE_MAX);
    CHECK(pcp_compress_bound(SIZE_MAX - 2) == SIZE_MAX);
}

static void test_round_trips(void) {
    enum { N = 3000 };
    int32_t i32v[N];
    int64_t i64v[N];
    float f32v[N];
    double f64v[N];
    for (int i = 0; i < N; i++) {
        i32v[i] = INT32_MIN / 2 + i * 7;
        i64v[i] = INT64_C(1700000000000) + i * 250;
        f32v[i] = (float)(i % 500) / 10.0f;
        f64v[i] = (10000 + i % 300) / 100.0;
    }
    struct {
        const void *data;
        size_t len;
        int32_t type;
        uint8_t mode;
    } cases[] = {
        {i32v, sizeof i32v, 1, 1}, {i64v, sizeof i64v, 2, 1}, {f32v, sizeof f32v, 4, 2},
        {f64v, sizeof f64v, 5, 2}, {f64v, sizeof f64v, -1, 3}, {f64v, sizeof f64v - 3, 5, 3},
    };
    for (size_t c = 0; c < sizeof cases / sizeof cases[0]; c++) {
        uint8_t *p;
        size_t written = 0;
        CHECK(compress(cases[c].data, cases[c].len, cases[c].type, -1, 1, &p, &written) == PCP_OK);
        CHECK(written < cases[c].len);
        CHECK(p[0] == 1 && p[1] == cases[c].mode);
        uint8_t *back = malloc(cases[c].len);
        CHECK(pcp_decompress(p, written, back, cases[c].len) == PCP_OK);
        CHECK(memcmp(back, cases[c].data, cases[c].len) == 0);
        CHECK(last_error_is(""));
        /* one byte more or less is a size error */
        CHECK(pcp_decompress(p, written, back, cases[c].len - 1) == PCP_ERR_SIZE);
        free(back);
        free(p);
    }
    /* every level, and decimal off */
    for (int32_t level = 0; level <= 12; level++) {
        uint8_t *p;
        size_t written = 0;
        CHECK(compress(f64v, sizeof f64v, 5, level, 0, &p, &written) == PCP_OK);
        CHECK(p[1] == 1);
        free(p);
    }
}

static void test_errors(void) {
    uint8_t out[16];
    uint8_t raw[] = {1, 0, 7, 8};
    CHECK(pcp_decompress(raw, sizeof raw, out, 2) == PCP_OK && out[0] == 7 && out[1] == 8);
    CHECK(pcp_decompress(raw, sizeof raw, out, 3) == PCP_ERR_SIZE);
    CHECK(last_error_is("pcodec payload: decodes to 2 bytes, expected 3"));
    uint8_t bad_version[] = {2, 0};
    CHECK(pcp_decompress(bad_version, 2, out, 0) == PCP_ERR_CORRUPT);
    CHECK(last_error_is("pcodec payload: unsupported version 2"));
    uint8_t bad_mode[] = {1, 7};
    CHECK(pcp_decompress(bad_mode, 2, out, 0) == PCP_ERR_CORRUPT);
    CHECK(last_error_is("pcodec payload: unknown mode 7"));
    CHECK(pcp_decompress(raw, 1, out, 0) == PCP_ERR_CORRUPT);
    CHECK(last_error_is("pcodec payload: truncated"));
    /* success clears the message */
    CHECK(pcp_decompress(raw, sizeof raw, out, 2) == PCP_OK);
    CHECK(last_error_is(""));
}

static void test_arguments(void) {
    uint8_t buf[64] = {1, 0};
    size_t written = 99;
    double dv[8] = {1.5, 2.5, 3.5, 4.5, 5.5, 6.5, 7.5, 8.5};
    const uint8_t *d = (const uint8_t *)dv;
    /* NULL with a length, NULL written, a small buffer, a bad level */
    CHECK(pcp_decompress(NULL, 4, buf, 0) == PCP_ERR_ARGUMENT);
    CHECK(pcp_decompress(buf, 2, NULL, 4) == PCP_ERR_ARGUMENT);
    CHECK(pcp_compress(d, sizeof dv, 5, -1, 1, buf, sizeof buf, NULL) == PCP_ERR_ARGUMENT);
    CHECK(pcp_compress(d, sizeof dv, 5, -1, 1, buf, sizeof dv + 1, &written) == PCP_ERR_ARGUMENT);
    CHECK(last_error_is("dst_cap 65 below the bound 66"));
    CHECK(pcp_compress(d, sizeof dv, 5, 13, 1, buf, sizeof buf, &written) == PCP_ERR_ARGUMENT);
    CHECK(pcp_compress(d, sizeof dv, 5, -2, 1, buf, sizeof buf, &written) == PCP_ERR_ARGUMENT);
    CHECK(written == 99);
    /* NULL is fine with length 0 */
    CHECK(pcp_compress(NULL, 0, 5, -1, 1, buf, 2, &written) == PCP_OK && written == 2);
    CHECK(pcp_decompress(buf, 2, NULL, 0) == PCP_OK);
    /* lengths above PTRDIFF_MAX, as a negative length cast to size_t */
    CHECK(pcp_decompress(buf, (size_t)-1, buf + 32, 1) == PCP_ERR_ARGUMENT);
    CHECK(pcp_decompress(buf, 2, buf + 32, (size_t)PTRDIFF_MAX + 1) == PCP_ERR_ARGUMENT);
    /* overlapping buffers */
    CHECK(pcp_decompress(buf, 10, buf + 4, 8) == PCP_ERR_ARGUMENT);
    CHECK(last_error_is("src and dst overlap"));
    CHECK(pcp_compress(buf, 10, -1, -1, 0, buf + 9, 20, &written) == PCP_ERR_ARGUMENT);
    buf[1] = 0;
    CHECK(pcp_decompress(buf, 10, buf + 10, 8) == PCP_OK);
}

static void test_last_error_buffers(void) {
    uint8_t v[] = {9, 0};
    CHECK(pcp_decompress(v, 2, NULL, 0) == PCP_ERR_CORRUPT);
    const char *msg = "pcodec payload: unsupported version 9";
    char buf[8];
    memset(buf, 0x55, sizeof buf);
    CHECK(pcp_last_error(buf, sizeof buf) == strlen(msg));
    CHECK(memcmp(buf, "pcodec \0", 8) == 0);
    CHECK(pcp_last_error(NULL, 0) == strlen(msg));
    CHECK(pcp_last_error(NULL, 100) == strlen(msg));
    char one[2] = {0x55, 0x55};
    pcp_last_error(one, 1);
    CHECK(one[0] == 0 && one[1] == 0x55);
}

/* ---------------------------------------------------------------------------- threads */

enum { THREADS = 12, ROUNDS = 40 };
static pthread_barrier_t barrier;

static void *worker(void *arg) {
    int id = (int)(intptr_t)arg;
    enum { N = 2048 };
    int64_t vals[N], back[N];
    char want[128];
    snprintf(want, sizeof want, "pcodec payload: decodes to 1 bytes, expected %d", 100 + id);
    uint8_t raw[] = {1, 0, 42};
    uint8_t out[256];
    for (int r = 0; r < ROUNDS; r++) {
        for (int i = 0; i < N; i++) vals[i] = (int64_t)i * (id + 1) + r;
        uint8_t *p;
        size_t written = 0;
        if (compress(vals, sizeof vals, 2, -1, 1, &p, &written) != PCP_OK) return (void *)1;
        if (pcp_decompress(p, written, (uint8_t *)back, sizeof back) != PCP_OK) return (void *)2;
        free(p);
        if (memcmp(vals, back, sizeof vals) != 0) return (void *)3;
        /* every thread fails with its own size, waits for all, then reads its own message */
        if (pcp_decompress(raw, sizeof raw, out, 100 + id) != PCP_ERR_SIZE) return (void *)4;
        pthread_barrier_wait(&barrier);
        if (!last_error_is(want)) return (void *)5;
        pthread_barrier_wait(&barrier);
    }
    return NULL;
}

static void test_threads(void) {
    pthread_t t[THREADS];
    pthread_barrier_init(&barrier, NULL, THREADS);
    for (int i = 0; i < THREADS; i++) CHECK(pthread_create(&t[i], NULL, worker, (void *)(intptr_t)i) == 0);
    for (int i = 0; i < THREADS; i++) {
        void *rc;
        pthread_join(t[i], &rc);
        CHECK(rc == NULL);
        if (rc != NULL) fprintf(stderr, "thread %d failed at step %d\n", i, (int)(intptr_t)rc);
    }
    pthread_barrier_destroy(&barrier);
}

/* a pthread key destructor runs after the library's thread-locals are gone */
static pthread_key_t key;
static int exit_rc = -1;
static size_t exit_len = 99;

static void at_exit(void *value) {
    (void)value;
    uint8_t v[] = {9, 0};
    exit_rc = pcp_decompress(v, 2, NULL, 0);
    char buf[16] = "x";
    exit_len = pcp_last_error(buf, sizeof buf);
}

static void *exiting(void *arg) {
    (void)arg;
    uint8_t v[] = {9, 0};
    pcp_decompress(v, 2, NULL, 0); /* the library's thread-local error now exists */
    pthread_setspecific(key, (void *)1);
    return NULL;
}

static void test_thread_exit(void) {
    pthread_t t;
    CHECK(pthread_key_create(&key, at_exit) == 0);
    CHECK(pthread_create(&t, NULL, exiting, NULL) == 0);
    pthread_join(t, NULL);
    CHECK(exit_rc == PCP_ERR_CORRUPT);
    CHECK(exit_len == 0 || exit_len == strlen("pcodec payload: unsupported version 9"));
    pthread_key_delete(key);
}

int main(void) {
    test_constants();
    test_round_trips();
    test_errors();
    test_arguments();
    test_last_error_buffers();
    test_threads();
    test_thread_exit();
    if (failures) {
        fprintf(stderr, "%d checks failed\n", failures);
        return 1;
    }
    printf("ok: C API tests passed\n");
    return 0;
}
