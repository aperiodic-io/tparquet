package io.aperiodic.tparquet.trino;

import io.aperiodic.tparquet.Canonical;
import io.trino.spi.Page;
import io.trino.spi.block.Block;
import io.trino.spi.block.IntArrayBlock;
import io.trino.spi.block.LongArrayBlock;
import io.trino.spi.block.SqlMap;
import io.trino.spi.block.SqlRow;
import io.trino.spi.type.ArrayType;
import io.trino.spi.type.BigintType;
import io.trino.spi.type.BooleanType;
import io.trino.spi.type.DateType;
import io.trino.spi.type.DecimalType;
import io.trino.spi.type.DoubleType;
import io.trino.spi.type.Int128;
import io.trino.spi.type.IntegerType;
import io.trino.spi.type.LongTimestampWithTimeZone;
import io.trino.spi.type.MapType;
import io.trino.spi.type.RealType;
import io.trino.spi.type.RowType;
import io.trino.spi.type.TimeType;
import io.trino.spi.type.TimestampType;
import io.trino.spi.type.TimestampWithTimeZoneType;
import io.trino.spi.type.Type;
import io.trino.spi.type.VarbinaryType;
import io.trino.spi.type.VarcharType;

import java.math.BigInteger;
import java.util.Base64;
import java.util.List;

import static io.trino.spi.type.DateTimeEncoding.unpackMillisUtc;

/**
 * Trino values as canonical JSON (testdata/README.md): what was stored, not what Trino shows.
 * Timestamps are their stored integer in the column's unit (the UTC instant for those with a time
 * zone), times their stored microseconds, decimals their unscaled integer, floats their bits.
 */
final class TrinoCanonical
{
    private TrinoCanonical() {}

    static String line(List<String> names, List<Type> types, Page page, int row)
    {
        StringBuilder out = new StringBuilder("{");
        for (int c = 0; c < names.size(); c++) {
            if (c > 0) {
                out.append(',');
            }
            out.append(Canonical.string(names.get(c))).append(':');
            value(out, types.get(c), page.getBlock(c), row);
        }
        return out.append('}').toString();
    }

    private static long rawLong(Block block, int position)
    {
        return ((LongArrayBlock) block.getUnderlyingValueBlock()).getLong(block.getUnderlyingValuePosition(position));
    }

    private static int rawInt(Block block, int position)
    {
        return ((IntArrayBlock) block.getUnderlyingValueBlock()).getInt(block.getUnderlyingValuePosition(position));
    }

    static void value(StringBuilder out, Type type, Block block, int position)
    {
        if (block.isNull(position)) {
            out.append("null");
            return;
        }
        switch (type) {
            case BigintType t -> out.append(t.getLong(block, position));
            case IntegerType t -> out.append(t.getInt(block, position));
            case DateType t -> out.append(t.getInt(block, position));
            case BooleanType t -> out.append(t.getBoolean(block, position));
            case RealType _ -> out.append('"').append(String.format("0x%08x", rawInt(block, position))).append('"');
            case DoubleType _ -> out.append('"').append(String.format("0x%016x", rawLong(block, position))).append('"');
            case VarcharType t -> out.append(Canonical.string(t.getSlice(block, position).toStringUtf8()));
            case VarbinaryType t -> out.append('"').append(Base64.getEncoder().encodeToString(t.getSlice(block, position).getBytes())).append('"');
            case DecimalType t when t.isShort() -> out.append(t.getLong(block, position));
            case DecimalType t -> out.append(((Int128) t.getObject(block, position)).toBigInteger());
            case TimestampType t when t.isShort() -> {
                // epoch microseconds; the stored value in the column's unit
                long micros = t.getLong(block, position);
                out.append(switch (t.getPrecision()) {
                    case 3 -> micros / 1000;
                    case 6 -> micros;
                    default -> throw new IllegalArgumentException(t.toString());
                });
            }
            case TimestampWithTimeZoneType t when t.isShort() -> {
                long millis = unpackMillisUtc(t.getLong(block, position));
                out.append(switch (t.getPrecision()) {
                    case 3 -> millis;
                    default -> throw new IllegalArgumentException(t.toString());
                });
            }
            case TimestampWithTimeZoneType t -> {
                LongTimestampWithTimeZone v = (LongTimestampWithTimeZone) t.getObject(block, position);
                BigInteger picos = BigInteger.valueOf(v.getEpochMillis()).multiply(BigInteger.valueOf(1_000_000_000)).add(BigInteger.valueOf(v.getPicosOfMilli()));
                BigInteger unit = BigInteger.TEN.pow(12 - t.getPrecision());
                out.append(picos.divide(unit));
            }
            case TimeType t -> {
                long picos = t.getLong(block, position);
                out.append(picos / (long) Math.pow(10, 12 - t.getPrecision()));
            }
            case ArrayType t -> {
                Block elements = t.getObject(block, position);
                out.append('[');
                for (int i = 0; i < elements.getPositionCount(); i++) {
                    if (i > 0) {
                        out.append(',');
                    }
                    value(out, t.getElementType(), elements, i);
                }
                out.append(']');
            }
            case MapType t -> {
                // the entries in stored order; Trino's map type has no names for an entry's fields,
                // and the vectors' maps have Parquet's standard ones, key and value
                SqlMap map = t.getObject(block, position);
                out.append('[');
                for (int i = 0; i < map.getSize(); i++) {
                    if (i > 0) {
                        out.append(',');
                    }
                    out.append("{\"key\":");
                    value(out, t.getKeyType(), map.getRawKeyBlock(), map.getRawOffset() + i);
                    out.append(",\"value\":");
                    value(out, t.getValueType(), map.getRawValueBlock(), map.getRawOffset() + i);
                    out.append('}');
                }
                out.append(']');
            }
            case RowType t -> {
                SqlRow row = t.getObject(block, position);
                out.append('{');
                List<RowType.Field> fields = t.getFields();
                for (int i = 0; i < fields.size(); i++) {
                    if (i > 0) {
                        out.append(',');
                    }
                    out.append(Canonical.string(fields.get(i).getName().orElseThrow())).append(':');
                    value(out, fields.get(i).getType(), row.getRawFieldBlock(i), row.getRawIndex());
                }
                out.append('}');
            }
            default -> throw new IllegalArgumentException("no canonical form for " + type);
        }
    }
}
