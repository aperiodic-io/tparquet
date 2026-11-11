package tparquet_test

import (
	"bytes"
	"fmt"
	"math"
	"math/rand/v2"
	"testing"

	"github.com/apache/arrow-go/v18/arrow"
	"github.com/apache/arrow-go/v18/arrow/array"
	"github.com/apache/arrow-go/v18/arrow/decimal128"
	"github.com/apache/arrow-go/v18/arrow/decimal256"
	"github.com/apache/arrow-go/v18/arrow/float16"
	"github.com/apache/arrow-go/v18/arrow/memory"
	"github.com/apache/arrow-go/v18/parquet"
	"github.com/apache/arrow-go/v18/parquet/compress"
	"github.com/apache/arrow-go/v18/parquet/file"
	"github.com/apache/arrow-go/v18/parquet/pqarrow"

	tparquet "github.com/aperiodic-io/tparquet/go"
)

func nf(name string, typ arrow.DataType) arrow.Field {
	return arrow.Field{Name: name, Type: typ, Nullable: true}
}

// everyType has a column of every Arrow type pqarrow v18 writes (durations, views, intervals
// and 32/64-bit decimals it does not), all nullable, and nested lists, maps and structs with
// nulls at every level.
var everyType = arrow.NewSchema([]arrow.Field{
	nf("i8", arrow.PrimitiveTypes.Int8), nf("i16", arrow.PrimitiveTypes.Int16),
	nf("i32", arrow.PrimitiveTypes.Int32), nf("i64", arrow.PrimitiveTypes.Int64),
	nf("u8", arrow.PrimitiveTypes.Uint8), nf("u16", arrow.PrimitiveTypes.Uint16),
	nf("u32", arrow.PrimitiveTypes.Uint32), nf("u64", arrow.PrimitiveTypes.Uint64),
	nf("f16", arrow.FixedWidthTypes.Float16), nf("f32", arrow.PrimitiveTypes.Float32),
	nf("f64", arrow.PrimitiveTypes.Float64),
	nf("dec9", &arrow.Decimal128Type{Precision: 9, Scale: 2}),
	nf("dec18", &arrow.Decimal128Type{Precision: 18, Scale: 6}),
	nf("dec38", &arrow.Decimal128Type{Precision: 38, Scale: 0}),
	nf("dec256_12", &arrow.Decimal256Type{Precision: 12, Scale: 3}),
	nf("dec256_60", &arrow.Decimal256Type{Precision: 60, Scale: 10}),
	nf("date32", arrow.FixedWidthTypes.Date32), nf("date64", arrow.FixedWidthTypes.Date64),
	nf("time32s", arrow.FixedWidthTypes.Time32s), nf("time32ms", arrow.FixedWidthTypes.Time32ms),
	nf("time64us", arrow.FixedWidthTypes.Time64us), nf("time64ns", arrow.FixedWidthTypes.Time64ns),
	nf("ts_s", arrow.FixedWidthTypes.Timestamp_s), nf("ts_ms", &arrow.TimestampType{Unit: arrow.Millisecond}),
	nf("ts_us", &arrow.TimestampType{Unit: arrow.Microsecond}), nf("ts_ns", &arrow.TimestampType{Unit: arrow.Nanosecond}),
	nf("ts_ms_utc", &arrow.TimestampType{Unit: arrow.Millisecond, TimeZone: "UTC"}),
	nf("ts_us_ny", &arrow.TimestampType{Unit: arrow.Microsecond, TimeZone: "America/New_York"}),
	nf("ts_ns_tokyo", &arrow.TimestampType{Unit: arrow.Nanosecond, TimeZone: "+09:00"}),
	nf("bool", arrow.FixedWidthTypes.Boolean),
	nf("str", arrow.BinaryTypes.String), nf("lstr", arrow.BinaryTypes.LargeString),
	nf("bin", arrow.BinaryTypes.Binary), nf("lbin", arrow.BinaryTypes.LargeBinary),
	nf("fsb", &arrow.FixedSizeBinaryType{ByteWidth: 16}),
	nf("null", arrow.Null),
	nf("dict_str", &arrow.DictionaryType{IndexType: arrow.PrimitiveTypes.Int8, ValueType: arrow.BinaryTypes.String}),
	nf("dict_i64", &arrow.DictionaryType{IndexType: arrow.PrimitiveTypes.Int32, ValueType: arrow.PrimitiveTypes.Int64}),
	nf("dict_f64", &arrow.DictionaryType{IndexType: arrow.PrimitiveTypes.Uint16, ValueType: arrow.PrimitiveTypes.Float64}),
	nf("list", arrow.ListOfField(nf("element", arrow.PrimitiveTypes.Int64))),
	nf("llist", arrow.LargeListOfField(nf("element", arrow.PrimitiveTypes.Float32))),
	// pqarrow cannot write a null fixed-size list ("lists with non-zero length null
	// components are not supported"): the list is required, its elements are not.
	{Name: "fsl", Type: arrow.FixedSizeListOfField(3, nf("element", arrow.PrimitiveTypes.Int32))},
	nf("list_list", arrow.ListOfField(nf("element", arrow.ListOfField(nf("element", arrow.PrimitiveTypes.Int32))))),
	nf("map", arrow.MapOfWithMetadata(arrow.BinaryTypes.String, arrow.Metadata{}, arrow.PrimitiveTypes.Float64, arrow.Metadata{})),
	nf("map_list", arrow.MapOf(arrow.PrimitiveTypes.Int32, arrow.ListOfField(nf("element", arrow.PrimitiveTypes.Int64)))),
	nf("struct", arrow.StructOf(
		nf("a", arrow.PrimitiveTypes.Int32),
		nf("b", arrow.StructOf(
			nf("c", arrow.PrimitiveTypes.Float64),
			nf("d", arrow.ListOfField(nf("element", arrow.StructOf(nf("e", arrow.PrimitiveTypes.Int64), nf("f", arrow.BinaryTypes.String))))),
		)),
		nf("g", arrow.FixedWidthTypes.Timestamp_us),
	)),
}, nil)

