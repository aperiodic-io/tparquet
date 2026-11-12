package io.aperiodic.pcodec.payload;

/**
 * A Zstandard implementation for ZSTD mode (FORMAT.md 4.4), which this library leaves to the
 * caller: Zstandard output depends on the implementation, and the upstream profile of the codec
 * (spec/UPSTREAM.md) drops the mode. Without one, the encoder writes RAW where the reference
 * writes ZSTD, and ZSTD payloads cannot be decoded.
 */
public interface ZstdCodec {
  /** One or more frames holding {@code src[off, off + len)}, at the given level. */
  byte[] compress(byte[] src, int off, int len, int level);

  /**
   * Decodes the frames in {@code src[off, off + len)} into {@code dst[dstOff, dstOff + dstLen)},
   * producing no more than {@code dstLen} bytes; returns the number of bytes the frames hold, or
   * -1 if they hold more than {@code dstLen}. Throws on malformed data.
   */
  int decompress(byte[] src, int off, int len, byte[] dst, int dstOff, int dstLen);
}
