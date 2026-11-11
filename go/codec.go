package tparquet

import (
	"fmt"

	"github.com/apache/arrow-go/v18/parquet"
	"github.com/apache/arrow-go/v18/parquet/compress"
)

// CodecID is the Parquet CompressionCodec value of PCODEC (0x5043, "PC").
const CodecID = 20547

// Compression is the arrow-go value of the PCODEC codec. Pass it to
// parquet.WithCompressionFor; importing this package registers the codec, so readers
// decode PCODEC column chunks with no further setup.
const Compression = compress.Compression(CodecID)

// FooterKey and FooterValue are the key-value pair writers add to the footer of files that
// have at least one PCODEC column chunk (spec section 2). Readers do not require it.
const (
	FooterKey   = "parquet.pcodec.version"
	FooterValue = "1"
)

// MaxPcodecLevel is the highest pcodec compression level; the default is 8.
const MaxPcodecLevel = 12

// levelTag marks a packed level: bits 16-30 hold 0x5043, bits 8-15 the physical type and
// bits 0-7 the pcodec level plus one (0 for pcodec's default).
const levelTag = 0x5043

// Level packs a column's physical type (the encoder's type hint) and a pcodec compression
// level (0-12, or compress.DefaultCompressionLevel for pcodec's default, 8) into one
// arrow-go compression level. arrow-go hands the column's compression level to the codec,
// which has no other way to learn the column type.
//
// The packing is private to this binding, not part of the file format:
// 0x5043<<16 | type<<8 | (pcoLevel+1). A level that does not carry the tag, such as
// compress.DefaultCompressionLevel or a plain 0-22, means no type hint: the payload then
// uses the ZSTD fallback mode and stays lossless.
//
// Level panics if t is not a Parquet physical type (BOOLEAN to FIXED_LEN_BYTE_ARRAY) or the
// level is out of range; both are programming errors.
func Level(t parquet.Type, pcoLevel int) int {
	if t < parquet.Types.Boolean || t > parquet.Types.FixedLenByteArray {
		panic(fmt.Sprintf("pcodec: %d is not a Parquet physical type", t))
	}
	if pcoLevel < compress.DefaultCompressionLevel || pcoLevel > MaxPcodecLevel {
		panic(fmt.Sprintf("pcodec: pcodec level %d is not in 0..%d", pcoLevel, MaxPcodecLevel))
	}
	return levelTag<<16 | int(t)<<8 | (pcoLevel + 1)
}

// unpackLevel returns the type hint and pcodec level that an arrow-go level carries: -1 and
// -1 (no hint, default level) unless it was made by Level.
func unpackLevel(level int) (physicalType, pcoLevel int32) {
	if level>>16 != levelTag {
		return -1, -1
	}
	return int32(level >> 8 & 0xff), int32(level&0xff) - 1
}

// compressFn is cCompress; tests replace it to make the library fail.
var compressFn = cCompress

type codec struct{}

// abiErr is why the linked library cannot be used, or nil. It is checked once, when the
// package is initialized, and returned (or, where arrow-go's interface has no error, raised as
// a panic) by every codec call and by NewPlan and NewFileWriter, before the library is called:
// a library of another ABI version may not lay out its arguments as this package does.
//
// The check does not panic at init: importing the package registers the codec, so a panic
// would stop every program that imports it, even one that never meets a PCODEC column, and
// before main could report anything.
var abiErr = checkABI(linkedABIVersion(), abiVersion)

// checkABI compares the ABI version of the linked library with the one this package needs.
func checkABI(linked, want uint32) error {
	if linked != want {
		return fmt.Errorf("pcodec: the linked libtparquet_capi has ABI version %d, this package needs %d: build the library from the same tparquet release", linked, want)
	}
	return nil
}

func init() {
	compress.RegisterCodec(Compression, codec{})
}

// Encode is EncodeLevel with compress.DefaultCompressionLevel: no type hint.
func (c codec) Encode(dst, src []byte) []byte {
	return c.EncodeLevel(dst, src, compress.DefaultCompressionLevel)
}

// EncodeLevel writes the payload of src, with the type hint and pcodec level that Level
// packed into level; for FLOAT and DOUBLE hints the encoder also tries DECIMAL mode. It
// writes into dst when cap(dst) is at least CompressBound, as arrow-go's page writer
// arranges.
//
// The interface has no error return. The library fails only on bad arguments, which this
// binding never passes, or on an internal error; in either case EncodeLevel writes a RAW
// payload (the input verbatim, spec section 4.1), which every reader decodes. A linked library
// of another ABI version is not called at all: EncodeLevel panics with that error, as Decode
// does, rather than write a file without the compression asked for.
func (codec) EncodeLevel(dst, src []byte, level int) []byte {
	if abiErr != nil {
		panic(abiErr)
	}
	bound := len(src) + 2
	if cap(dst) < bound {
		dst = make([]byte, bound)
	}
	dst = dst[:bound]
	pt, lvl := unpackLevel(level)
	n, err := compressFn(dst, src, pt, lvl)
	if err != nil {
		dst[0], dst[1] = 1, 0
		copy(dst[2:], src)
		return dst
	}
	return dst[:n]
}

// CompressBound is the largest payload for n input bytes: n + 2.
func (codec) CompressBound(n int64) int64 { return n + 2 }

// Decode is DecodeWithError that panics on a malformed payload, like arrow-go's own codecs.
// arrow-go calls DecodeWithError, so a corrupt page is a read error, not a panic.
func (c codec) Decode(dst, src []byte) []byte {
	out, err := c.DecodeWithError(dst, src)
	if err != nil {
		panic(err)
	}
	return out
}

// DecodeWithError decodes the payload src into dst. len(dst) must be the exact decoded size
// (the page's uncompressed size, minus the levels for a V2 page): the payload is rejected
// if it decodes to any other size. arrow-go sizes dst that way for every page type.
func (codec) DecodeWithError(dst, src []byte) ([]byte, error) {
	if abiErr != nil {
		return nil, abiErr
	}
	if err := cDecompress(dst, src); err != nil {
		return nil, err
	}
	return dst, nil
}