// typedRecord fills sc with n rows of seeded data: smooth numbers (what PCODEC is for),
// extremes, signed zeros and infinities, nulls at every level, empty and long lists. Its
// only NaN is the canonical one: arrow-go's dictionary encoder folds NaN payloads.
func typedRecord(sc *arrow.Schema, n int, seed uint64) arrow.RecordBatch {
	r := rand.New(rand.NewPCG(seed, 0x5043))
	b := array.NewRecordBuilder(memory.DefaultAllocator, sc)
	defer b.Release()
	for i := 0; i < n; i++ {
		for j, f := range sc.Fields() {
			appendTyped(b.Field(j), f.Type, f.Nullable, r, i)
		}
	}
	return b.NewRecordBatch()
}

var typedFloats = []float64{0, math.Copysign(0, -1), math.NaN(), math.Inf(1), math.Inf(-1), math.MaxFloat64,
	-math.MaxFloat64, math.SmallestNonzeroFloat64, 1e-310}

func walk(r *rand.Rand, i int) int64 { return int64(i)*37 + int64(r.IntN(9)) }

func appendTyped(b array.Builder, typ arrow.DataType, nullable bool, r *rand.Rand, i int) {
	if nullable && r.IntN(6) == 0 {
		b.AppendNull()
		return
	}
	extreme := r.IntN(40) == 0
	switch b := b.(type) {
	case *array.Int8Builder:
		b.Append(int8(walk(r, i)))
	case *array.Int16Builder:
		b.Append(int16(walk(r, i)))
	case *array.Int32Builder:
		v := int32(walk(r, i))
		if extreme {
			v = []int32{math.MinInt32, math.MaxInt32, -1}[r.IntN(3)]
		}
		b.Append(v)
	case *array.Int64Builder:
		v := 1_000_000_000 + walk(r, i)
		if extreme {
			v = []int64{math.MinInt64, math.MaxInt64, -1}[r.IntN(3)]
		}
		b.Append(v)
	case *array.Uint8Builder:
		b.Append(uint8(walk(r, i)))
	case *array.Uint16Builder:
		b.Append(uint16(walk(r, i)))
	case *array.Uint32Builder:
		v := uint32(walk(r, i))
		if extreme {
			v = math.MaxUint32
		}
		b.Append(v)
	case *array.Uint64Builder:
		v := uint64(walk(r, i))
		if extreme {
			v = math.MaxUint64 - uint64(r.IntN(3))
		}
		b.Append(v)
	case *array.Float16Builder:
		b.Append(float16.New(float32(r.IntN(2000)) / 8))
	case *array.Float32Builder:
		v := float32(walk(r, i)) / 100
		if extreme {
			v = float32(typedFloats[r.IntN(len(typedFloats))])
		}
		b.Append(v)
	case *array.Float64Builder:
		v := float64(walk(r, i)) / 100
		if extreme {
			v = typedFloats[r.IntN(len(typedFloats))]
		}
		b.Append(v)
	case *array.Decimal128Builder:
		p := typ.(*arrow.Decimal128Type).Precision
		v := decimal128.FromI64(walk(r, i) % int64(math.Pow10(int(min(p, 18)))))
		if p > 18 && extreme {
			v = decimal128.FromI64(math.MaxInt64).Mul(decimal128.FromI64(-1000))
		}
		b.Append(v)
	case *array.Decimal256Builder:
		p := typ.(*arrow.Decimal256Type).Precision
		v := decimal256.FromI64(walk(r, i) % int64(math.Pow10(int(min(p, 18)))))
		if p > 18 && extreme {
			v = decimal256.FromI64(math.MaxInt64).Mul(decimal256.FromI64(math.MaxInt64))
		}
		b.Append(v)
	case *array.Date32Builder:
		b.Append(arrow.Date32(19000 + i/10))
	case *array.Date64Builder:
		b.Append(arrow.Date64(int64(19000+i/10) * 86_400_000))
	case *array.Time32Builder:
		v := arrow.Time32(walk(r, i) % 86_400)
		if typ.(*arrow.Time32Type).Unit == arrow.Millisecond {
			v *= 1000
		}
		b.Append(v)
	case *array.Time64Builder:
		v := arrow.Time64(walk(r, i)%86_400) * 1_000_000
		if typ.(*arrow.Time64Type).Unit == arrow.Nanosecond {
			v = v*1000 + arrow.Time64(r.IntN(1000))
		}
		b.Append(v)
	case *array.TimestampBuilder:
		scale := map[arrow.TimeUnit]int64{arrow.Second: 1, arrow.Millisecond: 1e3, arrow.Microsecond: 1e6, arrow.Nanosecond: 1e9}
		b.Append(arrow.Timestamp((1_700_000_000 + walk(r, i)) * scale[typ.(*arrow.TimestampType).Unit]))
	case *array.BooleanBuilder:
		b.Append(r.IntN(3) == 0)
	case *array.StringBuilder:
		b.Append(fmt.Sprintf("s%d\x00é\"\\\n", r.IntN(20)))
	case *array.LargeStringBuilder:
		b.Append(fmt.Sprintf("long string %d %s", i, []string{"", "XNAS", "日本"}[r.IntN(3)]))
	case *array.BinaryBuilder: // binary and large binary
		if typ.ID() == arrow.LARGE_BINARY {
			b.Append(bytes.Repeat([]byte{byte(r.Uint32())}, r.IntN(40)))
		} else {
			b.Append(bytes.Repeat([]byte{byte(i), 0, 255}, r.IntN(4)))
		}
	case *array.FixedSizeBinaryBuilder:
		v := make([]byte, typ.(*arrow.FixedSizeBinaryType).ByteWidth)
		for k := range v {
			v[k] = byte(r.Uint32())
		}
		b.Append(v)
	case *array.NullBuilder:
		b.AppendNull()
	case *array.BinaryDictionaryBuilder:
		if err := b.AppendString([]string{"BTC", "ETH", "SOL", ""}[r.IntN(4)]); err != nil {
			panic(err)
		}
	case *array.Int64DictionaryBuilder:
		if err := b.Append(int64(r.IntN(5)) * 1_000_003); err != nil {
			panic(err)
		}
	case *array.Float64DictionaryBuilder:
		if err := b.Append(float64(r.IntN(7)) / 4); err != nil {
			panic(err)
		}
	case *array.ListBuilder:
		b.Append(true)
		elem := typ.(*arrow.ListType).ElemField()
		for k := r.IntN(5); k > 0; k-- {
			appendTyped(b.ValueBuilder(), elem.Type, elem.Nullable, r, i)
		}
	case *array.LargeListBuilder:
		b.Append(true)
		elem := typ.(*arrow.LargeListType).ElemField()
		for k := r.IntN(5); k > 0; k-- {
			appendTyped(b.ValueBuilder(), elem.Type, elem.Nullable, r, i)
		}
	case *array.FixedSizeListBuilder:
		b.Append(true)
		lt := typ.(*arrow.FixedSizeListType)
		for k := int32(0); k < lt.Len(); k++ {
			appendTyped(b.ValueBuilder(), lt.Elem(), lt.ElemField().Nullable, r, i)
		}
	case *array.MapBuilder:
		b.Append(true)
		mt := typ.(*arrow.MapType)
		for k := r.IntN(4); k > 0; k-- {
			appendTyped(b.KeyBuilder(), mt.KeyType(), false, r, i*10+k)
			appendTyped(b.ItemBuilder(), mt.ItemType(), mt.ItemField().Nullable, r, i)
		}
	case *array.StructBuilder:
		b.Append(true)
		for k, f := range typ.(*arrow.StructType).Fields() {
			appendTyped(b.FieldBuilder(k), f.Type, f.Nullable, r, i)
		}
	default:
		panic(fmt.Sprintf("appendTyped: no generator for %T", b))
	}
}

