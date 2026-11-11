package io.aperiodic.tparquet.trino;

import io.trino.parquet.AbstractParquetDataSource;
import io.trino.parquet.Column;
import io.trino.parquet.Field;
import io.trino.parquet.ParquetDataSource;
import io.trino.parquet.ParquetDataSourceId;
import io.trino.parquet.ParquetReaderOptions;
import io.trino.parquet.metadata.BlockMetadata;
import io.trino.parquet.metadata.FileMetadata;
import io.trino.parquet.metadata.ParquetMetadata;
import io.trino.parquet.reader.MetadataReader;
import io.trino.parquet.reader.ParquetReader;
import io.trino.parquet.reader.RowGroupInfo;
import io.trino.spi.Page;
import io.trino.spi.connector.SourcePage;
import io.trino.spi.type.ArrayType;
import io.trino.spi.type.DecimalType;
import io.trino.spi.type.MapType;
import io.trino.spi.type.RowType;
import io.trino.spi.type.TimeType;
import io.trino.spi.type.TimestampType;
import io.trino.spi.type.TimestampWithTimeZoneType;
import io.trino.spi.type.Type;
import io.trino.spi.type.TypeOperators;
import org.apache.parquet.column.ColumnDescriptor;
import org.apache.parquet.io.MessageColumnIO;
import org.apache.parquet.schema.GroupType;
import org.apache.parquet.schema.LogicalTypeAnnotation;
import org.apache.parquet.schema.LogicalTypeAnnotation.DecimalLogicalTypeAnnotation;
import org.apache.parquet.schema.LogicalTypeAnnotation.TimeLogicalTypeAnnotation;
import org.apache.parquet.schema.LogicalTypeAnnotation.TimestampLogicalTypeAnnotation;
import org.apache.parquet.schema.MessageType;
import org.apache.parquet.schema.PrimitiveType;
import org.joda.time.DateTimeZone;

import java.io.IOException;
import java.io.RandomAccessFile;
import java.io.UncheckedIOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.Optional;

import static io.trino.memory.context.AggregatedMemoryContext.newSimpleAggregatedMemoryContext;
import static io.trino.parquet.ParquetTypeUtils.constructField;
import static io.trino.parquet.ParquetTypeUtils.getColumnIO;
import static io.trino.parquet.ParquetTypeUtils.getDescriptors;
import static io.trino.parquet.ParquetTypeUtils.lookupColumnByName;
import static io.trino.parquet.metadata.PrunedBlockMetadata.createPrunedColumnsMetadata;
import static io.trino.spi.type.BigintType.BIGINT;
import static io.trino.spi.type.BooleanType.BOOLEAN;
import static io.trino.spi.type.DateType.DATE;
import static io.trino.spi.type.DoubleType.DOUBLE;
import static io.trino.spi.type.IntegerType.INTEGER;
import static io.trino.spi.type.RealType.REAL;
import static io.trino.spi.type.VarbinaryType.VARBINARY;
import static io.trino.spi.type.VarcharType.VARCHAR;

/** Reads Parquet files with Trino's reader (trino-parquet) into Trino pages, as a connector does. */
final class TrinoParquet
{
    private TrinoParquet() {}

    private static final TypeOperators TYPE_OPERATORS = new TypeOperators();

    /** A local file, read as Trino's file systems read it: by position. */
    static final class LocalDataSource
            extends AbstractParquetDataSource
    {
        private final RandomAccessFile file;

        LocalDataSource(Path path)
                throws IOException
        {
            super(new ParquetDataSourceId(path.toString()), Files.size(path), ParquetReaderOptions.defaultOptions());
            this.file = new RandomAccessFile(path.toFile(), "r");
        }

        @Override
        protected void readInternal(long position, byte[] buffer, int bufferOffset, int bufferLength)
                throws IOException
        {
            file.seek(position);
            file.readFully(buffer, bufferOffset, bufferLength);
        }

        @Override
        public void close()
                throws IOException
        {
            file.close();
        }
    }

    /** The rows of a file: its top-level column names and Trino types, and the pages read. */
    record Table(List<String> names, List<Type> types, List<Page> pages)
    {
        int rows()
        {
            return pages.stream().mapToInt(Page::getPositionCount).sum();
        }

        /** Every row as canonical JSON (testdata/README.md). */
        List<String> lines()
        {
            List<String> out = new ArrayList<>();
            for (Page page : pages) {
                for (int row = 0; row < page.getPositionCount(); row++) {
                    out.add(TrinoCanonical.line(names, types, page, row));
                }
            }
            return out;
        }
    }

