package tparquet

import (
	"bytes"
	"errors"
	"testing"

	"github.com/apache/arrow-go/v18/arrow"
	"github.com/apache/arrow-go/v18/parquet"
)

func TestCheckABI(t *testing.T) {
	if err := checkABI(1, 1); err != nil {
		t.Fatalf("checkABI(1, 1) = %v", err)
	}
	cases := []struct {
		linked, want uint32
		msg          string
	}{
		{2, 1, "pcodec: the linked libtparquet_capi has ABI version 2, this package needs 1: build the library from the same tparquet release"},
		{0, 1, "pcodec: the linked libtparquet_capi has ABI version 0, this package needs 1: build the library from the same tparquet release"},
		{1, 2, "pcodec: the linked libtparquet_capi has ABI version 1, this package needs 2: build the library from the same tparquet release"},
	}
	for _, tc := range cases {
		if err := checkABI(tc.linked, tc.want); err == nil || err.Error() != tc.msg {
			t.Errorf("checkABI(%d, %d) = %v, want %q", tc.linked, tc.want, err, tc.msg)
		}
	}
}

func TestTheLinkedLibraryHasThisPackagesABI(t *testing.T) {
	if abiErr != nil {
		t.Fatal(abiErr)
	}
	if v := linkedABIVersion(); v != abiVersion || abiVersion != 1 {
		t.Fatalf("linked ABI version %d, header %d", v, abiVersion)
	}
}

// withABIMismatch runs f as if the linked library had another ABI version.
func withABIMismatch(t *testing.T, f func(want error)) {
	orig, origCompress := abiErr, compressFn
	t.Cleanup(func() { abiErr, compressFn = orig, origCompress })
	abiErr = checkABI(abiVersion+1, abiVersion)
	compressFn = func([]byte, []byte, int32, int32) (int, error) {
		t.Fatal("the library was called despite the ABI mismatch")
		return 0, nil
	}
	f(abiErr)
	abiErr, compressFn = orig, origCompress
}

func wantPanic(t *testing.T, want error, f func()) {
	t.Helper()
	defer func() {
		if r := recover(); r != want {
			t.Fatalf("panic %v, want %v", r, want)
		}
	}()
	f()
}

func TestAnABIMismatchFailsEveryCodecCall(t *testing.T) {
	payload := []byte{1, 0, 9, 8, 7}
	withABIMismatch(t, func(want error) {
		if out, err := (codec{}).DecodeWithError(make([]byte, 3), payload); !errors.Is(err, want) || out != nil {
			t.Fatalf("DecodeWithError = %v, %v; want %v", out, err, want)
		}
		wantPanic(t, want, func() { codec{}.Decode(make([]byte, 3), payload) })
		wantPanic(t, want, func() { codec{}.EncodeLevel(nil, []byte{1, 2, 3}, Level(parquet.Types.Int64, 8)) })
		wantPanic(t, want, func() { codec{}.Encode(nil, []byte{1, 2, 3}) })
	})
	// and the same calls work with the linked library
	if out, err := (codec{}).DecodeWithError(make([]byte, 3), payload); err != nil || !bytes.Equal(out, []byte{9, 8, 7}) {
		t.Fatalf("DecodeWithError = %v, %v", out, err)
	}
}

func TestAnABIMismatchFailsTheWriterUpFront(t *testing.T) {
	sc := arrow.NewSchema([]arrow.Field{{Name: "v", Type: arrow.PrimitiveTypes.Int64}}, nil)
	withABIMismatch(t, func(want error) {
		if p, err := NewPlan(sc, nil); !errors.Is(err, want) || p != nil {
			t.Fatalf("NewPlan = %v, %v; want %v", p, err, want)
		}
		if w, err := NewFileWriter(sc, &bytes.Buffer{}); !errors.Is(err, want) || w != nil {
			t.Fatalf("NewFileWriter = %v, %v; want %v", w, err, want)
		}
	})
}
