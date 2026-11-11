package tparquet_test

import (
	"bytes"
	"context"
	"fmt"
	"math/rand/v2"

	"github.com/apache/arrow-go/v18/arrow"
	"github.com/apache/arrow-go/v18/arrow/array"
	"github.com/apache/arrow-go/v18/arrow/memory"
	"github.com/apache/arrow-go/v18/parquet"
	"github.com/apache/arrow-go/v18/parquet/compress"
	"github.com/apache/arrow-go/v18/parquet/file"
	"github.com/apache/arrow-go/v18/parquet/pqarrow"

	tparquet "github.com/aperiodic-io/tparquet/go"
)

// Write trades with Auto codec selection, then read them back with plain pqarrow: importing
// the package is all a reader needs.
func Example() {
	sc := arrow.NewSchema([]arrow.Field{
		{Name: "ts", Type: arrow.FixedWidthTypes.Timestamp_ms},
		{Name: "price", Type: arrow.PrimitiveTypes.Float64},
		{Name: "venue", Type: arrow.BinaryTypes.String},
	}, nil)
	b := array.NewRecordBuilder(memory.DefaultAllocator, sc)
	defer b.Release()
	r := rand.New(rand.NewPCG(1, 2))
	ticks := int64(6_543_210) // prices on a 0.01 grid
	for i := 0; i < 10000; i++ {
		ticks += r.Int64N(5) - 2
		b.Field(0).(*array.TimestampBuilder).Append(arrow.Timestamp(1_700_000_000_000 + int64(i)*25))
		b.Field(1).(*array.Float64Builder).Append(float64(ticks) / 100)
		b.Field(2).(*array.StringBuilder).Append("XNAS")
	}
	rec := b.NewRecordBatch()
	defer rec.Release()

	var buf bytes.Buffer
	w, err := tparquet.NewFileWriter(sc, &buf) // Auto, base codec ZSTD(3)
	if err != nil {
		panic(err)
	}
	if err := w.Write(rec); err != nil {
		panic(err)
	}
	if err := w.Close(); err != nil {
		panic(err)
	}
	fmt.Println("PCODEC columns:", w.Plan().PcodecColumns())

	rdr, err := file.NewParquetReader(bytes.NewReader(buf.Bytes()))
	if err != nil {
		panic(err)
	}
	defer rdr.Close()
	fmt.Println("footer:", *rdr.MetaData().KeyValueMetadata().FindValue(tparquet.FooterKey))
	fr, err := pqarrow.NewFileReader(rdr, pqarrow.ArrowReadProperties{}, memory.DefaultAllocator)
	if err != nil {
		panic(err)
	}
	tbl, err := fr.ReadTable(context.Background())
	if err != nil {
		panic(err)
	}
	defer tbl.Release()
	fmt.Println("rows:", tbl.NumRows(), "last price:", tbl.Column(1).Data().Chunk(0).(*array.Float64).Value(9999))
	// Output:
	// PCODEC columns: [ts price]
	// footer: 1
	// rows: 10000 last price: 65432.09
}

// Write one column with PCODEC using plain arrow-go writer properties.
func ExampleColumnProperties() {
	props := parquet.NewWriterProperties(append(
		tparquet.ColumnProperties("price", parquet.Types.Double, 8),
		parquet.WithDataPageVersion(parquet.DataPageV2))...)
	fmt.Println(props.CompressionFor("price") == tparquet.Compression, props.DictionaryEnabledFor("price"))
	// Output: true false
}

// The packed level of an INT64 column at pcodec level 8.
func ExampleLevel() {
	fmt.Printf("%#x\n", tparquet.Level(parquet.Types.Int64, 8))
	// Output: 0x50430209
}

// exampleTrades is n rows of a smooth timestamp, a price on a cent grid and a venue.
func exampleTrades(n int) arrow.RecordBatch {
	sc := arrow.NewSchema([]arrow.Field{
		{Name: "ts", Type: arrow.FixedWidthTypes.Timestamp_ms},
		{Name: "price", Type: arrow.PrimitiveTypes.Float64},
		{Name: "venue", Type: arrow.BinaryTypes.String},
	}, nil)
	b := array.NewRecordBuilder(memory.DefaultAllocator, sc)
	defer b.Release()
	for i := 0; i < n; i++ {
		b.Field(0).(*array.TimestampBuilder).Append(arrow.Timestamp(1_700_000_000_000 + int64(i)*25))
		b.Field(1).(*array.Float64Builder).Append(float64(6_543_210+i%17) / 100)
		b.Field(2).(*array.StringBuilder).Append([]string{"XNAS", "XNYS"}[i%2])
	}
	return b.NewRecordBatch()
}

