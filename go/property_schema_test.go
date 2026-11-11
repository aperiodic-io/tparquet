package tparquet_test

import (
	"fmt"
	mrand "math/rand"
	"math/rand/v2"
	"testing"
	"testing/quick"

	"github.com/apache/arrow-go/v18/arrow"
	"github.com/apache/arrow-go/v18/arrow/array"
	"github.com/apache/arrow-go/v18/arrow/memory"
	"github.com/apache/arrow-go/v18/parquet"
	"github.com/apache/arrow-go/v18/parquet/compress"
	"github.com/apache/arrow-go/v18/parquet/pqarrow"

	tparquet "github.com/aperiodic-io/tparquet/go"
)

var propertyLeaves = []arrow.DataType{
	arrow.PrimitiveTypes.Int8, arrow.PrimitiveTypes.Int16, arrow.PrimitiveTypes.Int32, arrow.PrimitiveTypes.Int64,
	arrow.PrimitiveTypes.Uint8, arrow.PrimitiveTypes.Uint16, arrow.PrimitiveTypes.Uint32, arrow.PrimitiveTypes.Uint64,
	arrow.PrimitiveTypes.Float32, arrow.PrimitiveTypes.Float64, arrow.FixedWidthTypes.Float16,
	arrow.FixedWidthTypes.Timestamp_us, &arrow.TimestampType{Unit: arrow.Nanosecond, TimeZone: "UTC"},
	arrow.FixedWidthTypes.Date32, arrow.FixedWidthTypes.Time64us, arrow.FixedWidthTypes.Boolean,
	arrow.BinaryTypes.String, arrow.BinaryTypes.Binary, &arrow.FixedSizeBinaryType{ByteWidth: 5},
	&arrow.Decimal128Type{Precision: 9, Scale: 3}, &arrow.Decimal128Type{Precision: 18, Scale: 0},
	&arrow.DictionaryType{IndexType: arrow.PrimitiveTypes.Int16, ValueType: arrow.BinaryTypes.String},
}

// randomType returns a leaf, or a list, struct or map of random types, depth levels deep.
func randomType(r *rand.Rand, depth int) arrow.DataType {
	if depth == 0 || r.IntN(3) > 0 {
		return propertyLeaves[r.IntN(len(propertyLeaves))]
	}
	switch r.IntN(4) {
	case 0:
		return arrow.ListOfField(arrow.Field{Name: "element", Type: randomType(r, depth-1), Nullable: r.IntN(2) == 0})
	case 1:
		return arrow.LargeListOfField(arrow.Field{Name: "element", Type: randomType(r, depth-1), Nullable: r.IntN(2) == 0})
	case 2:
		return arrow.MapOf(arrow.PrimitiveTypes.Int64, randomType(r, depth-1))
	}
	var fields []arrow.Field
	for i := 0; i < 1+r.IntN(3); i++ {
		fields = append(fields, arrow.Field{Name: fmt.Sprint("f", i), Type: randomType(r, depth-1), Nullable: r.IntN(2) == 0})
	}
	return arrow.StructOf(fields...)
}

func randomSchema(r *rand.Rand) *arrow.Schema {
	var fields []arrow.Field
	for i := 0; i < 1+r.IntN(6); i++ {
		fields = append(fields, arrow.Field{Name: fmt.Sprint("c", i), Type: randomType(r, 3), Nullable: r.IntN(2) == 0})
	}
	return arrow.NewSchema(fields, nil)
}

// TestPropertyRandomSchemas writes random nested schemas with random data, policies and
// writer properties, and checks that each file is written as planned and reads back as the
// same data written with plain ZSTD.
func TestPropertyRandomSchemas(t *testing.T) {
	codecs := []compress.Compression{compress.Codecs.Zstd, compress.Codecs.Snappy, compress.Codecs.Gzip,
		compress.Codecs.Uncompressed, compress.Codecs.Lz4Raw}
	for seed := uint64(1); seed <= 40; seed++ {
		t.Run(fmt.Sprint(seed), func(t *testing.T) {
			r := rand.New(rand.NewPCG(seed, 11))
			sc := randomSchema(r)
			layout := []parquet.WriterProperty{
				parquet.WithDataPageVersion(parquet.DataPageVersion(r.IntN(2))),
				parquet.WithDictionaryDefault(r.IntN(2) == 0),
				parquet.WithStoreDecimalAsInteger(r.IntN(2) == 0),
				parquet.WithMaxRowGroupLength(int64(50 + r.IntN(500))),
				parquet.WithDataPageSize(int64(256 + r.IntN(4096))),
			}
			opts := []tparquet.Option{
				tparquet.WithDefaultPolicy(tparquet.Policy(r.IntN(3))),
				tparquet.WithPcodecLevel(r.IntN(14) - 1),
				tparquet.WithBaseCodec(codecs[r.IntN(len(codecs))], compress.DefaultCompressionLevel),
				tparquet.WithWriterProperties(layout...),
				tparquet.WithArrowWriterProperties(pqarrow.NewArrowWriterProperties(pqarrow.WithStoreSchema())),
			}
			for path := range leaves(t, sc, layout...) {
				if r.IntN(3) == 0 {
					opts = append(opts, tparquet.WithPolicy(path, tparquet.Policy(r.IntN(3))))
				}
			}
			var recs []arrow.RecordBatch
			for i := 0; i < 1+r.IntN(3); i++ {
				recs = append(recs, typedRecord(sc, r.IntN(700), seed*100+uint64(i)))
			}
			defer func() {
				for _, rec := range recs {
					rec.Release()
				}
			}()
			data, plan := writeAll(t, sc, recs, opts...)
			checkPlanned(t, data, plan)
			ref := writeReference(t, recs, layout...)
			refTbl, err := readTable(t, ref)
			if err != nil {
				t.Fatal(err)
			}
			defer refTbl.Release()
			assertSameTable(t, data, refTbl)
		})
	}
}