// writeReference writes recs with plain pqarrow and ZSTD and the same layout options: what
// any file of the same data must read back as.
func writeReference(t testing.TB, recs []arrow.RecordBatch, props ...parquet.WriterProperty) []byte {
	t.Helper()
	return writeWithProps(t, recs, append([]parquet.WriterProperty{parquet.WithCompression(compress.Codecs.Zstd)}, props...)...)
}

func writeAll(t testing.TB, sc *arrow.Schema, recs []arrow.RecordBatch, opts ...tparquet.Option) ([]byte, *tparquet.Plan) {
	t.Helper()
	var buf bytes.Buffer
	w, err := tparquet.NewFileWriter(sc, &buf, opts...)
	if err != nil {
		t.Fatal(err)
	}
	for _, rec := range recs {
		if err := w.Write(rec); err != nil {
			t.Fatal(err)
		}
	}
	if err := w.Close(); err != nil {
		t.Fatal(err)
	}
	return buf.Bytes(), w.Plan()
}

// checkPlanned checks that the file is written as the plan says: the codec of every chunk,
// no dictionary and only V2 pages for PCODEC chunks, V2 pages everywhere if there is one,
// and the footer key.
func checkPlanned(t *testing.T, data []byte, plan *tparquet.Plan) {
	t.Helper()
	rdr, err := file.NewParquetReader(bytes.NewReader(data))
	if err != nil {
		t.Fatal(err)
	}
	defer rdr.Close()
	md := rdr.MetaData()
	if _, ok := footerValue(md, tparquet.FooterKey); ok != plan.UsesPcodec() {
		t.Fatalf("footer key %v, plan uses PCODEC %v", ok, plan.UsesPcodec())
	}
	for g := 0; g < md.NumRowGroups(); g++ {
		for c := 0; c < md.Schema.NumColumns(); c++ {
			cc, err := md.RowGroup(g).ColumnChunk(c)
			if err != nil {
				t.Fatal(err)
			}
			path := cc.PathInSchema().String()
			want, ok := plan.Codec(path)
			if !ok || cc.Compression() != want {
				t.Fatalf("%s: %v, planned %v", path, cc.Compression(), want)
			}
			pco := cc.Compression() == tparquet.Compression
			if pco && cc.HasDictionaryPage() {
				t.Fatalf("%s: a PCODEC chunk with a dictionary page", path)
			}
			if !plan.UsesPcodec() {
				continue
			}
			pr, err := rdr.RowGroup(g).GetColumnPageReader(c)
			if err != nil {
				t.Fatal(err)
			}
			for pr.Next() {
				switch pr.Page().(type) {
				case *file.DataPageV1:
					t.Fatalf("%s: a V1 page in a file with PCODEC columns", path)
				case *file.DictionaryPage:
					if pco {
						t.Fatalf("%s: a PCODEC dictionary page", path)
					}
				}
			}
			if err := pr.Err(); err != nil {
				t.Fatal(err)
			}
		}
	}
}

