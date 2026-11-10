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
    return res;
  }

  /** java's answer to a recorded request (compress, or decompress of the recorded output) */
  static Parity.Outcome java(byte[] request) {
    ByteBuffer b = ByteBuffer.wrap(request).order(ByteOrder.LITTLE_ENDIAN);
    int op = b.get();
    NumberType t = NumberType.fromTypeByte(b.get());
    if (op == 1) {
      Oracle.Cfg cfg = Oracle.Cfg.parse(b);
      long[] nums = Oracle.fromRaw(t, request, b.position(), b.remaining() / (t.bits() / 8));
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
      for (byte[] req : requests()) {
        Oracle.Answer a = o.call(req);
        write(out, req, a);
        n++;
        if (a.ok()) {
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
    }
    System.out.println("wrote " + n + " records to " + p);
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
