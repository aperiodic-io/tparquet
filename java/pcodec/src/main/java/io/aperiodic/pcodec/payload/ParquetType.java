package io.aperiodic.pcodec.payload;

import io.aperiodic.pcodec.NumberType;

/** The Parquet physical types PCODEC models, with their Parquet {@code Type} values (dtype). */
public enum ParquetType {
  INT32(1, 4, NumberType.I32),
  INT64(2, 8, NumberType.I64),
  FLOAT(4, 4, NumberType.F32),
  DOUBLE(5, 8, NumberType.F64);

  /** the Parquet {@code Type} value, the payload's dtype byte */
  public final int dtype;
  /** bytes per value in PLAIN encoding */
  public final int width;

  final NumberType numberType;

  ParquetType(int dtype, int width, NumberType numberType) {
    this.dtype = dtype;
    this.width = width;
    this.numberType = numberType;
  }

  /** The type of a Parquet {@code Type} value, or null for a type PCODEC does not model. */
  public static ParquetType fromParquet(int t) {
    for (ParquetType p : values()) {
      if (p.dtype == t) {
        return p;
      }
    }
    return null;
  }
}
