package tparquet

/*
#cgo CFLAGS: -I${SRCDIR}/include
#include "tparquet.h"

// The library keeps its last error per OS thread, and a goroutine may move to another
// thread between two cgo calls, so each wrapper reads the error in the same C call.

static int32_t pcpgo_compress(const uint8_t *src, size_t src_len, int32_t physical_type,
                              int32_t level, uint8_t *dst, size_t dst_cap, size_t *written,
                              char *err, size_t err_cap) {
	int32_t rc = pcp_compress(src, src_len, physical_type, level, 1, dst, dst_cap, written);
	if (rc != PCP_OK) pcp_last_error(err, err_cap);
	return rc;
}

static int32_t pcpgo_decompress(const uint8_t *src, size_t src_len, uint8_t *dst, size_t dst_len,
                                char *err, size_t err_cap) {
	int32_t rc = pcp_decompress(src, src_len, dst, dst_len);
	if (rc != PCP_OK) pcp_last_error(err, err_cap);
	return rc;
}
*/
import "C"

import (
	"errors"
	"unsafe"
)

const errCap = 512

func ptr(b []byte) *C.uint8_t { return (*C.uint8_t)(unsafe.Pointer(unsafe.SliceData(b))) }

func cError(buf *[errCap]byte) error {
	n := 0
	for n < len(buf) && buf[n] != 0 {
		n++
	}
	return errors.New(string(buf[:n]))
}

// cCompress writes the payload of src into dst, which holds at least compressBound(len(src))
// bytes, and returns its length.
func cCompress(dst, src []byte, physicalType, level int32) (int, error) {
	var written C.size_t
	var msg [errCap]byte
	rc := C.pcpgo_compress(ptr(src), C.size_t(len(src)), C.int32_t(physicalType), C.int32_t(level),
		ptr(dst), C.size_t(len(dst)), &written, (*C.char)(unsafe.Pointer(&msg[0])), errCap)
	if rc != C.PCP_OK {
		return 0, cError(&msg)
	}
	return int(written), nil
}

// cDecompress decodes the payload src into dst, which it must fill exactly.
func cDecompress(dst, src []byte) error {
	var msg [errCap]byte
	rc := C.pcpgo_decompress(ptr(src), C.size_t(len(src)), ptr(dst), C.size_t(len(dst)),
		(*C.char)(unsafe.Pointer(&msg[0])), errCap)
	if rc != C.PCP_OK {
		return cError(&msg)
	}
	return nil
}

func linkedCodecID() int32 { return int32(C.pcp_codec_id()) }

func linkedCompressBound(n int64) int64 { return int64(C.pcp_compress_bound(C.size_t(n))) }

func linkedABIVersion() uint32 { return uint32(C.pcp_abi_version()) }

// abiVersion is PCP_ABI_VERSION of the header this package is compiled with.
const abiVersion uint32 = C.PCP_ABI_VERSION