// chunkTotal is the compressed size of every column chunk of a file.
func chunkTotal(t *testing.T, data []byte) int64 {
	t.Helper()
	md := fileMetadata(t, data)
	var n int64
	for g := 0; g < md.NumRowGroups(); g++ {
		n += md.RowGroup(g).TotalCompressedSize()
	}
	return n
}

// TestPropertyAutoIsNeverLarger: with no column forced, the Auto file of one batch is never
// larger (in column chunk bytes) than the base-codec file: the whole-batch comparison of
// spec section 6 keeps the base codec otherwise.
func TestPropertyAutoIsNeverLarger(t *testing.T) {
	for seed := uint64(1); seed <= 24; seed++ {
		r := rand.New(rand.NewPCG(seed, 12))
		sc := randomSchema(r)
		rec := typedRecord(sc, 1+r.IntN(1500), seed)
		props := tparquet.WithWriterProperties(parquet.WithMaxRowGroupLength(int64(100 + r.IntN(2000))))
		auto, plan := writeAll(t, sc, []arrow.RecordBatch{rec}, props)
		base, _ := writeAll(t, sc, []arrow.RecordBatch{rec}, props, tparquet.WithDefaultPolicy(tparquet.Base))
		rec.Release()
		if a, b := chunkTotal(t, auto), chunkTotal(t, base); a > b || (a == b && plan.UsesPcodec()) {
			t.Errorf("seed %d (%v): Auto %d bytes, base %d", seed, plan.PcodecColumns(), a, b)
		}
	}
}

// TestQuickAutoOneColumnIsTheSmaller: for a file of one numeric column, Auto writes exactly
// the smaller of the base-codec and the PCODEC column (the base codec on a tie).
func TestQuickAutoOneColumnIsTheSmaller(t *testing.T) {
	var pcodecWon, baseWon bool
	f := func(vals []int64, shift uint8, float, cycle bool) bool {
		if len(vals) == 0 {
			return true // no rows: no trial (TestFileWriterOnlyEmptyBatches)
		}
		sc := arrow.NewSchema([]arrow.Field{{Name: "x", Type: arrow.PrimitiveTypes.Int64}}, nil)
		if float && !cycle {
			sc = arrow.NewSchema([]arrow.Field{{Name: "x", Type: arrow.PrimitiveTypes.Float64}}, nil)
		}
		b := array.NewRecordBuilder(memory.DefaultAllocator, sc)
		defer b.Release()
		for i, v := range vals {
			v >>= shift % 64
			for k := 0; k < 20; k++ { // some structure for either codec to find
				if cycle { // a few wide values over and over: the dictionary wins
					b.Field(0).(*array.Int64Builder).Append(vals[(i+k)%min(len(vals), 3)])
					continue
				}
				if float {
					b.Field(0).(*array.Float64Builder).Append(float64(v+int64(k*i)) / 100)
				} else {
					b.Field(0).(*array.Int64Builder).Append(v + int64(k))
				}
			}
		}
		rec := b.NewRecordBatch()
		defer rec.Release()
		auto, plan := writeAll(t, sc, []arrow.RecordBatch{rec})
		base, _ := writeAll(t, sc, []arrow.RecordBatch{rec}, tparquet.WithDefaultPolicy(tparquet.Base))
		pco, _ := writeAll(t, sc, []arrow.RecordBatch{rec}, tparquet.WithDefaultPolicy(tparquet.Pcodec))
		a, bs, p := chunkTotal(t, auto), chunkTotal(t, base), chunkTotal(t, pco)
		pcodecWon = pcodecWon || plan.UsesPcodec()
		baseWon = baseWon || !plan.UsesPcodec()
		if plan.UsesPcodec() != (p < bs) || a != min(bs, p) {
			t.Logf("%d values: auto %d (%v), base %d, pcodec %d", len(vals)*20, a, plan.UsesPcodec(), bs, p)
			return false
		}
		return true
	}
	if err := quick.Check(f, &quick.Config{MaxCount: 80, Rand: mrand.New(mrand.NewSource(3))}); err != nil {
		t.Fatal(err)
	}
	if !pcodecWon || !baseWon {
		t.Fatalf("the generator should make both codecs win: PCODEC %v, base %v", pcodecWon, baseWon)
	}
}
