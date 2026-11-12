# POC measurements

Base: commit 2d57dce. Each patch applied alone; every one leaves the recorded-vector suites
passing (no output byte changes). Throughput ratios are POC / baseline (higher is better),
geometric means over the 14 `Bench` cases (2^20 numbers each), Java only.

## Pass 1: every POC, one JVM launch each (BENCH_RUNS=7)

The baseline was timed just before each patch. Patches that cannot change decoding read
0.92 to 1.06 on decode, which is the noise of a single launch.

| POC | encode | decode | best encode case | best decode case |
|---|---:|---:|---|---|
| 01-staged-writer | 1.127 | 0.988 | u16 dictish (dict) 1.45 | u16 dictish (dict) 1.15 |
| 02-long-or-write | 1.009 | 0.941 | i32 quadratic conv1(2) 1.18 | f64 floatMult 1.19 |
| 03-fused-batch-write | 1.035 | 0.960 | f64 walk (auto) 1.40 | f64 walk (auto) 1.77 |
| 04-ensure-per-batch | 1.095 | 1.058 | i64 timestamps 1.45 | i64 timestamps 1.68 |
| 05-presized-writer | 1.029 | 0.978 | i64 classic no-op 1.12 | f64 floatMult 1.09 |
| 06-flat-next-states | 0.970 | 0.964 | u16 dictish (dict) 1.13 | i32 quadratic conv1(2) 1.04 |
| 07-unpacked-encode | 0.941 | 0.919 | f64 prices (auto) 1.03 | f64 prices (auto) 1.04 |
| 08-reuse-dissected | 1.024 | 0.990 | i32 quadratic conv1(2) 1.41 | u16 dictish (dict) 1.47 |
| 09-element-major-search | 1.139 | 0.985 | i32 walk 1.82 | f64 walk (auto) 1.10 |
| 10-no-search-fill | 1.023 | 1.016 | i64 classic no-op 1.19 | i64 classic no-op 1.39 |
| 11-soa-bin-infos | 0.967 | 0.979 | f64 prices (auto) 1.04 | i64 timestamps 1.01 |
| 12-local-ans-states | 0.970 | 0.989 | i64 classic no-op 1.06 | i32 quadratic conv1(2) 1.10 |
| 13-signed-domain-sort | 0.945 | 0.954 | f64 prices (auto) 1.01 | i64 classic no-op 1.05 |
| 14-partition-fewer-writes | 1.003 | 1.004 | i64 timestamps 1.12 | f64 walk (auto) 1.15 |
| 15-primitive-bounds | 1.010 | 1.010 | i64 classic no-op 1.14 | i64 uniform 1.10 |
| 16-cached-histogram | 0.969 | 0.984 | f64 walk (auto) 1.03 | f64 walk (auto) 1.04 |
| 17-pivot-no-alloc | 0.982 | 0.950 | i64 timestamps 1.12 | i64 timestamps 1.22 |
| 18-dedupe-candidates | 0.967 | 1.005 | i64 timestamps 1.10 | i64 timestamps 1.31 |
| 19-lookback-primitive-map | 1.022 | 1.041 | f64 quant 1.18 | f64 quant 1.40 |
| 20-lookback-loop | 0.996 | 1.043 | i64 classic no-op 1.35 | i64 classic no-op 1.72 |
| 21-binopt-table | 1.030 | 0.967 | i64 classic no-op 1.36 | u32 periodic lookback 1.27 |
| 22-est-bits-primitive-map | 1.000 | 0.999 | i64 timestamps 1.24 | f32 decimal 1.48 |
| 23-dict-primitive-map | 1.042 | 1.047 | f64 prices (auto) 1.35 | f64 prices (auto) 1.24 |
| 24-gcd-primitive-map | 0.999 | 1.014 | u16 dictish (dict) 1.29 | f64 sorted consec(2) 1.69 |
| 25-floatmult-gcd-loop | 0.987 | 0.968 | i64 mult (intMult) 1.21 | i64 mult (intMult) 1.13 |
| 26-final-float-types | 0.998 | 0.999 | i32 quadratic conv1(2) 1.07 | f64 prices (auto) 1.10 |
| 27-checkraw-skip-64 | 0.970 | 0.941 | i64 timestamps 1.17 | i64 timestamps 1.45 |
| 28-single-page-no-copy | 0.973 | 0.976 | f32 decimal 1.05 | f32 decimal 1.02 |
| 29-reuse-split-arrays | 1.000 | 1.036 | i64 uniform 1.17 | i32 walk 1.27 |
| 30-delta-64-specialized | 1.036 | 1.028 | u32 periodic lookback 1.24 | i32 quadratic conv1(2) 1.40 |
| 31-word-array-reader | 1.009 | 1.072 | f64 sorted consec(2) 1.30 | i64 classic no-op 1.45 |
| 32-window-reader | 1.017 | 0.903 | i64 uniform 1.17 | f64 prices (auto) 1.15 |
| 33-packed-decoder-table | 0.982 | 0.927 | i32 quadratic conv1(2) 1.10 | i64 timestamps 1.11 |
| 34-local-decoder-states | 0.988 | 0.900 | i32 quadratic conv1(2) 1.14 | f64 prices (auto) 1.04 |
| 35-branchless-join | 1.008 | 1.012 | f64 quant 1.36 | f64 quant 1.63 |
| 36-decode-into-output | 1.004 | 1.032 | i64 uniform 1.11 | i64 uniform 1.33 |
| 37-unmasked-64 | 0.961 | 1.000 | i64 mult (intMult) 1.06 | f64 quant 1.06 |
| 38-consecutive-decode-loop | 0.996 | 0.966 | u32 periodic lookback 1.14 | f32 decimal 1.09 |
| 39-fewer-bounds-checks | 1.034 | 1.030 | i64 uniform 1.17 | u16 dictish (dict) 1.24 |
| 40-single-word-peek | 0.982 | 1.006 | i64 classic no-op 1.56 | i64 classic no-op 1.74 |

