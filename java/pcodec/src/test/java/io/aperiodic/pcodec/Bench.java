package io.aperiodic.pcodec;

import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;
import java.util.Locale;

/**
 * Throughput of pcodec-java against the Rust reference (BENCHMARK.md), on the same numbers and
 * configuration. Both sides warm up for as many runs as they measure, then report the median,
 * in MB/s of uncompressed values; the reference is timed inside the oracle process, so its
 * numbers exclude the IPC.
 *
 * <p>{@code make bench} runs it; {@code -Dbench.n=<count> -Dbench.runs=<runs>
 * -Dbench.only=<case>} narrow it, and {@code -Dbench.java-only=true} skips the reference.
 */
final class Bench {
  private Bench() {}

  /** a benchmark case: numbers of a corpus shape under one configuration */
  static final class Case {
    final String name;
    final NumberType type;
    final String shape;
    final Oracle.Cfg cfg;

    Case(String name, NumberType type, String shape, Oracle.Cfg cfg) {
      this.name = name;
      this.type = type;
      this.shape = shape;
      this.cfg = cfg;
    }
  }

  static Oracle.Cfg cfg(int mode, int delta, long order) {
    Oracle.Cfg c = new Oracle.Cfg();
    c.mode = mode;
    c.delta = delta;
    c.deltaOrder = order;
    return c;
  }

  /** every mode and delta encoding of the reference, on numbers each one suits */
  static List<Case> cases() {
    List<Case> cs = new ArrayList<>();
    cs.add(new Case("f64 prices (auto)", NumberType.F64, "prices", cfg(0, 0, 0)));
    cs.add(new Case("f64 walk (auto)", NumberType.F64, "walk", cfg(0, 0, 0)));
    cs.add(new Case("f64 floatMult", NumberType.F64, "floatMult", cfg(0, 0, 0)));
    cs.add(new Case("f64 quant", NumberType.F64, "quant", cfg(0, 0, 0)));
    cs.add(new Case("f32 decimal", NumberType.F32, "decimal", cfg(0, 0, 0)));
    cs.add(new Case("i64 timestamps", NumberType.I64, "timestamps", cfg(0, 0, 0)));
    cs.add(new Case("i64 uniform", NumberType.I64, "uniform", cfg(0, 0, 0)));
    cs.add(new Case("i64 mult (intMult)", NumberType.I64, "mult", cfg(0, 0, 0)));
    cs.add(new Case("i32 walk", NumberType.I32, "walk", cfg(0, 0, 0)));
    cs.add(new Case("u32 periodic lookback", NumberType.U32, "periodic", cfg(0, 3, 0)));
    cs.add(new Case("i32 quadratic conv1(2)", NumberType.I32, "quadratic", cfg(0, 4, 2)));
    cs.add(new Case("u16 dictish (dict)", NumberType.U16, "dictish", cfg(5, 1, 0)));
    cs.add(new Case("i64 classic no-op", NumberType.I64, "walk", cfg(1, 1, 0)));
    cs.add(new Case("f64 sorted consec(2)", NumberType.F64, "sorted", cfg(0, 2, 2)));
    return cs;
  }

  /** median of a sample, sorted in place */
  static long median(long[] xs) {
    Arrays.sort(xs);
    return xs[xs.length / 2];
  }

  /** {median compress ns, median decompress ns, compressed size} of the Java implementation */
  static long[] java(Case c, long[] nums, int runs) {
    ChunkConfig config = c.cfg.toConfig();
    long[] cs = new long[runs];
    long[] ds = new long[runs];
    byte[] file = null;
    long sink = 0;
    for (int i = 0; i < 2 * runs; i++) {
      long a = System.nanoTime();
      file = Pco.compress(c.type, nums, config);
      long b = System.nanoTime();
      sink += Pco.decompress(file, c.type).nums.length;
      long d = System.nanoTime();
      if (i >= runs) {
        cs[i - runs] = b - a;
        ds[i - runs] = d - b;
      }
    }
    if (sink == 42) {
      System.out.print("");
    }
    return new long[] {median(cs), median(ds), file.length};
  }

  /** the same of the reference, timed in the oracle (op 11) */
  static long[] rust(Oracle o, Case c, long[] nums, int runs) {
    byte[] head = Oracle.compressRequest(c.type, c.cfg, new long[0]);
    byte[] raw = Oracle.rawBytes(c.type, nums);
    ByteBuffer req = ByteBuffer.allocate(head.length + 4 + raw.length).order(ByteOrder.LITTLE_ENDIAN);
    req.put(head).put(0, (byte) 11).putInt(runs).put(raw);
    Oracle.Answer a = o.call(req.array());
    if (!a.ok()) {
      throw new IllegalStateException("reference: " + a);
    }
    ByteBuffer r = ByteBuffer.wrap(a.body).order(ByteOrder.LITTLE_ENDIAN);
    return new long[] {r.getLong(), r.getLong(), r.getLong()};
  }

  static double mbps(long bytes, long ns) {
    return bytes / 1e6 / (ns / 1e9);
  }

  public static void main(String[] args) {
    int n = Integer.getInteger("bench.n", 1 << 20);
    int runs = Integer.getInteger("bench.runs", 15);
    String only = System.getProperty("bench.only");
    Oracle o = Boolean.getBoolean("bench.java-only") ? null : Oracle.get();
    System.out.printf(Locale.ROOT, "n = %d numbers per case, median of %d runs after %d warm-up runs, MB/s of values%n", n, runs, runs);
    System.out.printf(Locale.ROOT, "| %-24s | %9s | %9s | %6s | %9s | %9s | %6s | %6s |%n",
        "case", "java enc", "rust enc", "ratio", "java dec", "rust dec", "ratio", "size");
    System.out.println("|---|---:|---:|---:|---:|---:|---:|---:|");
    double logEnc = 0;
    double logDec = 0;
    int counted = 0;
    for (Case c : cases()) {
      if (only != null && !c.name.contains(only)) {
        continue;
      }
      long[] nums = Corpus.generate(c.type, c.shape, n, 1);
      long bytes = (long) n * c.type.bits() / 8;
      long[] j = java(c, nums, runs);
      long[] r = o == null ? null : rust(o, c, nums, runs);
      if (r != null && r[2] != j[2]) {
        throw new IllegalStateException(c.name + ": sizes differ, java " + j[2] + ", rust " + r[2]);
      }
      double je = mbps(bytes, j[0]);
      double jd = mbps(bytes, j[1]);
      if (r == null) {
        System.out.printf(Locale.ROOT, "| %-24s | %9.0f | %9s | %6s | %9.0f | %9s | %6s | %6.3f |%n",
            c.name, je, "-", "-", jd, "-", "-", (double) j[2] / bytes);
      } else {
        double re = mbps(bytes, r[0]);
        double rd = mbps(bytes, r[1]);
        logEnc += Math.log(je / re);
        logDec += Math.log(jd / rd);
        counted++;
        System.out.printf(Locale.ROOT, "| %-24s | %9.0f | %9.0f | %6.2f | %9.0f | %9.0f | %6.2f | %6.3f |%n",
            c.name, je, re, je / re, jd, rd, jd / rd, (double) j[2] / bytes);
      }
    }
    if (counted > 0) {
      System.out.printf(Locale.ROOT, "geometric mean of java/rust: encode %.2f, decode %.2f%n",
          Math.exp(logEnc / counted), Math.exp(logDec / counted));
    }
  }
}
