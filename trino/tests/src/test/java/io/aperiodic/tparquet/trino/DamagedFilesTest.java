package io.aperiodic.tparquet.trino;

import io.trino.parquet.ParquetCorruptionException;
import net.jqwik.api.Arbitraries;
import net.jqwik.api.Arbitrary;
import net.jqwik.api.Example;
import net.jqwik.api.ForAll;
import net.jqwik.api.Property;
import net.jqwik.api.Provide;
import net.jqwik.api.constraints.IntRange;
import net.jqwik.api.lifecycle.AfterContainer;
import net.jqwik.api.lifecycle.BeforeContainer;

import java.io.IOException;
import java.io.UncheckedIOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Comparator;
import java.util.List;
import java.util.stream.Stream;

import static org.assertj.core.api.Assertions.assertThat;

/**
 * Payloads are untrusted (spec section 7): PCODEC vectors with damaged page payloads, read by
 * Trino's reader, give rows or an exception, never an Error (a crash, an out-of-memory, a stack
 * overflow). A payload has no checksum, so damage can decode to other values, as with any codec
 * without page checksums; damage to a payload's header is always a ParquetCorruptionException.
 */
class DamagedFilesTest
{
    static Path dir;

    /** A vector and the payloads of its pages. */
    record Vector(String name, Path file, byte[] bytes, List<Corrupt.Payload> payloads)
    {
        @Override
        public String toString()
        {
            return name;
        }
    }

    static List<Vector> vectors;

    @BeforeContainer
    static void load()
            throws IOException
    {
        dir = Files.createTempDirectory("tparquet-damaged");
        // but those Trino cannot open, whatever their codec (Testdata.trinoRefuses)
        vectors = Testdata.pcodecVectorNames().stream().filter(name -> !Testdata.trinoRefuses(name)).map(name -> {
            try {
                Path f = Testdata.file(name);
                return new Vector(name, f, Files.readAllBytes(f), Corrupt.payloads(f));
            }
            catch (IOException e) {
                throw new UncheckedIOException(e);
            }
        }).filter(v -> !v.payloads().isEmpty()).toList();
    }

    @AfterContainer
    static void clean()
            throws IOException
    {
        try (Stream<Path> s = Files.walk(dir)) {
            for (Path p : s.sorted(Comparator.reverseOrder()).toList()) {
                Files.delete(p);
            }
        }
    }

    @Provide
    Arbitrary<Vector> vector()
    {
        return Arbitraries.of(vectors);
    }

    /** Reads {@code bytes} with Trino's reader: the rows as canonical JSON, or the exception. */
    static Object read(byte[] bytes)
            throws IOException
    {
        Path f = Files.createTempFile(dir, "damaged", ".parquet");
        try {
            Files.write(f, bytes);
            return TrinoParquet.read(f).lines();
        }
        catch (Exception e) {
            return e;
        }
        finally {
            Files.delete(f);
        }
    }

    static ParquetCorruptionException corruption(Throwable e)
    {
        for (Throwable t = e; t != null; t = t.getCause()) {
            if (t instanceof ParquetCorruptionException c) {
                return c;
            }
        }
        return null;
    }

    @Example
    void everyPcodecVectorWithPagesIsHere()
    {
        assertThat(vectors).hasSizeGreaterThanOrEqualTo(11);
    }

    @Property(tries = 300)
    void damagedPayloadBytes(@ForAll("vector") Vector v, @ForAll @IntRange(min = 0, max = 1 << 20) int page,
            @ForAll @IntRange(min = 0, max = 1 << 20) int at, @ForAll @IntRange(min = 1, max = 255) int xor)
            throws IOException
    {
        Corrupt.Payload p = v.payloads().get(page % v.payloads().size());
        byte[] damaged = v.bytes().clone();
        damaged[p.offset() + at % p.length()] ^= (byte) xor;
        // rows or an exception; an Error (or a dead JVM) fails the property
        assertThat(read(damaged)).isNotNull();
    }

    @Property(tries = 200)
    void damagedPayloadHeaders(@ForAll("vector") Vector v, @ForAll @IntRange(min = 0, max = 1 << 20) int page,
            @ForAll @IntRange(min = 0, max = 1) int which, @ForAll @IntRange(min = 4, max = 255) int value)
            throws IOException
    {
        Corrupt.Payload p = v.payloads().get(page % v.payloads().size());
        byte[] damaged = v.bytes().clone();
        // an unknown version (anything but 1) or an unknown mode (above 3)
        damaged[p.offset() + which] = (byte) (which == 0 && value == 1 ? 2 : value);
        Object got = read(damaged);
        assertThat(got).isInstanceOf(Exception.class);
        assertThat(corruption((Exception) got)).as("%s", got).isNotNull()
                .hasMessageContaining("Invalid PCODEC page: pcodec payload: " + (which == 0 ? "unsupported version" : "unknown mode"));
    }

    @Property(tries = 100)
    void anotherModeOfTheSameBody(@ForAll("vector") Vector v, @ForAll @IntRange(min = 0, max = 1 << 20) int page,
            @ForAll @IntRange(min = 0, max = 3) int mode)
            throws IOException
    {
        // a payload read as another mode than it was written with: the body is then malformed for
        // the decoder (a RAW body of another size, a pco stream that is not one, ...), or, rarely,
        // decodes to other bytes of the right size; a failure is always a corruption error
        Corrupt.Payload p = v.payloads().get(page % v.payloads().size());
        byte[] damaged = v.bytes().clone();
        damaged[p.offset() + 1] = (byte) mode;
        Object got = read(damaged);
        if (got instanceof Exception e) {
            assertThat(corruption(e)).as("%s", e).isNotNull();
        }
    }
}
