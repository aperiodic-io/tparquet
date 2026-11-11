package io.aperiodic.tparquet;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.ObjectMapper;
import java.io.IOException;
import java.io.UncheckedIOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.List;

/** The conformance vectors of {@code testdata/} (system property {@code tparquet.testdata}). */
final class Testdata {
  private Testdata() {}

  static Path dir() {
    String d = System.getProperty("tparquet.testdata");
    return Path.of(d == null ? "../../testdata" : d);
  }

  /** One entry of {@code payloads/manifest.json}. */
  record Payload(String name, byte[] payload, int expectedLen, boolean valid, byte[] decoded, String mode,
      PhysicalType hint, int level, boolean referenceEncoding, String error) {
    /** The vector's name, for the parameterized tests' names. */
    @Override
    public String toString() {
      return name;
    }
  }

  static List<Payload> payloads() {
    Path dir = dir().resolve("payloads");
    try {
      JsonNode m = new ObjectMapper().readTree(dir.resolve("manifest.json").toFile());
      List<Payload> out = new ArrayList<>();
      for (JsonNode v : m.get("vectors")) {
        boolean valid = v.get("valid").asBoolean();
        out.add(new Payload(
            v.get("name").asText(),
            Files.readAllBytes(dir.resolve(v.get("payload").asText())),
            v.get("expected_len").asInt(),
            valid,
            valid ? Files.readAllBytes(dir.resolve(v.get("decoded").asText())) : null,
            valid ? v.get("mode").asText() : null,
            valid && !v.get("hint").isNull() ? PhysicalType.valueOf(v.get("hint").asText()) : null,
            valid ? v.get("level").asInt() : -1,
            valid && v.get("reference_encoding").asBoolean(),
            valid ? null : v.get("error").asText()));
      }
      return out;
    } catch (IOException e) {
      throw new UncheckedIOException(e);
    }
  }
}
