package main

import (
	"bytes"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestRun(t *testing.T) {
	dir := t.TempDir()
	var out, errOut bytes.Buffer
	if code := run([]string{dir}, &out, &errOut); code != 0 {
		t.Fatalf("exit %d: %s", code, errOut.String())
	}
	if !strings.Contains(out.String(), "go_ints\n") {
		t.Fatalf("output %q", out.String())
	}
	out.Reset()
	if code := run([]string{"-check", dir}, &out, &errOut); code != 0 || !strings.HasSuffix(out.String(), " vectors match\n") {
		t.Fatalf("check: exit %d, %q %q", code, out.String(), errOut.String())
	}
	if strings.Contains(out.String(), "payloads") {
		t.Fatalf("no payloads to check: %q", out.String())
	}
	// With payload vectors, -check decodes them too.
	pd := filepath.Join(dir, "payloads")
	if err := os.MkdirAll(pd, 0o755); err != nil {
		t.Fatal(err)
	}
	manifest := `{"format_version":1,"vectors":[{"name":"raw","payload":"raw.bin","expected_len":1,"valid":true,"decoded":"raw.out","mode":"RAW","hint":null,"level":8}]}`
	for name, b := range map[string][]byte{"manifest.json": []byte(manifest), "raw.bin": {1, 0, 7}, "raw.out": {7}} {
		if err := os.WriteFile(filepath.Join(pd, name), b, 0o644); err != nil {
			t.Fatal(err)
		}
	}
	out.Reset()
	if code := run([]string{"-check", dir}, &out, &errOut); code != 0 || !strings.HasSuffix(out.String(), " vectors match\n1 payloads match\n") {
		t.Fatalf("check with payloads: exit %d, %q %q", code, out.String(), errOut.String())
	}
	if err := os.WriteFile(filepath.Join(pd, "raw.out"), []byte{8}, 0o644); err != nil {
		t.Fatal(err)
	}
	errOut.Reset()
	if code := run([]string{"-check", dir}, &out, &errOut); code != 1 || !strings.Contains(errOut.String(), "raw: decodes to other bytes") {
		t.Fatalf("a bad payload: exit %d, %q", code, errOut.String())
	}
	if err := os.WriteFile(filepath.Join(dir, "expected", "go_ints.jsonl"), []byte("{}\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	errOut.Reset()
	if code := run([]string{"-check", dir}, &out, &errOut); code != 1 || !strings.Contains(errOut.String(), "go_ints: ") {
		t.Fatalf("a bad vector: exit %d, %q", code, errOut.String())
	}
}

func TestRunErrors(t *testing.T) {
	var out, errOut bytes.Buffer
	for _, args := range [][]string{nil, {"a", "b"}, {"-nope", "a"}} {
		errOut.Reset()
		if code := run(args, &out, &errOut); code != 2 || !strings.Contains(errOut.String(), "usage: tparquet-go-gen") {
			t.Errorf("%v: exit %d, %q", args, code, errOut.String())
		}
	}
	f := filepath.Join(t.TempDir(), "f")
	if err := os.WriteFile(f, nil, 0o644); err != nil {
		t.Fatal(err)
	}
	if code := run([]string{f}, &out, &errOut); code != 1 {
		t.Errorf("generating into a file: exit %d", code)
	}
}
