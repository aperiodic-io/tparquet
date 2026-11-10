package io.aperiodic.pcodec;

import java.nio.ByteBuffer;
import java.nio.ByteOrder;

/**
 * Throughput of pcodec-java against the Rust reference, on the same numbers and configuration:
 * the fastest of several runs, in MB/s of uncompressed values. The reference is timed inside
 * the oracle process, so its numbers exclude the IPC. `java ... io.aperiodic.pcodec.Bench`.
 */
final class Bench {
  private Bench() {}

  public static void main(String[] args) {
    Oracle o = Oracle.get();
    int n = 1 << 20;
    String[][] sets = {
      {"F64", "prices"}, {"F64", "floatMult"}, {"F64", "walk"}, {"I64", "timestamps"},
      {"I32", "walk"}, {"F32", "decimal"}, {"I64", "uniform"}
    };
    System.out.printf("%-16s %10s %10s %10s %10s %8s%n", "data (2^20)", "java enc", "rust enc", "java dec", "rust dec", "ratio");
    for (String[] s : sets) {
      NumberType t = NumberType.valueOf(s[0]);
      long[] nums = Corpus.generate(t, s[1], n, 1);
      ChunkConfig cfg = new ChunkConfig();
      long bestC = Long.MAX_VALUE;
      long bestD = Long.MAX_VALUE;
      byte[] file = null;
      for (int i = 0; i < 8; i++) {
        long a = System.nanoTime();
        file = Pco.compress(t, nums, cfg);
        long b = System.nanoTime();
        Pco.decompress(file, t);
        long c = System.nanoTime();
        bestC = Math.min(bestC, b - a);
        bestD = Math.min(bestD, c - b);
      }
      double mb = (double) n * t.bits() / 8 / 1e6;
      String rc = "-";
      String rd = "-";
      if (o != null) {
        byte[] head = Oracle.compressRequest(t, new Oracle.Cfg(), new long[0]);
        byte[] raw = Oracle.rawBytes(t, nums);
        ByteBuffer req = ByteBuffer.allocate(head.length + 4 + raw.length).order(ByteOrder.LITTLE_ENDIAN);
        req.put(head).put(0, (byte) 11).putInt(8).put(raw);
        ByteBuffer r = ByteBuffer.wrap(o.call(req.array()).body).order(ByteOrder.LITTLE_ENDIAN);
        rc = String.format("%.0f", mb / (r.getLong() / 1e9));
        rd = String.format("%.0f", mb / (r.getLong() / 1e9));
      }
      System.out.printf("%-16s %10.0f %10s %10.0f %10s %8.2f%n", s[0] + " " + s[1], mb / (bestC / 1e9), rc,
          mb / (bestD / 1e9), rd, (double) file.length / (n * t.bits() / 8));
    }
  }
}
