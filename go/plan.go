package tparquet

import (
	"bytes"
	"errors"
	"fmt"
	"io"
	"slices"

	"github.com/apache/arrow-go/v18/arrow"
	"github.com/apache/arrow-go/v18/parquet"
	"github.com/apache/arrow-go/v18/parquet/compress"
	"github.com/apache/arrow-go/v18/parquet/pqarrow"
)

// Policy says how a column's codec is chosen.
type Policy int

const (
	// Auto writes the column with PCODEC if that is smaller than the base codec on the first
	// record batch with rows, and with the base codec otherwise (spec section 6). It applies only to
	// INT32, INT64, FLOAT and DOUBLE leaves; other leaves use the base codec.
	Auto Policy = iota
	// Pcodec writes the column with PCODEC. As the default policy it applies to INT32,
	// INT64, FLOAT and DOUBLE leaves; set per column, it applies to any leaf (other types
	// get the lossless ZSTD fallback payload).
	Pcodec
	// Base writes the column with the base codec (ZSTD level 3 unless WithBaseCodec says
	// otherwise) and the writer's normal encodings.
	Base
)

// Option configures a Plan or a FileWriter.
type Option func(*config)

type config struct {
	defPolicy  Policy
	policies   map[string]Policy
	baseCodec  compress.Compression
	baseLevel  int
	pcoLevel   int
	props      []parquet.WriterProperty
	arrowProps pqarrow.ArrowWriterProperties
}

func newConfig(opts []Option) config {
	c := config{
		policies:   map[string]Policy{},
		baseCodec:  compress.Codecs.Zstd,
		baseLevel:  3,
		pcoLevel:   compress.DefaultCompressionLevel,
		arrowProps: pqarrow.DefaultWriterProps(),
	}
	for _, o := range opts {
		o(&c)
	}
	return c
}

// WithDefaultPolicy sets the policy of the columns WithPolicy does not name; Auto if unset.
func WithDefaultPolicy(p Policy) Option { return func(c *config) { c.defPolicy = p } }

// WithPolicy sets the policy of one leaf column, named by its Parquet path as
// parquet.ColumnPath.String() spells it: "price", "quote.bid", "fills.list.element".
func WithPolicy(path string, p Policy) Option { return func(c *config) { c.policies[path] = p } }

// WithBaseCodec sets the codec and level of the columns that do not use PCODEC; ZSTD level
// 3 if unset. Set it here rather than with parquet.WithCompression. It must be a codec
// arrow-go implements, at a level it accepts, and not PCODEC (use WithDefaultPolicy(Pcodec)
// for that).
func WithBaseCodec(codec compress.Compression, level int) Option {
	return func(c *config) { c.baseCodec, c.baseLevel = codec, level }
}

// WithPcodecLevel sets the pcodec compression level, 0-12, or
// compress.DefaultCompressionLevel for pcodec's default, 8.
func WithPcodecLevel(level int) Option { return func(c *config) { c.pcoLevel = level } }

// WithWriterProperties adds Parquet writer properties (row group length, page size,
// statistics, ...). The plan's per-column settings for PCODEC columns come after them.
func WithWriterProperties(props ...parquet.WriterProperty) Option {
	return func(c *config) { c.props = append(c.props, props...) }
}

// WithArrowWriterProperties sets the pqarrow writer properties.
func WithArrowWriterProperties(p pqarrow.ArrowWriterProperties) Option {
	return func(c *config) { c.arrowProps = p }
}

// ColumnProperties returns the writer properties that write one leaf column with PCODEC, as
// the spec recommends: PCODEC with the column's physical type as the hint (see Level),
// PLAIN encoding and no dictionary. Use data page V2 (parquet.WithDataPageVersion) for
// files whose PCODEC columns are nullable or repeated.
func ColumnProperties(path string, t parquet.Type, pcoLevel int) []parquet.WriterProperty {
	return []parquet.WriterProperty{
		parquet.WithCompressionFor(path, Compression),
		parquet.WithCompressionLevelFor(path, Level(t, pcoLevel)),
		parquet.WithEncodingFor(path, parquet.Encodings.Plain),
		parquet.WithDictionaryFor(path, false),
	}
}

// Eligible reports whether PCODEC models a physical type: INT32, INT64, FLOAT and DOUBLE.
func Eligible(t parquet.Type) bool {
	switch t {
	case parquet.Types.Int32, parquet.Types.Int64, parquet.Types.Float, parquet.Types.Double:
		return true
	}
	return false
}

type leaf struct {
	path string
	typ  parquet.Type
}

// Plan is the codec of every leaf column of a file.
type Plan struct {
	cfg    config
	schema *arrow.Schema
	leaves []leaf
	pcodec map[string]bool
}