    /** The Trino type a connector maps a Parquet column to (the types of the conformance vectors). */
    static Type trinoType(org.apache.parquet.schema.Type parquet)
    {
        LogicalTypeAnnotation logical = parquet.getLogicalTypeAnnotation();
        if (!parquet.isPrimitive()) {
            GroupType group = parquet.asGroupType();
            if (logical instanceof LogicalTypeAnnotation.ListLogicalTypeAnnotation) {
                // LIST > repeated group list > element
                org.apache.parquet.schema.Type repeated = group.getType(0);
                org.apache.parquet.schema.Type element = repeated.isPrimitive() || repeated.asGroupType().getFieldCount() != 1
                        ? repeated : repeated.asGroupType().getType(0);
                return new ArrayType(trinoType(element));
            }
            if (logical instanceof LogicalTypeAnnotation.MapLogicalTypeAnnotation) {
                // MAP > repeated group entries > key, value
                GroupType entries = group.getType(0).asGroupType();
                return new MapType(trinoType(entries.getType(0)), trinoType(entries.getType(1)), TYPE_OPERATORS);
            }
            List<RowType.Field> fields = new ArrayList<>();
            for (org.apache.parquet.schema.Type f : group.getFields()) {
                fields.add(RowType.field(f.getName(), trinoType(f)));
            }
            return RowType.from(fields);
        }
        PrimitiveType p = parquet.asPrimitiveType();
        if (logical instanceof DecimalLogicalTypeAnnotation d) {
            return DecimalType.createDecimalType(d.getPrecision(), d.getScale());
        }
        if (logical instanceof TimestampLogicalTypeAnnotation t) {
            int precision = switch (t.getUnit()) {
                case MILLIS -> 3;
                case MICROS -> 6;
                case NANOS -> 9;
            };
            return t.isAdjustedToUTC() ? TimestampWithTimeZoneType.createTimestampWithTimeZoneType(precision) : TimestampType.createTimestampType(precision);
        }
        if (logical instanceof TimeLogicalTypeAnnotation t) {
            return TimeType.createTimeType(switch (t.getUnit()) {
                case MILLIS -> 3;
                case MICROS -> 6;
                case NANOS -> 9;
            });
        }
        if (logical instanceof LogicalTypeAnnotation.DateLogicalTypeAnnotation) {
            return DATE;
        }
        return switch (p.getPrimitiveTypeName()) {
            case INT32 -> INTEGER;
            case INT64 -> BIGINT;
            case FLOAT -> REAL;
            case DOUBLE -> DOUBLE;
            case BOOLEAN -> BOOLEAN;
            case BINARY -> logical instanceof LogicalTypeAnnotation.StringLogicalTypeAnnotation ? VARCHAR : VARBINARY;
            default -> throw new IllegalArgumentException("no Trino type for " + parquet);
        };
    }

    static ParquetMetadata footer(Path file)
            throws IOException
    {
        try (ParquetDataSource source = new LocalDataSource(file)) {
            return MetadataReader.readFooter(source, Optional.empty());
        }
    }

    /** Every top-level column of {@code file}, with the types of {@link #trinoType}. */
    static Table read(Path file)
            throws IOException
    {
        try (ParquetDataSource source = new LocalDataSource(file)) {
            ParquetMetadata metadata = MetadataReader.readFooter(source, Optional.empty());
            FileMetadata fileMetaData = metadata.getFileMetaData();
            MessageType schema = fileMetaData.getSchema();
            List<String> names = new ArrayList<>();
            List<Type> types = new ArrayList<>();
            for (org.apache.parquet.schema.Type f : schema.getFields()) {
                names.add(f.getName());
                types.add(trinoType(f));
            }
            return read(source, metadata, names, types);
        }
    }

    /** The named columns of a file, read as {@code types}. */
    static Table read(ParquetDataSource source, ParquetMetadata metadata, List<String> names, List<Type> types)
            throws IOException
    {
        FileMetadata fileMetaData = metadata.getFileMetaData();
        MessageType schema = fileMetaData.getSchema();
        MessageColumnIO columnIO = getColumnIO(schema, schema);
        List<Column> columns = new ArrayList<>();
        for (int i = 0; i < names.size(); i++) {
            Field field = constructField(types.get(i), lookupColumnByName(columnIO, names.get(i))).orElseThrow();
            columns.add(new Column(names.get(i), field));
        }
        Map<List<String>, ColumnDescriptor> descriptors = getDescriptors(schema, schema);
        List<RowGroupInfo> rowGroups = new ArrayList<>();
        long start = 0;
        for (BlockMetadata block : metadata.getBlocks()) {
            // as the connectors' PredicateUtils.getFilteredRowGroups: a row group without rows is
            // never read (its column chunks can be empty, which ParquetReader does not accept)
            if (block.rowCount() > 0) {
                rowGroups.add(new RowGroupInfo(createPrunedColumnsMetadata(block, source.getId(), descriptors), start, Optional.empty()));
            }
            start += block.rowCount();
        }
        List<Page> pages = new ArrayList<>();
        try (ParquetReader reader = new ParquetReader(
                Optional.ofNullable(fileMetaData.getCreatedBy()),
                columns,
                false,
                rowGroups,
                source,
                DateTimeZone.UTC,
                newSimpleAggregatedMemoryContext(),
                ParquetReaderOptions.defaultOptions(),
                exception -> exception instanceof RuntimeException r ? r : new RuntimeException(exception),
                Optional.empty(),
                Optional.empty(),
                Optional.empty())) {
            for (SourcePage page = reader.nextPage(); page != null; page = reader.nextPage()) {
                pages.add(page.getPage());
            }
        }
        return new Table(names, types, pages);
    }

    static List<String> lines(Path file)
    {
        try {
            return read(file).lines();
        }
        catch (IOException e) {
            throw new UncheckedIOException(e);
        }
    }
}
