package tparquet_test

import (
	"bytes"
	"fmt"
	"slices"
	"testing"

	"github.com/apache/arrow-go/v18/arrow"
	"github.com/apache/arrow-go/v18/arrow/array"
	"github.com/apache/arrow-go/v18/arrow/memory"
	"github.com/apache/arrow-go/v18/parquet"
	"github.com/apache/arrow-go/v18/parquet/compress"
	"github.com/apache/arrow-go/v18/parquet/file"
	"github.com/apache/arrow-go/v18/parquet/metadata"
	"github.com/apache/arrow-go/v18/parquet/pqarrow"

	tparquet "github.com/aperiodic-io/tparquet/go"
)

// TestWithPcodecLevelReachesEveryPage: each pcodec level (and the default) is packed, with
// the column's type, into the level arrow-go hands EncodeLevel for every page.
func TestWithPcodecLevelReachesEveryPage(t *testing.T) {
	real, _ := compress.GetCodec(tparquet.Compression)
	spy := &levelSpy{Codec: real}
	compress.RegisterCodec(tparquet.Compression, spy)
	t.Cleanup(func() { compress.RegisterCodec(tparquet.Compression, real) })

	rec := typedRecord(mixedSchema, 3000, 4)
	defer rec.Release()
	want := map[int]bool{}
	for lvl := -1; lvl <= tparquet.MaxPcodecLevel; lvl++ {
		spy.levels = nil
		writeAll(t, mixedSchema, []arrow.RecordBatch{rec}, tparquet.WithDefaultPolicy(tparquet.Pcodec),
			tparquet.WithPcodecLevel(lvl), tparquet.WithWriterProperties(parquet.WithDataPageSize(2048)))
		clear(want)
		for _, typ := range []parquet.Type{parquet.Types.Int64, parquet.Types.Double, parquet.Types.Int32, parquet.Types.Float} {
			want[tparquet.Level(typ, lvl)] = true
		}
		got := map[int]bool{}
		for _, l := range spy.levels {
			got[l] = true
		}
		if fmt.Sprint(got) != fmt.Sprint(want) || len(spy.levels) < 8 {
			t.Fatalf("level %d: %d pages with levels %v, want %v", lvl, len(spy.levels), got, want)
		}
	}
}

// TestWithBaseCodecWritesBaseColumns: every codec arrow-go implements can be the base codec;
// its level reaches the writer properties.
func TestWithBaseCodecWritesBaseColumns(t *testing.T) {
	rec := typedRecord(mixedSchema, 2000, 5)
	defer rec.Release()
	for _, c := range []compress.Compression{compress.Codecs.Uncompressed, compress.Codecs.Snappy, compress.Codecs.Gzip,
		compress.Codecs.Brotli, compress.Codecs.Zstd, compress.Codecs.Lz4Raw} {
		t.Run(c.String(), func(t *testing.T) {
			data, plan := writeAll(t, mixedSchema, []arrow.RecordBatch{rec},
				tparquet.WithBaseCodec(c, 1), tparquet.WithDefaultPolicy(tparquet.Base), tparquet.WithPolicy("price", tparquet.Pcodec))
			codecs := columnCodecs(t, fileMetadata(t, data))
			for path, got := range codecs {
				want := c.String()
				if path == "price" {
					want = "PCODEC"
				} else if got != want && !(c == compress.Codecs.Uncompressed && got == "UNCOMPRESSED") {
					t.Errorf("%s: %s, want %s", path, got, want)
				}
			}
			props := parquet.NewWriterProperties(plan.Properties()...)
			if props.CompressionFor("name") != c || props.CompressionLevelFor("name") != 1 {
				t.Errorf("base properties %v %d", props.CompressionFor("name"), props.CompressionLevelFor("name"))
			}
			assertSameData(t, data, rec)
		})
	}
}

