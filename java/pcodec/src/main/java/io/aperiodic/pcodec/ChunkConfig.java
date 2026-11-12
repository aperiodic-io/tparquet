package io.aperiodic.pcodec;

/** All configuration of the compressor ({@code pco::ChunkConfig}); immutable. */
public final class ChunkConfig {
  public static final int DEFAULT_COMPRESSION_LEVEL = 8;
  public static final int MAX_COMPRESSION_LEVEL = 12;

  public final int compressionLevel;
  public final ModeSpec modeSpec;
  public final DeltaSpec deltaSpec;
  public final PagingSpec pagingSpec;
  public final boolean enable8Bit;

  public ChunkConfig() {
    this(DEFAULT_COMPRESSION_LEVEL, ModeSpec.AUTO, DeltaSpec.AUTO, PagingSpec.DEFAULT, false);
  }

  private ChunkConfig(
      int compressionLevel,
      ModeSpec modeSpec,
      DeltaSpec deltaSpec,
      PagingSpec pagingSpec,
      boolean enable8Bit) {
    this.compressionLevel = compressionLevel;
    this.modeSpec = modeSpec;
    this.deltaSpec = deltaSpec;
    this.pagingSpec = pagingSpec;
    this.enable8Bit = enable8Bit;
  }

  public ChunkConfig withCompressionLevel(int level) {
    return new ChunkConfig(level, modeSpec, deltaSpec, pagingSpec, enable8Bit);
  }

  public ChunkConfig withModeSpec(ModeSpec spec) {
    return new ChunkConfig(compressionLevel, spec, deltaSpec, pagingSpec, enable8Bit);
  }

  public ChunkConfig withDeltaSpec(DeltaSpec spec) {
    return new ChunkConfig(compressionLevel, modeSpec, spec, pagingSpec, enable8Bit);
  }

  public ChunkConfig withPagingSpec(PagingSpec spec) {
    return new ChunkConfig(compressionLevel, modeSpec, deltaSpec, spec, enable8Bit);
  }

  public ChunkConfig withEnable8Bit(boolean enable) {
    return new ChunkConfig(compressionLevel, modeSpec, deltaSpec, pagingSpec, enable);
  }

  void validate(Latent latent) {
    if (Integer.compareUnsigned(compressionLevel, MAX_COMPRESSION_LEVEL) > 0) {
      throw PcoException.invalidArgument(
          "compression level may not exceed "
              + MAX_COMPRESSION_LEVEL
              + " (was "
              + Integer.toUnsignedString(compressionLevel)
              + ")");
    }
    if (deltaSpec.kind == DeltaSpec.Kind.TRY_CONSECUTIVE
        && Long.compareUnsigned(deltaSpec.order, DeltaEncoding.MAX_CONSECUTIVE_ORDER) > 0) {
      throw PcoException.invalidArgument(
          "consecutive delta order may not exceed " + DeltaEncoding.MAX_CONSECUTIVE_ORDER);
    }
    if (deltaSpec.kind == DeltaSpec.Kind.TRY_CONV1) {
      if (Long.compareUnsigned(deltaSpec.order, DeltaEncoding.MAX_CONV1_ORDER) > 0) {
        throw PcoException.invalidArgument(
            "conv1 delta order may not exceed " + DeltaEncoding.MAX_CONV1_ORDER);
      }
      if (latent == Latent.U64) {
        throw PcoException.invalidArgument(
            "Conv1 delta encoding is only supported for types with 32 or fewer bits");
      }
    }
    if (latent == Latent.U8 && !enable8Bit) {
      throw PcoException.invalidArgument(
          "compressing 8-bit types with Pco is often a mistake; "
              + "enable them on the ChunkConfig if you know what you're doing");
    }
  }

  @Override
  public String toString() {
    return "ChunkConfig{level="
        + compressionLevel
        + ", mode="
        + modeSpec
        + ", delta="
        + deltaSpec
        + ", paging="
        + pagingSpec
        + ", enable_8_bit="
        + enable8Bit
        + "}";
  }
}