// NewPlan chooses the codec of every leaf column of an Arrow schema. sample is the first
// record batch of the file, used to resolve Auto columns; with a nil or empty sample Auto
// columns use the base codec.
func NewPlan(sc *arrow.Schema, sample arrow.RecordBatch, opts ...Option) (*Plan, error) {
	if abiErr != nil {
		return nil, abiErr
	}
	cfg := newConfig(opts)
	if err := checkPolicy(cfg.defPolicy); err != nil {
		return nil, err
	}
	if cfg.pcoLevel < compress.DefaultCompressionLevel || cfg.pcoLevel > MaxPcodecLevel {
		return nil, fmt.Errorf("pcodec: pcodec level %d is not in 0..%d", cfg.pcoLevel, MaxPcodecLevel)
	}
	if cfg.baseCodec == Compression {
		return nil, errors.New("pcodec: the base codec cannot be PCODEC; use WithDefaultPolicy(Pcodec)")
	}
	base, err := compress.GetCodec(cfg.baseCodec)
	if err != nil {
		return nil, fmt.Errorf("pcodec: base codec: %w", err)
	}
	if err := tryLevel(base, cfg.baseLevel); err != nil {
		return nil, fmt.Errorf("pcodec: base codec %v at level %d: %w", cfg.baseCodec, cfg.baseLevel, err)
	}
	if sample != nil && !sample.Schema().Equal(sc) {
		return nil, errSampleSchema
	}
	pq, err := pqarrow.ToParquet(sc, parquet.NewWriterProperties(cfg.props...), cfg.arrowProps)
	if err != nil {
		return nil, err
	}
	p := &Plan{cfg: cfg, schema: sc, pcodec: map[string]bool{}}
	for i := 0; i < pq.NumColumns(); i++ {
		col := pq.Column(i)
		p.leaves = append(p.leaves, leaf{col.Path(), col.PhysicalType()})
	}
	for path, pol := range cfg.policies {
		if !slices.ContainsFunc(p.leaves, func(l leaf) bool { return l.path == path }) {
			return nil, fmt.Errorf("pcodec: no leaf column %q", path)
		}
		if err := checkPolicy(pol); err != nil {
			return nil, err
		}
	}
	var auto []string
	for _, l := range p.leaves {
		pol, set := cfg.policies[l.path]
		if !set {
			pol = cfg.defPolicy
		}
		if pol == Pcodec && (set || Eligible(l.typ)) {
			p.pcodec[l.path] = true
		} else if pol == Auto && Eligible(l.typ) && sample != nil && sample.NumRows() > 0 {
			auto = append(auto, l.path)
		}
	}
	if len(auto) > 0 {
		if err := p.trial(sample, auto); err != nil {
			return nil, err
		}
	}
	return p, nil
}

// tryLevel compresses one byte with codec at level: arrow-go's codecs panic on a level they
// refuse (gzip does), at the first page; this finds it before any file is written.
func tryLevel(codec compress.Codec, level int) (err error) {
	defer func() {
		if r := recover(); r != nil {
			err = fmt.Errorf("%v", r)
		}
	}()
	codec.EncodeLevel(nil, []byte{0}, level)
	return nil
}

var errSampleSchema = errors.New("pcodec: the sample's schema differs from the file's")

func checkPolicy(p Policy) error {
	if p < Auto || p > Base {
		return fmt.Errorf("pcodec: invalid policy %d", p)
	}
	return nil
}

// trial writes sample twice, with the auto columns in the base codec and in PCODEC, and
// switches each auto column whose PCODEC chunks are smaller.
//
// A file with PCODEC is written with V2 pages, which store levels uncompressed and can make
// the other columns larger (spec section 6). So when only auto columns would use PCODEC,
// the sample is also written with the chosen plan, and every column keeps the base codec if
// the file with no PCODEC (V1 pages) is smaller in total.
func (p *Plan) trial(sample arrow.RecordBatch, auto []string) error {
	forced := p.UsesPcodec()
	base, err := p.scratch(sample)
	if err != nil {
		return err
	}
	for _, path := range auto {
		p.pcodec[path] = true
	}
	pco, err := p.scratch(sample)
	if err != nil {
		return err
	}
	for _, path := range auto {
		p.pcodec[path] = smaller(pco[path], base[path])
	}
	if forced || !p.UsesPcodec() {
		return nil
	}
	chosen, err := p.scratch(sample)
	if err != nil {
		return err
	}
	if !smaller(total(chosen), total(base)) {
		for _, path := range auto {
			p.pcodec[path] = false
		}
	}
	return nil
}