// TestOptionsLastWins: options apply in order, so a later one overrides an earlier one.
func TestOptionsLastWins(t *testing.T) {
	p := newPlan(t, mixedSchema, nil,
		tparquet.WithDefaultPolicy(tparquet.Base), tparquet.WithDefaultPolicy(tparquet.Pcodec),
		tparquet.WithPolicy("price", tparquet.Pcodec), tparquet.WithPolicy("price", tparquet.Base),
		tparquet.WithBaseCodec(compress.Codecs.Gzip, 2), tparquet.WithBaseCodec(compress.Codecs.Snappy, 0),
		tparquet.WithPcodecLevel(1), tparquet.WithPcodecLevel(5),
		tparquet.WithArrowWriterProperties(pqarrow.NewArrowWriterProperties(pqarrow.WithStoreSchema())),
		tparquet.WithArrowWriterProperties(pqarrow.DefaultWriterProps()),
		tparquet.WithWriterProperties(parquet.WithMaxRowGroupLength(10)),
		tparquet.WithWriterProperties(parquet.WithDataPageSize(999)))
	wantCodecs(t, p, map[string]compress.Compression{"price": compress.Codecs.Snappy, "ts": tparquet.Compression})
	props := parquet.NewWriterProperties(p.Properties()...)
	if props.CompressionLevelFor("ts") != tparquet.Level(parquet.Types.Int64, 5) {
		t.Error("pcodec level")
	}
	// WithWriterProperties accumulates.
	if props.MaxRowGroupLength() != 10 || props.DataPageSize() != 999 {
		t.Errorf("row group length %d, page size %d", props.MaxRowGroupLength(), props.DataPageSize())
	}
	rec := typedRecord(mixedSchema, 20, 1)
	defer rec.Release()
	var buf bytes.Buffer
	w, err := p.NewFileWriter(&buf)
	if err != nil {
		t.Fatal(err)
	}
	if err := w.Write(rec); err != nil {
		t.Fatal(err)
	}
	if err := w.Close(); err != nil {
		t.Fatal(err)
	}
	if _, ok := footerValue(fileMetadata(t, buf.Bytes()), "ARROW:schema"); ok {
		t.Error("the second WithArrowWriterProperties should win")
	}
}

// TestPlanPropertiesKeepPcodecSettings: the plan's PCODEC column settings come after the
// user's writer properties, which cannot undo them; the user's settings stay on the others.
func TestPlanPropertiesKeepPcodecSettings(t *testing.T) {
	p := newPlan(t, mixedSchema, nil, tparquet.WithPolicy("ts", tparquet.Pcodec),
		tparquet.WithWriterProperties(
			parquet.WithDataPageVersion(parquet.DataPageV1),
			parquet.WithDictionaryFor("ts", true), parquet.WithEncodingFor("ts", parquet.Encodings.DeltaBinaryPacked),
			parquet.WithCompressionFor("ts", compress.Codecs.Gzip), parquet.WithCompressionLevelFor("ts", 9),
			parquet.WithEncodingFor("price", parquet.Encodings.ByteStreamSplit),
			parquet.WithCompressionFor("name", compress.Codecs.Snappy)))
	props := parquet.NewWriterProperties(p.Properties()...)
	if props.CompressionFor("ts") != tparquet.Compression || props.CompressionLevelFor("ts") != tparquet.Level(parquet.Types.Int64, -1) ||
		props.DictionaryEnabledFor("ts") || props.EncodingFor("ts") != parquet.Encodings.Plain || props.DataPageVersion() != parquet.DataPageV2 {
		t.Error("the user's properties override the PCODEC settings of ts")
	}
	if props.EncodingFor("price") != parquet.Encodings.ByteStreamSplit || props.CompressionFor("name") != compress.Codecs.Snappy {
		t.Error("the user's properties of the other columns are lost")
	}
}

