package io.aperiodic.pcodec;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import org.junit.jupiter.api.Test;

/** The public API around the codec: configuration, errors, headers and metadata inspection. */
class ApiTest {
  @Test
  void specsPrintTheirValues() {
    assertEquals("AUTO", ModeSpec.AUTO.toString());
    assertEquals("TryFloatMult(0.5)", ModeSpec.tryFloatMult(0.5).toString());
    assertEquals("TryFloatQuant(4294967295)", ModeSpec.tryFloatQuant(-1).toString());
    assertEquals("TryIntMult(18446744073709551615)", ModeSpec.tryIntMult(-1).toString());
    assertEquals("TRY_CONSECUTIVE(3)", DeltaSpec.tryConsecutive(3).toString());
    assertEquals("TRY_CONV1(6)", DeltaSpec.tryConv1(6).toString());
    assertEquals("NO_OP", DeltaSpec.NO_OP.toString());
    assertEquals("EqualPagesUpTo(262144)", PagingSpec.DEFAULT.toString());
    assertEquals("Exact[1, 2]", PagingSpec.exact(1, 2).toString());
    assertTrue(new ChunkConfig().toString().contains("level=8"));
    assertEquals("u32", Latent.U32.toString());
  }

  @Test
  void configErrors() {
    ChunkConfig c = new ChunkConfig();
    PcoException e = assertThrows(PcoException.class, () -> Pco.compress(NumberType.I32, new long[] {1}, c.withCompressionLevel(13)));
    assertEquals(PcoException.Kind.INVALID_ARGUMENT, e.kind());
    assertThrows(PcoException.class, () -> Pco.compress(NumberType.U8, new long[] {1}, c));
    assertEquals(1, Pco.decompress(Pco.compress(NumberType.U8, new long[] {1}, c.withEnable8Bit(true)), NumberType.U8).nums.length);
    assertThrows(PcoException.class, () -> Pco.compress(NumberType.I64, new long[] {1}, c.withDeltaSpec(DeltaSpec.tryConv1(2))));
    assertThrows(PcoException.class, () -> Pco.compress(NumberType.I64, new long[] {1}, c.withDeltaSpec(DeltaSpec.tryConsecutive(8))));
    assertThrows(PcoException.class, () -> Pco.compress(NumberType.I64, new long[] {1}, c.withPagingSpec(PagingSpec.exact(2))));
    assertThrows(PcoException.class, () -> Pco.compress(NumberType.I64, new long[] {1, 2}, c.withPagingSpec(PagingSpec.exact(2, 0))));
    assertThrows(ArithmeticException.class, () -> PagingSpec.equalPagesUpTo(0).nPerPage(5));
    assertArrayEquals(new long[0], PagingSpec.equalPagesUpTo(0).nPerPage(0));
    assertArrayEquals(new long[] {4, 3, 3}, PagingSpec.equalPagesUpTo(4).nPerPage(10));
    assertThrows(PcoException.class, () -> Pco.compress(NumberType.F64, new long[] {1}, c.withModeSpec(ModeSpec.tryIntMult(3))));
    assertThrows(PcoException.class, () -> Pco.compress(NumberType.I64, new long[] {1}, c.withModeSpec(ModeSpec.tryFloatQuant(3))));
    assertThrows(PcoException.class, () -> Pco.chunkMeta(NumberType.I64, new long[0], c));
    assertThrows(PcoException.class, () -> Pco.chunkMeta(NumberType.I64, new long[(1 << 24) + 1], c.withModeSpec(ModeSpec.CLASSIC).withDeltaSpec(DeltaSpec.NO_OP)));
  }

  @Test
  void headerAndPeeking() {
    byte[] f = Pco.compress(new int[] {1, 2, 3}, new ChunkConfig());
    Pco.Header h = Pco.readHeader(f, 0, f.length);
    assertEquals(3, h.standaloneVersion);
    assertNull(h.uniformType);
    assertEquals(3, h.nHint);
    assertEquals(4, h.formatMajor);
    assertEquals(1, h.formatMinor);
    assertEquals(NumberType.I32, Pco.peekNumberType(f));
    byte[] bad = f.clone();
    bad[0] = 'x';
    assertEquals(PcoException.Kind.CORRUPTION, assertThrows(PcoException.class, () -> Pco.readHeader(bad, 0, bad.length)).kind());
    byte[] noChunks = java.util.Arrays.copyOf(f, h.bodyStart);
    assertEquals(PcoException.Kind.INSUFFICIENT_DATA, assertThrows(PcoException.class, () -> Pco.peekNumberType(noChunks)).kind());
    byte[] unknown = f.clone();
    unknown[h.bodyStart] = 99;
    assertThrows(PcoException.class, () -> Pco.peekNumberType(unknown));
    byte[] uniform = f.clone();
    uniform[5] = 3;
    assertEquals(NumberType.I32, Pco.peekNumberType(uniform));
    byte[] badUniform = f.clone();
    badUniform[5] = 99;
    assertThrows(PcoException.class, () -> Pco.readHeader(badUniform, 0, badUniform.length));
    byte[] mismatch = f.clone();
    mismatch[5] = 4;
    assertThrows(PcoException.class, () -> Pco.decompress(mismatch, NumberType.I32));
    byte[] future = f.clone();
    future[4] = 4;
    assertThrows(PcoException.class, () -> Pco.readHeader(future, 0, future.length));
    // the decoded numbers end at the footer; bytes after it are not read
    byte[] trailing = java.util.Arrays.copyOf(f, f.length + 3);
    assertEquals(f.length, Pco.decompress(trailing, NumberType.I32).end);
    assertEquals(NumberType.U16, NumberType.fromTypeByte(7));
    assertNull(NumberType.fromTypeByte(0));
    assertEquals(16, NumberType.F16.bits());
    assertTrue(NumberType.F16.isFloat());
    assertFalse(NumberType.U16.isFloat());
  }

