package io.aperiodic.pcodec;

import io.aperiodic.pcodec.payload.ZstdCodec;
import io.airlift.compress.zstd.ZstdCompressor;
import io.airlift.compress.zstd.ZstdDecompressor;

/** A pure Java ZstdCodec (aircompressor) for the ZSTD-mode vectors. */
final class AircompressorZstd implements ZstdCodec {
  @Override
  public byte[] compress(byte[] src, int off, int len, int level) {
    ZstdCompressor c = new ZstdCompressor();
    byte[] out = new byte[c.maxCompressedLength(len)];
    int n = c.compress(src, off, len, out, 0, out.length);
    return java.util.Arrays.copyOf(out, n);
  }

  @Override
  public int decompress(byte[] src, int off, int len, byte[] dst, int dstOff, int dstLen) {
    byte[] tmp = new byte[dstLen + 1];
    int n;
    try {
      n = new ZstdDecompressor().decompress(src, off, len, tmp, 0, tmp.length);
    } catch (RuntimeException e) {
      if (String.valueOf(e.getMessage()).contains("Output buffer too small")) {
        return -1;
      }
      throw e;
    }
    if (n > dstLen) {
      return -1;
    }
    System.arraycopy(tmp, 0, dst, dstOff, n);
    return n;
  }
}
