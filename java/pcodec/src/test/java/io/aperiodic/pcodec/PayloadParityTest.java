package io.aperiodic.pcodec;

import static org.junit.jupiter.api.Assumptions.assumeTrue;

import io.aperiodic.pcodec.payload.ParquetType;
import io.aperiodic.pcodec.payload.PayloadException;
import io.aperiodic.pcodec.payload.PcodecPayload;
import java.util.SplittableRandom;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;

/** The FORMAT.md payload against tparquet-core, the reference encoder and decoder. */
class PayloadParityTest {
  static Oracle o;
  static int scale;
  static final PcodecPayload JAVA = new PcodecPayload(new AircompressorZstd());

  @BeforeAll
  static void setUp() {
    o = Oracle.get();
    scale = Integer.getInteger("pcodec.parity.scale", 1);
  }

  static Parity.Outcome java(ParityRun r) {
    try {
      return new Parity.Outcome(0, r.run(), null);
    } catch (PayloadException e) {
      return new Parity.Outcome(1, null, e.getMessage());
    }
  }

  interface ParityRun {
    byte[] run();
  }

  static final ParquetType[] HINTS = {ParquetType.INT32, ParquetType.INT64, ParquetType.FLOAT, ParquetType.DOUBLE};

  static NumberType numberType(ParquetType t) {
    switch (t) {
      case INT32:
        return NumberType.I32;
      case INT64:
        return NumberType.I64;
      case FLOAT:
        return NumberType.F32;
      default:
        return NumberType.F64;
    }
  }

  @Test
  void encoderMatchesReference() {
    assumeTrue(o != null, "oracle not built");
    ParityTest.Report rep = new ParityTest.Report();
    SplittableRandom r = new SplittableRandom(5);
    for (ParquetType hint : HINTS) {
      NumberType nt = numberType(hint);
      for (Corpus.Case c : Corpus.cases(new NumberType[] {nt}, new int[] {0, 1, 7, 99, 100, 101, 500, 4096, 5000, 20000}, scale)) {
        byte[] src = Oracle.rawBytes(nt, c.nums);
        int level = r.nextInt(14);
        boolean decimal = r.nextInt(4) != 0;
        // hints of another width, or none, too
        ParquetType h = r.nextInt(6) == 0 ? HINTS[r.nextInt(4)] : hint;
        boolean none = r.nextInt(10) == 0;
        Oracle.Answer want = o.payloadEncode(none ? 0 : h.dtype, level, decimal, src);
        Parity.Outcome got = java(() -> new PcodecPayload().compress(src, 0, src.length, none ? null : h, level, decimal));
        if (want.ok() && want.body.length > 1 && want.body[1] == PcodecPayload.MODE_ZSTD) {
          // ZSTD bytes are libzstd's: the Java codec without Zstandard writes RAW instead
          if (got.status == 0 && got.body[1] == PcodecPayload.MODE_RAW) {
            rep.cases++;
            continue;
          }
        }
        rep.check(c + " hint=" + (none ? null : h) + " level=" + level + " decimal=" + decimal, want, got);
        if (want.ok()) {
          Oracle.Answer dec = o.payloadDecode(src.length, want.body);
          rep.check(c + " decode", dec, java(() -> JAVA.decompress(want.body, src.length)));
        }
      }
    }
    rep.done();
  }

  @Test
  void decoderMatchesReferenceOnCorruptPayloads() {
    assumeTrue(o != null, "oracle not built");
    ParityTest.Report rep = new ParityTest.Report();
    SplittableRandom r = new SplittableRandom(6);
    for (ParquetType hint : HINTS) {
      NumberType nt = numberType(hint);
      for (Corpus.Case c : Corpus.cases(new NumberType[] {nt}, new int[] {0, 3, 120, 700}, scale)) {
        byte[] src = Oracle.rawBytes(nt, c.nums);
        Oracle.Answer good = o.payloadEncode(hint.dtype, 8, true, src);
        for (int k = 0; k < 16; k++) {
          byte[] bad = good.body.clone();
          int len = src.length;
          switch (r.nextInt(6)) {
            case 0:
              if (bad.length > 0) {
                bad[r.nextInt(bad.length)] ^= (byte) (1 << r.nextInt(8));
              }
              break;
            case 1:
              bad = java.util.Arrays.copyOf(bad, r.nextInt(bad.length + 1));
              break;
            case 2:
              len += r.nextInt(17) - 8;
              if (len < 0) {
                len = 0;
              }
              break;
            case 3:
              bad = java.util.Arrays.copyOf(bad, bad.length + 1 + r.nextInt(4));
              break;
            case 4:
              if (bad.length > 2) {
                bad[1] = (byte) r.nextInt(5);
                bad[2] = (byte) r.nextInt(7);
              }
              break;
            default:
              if (bad.length > 4) {
                bad[3 + r.nextInt(Math.min(6, bad.length - 3))] = (byte) r.nextInt();
              }
              break;
          }
          byte[] f = bad;
          int l = len;
          rep.check(c + " corrupt " + k, o.payloadDecode(l, f), java(() -> JAVA.decompress(f, l)));
        }
      }
    }
    rep.done();
  }
}
