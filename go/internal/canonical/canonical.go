// Package canonical writes Arrow data as the canonical JSON lines of the conformance vectors
// (testdata/expected/*.jsonl) and compares two such documents.
//
// One JSON object per row, with the top-level fields as keys in schema order. Integers of
// every width, timestamps, dates, times and the unscaled value of decimals are JSON numbers;
// half float, FLOAT and DOUBLE values are strings of their IEEE bits in lower-case hex
// ("0x3c00", "0x3f800000", "0x3ff0000000000000"); strings are JSON strings, binary values
// base64 strings, booleans JSON booleans and nulls null; lists are arrays, structs objects in
// field order, and maps arrays of {"key":...,"value":...} entries in stored order.
package canonical

import (
	"bufio"
	"bytes"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"math"
	"math/big"
	"strconv"

	"github.com/apache/arrow-go/v18/arrow"
	"github.com/apache/arrow-go/v18/arrow/array"
)

// WriteTable writes every row of tbl.
func WriteTable(w io.Writer, tbl arrow.Table) error {
	tr := array.NewTableReader(tbl, 0)
	defer tr.Release()
	for tr.Next() {
		if err := WriteRecord(w, tr.RecordBatch()); err != nil {
			return err
		}
	}
	return tr.Err()
}

// WriteRecord writes every row of rec, one line each.
func WriteRecord(w io.Writer, rec arrow.RecordBatch) error {
	var buf bytes.Buffer
	fields := rec.Schema().Fields()
	for i := 0; i < int(rec.NumRows()); i++ {
		buf.Reset()
		buf.WriteByte('{')
		for j, f := range fields {
			if j > 0 {
				buf.WriteByte(',')
			}
			writeString(&buf, f.Name)
			buf.WriteByte(':')
			if err := value(&buf, rec.Column(j), i); err != nil {
				return err
			}
		}
		buf.WriteString("}\n")
		if _, err := w.Write(buf.Bytes()); err != nil {
			return err
		}
	}
	return nil
}

func writeString(buf *bytes.Buffer, s string) {
	enc := json.NewEncoder(buf)
	enc.SetEscapeHTML(false)
	_ = enc.Encode(s)           // a string always encodes
	buf.Truncate(buf.Len() - 1) // the encoder's newline
}

func value(buf *bytes.Buffer, arr arrow.Array, i int) error {
	if arr.IsNull(i) || arr.DataType().ID() == arrow.NULL { // a null array has no bitmap
		buf.WriteString("null")
		return nil
	}
	b := buf.AvailableBuffer()
	switch a := arr.(type) {
	case *array.Int8:
		b = strconv.AppendInt(b, int64(a.Value(i)), 10)
	case *array.Int16:
		b = strconv.AppendInt(b, int64(a.Value(i)), 10)
	case *array.Int32:
		b = strconv.AppendInt(b, int64(a.Value(i)), 10)
	case *array.Int64:
		b = strconv.AppendInt(b, a.Value(i), 10)
	case *array.Uint8:
		b = strconv.AppendUint(b, uint64(a.Value(i)), 10)
	case *array.Uint16:
		b = strconv.AppendUint(b, uint64(a.Value(i)), 10)
	case *array.Uint32:
		b = strconv.AppendUint(b, uint64(a.Value(i)), 10)
	case *array.Uint64:
		b = strconv.AppendUint(b, a.Value(i), 10)
	case *array.Timestamp:
		b = strconv.AppendInt(b, int64(a.Value(i)), 10)
	case *array.Date32:
		b = strconv.AppendInt(b, int64(a.Value(i)), 10)
	case *array.Date64:
		b = strconv.AppendInt(b, int64(a.Value(i)), 10)
	case *array.Time32:
		b = strconv.AppendInt(b, int64(a.Value(i)), 10)
	case *array.Time64:
		b = strconv.AppendInt(b, int64(a.Value(i)), 10)
	case *array.Duration:
		b = strconv.AppendInt(b, int64(a.Value(i)), 10)
	case *array.MonthInterval:
		b = interval(b, int64(a.Value(i)), 0, 0)
	case *array.DayTimeInterval:
		v := a.Value(i)
		b = interval(b, 0, int64(v.Days), int64(v.Milliseconds)*1_000_000)
	case *array.MonthDayNanoInterval:
		v := a.Value(i)
		b = interval(b, int64(v.Months), int64(v.Days), v.Nanoseconds)
	case *array.Decimal128:
		b = a.Value(i).BigInt().Append(b, 10)
	case *array.Decimal256:
		b = a.Value(i).BigInt().Append(b, 10)
	case *array.Float16:
		b = fmt.Appendf(b, `"0x%04x"`, a.Value(i).Uint16())
	case *array.Float32:
		b = fmt.Appendf(b, `"0x%08x"`, math.Float32bits(a.Value(i)))
	case *array.Float64:
		b = fmt.Appendf(b, `"0x%016x"`, math.Float64bits(a.Value(i)))
	case *array.Boolean:
		b = strconv.AppendBool(b, a.Value(i))
	case *array.String:
		writeString(buf, a.Value(i))
		return nil
	case *array.LargeString:
		writeString(buf, a.Value(i))
		return nil
	case *array.Binary:
		return writeBase64(buf, a.Value(i))
	case *array.LargeBinary:
		return writeBase64(buf, a.Value(i))
	case *array.FixedSizeBinary:
		return writeBase64(buf, a.Value(i))
	case *array.Map: // entries are structs: {"key":...,"value":...}
		start, end := a.ValueOffsets(i)
		return list(buf, a.ListValues(), int(start), int(end))
	case *array.List:
		start, end := a.ValueOffsets(i)
		return list(buf, a.ListValues(), int(start), int(end))
	case *array.LargeList:
		start, end := a.ValueOffsets(i)
		return list(buf, a.ListValues(), int(start), int(end))
	case *array.FixedSizeList:
		start, end := a.ValueOffsets(i)
		return list(buf, a.ListValues(), int(start), int(end))
	case *array.Struct:
		st := a.DataType().(*arrow.StructType)
		buf.WriteByte('{')
		for j, f := range st.Fields() {
			if j > 0 {
				buf.WriteByte(',')
			}
			writeString(buf, f.Name)
			buf.WriteByte(':')
			if err := value(buf, a.Field(j), i); err != nil {
				return err
			}
		}
		buf.WriteByte('}')
		return nil
	case *array.Dictionary:
		return value(buf, a.Dictionary(), a.GetValueIndex(i))
	default:
		return fmt.Errorf("canonical: no JSON form for %s", arr.DataType().Name())
	}
	buf.Write(b)
	return nil
}