// TestWriterPropertiesWithPcodecColumns: statistics, page indexes, bloom filters, sorting
// columns, created-by and row group and page limits all work on PCODEC columns.
func TestWriterPropertiesWithPcodecColumns(t *testing.T) {
	sc := arrow.NewSchema([]arrow.Field{
		{Name: "ts", Type: arrow.PrimitiveTypes.Int64},
		{Name: "price", Type: arrow.PrimitiveTypes.Float64, Nullable: true},
		{Name: "name", Type: arrow.BinaryTypes.String},
	}, nil)
	b := array.NewRecordBuilder(memory.DefaultAllocator, sc)
	defer b.Release()
	for i := 0; i < 10000; i++ {
		b.Field(0).(*array.Int64Builder).Append(1_700_000_000_000 + int64(i)*10)
		if i%5 == 0 {
			b.Field(1).AppendNull()
		} else {
			b.Field(1).(*array.Float64Builder).Append(float64(i%1000) / 4)
		}
		b.Field(2).(*array.StringBuilder).Append(fmt.Sprint("n", i%7))
	}
	rec := b.NewRecordBatch()
	defer rec.Release()
	data, plan := writeAll(t, sc, []arrow.RecordBatch{rec}, tparquet.WithDefaultPolicy(tparquet.Pcodec),
		tparquet.WithWriterProperties(
			parquet.WithStats(true), parquet.WithPageIndexEnabled(true),
			parquet.WithBloomFilterEnabledFor("ts", true), parquet.WithBloomFilterEnabledFor("price", true),
			parquet.WithSortingColumns([]parquet.SortingColumn{{ColumnIdx: 0}}),
			parquet.WithCreatedBy("tparquet test"),
			parquet.WithMaxRowGroupLength(4000), parquet.WithDataPageSize(4096), parquet.WithBatchSize(512)))
	if got := plan.PcodecColumns(); !slices.Equal(got, []string{"ts", "price"}) {
		t.Fatalf("plan %v", got)
	}
	assertSameData(t, data, rec)
	rdr, err := file.NewParquetReader(bytes.NewReader(data))
	if err != nil {
		t.Fatal(err)
	}
	defer rdr.Close()
	md := rdr.MetaData()
	if md.NumRowGroups() != 3 || md.GetCreatedBy() != "tparquet test" {
		t.Fatalf("%d row groups, created by %q", md.NumRowGroups(), md.GetCreatedBy())
	}
	pir := rdr.GetPageIndexReader()
	bfr := rdr.GetBloomFilterReader()
	for g := 0; g < md.NumRowGroups(); g++ {
		rg := md.RowGroup(g)
		if sc := rg.SortingColumns(); len(sc) != 1 || sc[0].ColumnIdx != 0 {
			t.Errorf("row group %d: sorting columns %v", g, sc)
		}
		rgi, err := pir.RowGroup(g)
		if err != nil {
			t.Fatal(err)
		}
		rgb, err := bfr.RowGroup(g)
		if err != nil {
			t.Fatal(err)
		}
		for c := 0; c < 2; c++ {
			cc, err := rg.ColumnChunk(c)
			if err != nil {
				t.Fatal(err)
			}
			if cc.Compression() != tparquet.Compression {
				t.Fatalf("%s is not PCODEC", cc.PathInSchema())
			}
			stats, err := cc.Statistics()
			if err != nil || stats == nil || !stats.HasMinMax() {
				t.Fatalf("%s: statistics %v %v", cc.PathInSchema(), stats, err)
			}
			oi, err := rgi.GetOffsetIndex(c)
			if err != nil || oi == nil {
				t.Fatalf("%s: offset index %v %v", cc.PathInSchema(), oi, err)
			}
			pr, err := rdr.RowGroup(g).GetColumnPageReader(c)
			if err != nil {
				t.Fatal(err)
			}
			pages := 0
			for pr.Next() {
				pages++
			}
			if pages != len(oi.GetPageLocations()) || pages < 2 {
				t.Errorf("%s: %d pages, %d in the offset index", cc.PathInSchema(), pages, len(oi.GetPageLocations()))
			}
			ci, err := rgi.GetColumnIndex(c)
			if err != nil || ci == nil || len(ci.GetNullPages()) != pages {
				t.Errorf("%s: column index %v %v", cc.PathInSchema(), ci, err)
			}
			bf, err := rgb.GetColumnBloomFilter(c)
			if err != nil || bf == nil {
				t.Fatalf("%s: bloom filter %v %v", cc.PathInSchema(), bf, err)
			}
			if c == 0 {
				typed := metadata.TypedBloomFilter[int64]{BloomFilter: bf}
				if !typed.Check(1_700_000_000_000+int64(g)*40000) || typed.Check(42) && typed.Check(43) && typed.Check(44) {
					t.Errorf("row group %d: the bloom filter of ts does not know its values", g)
				}
			}
		}
	}
}

