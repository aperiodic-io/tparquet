package io.aperiodic.tparquet.trino;

import io.trino.parquet.metadata.ColumnChunkMetadata;
import io.trino.parquet.metadata.ParquetMetadata;
import org.apache.parquet.format.DataPageHeaderV2;
import org.apache.parquet.format.PageHeader;
import org.apache.parquet.format.PageType;
import org.apache.parquet.format.Util;
import org.apache.parquet.hadoop.metadata.CompressionCodecName;

import java.io.ByteArrayInputStream;
import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.List;

/** The PCODEC payloads of a file's pages, and damaged copies of files. */
final class Corrupt
{
    private Corrupt() {}

    /** Where a page's payload (the compressed part) is in the file. */
    record Payload(String column, PageType type, int offset, int length) {}

    /**
     * The payload of every page of every PCODEC column chunk of {@code file} that a reader decodes:
     * not those of expected size 0 (a V2 page whose values are all null, an empty dictionary page),
     * which Trino's reader, like arrow-rs's, does not decode (spec section 3, "Expected size 0").
     */
    static List<Payload> payloads(Path file)
            throws IOException
    {
        byte[] bytes = Files.readAllBytes(file);
        ParquetMetadata footer = TrinoParquet.footer(file);
        List<Payload> out = new ArrayList<>();
        for (var block : footer.getBlocks()) {
            if (block.rowCount() == 0) {
                // a row group without rows (the empty vector) has no pages to damage
                continue;
            }
            for (ColumnChunkMetadata chunk : block.columns()) {
                if (chunk.getCodec() != CompressionCodecName.PCODEC) {
                    // the vectors' ZSTD string columns
                    continue;
                }
                long start = chunk.getDictionaryPageOffset() > 0 ? Math.min(chunk.getDictionaryPageOffset(), chunk.getFirstDataPageOffset()) : chunk.getFirstDataPageOffset();
                int end = (int) (start + chunk.getTotalSize());
                ByteArrayInputStream in = new ByteArrayInputStream(bytes, (int) start, (int) chunk.getTotalSize());
                while (in.available() > 0) {
                    PageHeader h;
                    try {
                        h = Util.readPageHeader(in);
                    }
                    catch (IOException e) {
                        throw new IOException("%s, %s at %s of [%s, %s): %s".formatted(file, chunk.getPath(), end - in.available(), start, end, chunk), e);
                    }
                    int at = end - in.available();
                    int levels = 0;
                    if (h.getType() == PageType.DATA_PAGE_V2) {
                        DataPageHeaderV2 v2 = h.getData_page_header_v2();
                        levels = v2.getRepetition_levels_byte_length() + v2.getDefinition_levels_byte_length();
                    }
                    boolean compressed = h.getType() != PageType.DATA_PAGE_V2 || h.getData_page_header_v2().isIs_compressed();
                    boolean decoded = h.getUncompressed_page_size() > levels;
                    if (compressed && decoded && h.getCompressed_page_size() > levels) {
                        out.add(new Payload(chunk.getPath().toDotString(), h.getType(), at + levels, h.getCompressed_page_size() - levels));
                    }
                    in.skipNBytes(h.getCompressed_page_size());
                }
            }
        }
        return out;
    }

    /** A copy of {@code file} whose first payload of {@code column} has version 2 (an unknown payload version). */
    static void damageFirstPayload(Path file, String column, Path copy)
            throws IOException
    {
        Payload p = payloads(file).stream().filter(x -> x.column().equals(column)).findFirst().orElseThrow();
        byte[] bytes = Files.readAllBytes(file);
        if (bytes[p.offset()] != 1) {
            throw new IllegalStateException("not a PCODEC payload at " + p);
        }
        bytes[p.offset()] = 2;
        Files.write(copy, bytes);
    }
}
