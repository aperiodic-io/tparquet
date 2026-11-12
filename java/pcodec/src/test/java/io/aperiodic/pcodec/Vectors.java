package io.aperiodic.pcodec;

import java.io.ByteArrayOutputStream;
import java.io.DataInputStream;
import java.io.DataOutputStream;
import java.io.EOFException;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.ArrayList;
import java.util.List;
import java.util.SplittableRandom;
import java.util.zip.GZIPInputStream;
import java.util.zip.GZIPOutputStream;

/**
 * Golden vectors: a recording of requests to the reference oracle and its answers
 * (vectors/pco-1.0.4.bin.gz), so that the Java implementation is checked against the reference
 * without a Rust toolchain. {@link #main} regenerates the file from the oracle; CI regenerates
 * it and fails if a byte differs.
 */
final class Vectors {
  private Vectors() {}

  static final String FILE = "pco-1.0.4.bin.gz";

  /** the number of leading requests whose files are also decoded and corrupted */
  static int base;

  static final class Record {
    final byte[] request;
    final int status;
    final byte[] body;

    Record(byte[] request, int status, byte[] body) {
      this.request = request;
      this.status = status;
      this.body = body;
    }
  }

  static Path path() {
    return Paths.get(System.getProperty("pcodec.vectors", "vectors"), FILE);
  }

  static List<Record> read(Path p) throws IOException {
    List<Record> res = new ArrayList<>();
    try (InputStream in = new GZIPInputStream(Files.newInputStream(p), 1 << 16);
        DataInputStream d = new DataInputStream(in)) {
      while (true) {
        int n;
        try {
          n = d.readInt();
        } catch (EOFException e) {
          return res;
        }
        byte[] req = new byte[n];
        d.readFully(req);
        int status = d.readUnsignedByte();
        byte[] body = new byte[d.readInt()];
        d.readFully(body);
        res.add(new Record(req, status, body));
      }
    }
  }

