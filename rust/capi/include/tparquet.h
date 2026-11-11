/* C ABI of the PCODEC Parquet compression codec payload (spec/FORMAT.md).
 *
 * Thread-safe. No function allocates memory the caller must free. Pointers may be NULL only
 * when their length is 0, lengths may not exceed PTRDIFF_MAX, and a call's src and dst may not
 * overlap; any of these is PCP_ERR_ARGUMENT. */
#ifndef TPARQUET_H
#define TPARQUET_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define PCP_ABI_VERSION 1

/* return codes */
#define PCP_OK 0
#define PCP_ERR_ARGUMENT 1 /* bad argument: level above 12, NULL with a length, small buffer,
                              overlapping buffers, length above PTRDIFF_MAX */
#define PCP_ERR_CORRUPT 2  /* malformed payload */
#define PCP_ERR_SIZE 3     /* payload decodes to a different size than dst_len */
#define PCP_ERR_PANIC 4    /* internal error; please report it */

/* parquet CompressionCodec value of PCODEC (20547) */
int32_t pcp_codec_id(void);
/* PCP_ABI_VERSION of the linked library */
uint32_t pcp_abi_version(void);

/* largest payload pcp_compress can write for src_len input bytes (src_len + 2, saturating at
 * SIZE_MAX) */
size_t pcp_compress_bound(size_t src_len);

/* Encodes src into dst (capacity dst_cap >= pcp_compress_bound(src_len)), writing the payload
 * length to *written (only on success).
 *   physical_type: Parquet Type value of the column (1 INT32, 2 INT64, 4 FLOAT, 5 DOUBLE), or any
 *                  other value (e.g. -1) for none;
 *   level:         pcodec level 0..12, or -1 for the default (8);
 *   decimal:       non-zero to try DECIMAL mode for floats. */
int32_t pcp_compress(const uint8_t *src, size_t src_len, int32_t physical_type, int32_t level,
                     int32_t decimal, uint8_t *dst, size_t dst_cap, size_t *written);

/* Decodes a payload into dst, which it must fill exactly (dst_len = the page's uncompressed
 * size). */
int32_t pcp_decompress(const uint8_t *src, size_t src_len, uint8_t *dst, size_t dst_len);

/* Copies the calling thread's last error message, NUL-terminated and truncated to cap bytes,
 * into buf (nothing if buf is NULL or cap is 0). Returns the message length without the NUL
 * (0 if the last call succeeded). Calls made while the thread exits (from a thread-local or
 * pthread key destructor) work but keep no message. */
size_t pcp_last_error(char *buf, size_t cap);

#ifdef __cplusplus
}
#endif

#endif /* TPARQUET_H */