## Pass 2: the 20 candidates at or above 1.01 in pass 1, three alternating launches each (REPEAT=3, BENCH_RUNS=9)

Each case's median over the three launches; the noise drops to about +-2% (decode-neutral
patches read 0.98 to 1.02).

| POC | encode | decode | best encode case | best decode case |
|---|---:|---:|---|---|
| 01-staged-writer | 1.160 | 0.990 | i64 classic no-op 1.35 | i64 uniform 1.02 |
| 03-fused-batch-write | 1.051 | 1.001 | i64 classic no-op 1.09 | f64 sorted consec(2) 1.08 |
| 04-ensure-per-batch | 1.073 | 1.017 | u32 periodic lookback 1.13 | i64 uniform 1.10 |
| 05-presized-writer | 1.046 | 0.991 | i64 uniform 1.08 | u16 dictish (dict) 1.03 |
| 08-reuse-dissected | 1.007 | 0.998 | i64 classic no-op 1.11 | f64 sorted consec(2) 1.11 |
| 09-element-major-search | 1.132 | 1.017 | i32 walk 1.66 | i64 timestamps 1.22 |
| 10-no-search-fill | 1.019 | 0.989 | i64 timestamps 1.11 | f32 decimal 1.19 |
| 15-primitive-bounds | 0.979 | 0.969 | i64 timestamps 1.08 | i64 timestamps 1.05 |
| 19-lookback-primitive-map | 0.995 | 1.015 | f64 floatMult 1.16 | u16 dictish (dict) 1.21 |
| 20-lookback-loop | 0.982 | 0.990 | f64 floatMult 1.06 | i32 quadratic conv1(2) 1.05 |
| 21-binopt-table | 1.012 | 0.996 | i64 uniform 1.11 | f64 prices (auto) 1.12 |
| 22-est-bits-primitive-map | 1.023 | 1.039 | f64 floatMult 1.14 | f64 walk (auto) 1.24 |
| 23-dict-primitive-map | 1.024 | 0.996 | u16 dictish (dict) 1.27 | i32 walk 1.08 |
| 24-gcd-primitive-map | 1.006 | 0.989 | u16 dictish (dict) 1.13 | u16 dictish (dict) 1.03 |
| 29-reuse-split-arrays | 1.003 | 1.007 | i64 uniform 1.14 | f64 sorted consec(2) 1.09 |
| 30-delta-64-specialized | 1.007 | 0.986 | f64 prices (auto) 1.14 | u16 dictish (dict) 1.19 |
| 31-word-array-reader | 1.018 | 1.077 | i64 uniform 1.20 | f64 sorted consec(2) 1.26 |
| 35-branchless-join | 0.997 | 0.993 | f64 prices (auto) 1.03 | i64 mult (intMult) 1.06 |
| 36-decode-into-output | 1.015 | 1.041 | i64 mult (intMult) 1.25 | i64 uniform 1.15 |
| 39-fewer-bounds-checks | 0.971 | 0.980 | i64 timestamps 1.16 | f32 decimal 1.33 |

## Chosen: the ten most impactful

01 staged-writer, 09 element-major-search, 31 word-array-reader, 04 ensure-per-batch,
03 fused-batch-write, 05 presized-writer, 36 decode-into-output, 22 est-bits-primitive-map,
23 dict-primitive-map, 21 binopt-table. 10 no-search-fill (1.02) is subsumed by 09, which
removes the same fill; 01, 03, 04 and 05 all change the bit writer and are merged into one.
