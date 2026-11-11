// Command tparquet-go-gen writes the Go-produced PCODEC conformance vectors into a directory,
// in the testdata layout (files/<name>.parquet, expected/<name>.jsonl), for other
// implementations to read:
//
//	tparquet-go-gen DIR
//
// With -check it instead reads every DIR/files/*.parquet and compares it with
// DIR/expected/<name>.jsonl, and decodes every payload of DIR/payloads/manifest.json if
// there is one.
package main

import (
	"errors"
	"flag"
	"fmt"
	"io"
	"os"

	"github.com/aperiodic-io/tparquet/go/internal/vectors"
)

func main() { os.Exit(run(os.Args[1:], os.Stdout, os.Stderr)) }

func run(args []string, stdout, stderr io.Writer) int {
	fs := flag.NewFlagSet("tparquet-go-gen", flag.ContinueOnError)
	fs.SetOutput(stderr)
	check := fs.Bool("check", false, "check the vectors in DIR instead of writing them")
	fs.Usage = func() {
		fmt.Fprintln(stderr, "usage: tparquet-go-gen [-check] DIR")
		fs.PrintDefaults()
	}
	if err := fs.Parse(args); err != nil {
		return 2
	}
	if fs.NArg() != 1 {
		fs.Usage()
		return 2
	}
	dir := fs.Arg(0)
	if *check {
		n, err := vectors.Check(dir)
		if err != nil {
			fmt.Fprintln(stderr, err)
			return 1
		}
		fmt.Fprintf(stdout, "%d vectors match\n", n)
		n, err = vectors.CheckPayloads(dir)
		if errors.Is(err, vectors.ErrNoPayloads) {
			return 0
		}
		if err != nil {
			fmt.Fprintln(stderr, err)
			return 1
		}
		fmt.Fprintf(stdout, "%d payloads match\n", n)
		return 0
	}
	names, err := vectors.Generate(dir)
	if err != nil {
		fmt.Fprintln(stderr, err)
		return 1
	}
	for _, n := range names {
		fmt.Fprintln(stdout, n)
	}
	return 0
}