func total(sizes map[string]int64) int64 {
	var n int64
	for _, s := range sizes {
		n += s
	}
	return n
}

// smaller reports whether the PCODEC size wins; a tie keeps the base codec.
func smaller(pco, base int64) bool { return pco < base }

// scratch writes sample with the current plan to memory and returns the compressed size of
// each leaf column, summed over row groups.
//
// The scratch files are not encrypted: arrow-go lets FileEncryptionProperties make one
// file only, and wipes their keys when that file closes. Encryption adds about the same
// bytes to both candidates.
func (p *Plan) scratch(sample arrow.RecordBatch) (map[string]int64, error) {
	var buf bytes.Buffer
	props := append(p.Properties(), parquet.WithEncryptionProperties(nil))
	w, err := p.newFileWriter(&buf, props)
	if err != nil {
		return nil, err
	}
	if err := w.Write(sample); err != nil {
		return nil, err
	}
	if err := w.Close(); err != nil {
		return nil, err
	}
	md, err := w.FileMetadata()
	if err != nil {
		return nil, err
	}
	sizes := map[string]int64{}
	for i := 0; i < md.NumRowGroups(); i++ {
		rg := md.RowGroup(i)
		for j := 0; j < rg.NumColumns(); j++ {
			cc, err := rg.ColumnChunk(j)
			if err != nil {
				return nil, err
			}
			sizes[cc.PathInSchema().String()] += cc.TotalCompressedSize()
		}
	}
	return sizes, nil
}

// Codec returns the codec of a leaf column, and false if the schema has no such leaf.
func (p *Plan) Codec(path string) (compress.Compression, bool) {
	if !slices.ContainsFunc(p.leaves, func(l leaf) bool { return l.path == path }) {
		return 0, false
	}
	if p.pcodec[path] {
		return Compression, true
	}
	return p.cfg.baseCodec, true
}

// PcodecColumns returns the paths of the PCODEC leaf columns in schema order.
func (p *Plan) PcodecColumns() []string {
	var out []string
	for _, l := range p.leaves {
		if p.pcodec[l.path] {
			out = append(out, l.path)
		}
	}
	return out
}

// UsesPcodec reports whether any leaf column uses PCODEC.
func (p *Plan) UsesPcodec() bool { return len(p.PcodecColumns()) > 0 }

// Properties returns the Parquet writer properties of the plan: the base codec as the
// default, the WithWriterProperties options, then ColumnProperties for each PCODEC column
// and data page V2 if there is one.
func (p *Plan) Properties() []parquet.WriterProperty {
	props := []parquet.WriterProperty{
		parquet.WithCompression(p.cfg.baseCodec),
		parquet.WithCompressionLevel(p.cfg.baseLevel),
	}
	props = append(props, p.cfg.props...)
	for _, l := range p.leaves {
		if p.pcodec[l.path] {
			props = append(props, ColumnProperties(l.path, l.typ, p.cfg.pcoLevel)...)
		}
	}
	if p.UsesPcodec() {
		props = append(props, parquet.WithDataPageVersion(parquet.DataPageV2))
	}
	return props
}

// NewFileWriter returns a pqarrow writer of the plan's schema with the plan's properties.
// If any column uses PCODEC it adds FooterKey = FooterValue to the footer. pqarrow copies
// the schema metadata into the footer; a FooterKey there is left out, so that the footer
// has the key once, and only when the file has PCODEC columns.
func (p *Plan) NewFileWriter(w io.Writer) (*pqarrow.FileWriter, error) {
	return p.newFileWriter(w, p.Properties())
}

func (p *Plan) newFileWriter(w io.Writer, props []parquet.WriterProperty) (*pqarrow.FileWriter, error) {
	fw, err := pqarrow.NewFileWriter(withoutFooterKey(p.schema), w, parquet.NewWriterProperties(props...), p.cfg.arrowProps)
	if err != nil {
		return nil, err
	}
	if p.UsesPcodec() {
		if err := fw.AppendKeyValueMetadata(FooterKey, FooterValue); err != nil {
			return nil, err
		}
	}
	return fw, nil
}

// withoutFooterKey returns sc without FooterKey in its metadata. pqarrow checks record
// batches against the writer's schema without the schema metadata, so they still match.
func withoutFooterKey(sc *arrow.Schema) *arrow.Schema {
	md := sc.Metadata()
	if md.FindKey(FooterKey) < 0 {
		return sc
	}
	var keys, values []string
	for i, k := range md.Keys() {
		if k != FooterKey {
			keys, values = append(keys, k), append(values, md.Values()[i])
		}
	}
	kept := arrow.NewMetadata(keys, values)
	return arrow.NewSchemaWithEndian(sc.Fields(), &kept, sc.Endianness())
}
