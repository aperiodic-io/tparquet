package io.aperiodic.pcodec;

import static org.junit.jupiter.api.Assertions.assertTrue;

import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;
import org.junit.jupiter.api.Test;

/** Replays the golden vectors (Vectors): the reference's answers, without the reference. */
class VectorsTest {
  @Test
  void javaGivesTheRecordedAnswers() throws Exception {
    List<Vectors.Record> records = Vectors.read(Vectors.path());
    assertTrue(records.size() > 5000, "too few vectors: " + records.size());
    List<String> fails = new ArrayList<>();
    int[] statuses = new int[3];
    for (Vectors.Record rec : records) {
      Parity.Outcome got = Vectors.java(rec.request);
      statuses[rec.status]++;
      boolean same = got.status == rec.status && (rec.status != 0 || Arrays.equals(got.body, rec.body));
      if (!same && fails.size() < 10) {
        fails.add("request " + records.indexOf(rec) + ": recorded status " + rec.status + ", java " + got);
      }
    }
    if (!fails.isEmpty()) {
      throw new AssertionError(String.join("\n", fails));
    }
    System.out.println(records.size() + " vectors agree (ok " + statuses[0] + ", errors " + statuses[1] + ", panics " + statuses[2] + ")");
  }

  @Test
  void theVectorsAreTheCurrentRequests() throws Exception {
    // the recorded requests are exactly those Vectors.requests() makes (plus decodes), so a
    // change to the corpus must regenerate the file
    List<Vectors.Record> records = Vectors.read(Vectors.path());
    List<byte[]> reqs = Vectors.requests();
    int i = 0;
    for (Vectors.Record rec : records) {
      if (rec.request[0] == 1) {
        assertTrue(Arrays.equals(reqs.get(i++), rec.request), "compress request " + (i - 1) + " differs");
      }
    }
    assertTrue(i == reqs.size(), "recorded " + i + " compress requests, corpus has " + reqs.size());
  }
}
