package io.aperiodic.pcodec;

/**
 * The number types of pcodec, with their type bytes in the standalone format.
 *
 * <p>Numbers are passed around as their raw bits in a {@code long}, zero-extended from the type's
 * width: an {@code int} {@code -1} is {@code 0xffffffffL}, a {@code float} is its {@link
 * Float#floatToRawIntBits} zero-extended. The {@code Pco} entry points convert Java arrays.
 */
public enum NumberType {
  U32(1, Kind.UNSIGNED, Latent.U32),
  U64(2, Kind.UNSIGNED, Latent.U64),
  I32(3, Kind.SIGNED, Latent.U32),
  I64(4, Kind.SIGNED, Latent.U64),
  F32(5, Kind.FLOAT, Latent.U32),
  F64(6, Kind.FLOAT, Latent.U64),
  U16(7, Kind.UNSIGNED, Latent.U16),
  I16(8, Kind.SIGNED, Latent.U16),
  F16(9, Kind.FLOAT, Latent.U16),
  U8(10, Kind.UNSIGNED, Latent.U8),
  I8(11, Kind.SIGNED, Latent.U8);

  enum Kind {
    UNSIGNED,
    SIGNED,
    FLOAT
  }

  /** the type byte of the standalone format ({@code NUMBER_TYPE_BYTE}) */
  public final int typeByte;

  final Kind kind;
  final Latent latent;

  NumberType(int typeByte, Kind kind, Latent latent) {
    this.typeByte = typeByte;
    this.kind = kind;
    this.latent = latent;
  }

  /** width in bits */
  public int bits() {
    return latent.bits;
  }

  public boolean isFloat() {
    return kind == Kind.FLOAT;
  }

  /** The number type of a type byte, or {@code null}. */
  public static NumberType fromTypeByte(int b) {
    for (NumberType t : values()) {
      if (t.typeByte == b) {
        return t;
      }
    }
    return null;
  }

  /** {@code to_latent_ordered}: an order-preserving map to the unsigned latent. */
  long toLatentOrdered(long raw) {
    switch (kind) {
      case UNSIGNED:
        return raw;
      case SIGNED:
        // self.wrapping_sub(MIN) as latent
        return raw ^ latent.mid;
      default:
        {
          long flipAll = (latent.mid & raw) != 0 ? latent.mask : 0;
          return raw ^ (flipAll | latent.mid);
        }
    }
  }

  /** {@code from_latent_ordered}, the inverse of {@link #toLatentOrdered}. */
  long fromLatentOrdered(long l) {
    switch (kind) {
      case UNSIGNED:
        return l;
      case SIGNED:
        return l ^ latent.mid;
      default:
        {
          long flipAll = (latent.mid & l) != 0 ? 0 : latent.mask;
          return l ^ (flipAll | latent.mid);
        }
    }
  }
}
