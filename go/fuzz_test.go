package tparquet_test

import (
	"bytes"
	"encoding/binary"
	"math"
	"testing"

	"github.com/apache/arrow-go/v18/arrow"
	"github.com/apache/arrow-go/v18/arrow/array"
	"github.com/apache/arrow-go/v18/arrow/memory"
	"github.com/apache/arrow-go/v18/parquet"
	"github.com/apache/arrow-go/v18/parquet/compress"

	tparquet "github.com/aperiodic-io/tparquet/go"
)

func seedPayloads(f *testing.F) {
	c, _ := compress.GetCodec(tparquet.Compression)
	srcs := [][]byte{nil, ramp(50), float64Bytes(1.25, 3.5, 100.75, 2.0), []byte("not numbers at all")}
	for _, src := range srcs {
		for _, typ := range []parquet.Type{parquet.Types.Int32, parquet.Types.Int64, parquet.Types.Float, parquet.Types.Double} {
			f.Add(c.EncodeLevel(nil, src, tparquet.Level(typ, 8)), uint16(len(src)))
		}
		f.Add(c.Encode(nil, src), uint16(len(src)))
	}
}

// FuzzDecode: any payload and size either decodes to exactly that many bytes or fails; it
// never panics or writes past dst.
func FuzzDecode(f *testing.F) {
	seedPayloads(f)
	c, _ := compress.GetCodec(tparquet.Compression)
	ec := c.(compress.ErrorCodec)
	f.Fuzz(func(t *testing.T, payload []byte, size uint16) {
		buf := make([]byte, int(size)+16)
		for i := range buf {
			buf[i] = 0xa5
		}
		out, err := ec.DecodeWithError(buf[:size:size], payload)
		if err == nil && len(out) != int(size) {
			t.Fatalf("decoded %d bytes into a dst of %d", len(out), size)
		}
		if !bytes.Equal(buf[size:], bytes.Repeat([]byte{0xa5}, 16)) {
			t.Fatal("the decoder wrote past dst")
		}
	})
}

// FuzzRoundTrip: any input with any hint and level decodes back to itself.
func FuzzRoundTrip(f *testing.F) {
	f.Add(ramp(40), uint8(2), uint8(9))
	f.Add(float64Bytes(0.1, 0.2, 0.30000000000000004), uint8(5), uint8(0))
	f.Add([]byte{1, 2, 3}, uint8(1), uint8(13))
	c, _ := compress.GetCodec(tparquet.Compression)
	f.Fuzz(func(t *testing.T, data []byte, typ, lvl uint8) {
		level := tparquet.Level(parquet.Type(typ%8), int(lvl%14)-1)
		enc := c.EncodeLevel(nil, data, level)
		if int64(len(enc)) > c.CompressBound(int64(len(data))) {
			t.Fatalf("payload %d bytes over the bound", len(enc))
		}
		if dec := c.Decode(make([]byte, len(data)), enc); !bytes.Equal(dec, data) {
			t.Fatal("round trip changed the data")
		}
	})
}

// FuzzEncodeAnyLevel: any input with any arrow-go level, tagged or not, decodes back to
// itself; a level the binding cannot use falls back to a RAW payload.
func FuzzEncodeAnyLevel(f *testing.F) {
	f.Add(ramp(10), int64(tparquet.Level(parquet.Types.Int64, 3)))
	f.Add([]byte{1, 2, 3, 4}, int64(-1))
	f.Add(float64Bytes(1.5, 2.5), int64(0x5043_05ff))
	f.Add([]byte{}, int64(1)<<40)
	c, _ := compress.GetCodec(tparquet.Compression)
	f.Fuzz(func(t *testing.T, data []byte, level int64) {
		enc := c.EncodeLevel(nil, data, int(level))
		if len(enc) < 2 || int64(len(enc)) > c.CompressBound(int64(len(data))) {
			t.Fatalf("payload of %d bytes for %d", len(enc), len(data))
		}
		if dec := c.Decode(make([]byte, len(data)), enc); !bytes.Equal(dec, data) {
			t.Fatal("round trip changed the data")
		}
	})
}

// FuzzPageInFile: any bytes in place of a real page payload (of each mode) of a file: the
// pqarrow and page readers return data or an error, and never panic.
func FuzzPageInFile(f *testing.F) {
	type page struct {
		data       []byte
		start, end int
	}
	var pages []page
	for mode := byte(0); mode < 4; mode++ {
		data, rec, start, end := modeFile(f, mode)
		rec.Release()
		pages = append(pages, page{data, start, end})
		f.Add(bytes.Clone(data[start:min(end, start+64)]), mode)
	}
	f.Add([]byte{1, 1, 2, 0xff, 0xff}, byte(1))
	f.Add([]byte{1, 2, 5, 18, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01}, byte(2))
	f.Fuzz(func(t *testing.T, payload []byte, mode byte) {
		p := pages[mode%4]
		bad := bytes.Clone(p.data)
		copy(bad[p.start:p.end], payload)
		readErr(t, bad)
	})
}

// FuzzWriteRead: numbers from any bytes, with nulls, written with any policy and page
// version, read back as written.
func FuzzWriteRead(f *testing.F) {
	f.Add(ramp(64), uint8(0), false)
	f.Add(float64Bytes(0.1, 0.25, math.Copysign(0, -1), 1e300), uint8(1), true)
	f.Add([]byte{}, uint8(2), true)
	sc := arrow.NewSchema([]arrow.Field{
		{Name: "i", Type: arrow.PrimitiveTypes.Int64, Nullable: true},
		{Name: "f", Type: arrow.PrimitiveTypes.Float64},
		{Name: "g", Type: arrow.PrimitiveTypes.Float32, Nullable: true},
		{Name: "l", Type: arrow.ListOf(arrow.PrimitiveTypes.Int32)},
	}, nil)
	f.Fuzz(func(t *testing.T, data []byte, policy uint8, v2 bool) {
		b := array.NewRecordBuilder(memory.DefaultAllocator, sc)
		defer b.Release()
		for i := 0; i+8 <= len(data); i += 8 {
			u := binary.LittleEndian.Uint64(data[i:])
			if u%7 == 0 {
				b.Field(0).AppendNull()
				b.Field(2).AppendNull()
			} else {
				b.Field(0).(*array.Int64Builder).Append(int64(u))
				b.Field(2).(*array.Float32Builder).Append(math.Float32frombits(uint32(u)))
			}
			b.Field(1).(*array.Float64Builder).Append(math.Float64frombits(u))
			lb := b.Field(3).(*array.ListBuilder)
			lb.Append(true)
			for k := 0; k < int(u%4); k++ {
				lb.ValueBuilder().(*array.Int32Builder).Append(int32(u >> (8 * k)))
			}
		}
		rec := b.NewRecordBatch()
		defer rec.Release()
		version := parquet.DataPageV1
		if v2 {
			version = parquet.DataPageV2
		}
		data2, _ := writeAll(t, sc, []arrow.RecordBatch{rec}, tparquet.WithDefaultPolicy(tparquet.Policy(policy%3)),
			tparquet.WithWriterProperties(parquet.WithDataPageVersion(version), parquet.WithDictionaryDefault(false),
				parquet.WithDataPageSize(512)))
		assertSameData(t, data2, rec)
	})
}
