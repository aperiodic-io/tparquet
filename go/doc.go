// Package tparquet adds PCODEC, pcodec as a Parquet compression codec (spec/FORMAT.md of
// github.com/aperiodic-io/tparquet), to Apache arrow-go.
//
// # Reading
//
// Importing the package registers the codec under compress.Compression(20547) with
// compress.RegisterCodec, so file.Reader and pqarrow read PCODEC column chunks with no other
// change. The payload is self-describing: decoding needs no column type.
//
//	import _ "github.com/aperiodic-io/tparquet/go"
//
// A corrupt payload is a read error (the codec implements compress.ErrorCodec), never a
// panic.
//
// # Writing
//
// FileWriter chooses each column's codec with a Policy: Auto (the default) writes the first
// record batch with rows with the base codec and with PCODEC and keeps the smaller per column, Pcodec
// and Base force one. The base codec is ZSTD level 3 unless WithBaseCodec says otherwise.
// PCODEC columns are written as the spec recommends: PLAIN, no dictionary, data page V2, and
// the column's physical type as the encoder's type hint. Files with a PCODEC column get
// parquet.pcodec.version = "1" in the footer.
//
// NewPlan and Plan.Properties give the same choices as parquet.WriterProperty options for
// use with pqarrow or file.Writer directly, and ColumnProperties the options for one column.
//
// # The type hint travels in the compression level
//
// arrow-go's compress.Codec is not told the column type, so the type hint is packed into the
// per-column compression level, which arrow-go passes to EncodeLevel unchanged (it does not
// validate levels). Level packs it as
//
//	0x5043<<16 | physicalType<<8 | (pcodecLevel+1)
//
// with pcodecLevel+1 = 0 for pcodec's default level, 8. Any level without the 0x5043 tag, such
// as compress.DefaultCompressionLevel, means no hint: the payload then uses the ZSTD fallback
// mode and stays lossless. The packing is private to this binding and not part of the file
// format; readers never see it.
//
// # Building
//
// The package is a cgo binding of the Rust C ABI (rust/capi, include/tparquet.h is
// vendored here). By default it links
// ${SRCDIR}/../target/release/libtparquet_capi.a, which exists in a checkout of the
// repository after
//
//	cargo build --release -p tparquet-capi
//
// (make lib in the go directory). A consumer that uses the module from the module cache has
// no such path; it builds the static library once and links it with the tparquet_external tag:
//
//	CGO_LDFLAGS=/path/to/libtparquet_capi.a go build -tags tparquet_external ./...
//
// or, with the Makefile, make test TPARQUET_LIB_DIR=/path/to. On Linux the library also needs
// -lgcc_s -lutil -lrt -lpthread -lm -ldl, which the package adds itself. Only linux/amd64 is
// tested.
//
// The library must have the ABI version of the vendored header (PCP_ABI_VERSION). The package
// checks it once at init without panicking; with another version every codec call fails with
// an error that says so (Decode and EncodeLevel, which have no error result, panic with it),
// and NewPlan and NewFileWriter return it, before the library is called.
package tparquet
