# tparquet for Go

PCODEC ([spec](../spec/FORMAT.md)) for [Apache arrow-go](https://github.com/apache/arrow-go)
v18.8.0: a Parquet compression codec that runs [pcodec](https://github.com/pcodec/pcodec) on
numeric columns. No fork of arrow-go is needed: the package registers the codec with
`compress.RegisterCodec` on import.

```go
import tparquet "github.com/aperiodic-io/tparquet/go"

// Read: importing the package is enough for file.Reader and pqarrow.

// Write with automatic per-column selection (spec section 6):
w, err := tparquet.NewFileWriter(schema, out)        // Auto, base codec ZSTD(3)
err = w.Write(firstBatch)                           // runs the trial, then writes
err = w.Close()

// Or force codecs, and get plain arrow-go writer properties:
plan, err := tparquet.NewPlan(schema, nil,
	tparquet.WithDefaultPolicy(tparquet.Pcodec),
	tparquet.WithPolicy("venue", tparquet.Base),
	tparquet.WithPcodecLevel(8))
props := parquet.NewWriterProperties(plan.Properties()...)
```

See the package documentation (`go doc`) for the policies, how the type hint travels in the
compression level, and the runnable examples.

## Building

[`INSTALL.md`](../INSTALL.md#go) has the steps for using the package from your own module.

The package is a cgo binding of the Rust C ABI in `rust/capi`. It needs a C compiler and the
static library `libtparquet_capi.a`.

**In a checkout of this repository** (or with a `replace` directive pointing at one), build
the library and use the package as is; it links `../target/release/libtparquet_capi.a`:

```sh
cargo build --release -p tparquet-capi   # or: make -C go lib
cd go && go test ./...
```

**From the module cache**, that relative path does not exist. Build the library once from a
checkout of the same version, then link it by path with the `tparquet_external` build tag:

```sh
cargo build --release -p tparquet-capi
export CGO_LDFLAGS=$PWD/target/release/libtparquet_capi.a
go build -tags tparquet_external ./...
```

The Makefile does the same with `make test TPARQUET_LIB_DIR=/path/to/dir`. On Linux the package
adds the system libraries the Rust library needs (`-lgcc_s -lutil -lrt -lpthread -lm -ldl`).
Only linux/amd64 is tested. Linking a second Rust static library into the same binary can
clash on Rust runtime symbols.

## Checks

| | |
|---|---|
| `make test` | unit, integration, property and acceptance tests, conformance vectors |
| `make check` | `go vet`, staticcheck, `go test -race`, 30 s of each of the five fuzz targets |
| `make mutation` | gremlins; every viable mutant must be killed (`.gremlins.yaml`) |
| `make vectors OUT=dir` | write the Go-produced conformance vectors (`cmd/tparquet-go-gen`) |

`TestConformanceVectors` checks every `../testdata/files/*.parquet` against
`../testdata/expected/<name>.jsonl` (or `$TPARQUET_TESTDATA`) and skips while there are none;
`TestPayloadVectors` decodes every payload of `../testdata/payloads/manifest.json`.