// TestEveryArrowType writes everyType with each policy, page version and dictionary setting
// (and with every leaf forced to PCODEC), and checks that the file is written as planned and
// reads back, in every reader, as the same data written with plain ZSTD does.
func TestEveryArrowType(t *testing.T) {
	recs := []arrow.RecordBatch{typedRecord(everyType, 600, 1), typedRecord(everyType, 0, 2), typedRecord(everyType, 200, 3)}
	defer func() {
		for _, r := range recs {
			r.Release()
		}
	}()
	allLeafPaths := leaves(t, everyType, parquet.WithStoreDecimalAsInteger(true))
	arrowProps := tparquet.WithArrowWriterProperties(pqarrow.NewArrowWriterProperties(pqarrow.WithStoreSchema()))
	for _, version := range []parquet.DataPageVersion{parquet.DataPageV1, parquet.DataPageV2} {
		for _, dict := range []bool{true, false} {
			layout := []parquet.WriterProperty{parquet.WithDataPageVersion(version), parquet.WithDictionaryDefault(dict),
				parquet.WithStoreDecimalAsInteger(true), parquet.WithMaxRowGroupLength(250), parquet.WithDataPageSize(1024)}
			ref := writeReference(t, recs, layout...)
			refTbl, err := readTable(t, ref)
			if err != nil {
				t.Fatal(err)
			}
			want := jsonlOfTable(t, refTbl)
			refTbl.Release()
			forceAll := []tparquet.Option{tparquet.WithDefaultPolicy(tparquet.Base)}
			for path := range allLeafPaths {
				forceAll = append(forceAll, tparquet.WithPolicy(path, tparquet.Pcodec))
			}
			for name, opts := range map[string][]tparquet.Option{
				"auto":       nil,
				"pcodec":     {tparquet.WithDefaultPolicy(tparquet.Pcodec)},
				"base":       {tparquet.WithDefaultPolicy(tparquet.Base)},
				"every leaf": forceAll,
			} {
				t.Run(fmt.Sprintf("V%d/dict=%v/%s", version+1, dict, name), func(t *testing.T) {
					data, plan := writeAll(t, everyType, recs, append(opts, arrowProps, tparquet.WithWriterProperties(layout...))...)
					checkPlanned(t, data, plan)
					switch name {
					case "pcodec":
						for path, typ := range allLeafPaths {
							if c, _ := plan.Codec(path); (c == tparquet.Compression) != tparquet.Eligible(typ) {
								t.Errorf("%s (%v): %v", path, typ, c)
							}
						}
					case "every leaf":
						if len(plan.PcodecColumns()) != len(allLeafPaths) {
							t.Errorf("%d PCODEC columns of %d", len(plan.PcodecColumns()), len(allLeafPaths))
						}
					case "base":
						if plan.UsesPcodec() {
							t.Error("base uses PCODEC")
						}
					case "auto":
						if !plan.UsesPcodec() {
							t.Error("auto chose no PCODEC column at all")
						}
					}
					// Every reader on two of the files; one whole-table read on the others.
					if (version == parquet.DataPageV2 && !dict && name == "every leaf") || (version == parquet.DataPageV1 && dict && name == "auto") {
						checkReaders(t, data, want)
						return
					}
					tbl, err := readTable(t, data)
					if err != nil {
						t.Fatal(err)
					}
					defer tbl.Release()
					sameJSONL(t, "ReadTable", jsonlOfTable(t, tbl), want)
				})
			}
		}
	}
}