// interval appends the canonical form of every interval type: months, days and nanoseconds.
func interval(b []byte, months, days, nanoseconds int64) []byte {
	b = strconv.AppendInt(append(b, `{"months":`...), months, 10)
	b = strconv.AppendInt(append(b, `,"days":`...), days, 10)
	b = strconv.AppendInt(append(b, `,"nanoseconds":`...), nanoseconds, 10)
	return append(b, '}')
}

func writeBase64(buf *bytes.Buffer, v []byte) error {
	buf.WriteByte('"')
	buf.WriteString(base64.StdEncoding.EncodeToString(v))
	buf.WriteByte('"')
	return nil
}

func list(buf *bytes.Buffer, values arrow.Array, start, end int) error {
	buf.WriteByte('[')
	for k := start; k < end; k++ {
		if k > start {
			buf.WriteByte(',')
		}
		if err := value(buf, values, k); err != nil {
			return err
		}
	}
	buf.WriteByte(']')
	return nil
}

// Equal compares two JSON-lines documents value by value: key order, integer values (of any
// size), strings after unescaping, booleans and nulls must match; whitespace and blank lines
// do not matter. It returns a description of the first difference.
func Equal(got, want []byte) (bool, string) {
	g, err := normLines(got)
	if err != nil {
		return false, "got: " + err.Error()
	}
	w, err := normLines(want)
	if err != nil {
		return false, "want: " + err.Error()
	}
	for i := range min(len(g), len(w)) {
		if g[i] != w[i] {
			return false, fmt.Sprintf("line %d: got %s, want %s", i+1, g[i], w[i])
		}
	}
	if len(g) != len(w) {
		return false, fmt.Sprintf("%d rows, want %d", len(g), len(w))
	}
	return true, ""
}

func normLines(doc []byte) ([]string, error) {
	var out []string
	sc := bufio.NewScanner(bytes.NewReader(doc))
	sc.Buffer(nil, 1<<30)
	n := 0
	for sc.Scan() {
		n++
		line := bytes.TrimSpace(sc.Bytes())
		if len(line) == 0 {
			continue
		}
		dec := json.NewDecoder(bytes.NewReader(line))
		dec.UseNumber()
		var b bytes.Buffer
		if err := norm(dec, &b); err != nil {
			return nil, fmt.Errorf("line %d: %w", n, err)
		}
		if _, err := dec.Token(); err != io.EOF {
			return nil, fmt.Errorf("line %d: trailing data", n)
		}
		out = append(out, b.String())
	}
	return out, sc.Err()
}

// norm writes one JSON value in a normal form: no whitespace, strings re-escaped, integers
// in decimal and other numbers tagged with their literal.
func norm(dec *json.Decoder, b *bytes.Buffer) error {
	tok, err := dec.Token()
	if err != nil {
		return err
	}
	switch v := tok.(type) {
	case json.Delim:
		b.WriteRune(rune(v))
		for i := 0; dec.More(); i++ {
			if i > 0 {
				b.WriteByte(',')
			}
			if v == '{' {
				key, err := dec.Token()
				if err != nil {
					return err
				}
				writeString(b, key.(string))
				b.WriteByte(':')
			}
			if err := norm(dec, b); err != nil {
				return err
			}
		}
		end, err := dec.Token()
		if err != nil {
			return err
		}
		b.WriteRune(rune(end.(json.Delim)))
	case string:
		writeString(b, v)
	case json.Number:
		if n, ok := new(big.Int).SetString(string(v), 10); ok {
			b.WriteString(n.String())
		} else {
			b.WriteString("number:" + string(v))
		}
	case bool:
		b.WriteString(strconv.FormatBool(v))
	case nil:
		b.WriteString("null")
	default:
		return errors.New("unexpected JSON token")
	}
	return nil
}
