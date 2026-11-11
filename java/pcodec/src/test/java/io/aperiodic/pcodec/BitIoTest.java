package io.aperiodic.pcodec;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;

import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.SplittableRandom;
import org.junit.jupiter.api.Test;

/**
 * The staged {@link BitWriter} against a plain bit-by-bit model, on random sequences of every
 * operation (from an unreserved buffer, so that its own capacity checks are what keep it in
 * bounds), and {@link LongIndexMap} against a {@code LinkedHashMap}.
 */
class BitIoTest {
  /** the bit-by-bit model: bit i of the output is bits.get(i) */
  static final class Model {
    final List<Boolean> bits = new ArrayList<>();

    void writeUint(long x, int n) {
      for (int i = 0; i < n; i++) {
        bits.add(((x >>> i) & 1) != 0);
      }
    }

    void finishByte() {
      while (bits.size() % 8 != 0) {
        bits.add(false);
      }
    }

    void writeAlignedBytes(byte[] b) {
      for (byte x : b) {
        writeUint(x & 0xff, 8);
      }
    }

    byte[] bytes() {
      byte[] out = new byte[(bits.size() + 7) / 8];
      for (int i = 0; i < bits.size(); i++) {
        if (bits.get(i)) {
          out[i / 8] |= (byte) (1 << (i % 8));
        }
      }
      return out;
    }
  }

  static long value(SplittableRandom r, int n) {
    long x = r.nextLong();
    return n == 64 ? x : x & ((1L << n) - 1);
  }

  @Test
  void stagedWriterWritesWhatTheBitModelWrites() {
    SplittableRandom r = new SplittableRandom(5);
    for (int round = 0; round < 400; round++) {
      BitWriter w = new BitWriter();
      Model m = new Model();
      int ops = 1 + r.nextInt(400);
      for (int op = 0; op < ops; op++) {
        switch (r.nextInt(6)) {
          case 0:
          case 1:
            {
              int n = r.nextInt(65);
              long x = value(r, n);
              w.writeUint(x, n);
              m.writeUint(x, n);
              break;
            }
          case 2:
            w.finishByte();
            m.finishByte();
            break;
          case 3:
            {
              w.finishByte();
              m.finishByte();
              byte[] b = new byte[r.nextInt(20)];
              r.nextBytes(b);
              w.writeAlignedBytes(b);
              m.writeAlignedBytes(b);
              break;
            }
          case 4:
            if (r.nextInt(8) == 0) {
              w.reserve(r.nextInt(64));
            }
            break;
          default:
            {
              int len = 1 + r.nextInt(300);
              int from = r.nextInt(3);
              int[] ansVals = new int[from + len];
              int[] ansBits = new int[from + len];
              long[] offsets = new long[from + len];
              int[] offsetBits = new int[from + len];
              for (int i = 0; i < from + len; i++) {
                ansBits[i] = r.nextInt(33);
                ansVals[i] = (int) value(r, ansBits[i]);
                offsetBits[i] = r.nextInt(4) == 0 ? 64 : r.nextInt(65);
                offsets[i] = value(r, offsetBits[i]);
              }
              boolean withAns = r.nextBoolean();
              w.writeBatch(ansVals, ansBits, offsets, offsetBits, from, from + len, withAns);
              if (withAns) {
                for (int i = from; i < from + len; i++) {
                  m.writeUint(ansVals[i] & 0xffffffffL, ansBits[i]);
                }
              }
              for (int i = from; i < from + len; i++) {
                m.writeUint(offsets[i], offsetBits[i]);
              }
            }
        }
      }
      assertArrayEquals(m.bytes(), w.toByteArray(), "round " + round);
    }
  }

  @Test
  void aBatchOfWideValuesGrowsAnUnreservedWriter() {
    // 300 offsets of 64 bits: 2400 bytes from a 256-byte buffer, all in one writeBatch
    BitWriter w = new BitWriter();
    Model m = new Model();
    w.writeUint(5, 3);
    m.writeUint(5, 3);
    long[] offsets = new long[300];
    int[] bits = new int[300];
    for (int i = 0; i < offsets.length; i++) {
      offsets[i] = 0x8000_0000_0000_0001L ^ (i * 0x9e3779b97f4a7c15L);
      bits[i] = 64;
    }
    w.writeBatch(new int[300], new int[300], offsets, bits, 0, 300, false);
    for (long o : offsets) {
      m.writeUint(o, 64);
    }
    assertArrayEquals(m.bytes(), w.toByteArray());
  }

  @Test
  void longIndexMapNumbersKeysInFirstInsertionOrder() {
    SplittableRandom r = new SplittableRandom(9);
    long[] specials = {0, -1, Long.MIN_VALUE, Long.MAX_VALUE, 1, 0x9e3779b97f4a7c15L};
    for (int round = 0; round < 50; round++) {
      LongIndexMap map = new LongIndexMap(r.nextInt(40));
      Map<Long, Integer> want = new LinkedHashMap<>();
      int n = r.nextInt(5000);
      int range = 1 + r.nextInt(round % 2 == 0 ? 50 : 1 << 20);
      for (int i = 0; i < n; i++) {
        long k = r.nextInt(8) == 0 ? specials[r.nextInt(specials.length)] : r.nextInt(range) * 0x10001L;
        Integer idx = want.get(k);
        if (idx == null) {
          idx = want.size();
          want.put(k, idx);
        }
        assertEquals((int) idx, map.add(k), "add " + k);
      }
      assertEquals(want.size(), map.size());
      int i = 0;
      for (Map.Entry<Long, Integer> e : want.entrySet()) {
        assertEquals((long) e.getKey(), map.key(i));
        assertEquals(i, map.get(e.getKey()));
        i++;
      }
      for (int j = 0; j < 200; j++) {
        long k = r.nextLong();
        assertEquals(want.containsKey(k) ? want.get(k) : -1, map.get(k));
      }
    }
  }
}