// TestArrowWriterPropertiesReachTheFile: coerced timestamps, INT96 timestamps and the stored
// Arrow schema, with Auto and with forced PCODEC.
func TestArrowWriterPropertiesReachTheFile(t *testing.T) {
	sc := arrow.NewSchema([]arrow.Field{{Name: "t", Type: arrow.FixedWidthTypes.Timestamp_ns},
		{Name: "x", Type: arrow.PrimitiveTypes.Int64}}, nil)
	b := array.NewRecordBuilder(memory.DefaultAllocator, sc)
	defer b.Release()
	for i := 0; i < 5000; i++ {
		b.Field(0).(*array.TimestampBuilder).Append(arrow.Timestamp(1_700_000_000_000_000_000 + int64(i)*1_000_000))
		b.Field(1).(*array.Int64Builder).Append(int64(i))
	}
	rec := b.NewRecordBatch()
	defer rec.Release()
	cases := []struct {
		name  string
		props pqarrow.ArrowWriterProperties
		typ   parquet.Type
		unit  string
	}{
		{"coerce to ms", pqarrow.NewArrowWriterProperties(pqarrow.WithCoerceTimestamps(arrow.Millisecond), pqarrow.WithTruncatedTimestamps(true)), parquet.Types.Int64, "milliseconds"},
		{"int96", pqarrow.NewArrowWriterProperties(pqarrow.WithDeprecatedInt96Timestamps(true)), parquet.Types.Int96, ""},
		{"store schema", pqarrow.NewArrowWriterProperties(pqarrow.WithStoreSchema()), parquet.Types.Int64, "nanoseconds"},
	}
	for _, tc := range cases {
		for _, pol := range []tparquet.Policy{tparquet.Auto, tparquet.Pcodec} {
			t.Run(fmt.Sprint(tc.name, pol), func(t *testing.T) {
				data, plan := writeAll(t, sc, []arrow.RecordBatch{rec}, tparquet.WithDefaultPolicy(pol),
					tparquet.WithPolicy("t", pol), tparquet.WithArrowWriterProperties(tc.props))
				md := fileMetadata(t, data)
				col := md.Schema.Column(0)
				if col.PhysicalType() != tc.typ {
					t.Fatalf("t is %v, want %v", col.PhysicalType(), tc.typ)
				}
				if tc.unit != "" && !bytes.Contains([]byte(col.LogicalType().String()), []byte(tc.unit)) {
					t.Fatalf("t: logical type %v, want %s", col.LogicalType(), tc.unit)
				}
				c, _ := plan.Codec("t")
				if pol == tparquet.Pcodec && c != tparquet.Compression {
					t.Fatal("t is forced to PCODEC")
				}
				if tc.typ == parquet.Types.Int96 && pol == tparquet.Auto && c == tparquet.Compression {
					t.Fatal("Auto chose PCODEC for INT96")
				}
				if _, stored := footerValue(md, "ARROW:schema"); stored != (tc.name == "store schema") {
					t.Fatalf("ARROW:schema stored: %v", stored)
				}
				tbl, err := readTable(t, data)
				if err != nil {
					t.Fatal(err)
				}
				defer tbl.Release()
				if tbl.NumRows() != 5000 {
					t.Fatalf("%d rows", tbl.NumRows())
				}
			})
		}
	}
}

// TestPcodecDictionaryFallback: a PCODEC column that keeps its dictionary (column properties
// set by hand) and outgrows it falls back to PLAIN pages mid-chunk; every page kind is
// PCODEC and the data reads back.
func TestPcodecDictionaryFallback(t *testing.T) {
	sc := arrow.NewSchema([]arrow.Field{{Name: "x", Type: arrow.PrimitiveTypes.Int64, Nullable: true}}, nil)
	b := array.NewRecordBuilder(memory.DefaultAllocator, sc)
	defer b.Release()
	for i := 0; i < 20000; i++ {
		if i%9 == 0 {
			b.Field(0).AppendNull()
		} else if i < 5000 {
			b.Field(0).(*array.Int64Builder).Append(int64(i % 10))
		} else {
			b.Field(0).(*array.Int64Builder).Append(int64(i) * 1000)
		}
	}
	rec := b.NewRecordBatch()
	defer rec.Release()
	for _, version := range []parquet.DataPageVersion{parquet.DataPageV1, parquet.DataPageV2} {
		data := writeWithProps(t, []arrow.RecordBatch{rec}, parquet.WithDataPageVersion(version),
			parquet.WithCompressionFor("x", tparquet.Compression), parquet.WithCompressionLevelFor("x", tparquet.Level(parquet.Types.Int64, 3)),
			parquet.WithDictionaryPageSizeLimit(4096), parquet.WithDataPageSize(2048))
		rdr, err := file.NewParquetReader(bytes.NewReader(data))
		if err != nil {
			t.Fatal(err)
		}
		pr, err := rdr.RowGroup(0).GetColumnPageReader(0)
		if err != nil {
			t.Fatal(err)
		}
		encodings := map[parquet.Encoding]int{}
		for pr.Next() {
			if dp, ok := pr.Page().(file.DataPage); ok {
				encodings[parquet.Encoding(dp.Encoding())]++
			}
		}
		if err := pr.Err(); err != nil {
			t.Fatal(err)
		}
		rdr.Close()
		if encodings[parquet.Encodings.RLEDict] == 0 || encodings[parquet.Encodings.Plain] == 0 {
			t.Fatalf("V%d: page encodings %v, want dictionary and plain pages", version+1, encodings)
		}
		assertSameData(t, data, rec)
	}
}