  /** the requests the vectors record: deterministic */
  static List<byte[]> requests() {
    List<byte[]> res = new ArrayList<>();
    SplittableRandom r = new SplittableRandom(20261110);
    for (Corpus.Case c : Corpus.cases(NumberType.values(), new int[] {1, 10, 257, 700}, 1)) {
      List<Oracle.Cfg> cfgs = new ArrayList<>();
      Oracle.Cfg def = new Oracle.Cfg();
      def.enable8Bit = c.type.bits() == 8;
      cfgs.add(def);
      Oracle.Cfg[] modes = ParityTest.modeSpecs(c.type, r);
      Oracle.Cfg[] deltas = ParityTest.deltaSpecs();
      for (int k = 0; k < 2; k++) {
        Oracle.Cfg cfg = modes[r.nextInt(modes.length)];
        Oracle.Cfg d = deltas[r.nextInt(deltas.length)];
        cfg.delta = d.delta;
        cfg.deltaOrder = d.deltaOrder;
        cfg.level = r.nextInt(14);
        cfg.enable8Bit = r.nextInt(4) != 0;
        if (cfg.mode == 5 && !ParityTest.dictReproducible(c.type, c.nums)) {
          continue;
        }
        cfgs.add(cfg);
      }
      for (Oracle.Cfg cfg : cfgs) {
        res.add(Oracle.compressRequest(c.type, cfg, c.nums));
      }
    }
    // targeted, decoded and corrupted too: conv1 of every order, and dict mode
    for (NumberType t : new NumberType[] {NumberType.U8, NumberType.I16, NumberType.U16, NumberType.I32, NumberType.U32}) {
      for (String shape : new String[] {"trend", "quadratic", "walk"}) {
        for (int n : new int[] {40, 515, 1100}) {
          long[] nums = Corpus.generate(t, shape, n, 3);
          for (long order : new long[] {1, 2, 3, 5, 6, 8, 16}) {
            Oracle.Cfg c = new Oracle.Cfg();
            c.delta = 4;
            c.deltaOrder = order;
            c.enable8Bit = true;
            res.add(Oracle.compressRequest(t, c, nums));
          }
        }
      }
    }
    for (NumberType t : new NumberType[] {NumberType.I32, NumberType.U64, NumberType.F64, NumberType.U16}) {
      for (String shape : new String[] {"dictish", "small", "sparse"}) {
        long[] nums = Corpus.generate(t, shape, 900, 7);
        if (!ParityTest.dictReproducible(t, nums)) {
          continue;
        }
        for (int delta : new int[] {0, 1}) {
          Oracle.Cfg c = new Oracle.Cfg();
          c.mode = 5;
          c.delta = delta;
          res.add(Oracle.compressRequest(t, c, nums));
        }
      }
    }
    base = res.size();
    // targeted (compression only): large inputs whose histograms meet bad pivots
    for (NumberType t : new NumberType[] {NumberType.I32, NumberType.U64}) {
      for (String shape : new String[] {"dictish", "decimal", "bimodal", "constant", "spiky"}) {
        long[] nums = Corpus.generate(t, shape, 10000, 0);
        for (int level : new int[] {8, 12}) {
          Oracle.Cfg c = new Oracle.Cfg();
          c.mode = 1;
          c.delta = 1;
          c.level = level;
          res.add(Oracle.compressRequest(t, c, nums));
        }
      }
    }
    // each other delta encoding, mode parameter, level and paging spec
    for (NumberType t : new NumberType[] {NumberType.I64, NumberType.F64, NumberType.U32, NumberType.F32}) {
      for (String shape : new String[] {"periodic", "dictish", "spiky", "steps"}) {
        for (int n : new int[] {20, 300, 1500}) {
          long[] nums = Corpus.generate(t, shape, n, 4);
          Oracle.Cfg lb = new Oracle.Cfg();
          lb.delta = 3;
          res.add(Oracle.compressRequest(t, lb, nums));
          res.add(Oracle.compressRequest(t, new Oracle.Cfg(), nums));
          for (long order = 1; order <= 7; order++) {
            Oracle.Cfg c = new Oracle.Cfg();
            c.delta = 2;
            c.deltaOrder = order;
            res.add(Oracle.compressRequest(t, c, Corpus.generate(t, "quadratic", n, 4)));
          }
        }
      }
    }
    for (NumberType t : new NumberType[] {NumberType.F64, NumberType.F32, NumberType.F16}) {
      for (String shape : new String[] {"floatMult", "quant", "decimal", "prices", "multNoise"}) {
        for (int n : new int[] {30, 2000}) {
          long[] nums = Corpus.generate(t, shape, n, 5);
          res.add(Oracle.compressRequest(t, new Oracle.Cfg(), nums));
          for (double b : new double[] {0.1, 0.01, 0.25, 1.0 / 3}) {
            Oracle.Cfg c = new Oracle.Cfg();
            c.mode = 2;
            c.modeF64 = b;
            res.add(Oracle.compressRequest(t, c, nums));
          }
          for (int k : new int[] {1, 4, 10, 20}) {
            Oracle.Cfg c = new Oracle.Cfg();
            c.mode = 3;
            c.modeK = k;
            res.add(Oracle.compressRequest(t, c, nums));
          }
        }
      }
    }
    for (NumberType t : new NumberType[] {NumberType.I64, NumberType.U16, NumberType.I32}) {
      for (String shape : new String[] {"mult", "multNoise", "timestamps"}) {
        long[] nums = Corpus.generate(t, shape, 1300, 6);
        res.add(Oracle.compressRequest(t, new Oracle.Cfg(), nums));
        for (long b : new long[] {2, 3, 7, 1000}) {
          Oracle.Cfg c = new Oracle.Cfg();
          c.mode = 4;
          c.modeBase = b;
          res.add(Oracle.compressRequest(t, c, nums));
        }
        for (int level = 0; level <= 12; level++) {
          Oracle.Cfg c = new Oracle.Cfg();
          c.level = level;
          res.add(Oracle.compressRequest(t, c, nums));
        }
        for (long up : new long[] {1, 100, 1000}) {
          Oracle.Cfg c = new Oracle.Cfg();
          c.pagingValues = new long[] {up};
          res.add(Oracle.compressRequest(t, c, java.util.Arrays.copyOf(nums, 1200)));
        }
      }
    }
    return res;
  }

