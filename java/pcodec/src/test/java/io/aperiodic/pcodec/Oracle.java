package io.aperiodic.pcodec;

import java.io.BufferedInputStream;
import java.io.BufferedOutputStream;
import java.io.DataInputStream;
import java.io.File;
import java.io.IOException;
import java.io.OutputStream;
import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.nio.charset.StandardCharsets;

/** A client of the Rust reference oracle (oracle/src/main.rs). */
final class Oracle implements AutoCloseable {
  /** an oracle answer: status 0 ok, 1 error, 2 panic */
  static final class Answer {
    final int status;
    final byte[] body;

    Answer(int status, byte[] body) {
      this.status = status;
      this.body = body;
    }

    boolean ok() {
      return status == 0;
    }

    String message() {
      return new String(body, StandardCharsets.UTF_8);
    }

    @Override
    public String toString() {
      return status == 0 ? "ok(" + body.length + " bytes)" : (status == 1 ? "error: " : "panic: ") + message();
    }
  }

  private static Oracle shared;

  /** the shared oracle, or null when it is not built (and not required) */
  static synchronized Oracle get() {
    if (shared == null) {
      String path = System.getProperty("pcodec.oracle", "oracle/target/release/pcodec-oracle");
      if (!new File(path).canExecute()) {
        if (Boolean.getBoolean("pcodec.oracle.required")) {
          throw new IllegalStateException("the reference oracle is required but missing: " + path);
        }
        return null;
      }
      try {
        shared = new Oracle(path);
      } catch (IOException e) {
        throw new IllegalStateException(e);
      }
      Runtime.getRuntime().addShutdownHook(new Thread(() -> shared.close()));
    }
    return shared;
  }

  private final Process process;
  private final OutputStream out;
  private final DataInputStream in;

  /** a new oracle process (its own HashMap seeds), or null when not built */
  static Oracle start() {
    String path = System.getProperty("pcodec.oracle", "oracle/target/release/pcodec-oracle");
    if (!new File(path).canExecute()) {
      return null;
    }
    try {
      return new Oracle(path);
    } catch (IOException e) {
      throw new IllegalStateException(e);
    }
  }

  private Oracle(String path) throws IOException {
    process = new ProcessBuilder(path).redirectError(ProcessBuilder.Redirect.INHERIT).start();
    out = new BufferedOutputStream(process.getOutputStream(), 1 << 16);
    in = new DataInputStream(new BufferedInputStream(process.getInputStream(), 1 << 16));
  }

  synchronized Answer call(byte[] request) {
    try {
      byte[] len = ByteBuffer.allocate(4).order(ByteOrder.LITTLE_ENDIAN).putInt(request.length).array();
      out.write(len);
      out.write(request);
      out.flush();
      byte[] rl = new byte[4];
      in.readFully(rl);
      int n = ByteBuffer.wrap(rl).order(ByteOrder.LITTLE_ENDIAN).getInt();
      byte[] resp = new byte[n];
      in.readFully(resp);
      byte[] body = new byte[n - 1];
      System.arraycopy(resp, 1, body, 0, n - 1);
      return new Answer(resp[0], body);
    } catch (IOException e) {
      throw new IllegalStateException(e);
    }
  }

  @Override
  public synchronized void close() {
    process.destroy();
  }

  // ------------------------------------------------------------ requests

  /** a config as the oracle's wire format describes it */
  static final class Cfg {
    int level = 8;
    int mode; // 0 Auto 1 Classic 2 FloatMult 3 FloatQuant 4 IntMult 5 Dict
    double modeF64;
    int modeK;
    long modeBase;
    int delta; // 0 Auto 1 NoOp 2 Consecutive 3 Lookback 4 Conv1
    long deltaOrder;
    int paging; // 0 EqualPagesUpTo 1 Exact
    long[] pagingValues = {PagingSpec.DEFAULT_MAX_PAGE_N};
    boolean enable8Bit;

    ChunkConfig toConfig() {
      ModeSpec ms;
      switch (mode) {
        case 0:
          ms = ModeSpec.AUTO;
          break;
        case 1:
          ms = ModeSpec.CLASSIC;
          break;
        case 2:
          ms = ModeSpec.tryFloatMult(modeF64);
          break;
        case 3:
          ms = ModeSpec.tryFloatQuant(modeK);
          break;
        case 4:
          ms = ModeSpec.tryIntMult(modeBase);
          break;
        default:
          ms = ModeSpec.TRY_DICT;
      }
      DeltaSpec ds;
      switch (delta) {
        case 0:
          ds = DeltaSpec.AUTO;
          break;
        case 1:
          ds = DeltaSpec.NO_OP;
          break;
        case 2:
          ds = DeltaSpec.tryConsecutive(deltaOrder);
          break;
        case 3:
          ds = DeltaSpec.TRY_LOOKBACK;
          break;
        default:
          ds = DeltaSpec.tryConv1(deltaOrder);
      }
      PagingSpec ps = paging == 0 ? PagingSpec.equalPagesUpTo(pagingValues[0]) : PagingSpec.exact(pagingValues);
      return new ChunkConfig()
          .withCompressionLevel(level)
          .withModeSpec(ms)
          .withDeltaSpec(ds)
          .withPagingSpec(ps)
          .withEnable8Bit(enable8Bit);
    }

    @Override
    public String toString() {
      return toConfig().toString();
    }

