package vectors

import (
	"encoding/json"
	"errors"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/apache/arrow-go/v18/parquet"
	"github.com/apache/arrow-go/v18/parquet/compress"

	tparquet "github.com/aperiodic-io/tparquet/go"
)

func testdataDir() string {
	if dir := os.Getenv("TPARQUET_TESTDATA"); dir != "" {
		return dir
	}
	return "../../../testdata"
}

// TestRepositoryPayloadVectors checks testdata/payloads, the payload vectors every
// implementation decodes.
func TestRepositoryPayloadVectors(t *testing.T) {
	n, err := CheckPayloads(testdataDir())
	if errors.Is(err, ErrNoPayloads) {
		t.Skip(err)
	}
	if err != nil {
		t.Fatal(err)
	}
	if n < 30 {
		t.Fatalf("only %d payload vectors", n)
	}
}

// writePayloads writes a payloads directory with the given manifest entries and files.
func writePayloads(t *testing.T, version int, entries []map[string]any, files map[string][]byte) string {
	t.Helper()
	dir := t.TempDir()
	pd := filepath.Join(dir, "payloads")
	if err := os.MkdirAll(pd, 0o755); err != nil {
		t.Fatal(err)
	}
	m, err := json.Marshal(map[string]any{"format_version": version, "vectors": entries})
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(pd, "manifest.json"), m, 0o644); err != nil {
		t.Fatal(err)
	}
	for name, b := range files {
		if err := os.WriteFile(filepath.Join(pd, name), b, 0o644); err != nil {
			t.Fatal(err)
		}
	}
	return dir
}

func le64s(vs ...int64) []byte {
	b := make([]byte, 0, 8*len(vs))
	for _, v := range vs {
		for i := 0; i < 8; i++ {
			b = append(b, byte(uint64(v)>>(8*i)))
		}
	}
	return b
}

func TestCheckPayloads(t *testing.T) {
	c, _ := compress.GetCodec(tparquet.Compression)
	ints := make([]int64, 500)
	for i := range ints {
		ints[i] = 1_700_000_000 + int64(i)*7
	}
	data := le64s(ints...)
	pco := c.EncodeLevel(nil, data, tparquet.Level(parquet.Types.Int64, 8))
	zstd := c.EncodeLevel(nil, data, compress.DefaultCompressionLevel)
	// a hand-made RAW payload of data, for which the encoder writes PCO
	raw := append([]byte{1, 0}, data...)
	files := map[string][]byte{"pco.bin": pco, "zstd.bin": zstd, "raw.bin": raw, "data.out": data, "other.out": data[8:],
		"bad.bin": {9, 0}}
	entry := func(name, payload string, valid bool, extra map[string]any) map[string]any {
		e := map[string]any{"name": name, "payload": payload, "expected_len": len(data), "valid": valid}
		for k, v := range extra {
			e[k] = v
		}
		return e
	}
	good := []map[string]any{
		entry("pco", "pco.bin", true, map[string]any{"decoded": "data.out", "mode": "PCO", "hint": "INT64", "level": 8}),
		entry("zstd", "zstd.bin", true, map[string]any{"decoded": "data.out", "mode": "ZSTD", "hint": nil, "level": 8}),
		entry("bad", "bad.bin", false, nil),
		entry("made", "raw.bin", true, map[string]any{"decoded": "data.out", "mode": "RAW", "hint": "INT64", "level": 8,
			"reference_encoding": false}),
		entry("pco_marked", "pco.bin", true, map[string]any{"decoded": "data.out", "mode": "PCO", "hint": "INT64", "level": 8,
			"reference_encoding": true}),
	}
	n, err := CheckPayloads(writePayloads(t, 1, good, files))
	if err != nil || n != 5 {
		t.Fatalf("CheckPayloads: %d, %v", n, err)
	}

	cases := []struct {
		entry map[string]any
		msg   string
	}{
		{entry("e1", "nope.bin", false, nil), "e1: open "},
		{entry("e2", "pco.bin", false, nil), "e2: an invalid payload decoded"},
		{entry("e3", "bad.bin", true, nil), "e3: pcodec payload: unsupported version 9"},
		{entry("e4", "pco.bin", true, map[string]any{"decoded": "nope.out"}), "e4: open "},
		{entry("e5", "pco.bin", true, map[string]any{"decoded": "other.out"}), "e5: decodes to other bytes than other.out"},
		{entry("e6", "pco.bin", true, map[string]any{"decoded": "data.out", "hint": "INT96"}), `e6: unknown hint "INT96"`},
		{entry("e7", "pco.bin", true, map[string]any{"decoded": "data.out", "hint": "INT64", "mode": "DECIMAL"}),
			"e7: encodes in mode PCO, the manifest says DECIMAL"},
		{entry("e8", "zstd.bin", true, map[string]any{"decoded": "data.out", "hint": nil, "mode": "PCO", "level": 8}),
			"e8: encodes in mode ZSTD, the manifest says PCO"},
		// a hand-made payload still has to decode to its bytes
		{entry("e9", "raw.bin", true, map[string]any{"decoded": "other.out", "mode": "RAW", "reference_encoding": false}),
			"e9: decodes to other bytes than other.out"},
		// and one the encoder writes is checked
		{entry("e10", "raw.bin", true, map[string]any{"decoded": "data.out", "hint": "INT64", "mode": "RAW", "level": 8,
			"reference_encoding": true}), "e10: encodes in mode PCO, the manifest says RAW"},
	}
	var entries []map[string]any
	for _, tc := range cases {
		entries = append(entries, tc.entry)
	}
	n, err = CheckPayloads(writePayloads(t, 1, entries, files))
	if n != len(cases) || err == nil {
		t.Fatalf("CheckPayloads: %d, %v", n, err)
	}
	lines := strings.Split(err.Error(), "\n")
	if len(lines) != len(cases) {
		t.Fatalf("%d errors, want %d: %v", len(lines), len(cases), err)
	}
	for i, tc := range cases {
		if !strings.HasPrefix(lines[i], tc.msg) {
			t.Errorf("error %d: %q, want it to start with %q", i, lines[i], tc.msg)
		}
	}
}

func TestCheckPayloadsManifestErrors(t *testing.T) {
	if _, err := CheckPayloads(t.TempDir()); !errors.Is(err, ErrNoPayloads) {
		t.Fatalf("no manifest: %v", err)
	}
	if _, err := CheckPayloads(writePayloads(t, 2, nil, nil)); err == nil || err.Error() != "payloads/manifest.json: format_version 2, this checker reads 1" {
		t.Fatalf("version: %v", err)
	}
	dir := writePayloads(t, 1, nil, nil)
	if err := os.WriteFile(filepath.Join(dir, "payloads", "manifest.json"), []byte("{"), 0o644); err != nil {
		t.Fatal(err)
	}
	if _, err := CheckPayloads(dir); err == nil || !strings.HasPrefix(err.Error(), "payloads/manifest.json: ") {
		t.Fatalf("bad JSON: %v", err)
	}
	// A directory where the manifest should be: not "no payloads", a read error.
	dir = t.TempDir()
	if err := os.MkdirAll(filepath.Join(dir, "payloads", "manifest.json"), 0o755); err != nil {
		t.Fatal(err)
	}
	if _, err := CheckPayloads(dir); err == nil || errors.Is(err, ErrNoPayloads) {
		t.Fatalf("unreadable manifest: %v", err)
	}
	if n, err := CheckPayloads(writePayloads(t, 1, nil, nil)); n != 0 || err != nil {
		t.Fatalf("an empty manifest: %d, %v", n, err)
	}
}
