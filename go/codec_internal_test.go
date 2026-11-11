package tparquet

import (
	"bytes"
	"errors"
	mrand "math/rand"
	"os"
	"testing"
	"testing/quick"

	"github.com/apache/arrow-go/v18/parquet"
	"github.com/apache/arrow-go/v18/parquet/compress"
)

func TestUnpackLevel(t *testing.T) {
	cases := []struct {
		level   int
		pt, lvl int32
	}{
		{Level(parquet.Types.Int64, 3), 2, 3},
		{Level(parquet.Types.Float, compress.DefaultCompressionLevel), 4, -1},
		{Level(parquet.Types.FixedLenByteArray, 12), 7, 12},
		{compress.DefaultCompressionLevel, -1, -1},
		{0, -1, -1},
		{9, -1, -1},
		{0x5042_0201, -1, -1},
		{0x5044_0201, -1, -1},
		{0x5043_0201 + 1<<31, -1, -1},
		{-0x5043_0201, -1, -1},
		{0x5043_ff0e, 255, 13}, // not made by Level: passed on, and the library refuses level 13
	}
	for _, tc := range cases {
		pt, lvl := unpackLevel(tc.level)
		if pt != tc.pt || lvl != tc.lvl {
			t.Errorf("unpackLevel(%#x) = %d, %d, want %d, %d", tc.level, pt, lvl, tc.pt, tc.lvl)
		}
	}
}

func TestEncodeFallsBackToRawWhenTheLibraryFails(t *testing.T) {
	orig := compressFn
	t.Cleanup(func() { compressFn = orig })
	compressFn = func(dst, src []byte, _, _ int32) (int, error) {
		for i := range dst {
			dst[i] = 0xee
		}
		return 0, errors.New("boom")
	}
	src := []byte{1, 2, 3, 4, 5, 6, 7, 8, 9}
	enc := codec{}.EncodeLevel(nil, src, Level(parquet.Types.Int64, 8))
	if !bytes.Equal(enc, append([]byte{1, 0}, src...)) {
		t.Fatalf("payload %v, want a RAW payload", enc)
	}
	compressFn = orig
	if out := (codec{}).Decode(make([]byte, len(src)), enc); !bytes.Equal(out, src) {
		t.Fatal("the RAW payload does not decode")
	}
}

func TestLibraryRefusesAnOutOfRangeLevel(t *testing.T) {
	// A tagged level that Level cannot make reaches the library, which refuses it; the
	// payload is then RAW.
	src := bytes.Repeat([]byte{1, 0, 0, 0, 0, 0, 0, 0}, 64)
	enc := codec{}.EncodeLevel(nil, src, 0x5043_020e)
	if enc[0] != 1 || enc[1] != 0 || !bytes.Equal(enc[2:], src) {
		t.Fatalf("payload header %v, want RAW", enc[:2])
	}
	if _, err := cCompress(make([]byte, len(src)+2), src, 2, 13); err == nil || err.Error() != "pcodec payload: compression level 13 above 12" {
		t.Fatalf("err = %v", err)
	}
}

func TestVendoredHeaderMatchesTheCAPI(t *testing.T) {
	want, err := os.ReadFile("../rust/capi/include/tparquet.h")
	if errors.Is(err, os.ErrNotExist) {
		t.Skip("not in the repository checkout: ../rust/capi is absent")
	}
	if err != nil {
		t.Fatal(err)
	}
	got, err := os.ReadFile("include/tparquet.h")
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(got, want) {
		t.Fatal("include/tparquet.h differs from rust/capi/include/tparquet.h; copy it again")
	}
}

func TestSmallerKeepsBaseOnATie(t *testing.T) {
	if !smaller(9, 10) || smaller(10, 10) || smaller(11, 10) {
		t.Fatal("smaller")
	}
}

// TestQuickLevelPacking: unpackLevel inverts Level for every type and pcodec level, and any
// level without the tag carries no hint.
func TestQuickLevelPacking(t *testing.T) {
	inverse := func(typ uint8, lvl uint8) bool {
		pt, l := parquet.Type(typ%8), int(lvl%14)-1
		gotT, gotL := unpackLevel(Level(pt, l))
		return gotT == int32(pt) && gotL == int32(l)
	}
	untagged := func(level int) bool {
		if level>>16 == levelTag {
			return true
		}
		pt, l := unpackLevel(level)
		return pt == -1 && l == -1
	}
	cfg := &quick.Config{MaxCount: 5000, Rand: mrand.New(mrand.NewSource(1))}
	if err := quick.Check(inverse, cfg); err != nil {
		t.Fatal(err)
	}
	if err := quick.Check(untagged, cfg); err != nil {
		t.Fatal(err)
	}
}