  /** java's answer to a recorded request (compress, or decompress of the recorded output) */
  static Parity.Outcome java(byte[] request) {
    ByteBuffer b = ByteBuffer.wrap(request).order(ByteOrder.LITTLE_ENDIAN);
    int op = b.get();
    NumberType t = NumberType.fromTypeByte(b.get());
    if (op == 1 || op == 3) {
      Oracle.Cfg cfg = Oracle.Cfg.parse(b);
      long[] nums = Oracle.fromRaw(t, request, b.position(), b.remaining() / (t.bits() / 8));
      if (op == 3) {
        return Parity.java(() -> Pco.chunkMeta(t, nums, cfg.toConfig()).toString().getBytes(java.nio.charset.StandardCharsets.UTF_8));
      }
      return Parity.java(() -> Pco.compress(t, nums, cfg.toConfig()));
    }
    byte[] file = new byte[b.remaining()];
    b.get(file);
    return Parity.java(() -> Parity.decodeBody(t, file));
  }

  public static void main(String[] args) throws IOException {
    Oracle o = Oracle.get();
    if (o == null) {
      throw new IllegalStateException("the oracle is needed to record vectors");
    }
    Path p = args.length > 0 ? Paths.get(args[0]) : path();
    SplittableRandom r = new SplittableRandom(7);
    int n = 0;
    try (OutputStream f = Files.newOutputStream(p);
        GZIPOutputStream gz = new GZIPOutputStream(f, 1 << 16) {
          {
            def.setLevel(9);
          }
        };
        DataOutputStream out = new DataOutputStream(gz)) {
      List<byte[]> reqs = requests();
      for (int i = 0; i < reqs.size(); i++) {
        byte[] req = reqs.get(i);
        Oracle.Answer a = o.call(req);
        write(out, req, a);
        n++;
        if (i < base && i % 4 == 0) {
          // the chunk metadata, as the reference's Debug prints it
          byte[] meta = req.clone();
          meta[0] = 3;
          write(out, meta, o.call(meta));
          n++;
        }
        if (a.ok() && i < base) {
          // the decoder on the file, and on a corrupted copy
          byte[] dec = Oracle.decompressRequest(NumberType.fromTypeByte(req[1]), a.body);
          write(out, dec, o.call(dec));
          byte[] bad = a.body.clone();
          if (r.nextBoolean()) {
            bad[r.nextInt(bad.length)] ^= (byte) (1 << r.nextInt(8));
          } else {
            bad = java.util.Arrays.copyOf(bad, r.nextInt(bad.length));
          }
          byte[] decBad = Oracle.decompressRequest(NumberType.fromTypeByte(req[1]), bad);
          write(out, decBad, o.call(decBad));
          n += 2;
        }
      }
      // headers cut after the format version's major byte, or after its minor: the reference
      // checks the version before the bounds, so an unsupported major is corruption, not
      // insufficient data
      byte[] file = o.call(Oracle.compressRequest(NumberType.I64, new Oracle.Cfg(), new long[] {5})).body;
      int versionAt = 4 + 1 + 1 + 1; // magic, standalone version, number type, size hint
      for (int major : new int[] {0, 3, 4, 5, 6, 255}) {
        for (int len = 1; len <= 3; len++) {
          byte[] cut = java.util.Arrays.copyOf(file, versionAt + len);
          cut[versionAt] = (byte) major;
          byte[] dec = Oracle.decompressRequest(NumberType.I64, cut);
          write(out, dec, o.call(dec));
          n++;
        }
      }
    }
    System.out.println("wrote " + n + " records to " + p);
    MathParityTest.o = o;
    Files.write(p.resolveSibling("math.txt"), MathVectorsTest.record());
  }

  private static void write(DataOutputStream out, byte[] req, Oracle.Answer a) throws IOException {
    out.writeInt(req.length);
    out.write(req);
    out.writeByte(a.status);
    out.writeInt(a.body.length);
    out.write(a.body);
  }

  static byte[] concat(byte[]... parts) {
    ByteArrayOutputStream b = new ByteArrayOutputStream();
    for (byte[] p : parts) {
      b.write(p, 0, p.length);
    }
    return b.toByteArray();
  }
}
