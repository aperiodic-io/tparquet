package io.aperiodic.pcodec;

import java.util.Arrays;

/** How a chunk splits numbers into latents ({@code pco::metadata::Mode}). */
public final class Mode {
  public enum Kind {
    CLASSIC,
    INT_MULT,
    FLOAT_MULT,
    FLOAT_QUANT,
    DICT
  }

  static final int BITS_VARIANT = 4;
  static final int BITS_QUANTIZE_K = 8;
  static final int BITS_DICT_LEN = 25;

  static final Mode CLASSIC = new Mode(Kind.CLASSIC, 0, 0, null);

  public final Kind kind;
  /** IntMult: the base latent; FloatMult: the base's ordered latent */
  final long base;
  /** FloatQuant: k */
  public final int k;
  /** Dict: the dictionary's latents */
  final long[] dict;

  private Mode(Kind kind, long base, int k, long[] dict) {
    this.kind = kind;
    this.base = base;
    this.k = k;
    this.dict = dict;
  }

  static Mode intMult(long base) {
    return new Mode(Kind.INT_MULT, base, 0, null);
  }

  static Mode floatMult(long baseLatentOrdered) {
    return new Mode(Kind.FLOAT_MULT, baseLatentOrdered, 0, null);
  }

  static Mode floatQuant(int k) {
    return new Mode(Kind.FLOAT_QUANT, 0, k, null);
  }

  static Mode dict(long[] dict) {
    return new Mode(Kind.DICT, 0, 0, dict);
  }

  public long baseLatent() {
    return base;
  }

  public long[] dictLatents() {
    return dict == null ? null : dict.clone();
  }

  Latent primaryLatentType(Latent numberLatent) {
    return kind == Kind.DICT ? Latent.U32 : numberLatent;
  }

  Latent secondaryLatentType(Latent numberLatent) {
    return kind == Kind.CLASSIC || kind == Kind.DICT ? null : numberLatent;
  }

  int maxBitSize(Latent numberLatent) {
    int payload;
    switch (kind) {
      case CLASSIC:
        payload = 0;
        break;
      case DICT:
        payload = BITS_DICT_LEN + 7 + dict.length * numberLatent.bits;
        break;
      case FLOAT_QUANT:
        payload = BITS_QUANTIZE_K;
        break;
      default:
        payload = numberLatent.bits;
        break;
    }
    return BITS_VARIANT + payload;
  }

  void writeTo(BitWriter w, Latent numberLatent) {
    w.writeUint(kind.ordinal(), BITS_VARIANT);
    switch (kind) {
      case CLASSIC:
        break;
      case INT_MULT:
      case FLOAT_MULT:
        w.writeUint(base, numberLatent.bits);
        break;
      case FLOAT_QUANT:
        w.writeUint(k, BITS_QUANTIZE_K);
        break;
      default:
        w.writeUint(dict.length, BITS_DICT_LEN);
        w.finishByte();
        for (long x : dict) {
          w.writeUint(x, numberLatent.bits);
        }
        break;
    }
  }

  static Mode readFrom(BitReader r, int formatMajor, Latent numberLatent) {
    int variant = r.readInt(BITS_VARIANT);
    Mode mode;
    switch (variant) {
      case 0:
        mode = CLASSIC;
        break;
      case 1:
        if (formatMajor == 0) {
          throw PcoException.corruption(
              "unable to decompress data from yanked v0.0.0 of pco with different GCD encoding");
        }
        mode = intMult(r.readUint(numberLatent.bits));
        break;
      case 2:
        mode = floatMult(r.readUint(numberLatent.bits));
        break;
      case 3:
        mode = floatQuant(r.readInt(BITS_QUANTIZE_K));
        break;
      case 4:
        {
          int nUnique = r.readInt(BITS_DICT_LEN);
          r.drainEmptyByte("expected zeros between dict mode length and values");
          r.checkInBounds();
          long[] dict = new long[0];
          // read in batches of 512, each bounds-checked, so a length the data does not back
          // allocates no more than the data does
          int read = 0;
          while (read < nUnique) {
            int end = Math.min(read + 512, nUnique);
            if (end > dict.length) {
              dict = Arrays.copyOf(dict, Math.min(nUnique, Math.max(end, dict.length * 2)));
            }
            for (int i = read; i < end; i++) {
              dict[i] = r.readUint(numberLatent.bits);
            }
            r.checkInBounds();
            read = end;
          }
          return dict(dict.length == nUnique ? dict : Arrays.copyOf(dict, nUnique));
        }
      default:
        throw PcoException.corruption("unknown mode variant " + variant);
    }
    r.checkInBounds();
    return mode;
  }

  @Override
  public boolean equals(Object o) {
    if (!(o instanceof Mode)) {
      return false;
    }
    Mode m = (Mode) o;
    return kind == m.kind && base == m.base && k == m.k && Arrays.equals(dict, m.dict);
  }

  @Override
  public int hashCode() {
    return kind.hashCode() * 31 + Long.hashCode(base) + k;
  }

  @Override
  public String toString() {
    switch (kind) {
      case CLASSIC:
        return "Classic";
      case INT_MULT:
        return "IntMult(" + Long.toUnsignedString(base) + ")";
      case FLOAT_MULT:
        return "FloatMult(" + Long.toUnsignedString(base) + ")";
      case FLOAT_QUANT:
        return "FloatQuant(" + k + ")";
      default:
        return "Dict(" + dict.length + ")";
    }
  }
}