    /** the inverse of the wire format written by compress */
    static Cfg parse(ByteBuffer b) {
      Cfg c = new Cfg();
      c.level = b.getInt();
      c.mode = b.get();
      c.modeF64 = b.getDouble();
      c.modeK = b.getInt();
      c.modeBase = b.getLong();
      c.delta = b.get();
      c.deltaOrder = b.getLong();
      c.paging = b.get();
      c.pagingValues = new long[b.getInt()];
      for (int i = 0; i < c.pagingValues.length; i++) {
        c.pagingValues[i] = b.getLong();
      }
      c.enable8Bit = b.get() != 0;
      return c;
    }
  }

  private static ByteBuffer buf(int n) {
    return ByteBuffer.allocate(n).order(ByteOrder.LITTLE_ENDIAN);
  }

  static byte[] rawBytes(NumberType t, long[] nums) {
    int w = t.bits() / 8;
    ByteBuffer b = buf(w * nums.length);
    for (long x : nums) {
      for (int i = 0; i < w; i++) {
        b.put((byte) (x >>> (8 * i)));
      }
    }
    return b.array();
  }

  static long[] fromRaw(NumberType t, byte[] b, int off, int n) {
    int w = t.bits() / 8;
    long[] res = new long[n];
    for (int k = 0; k < n; k++) {
      long x = 0;
      for (int i = w - 1; i >= 0; i--) {
        x = (x << 8) | (b[off + k * w + i] & 0xffL);
      }
      res[k] = x;
    }
    return res;
  }

  private static byte[] withConfig(int op, NumberType t, Cfg c, long[] nums) {
    byte[] raw = rawBytes(t, nums);
    ByteBuffer b = buf(2 + 4 + 1 + 8 + 4 + 8 + 1 + 8 + 1 + 4 + 8 * c.pagingValues.length + 1 + raw.length);
    b.put((byte) op).put((byte) t.typeByte);
    b.putInt(c.level).put((byte) c.mode).putDouble(c.modeF64).putInt(c.modeK).putLong(c.modeBase);
    b.put((byte) c.delta).putLong(c.deltaOrder);
    b.put((byte) c.paging).putInt(c.pagingValues.length);
    for (long v : c.pagingValues) {
      b.putLong(v);
    }
    b.put((byte) (c.enable8Bit ? 1 : 0));
    b.put(raw);
    return b.array();
  }

  Answer compress(NumberType t, Cfg c, long[] nums) {
    return call(withConfig(1, t, c, nums));
  }

  static byte[] compressRequest(NumberType t, Cfg c, long[] nums) {
    return withConfig(1, t, c, nums);
  }

  static byte[] decompressRequest(NumberType t, byte[] file) {
    ByteBuffer b = buf(2 + file.length);
    b.put((byte) 2).put((byte) t.typeByte).put(file);
    return b.array();
  }

  Answer meta(NumberType t, Cfg c, long[] nums) {
    return call(withConfig(3, t, c, nums));
  }

  /** decompress: body = n:u32 values end:u32 */
  Answer decompress(NumberType t, byte[] file) {
    ByteBuffer b = buf(2 + file.length);
    b.put((byte) 2).put((byte) t.typeByte).put(file);
    return call(b.array());
  }

  Answer payloadEncode(int hint, int level, boolean decimal, byte[] src) {
    ByteBuffer b = buf(4 + src.length);
    b.put((byte) 4).put((byte) hint).put((byte) level).put((byte) (decimal ? 1 : 0)).put(src);
    return call(b.array());
  }

  Answer payloadDecode(long expectedLen, byte[] payload) {
    ByteBuffer b = buf(9 + payload.length);
    b.put((byte) 5).putLong(expectedLen).put(payload);
    return call(b.array());
  }

  double[] math(int fn, double[] xs) {
    ByteBuffer b = buf(6 + 8 * xs.length);
    b.put((byte) 6).put((byte) fn).putInt(xs.length);
    for (double x : xs) {
      b.putDouble(x);
    }
    Answer a = call(b.array());
    ByteBuffer r = ByteBuffer.wrap(a.body).order(ByteOrder.LITTLE_ENDIAN);
    double[] res = new double[xs.length];
    for (int i = 0; i < res.length; i++) {
      res[i] = r.getDouble();
    }
    return res;
  }

  /** math as raw u64 results (op 6) */
  long[] mathBits(int fn, double[] xs) {
    ByteBuffer b = buf(6 + 8 * xs.length);
    b.put((byte) 6).put((byte) fn).putInt(xs.length);
    for (double x : xs) {
      b.putDouble(x);
    }
    ByteBuffer r = ByteBuffer.wrap(call(b.array()).body).order(ByteOrder.LITTLE_ENDIAN);
    long[] res = new long[xs.length];
    for (int i = 0; i < res.length; i++) {
      res[i] = r.getLong();
    }
    return res;
  }

  /** f16 conversions: fn 0 takes f64 values (as bits), fn 1 f32 bits */
  int[] f16(int fn, long[] xs) {
    ByteBuffer b = buf(6 + 8 * xs.length);
    b.put((byte) 7).put((byte) fn).putInt(xs.length);
    for (long x : xs) {
      if (fn == 0) {
        b.putLong(x);
      } else {
        b.putInt((int) x);
      }
    }
    Answer a = call(java.util.Arrays.copyOf(b.array(), b.position()));
    ByteBuffer r = ByteBuffer.wrap(a.body).order(ByteOrder.LITTLE_ENDIAN);
    int[] res = new int[xs.length];
    for (int i = 0; i < res.length; i++) {
      res[i] = r.getShort() & 0xffff;
    }
    return res;
  }
}