  @Test
  void countBoundStopsBeforeDecoding() {
    byte[] f = Pco.compress(NumberType.I64, new long[1000], new ChunkConfig());
    Pco.CountExceeded e = assertThrows(Pco.CountExceeded.class, () -> Pco.decompress(f, 0, f.length, NumberType.I64, 999, (b, n) -> { }));
    assertEquals(1000, e.count);
    // over several chunks: the bound counts every chunk, and a bound of exactly the count holds
    byte[] g = Pco.compress(NumberType.I64, new long[1000], new ChunkConfig().withPagingSpec(PagingSpec.equalPagesUpTo(100)));
    long[] seen = new long[1];
    assertEquals(g.length, Pco.decompress(g, 0, g.length, NumberType.I64, 1000, (b, n) -> seen[0] += n));
    assertEquals(1000, seen[0]);
    Pco.CountExceeded e2 = assertThrows(Pco.CountExceeded.class, () -> Pco.decompress(g, 0, g.length, NumberType.I64, 999, (b, n) -> { }));
    assertEquals(1000, e2.count);
  }

  @Test
  void metadataInspection() {
    long[] nums = new long[2000];
    for (int i = 0; i < nums.length; i++) {
      nums[i] = i * 7L + 3;
    }
    ChunkMeta m = Pco.chunkMeta(NumberType.I64, nums, new ChunkConfig().withModeSpec(ModeSpec.CLASSIC));
    assertEquals(Mode.Kind.CLASSIC, m.mode.kind);
    assertEquals(DeltaEncoding.Kind.CONSECUTIVE, m.deltaEncoding.kind);
    assertEquals(1, m.deltaEncoding.order);
    assertNull(m.deltaVar());
    assertNull(m.secondaryVar());
    assertEquals(64, m.primaryVar().latentBits());
    assertTrue(m.primaryVar().bins.length >= 1);
    ChunkMeta im = Pco.chunkMeta(NumberType.I64, nums, new ChunkConfig().withModeSpec(ModeSpec.tryIntMult(7)).withDeltaSpec(DeltaSpec.NO_OP));
    assertEquals(7, im.mode.baseLatent());
    assertTrue(im.secondaryVar() != null);
    ChunkMeta dict = Pco.chunkMeta(NumberType.I64, new long[] {5, 5, 5, 9}, new ChunkConfig().withModeSpec(ModeSpec.TRY_DICT));
    assertEquals(Mode.Kind.DICT, dict.mode.kind);
    assertArrayEquals(new long[] {NumberType.I64.toLatentOrdered(5), NumberType.I64.toLatentOrdered(9)}, dict.mode.dictLatents());
    assertNull(Mode.CLASSIC.dictLatents());
    ChunkMeta conv = Pco.chunkMeta(NumberType.I32, nums, new ChunkConfig().withDeltaSpec(DeltaSpec.tryConv1(2)));
    if (conv.deltaEncoding.kind == DeltaEncoding.Kind.CONV1) {
      assertEquals(2, conv.deltaEncoding.conv1Weights().length);
      assertTrue(conv.deltaEncoding.conv1Bias() != Long.MIN_VALUE);
    }
    assertNull(DeltaEncoding.NO_OP.conv1Weights());
    assertEquals(Mode.floatQuant(3), Mode.floatQuant(3));
    assertEquals(Mode.floatQuant(3).hashCode(), Mode.floatQuant(3).hashCode());
    assertFalse(Mode.floatQuant(3).equals(Mode.floatQuant(4)));
    assertFalse(Mode.floatQuant(3).equals("FloatQuant(3)"));
    assertEquals("Dict(2)", dict.mode.kind == Mode.Kind.DICT ? "Dict(" + dict.mode.dictLatents().length + ")" : "");
  }

  @Test
  void bitWriterRefusesUnalignedBytes() {
    BitWriter w = new BitWriter();
    w.writeUint(1, 3);
    assertThrows(PcoException.class, () -> w.writeAlignedBytes(new byte[] {1}));
    BitWriter big = new BitWriter();
    for (int i = 0; i < 100000; i++) {
      big.writeUint(i & 0xff, 8);
    }
    byte[] b = big.toByteArray();
    assertEquals(100000, b.length);
    assertEquals((byte) 99999, b[99999]);
    BitReader r = new BitReader(b, 0, b.length);
    r.readUint(3);
    assertThrows(PcoException.class, () -> r.readAlignedBytes(1));
  }
}
