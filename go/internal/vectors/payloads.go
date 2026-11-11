package vectors

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"

	"github.com/apache/arrow-go/v18/parquet"
	"github.com/apache/arrow-go/v18/parquet/compress"

	tparquet "github.com/aperiodic-io/tparquet/go"
)

// ErrNoPayloads means the directory has no payloads/manifest.json.
var ErrNoPayloads = errors.New("vectors: no payloads/manifest.json")

// payload is one entry of payloads/manifest.json (testdata/README.md).
type payload struct {
	Name        string  `json:"name"`
	Payload     string  `json:"payload"`
	ExpectedLen int     `json:"expected_len"`
	Valid       bool    `json:"valid"`
	Decoded     string  `json:"decoded"`
	Mode        string  `json:"mode"`
	Hint        *string `json:"hint"`
	Level       int     `json:"level"`
	// ReferenceEncoding is false for a hand-made payload, which the reference encoder does
	// not write for its output (a non-minimal varint, a PCO payload without numbers).
	ReferenceEncoding *bool `json:"reference_encoding"`
}

// modes are the payload mode names of the manifest, by mode byte (spec section 4).
var modes = []string{"RAW", "PCO", "DECIMAL", "ZSTD"}

var hints = map[string]parquet.Type{"INT32": parquet.Types.Int32, "INT64": parquet.Types.Int64,
	"FLOAT": parquet.Types.Float, "DOUBLE": parquet.Types.Double}

// CheckPayloads decodes every payload of dir/payloads/manifest.json with this binding: a
// valid one must decode to its .out file, and encoding that output again with its hint and
// level must give the same mode (unless the manifest says the payload was made by hand, with
// reference_encoding false); an invalid one must fail. It returns the number of
// payloads and every difference.
func CheckPayloads(dir string) (int, error) {
	raw, err := os.ReadFile(filepath.Join(dir, "payloads", "manifest.json"))
	if errors.Is(err, os.ErrNotExist) {
		return 0, fmt.Errorf("%w in %s", ErrNoPayloads, dir)
	}
	if err != nil {
		return 0, err
	}
	var m struct {
		FormatVersion int       `json:"format_version"`
		Vectors       []payload `json:"vectors"`
	}
	if err := json.Unmarshal(raw, &m); err != nil {
		return 0, fmt.Errorf("payloads/manifest.json: %w", err)
	}
	if m.FormatVersion != 1 {
		return 0, fmt.Errorf("payloads/manifest.json: format_version %d, this checker reads 1", m.FormatVersion)
	}
	var errs []error
	for _, p := range m.Vectors {
		if err := p.check(filepath.Join(dir, "payloads")); err != nil {
			errs = append(errs, fmt.Errorf("%s: %w", p.Name, err))
		}
	}
	return len(m.Vectors), errors.Join(errs...)
}

func (p payload) check(dir string) error {
	src, err := os.ReadFile(filepath.Join(dir, p.Payload))
	if err != nil {
		return err
	}
	c, _ := compress.GetCodec(tparquet.Compression) // registered by the import
	got, err := c.(compress.ErrorCodec).DecodeWithError(make([]byte, p.ExpectedLen), src)
	if !p.Valid {
		if err == nil {
			return errors.New("an invalid payload decoded")
		}
		return nil
	}
	if err != nil {
		return err
	}
	want, err := os.ReadFile(filepath.Join(dir, p.Decoded))
	if err != nil {
		return err
	}
	if !bytes.Equal(got, want) {
		return fmt.Errorf("decodes to other bytes than %s", p.Decoded)
	}
	if p.ReferenceEncoding != nil && !*p.ReferenceEncoding {
		return nil
	}
	level := p.Level // no hint: a plain level, which carries none
	if p.Hint != nil {
		t, ok := hints[*p.Hint]
		if !ok {
			return fmt.Errorf("unknown hint %q", *p.Hint)
		}
		level = tparquet.Level(t, p.Level)
	}
	enc := c.EncodeLevel(nil, want, level)
	if mode := modes[enc[1]]; mode != p.Mode {
		return fmt.Errorf("encodes in mode %s, the manifest says %s", mode, p.Mode)
	}
	return nil
}