// Choose the codecs on a sample, then write with plain pqarrow and the plan's properties.
func ExampleNewPlan() {
	rec := exampleTrades(5000)
	defer rec.Release()
	plan, err := tparquet.NewPlan(rec.Schema(), rec)
	if err != nil {
		panic(err)
	}
	for _, path := range []string{"ts", "price", "venue"} {
		c, _ := plan.Codec(path)
		fmt.Println(path, c == tparquet.Compression)
	}
	var buf bytes.Buffer
	w, err := plan.NewFileWriter(&buf) // a *pqarrow.FileWriter
	if err != nil {
		panic(err)
	}
	if err := w.Write(rec); err != nil {
		panic(err)
	}
	if err := w.Close(); err != nil {
		panic(err)
	}
	// Output:
	// ts true
	// price true
	// venue false
}

// Force PCODEC on one column, keep the base codec on another, and use Snappy as the base.
func ExampleWithPolicy() {
	sc := exampleTrades(0).Schema()
	plan, err := tparquet.NewPlan(sc, nil,
		tparquet.WithPolicy("price", tparquet.Pcodec),
		tparquet.WithPolicy("ts", tparquet.Base),
		tparquet.WithBaseCodec(compress.Codecs.Snappy, compress.DefaultCompressionLevel))
	if err != nil {
		panic(err)
	}
	fmt.Println(plan.PcodecColumns())
	c, _ := plan.Codec("ts")
	fmt.Println(c)
	// Output:
	// [price]
	// SNAPPY
}

// Which physical types PCODEC models.
func ExampleEligible() {
	for _, t := range []parquet.Type{parquet.Types.Int32, parquet.Types.Double, parquet.Types.ByteArray, parquet.Types.Int96} {
		fmt.Println(t, tparquet.Eligible(t))
	}
	// Output:
	// INT32 true
	// DOUBLE true
	// BYTE_ARRAY false
	// INT96 false
}

// Read one column of one row group of a PCODEC file with arrow-go's low-level reader.
func Example_lowLevelReader() {
	rec := exampleTrades(3000)
	defer rec.Release()
	var buf bytes.Buffer
	w, err := tparquet.NewFileWriter(rec.Schema(), &buf, tparquet.WithDefaultPolicy(tparquet.Pcodec),
		tparquet.WithWriterProperties(parquet.WithMaxRowGroupLength(1000)))
	if err != nil {
		panic(err)
	}
	if err := w.Write(rec); err != nil {
		panic(err)
	}
	if err := w.Close(); err != nil {
		panic(err)
	}
	rdr, err := file.NewParquetReader(bytes.NewReader(buf.Bytes()))
	if err != nil {
		panic(err)
	}
	defer rdr.Close()
	cr, err := rdr.RowGroup(2).Column(0) // ts, rows 2000-2999
	if err != nil {
		panic(err)
	}
	values := make([]int64, 1000)
	n, _, err := cr.(*file.Int64ColumnChunkReader).ReadBatch(1000, values, nil, nil)
	if err != nil {
		panic(err)
	}
	cc, _ := rdr.MetaData().RowGroup(2).ColumnChunk(0)
	fmt.Println(n, values[0], cc.Compression() == tparquet.Compression)
	// Output: 1000 1700000050000 true
}

// Read only some row groups and columns with pqarrow.
func Example_readRowGroups() {
	rec := exampleTrades(3000)
	defer rec.Release()
	var buf bytes.Buffer
	w, err := tparquet.NewFileWriter(rec.Schema(), &buf, tparquet.WithWriterProperties(parquet.WithMaxRowGroupLength(1000)))
	if err != nil {
		panic(err)
	}
	if err := w.Write(rec); err != nil {
		panic(err)
	}
	if err := w.Close(); err != nil {
		panic(err)
	}
	rdr, err := file.NewParquetReader(bytes.NewReader(buf.Bytes()))
	if err != nil {
		panic(err)
	}
	defer rdr.Close()
	fr, err := pqarrow.NewFileReader(rdr, pqarrow.ArrowReadProperties{}, memory.DefaultAllocator)
	if err != nil {
		panic(err)
	}
	tbl, err := fr.ReadRowGroups(context.Background(), []int{1}, []int{0, 2}) // price, 2 row groups
	if err != nil {
		panic(err)
	}
	defer tbl.Release()
	fmt.Println(tbl.NumRows(), tbl.Schema().Field(0).Name)
	// Output: 2000 price
}
