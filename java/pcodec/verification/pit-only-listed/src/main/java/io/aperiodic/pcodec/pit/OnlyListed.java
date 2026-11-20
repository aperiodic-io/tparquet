package io.aperiodic.pcodec.pit;

import java.io.IOException;
import java.io.UncheckedIOException;
import java.nio.file.Files;
import java.nio.file.Paths;
import java.util.Collection;
import java.util.HashSet;
import java.util.Set;
import java.util.stream.Collectors;
import org.pitest.bytecode.analysis.ClassTree;
import org.pitest.mutationtest.build.InterceptorParameters;
import org.pitest.mutationtest.build.InterceptorType;
import org.pitest.mutationtest.build.MutationInterceptor;
import org.pitest.mutationtest.build.MutationInterceptorFactory;
import org.pitest.mutationtest.engine.MutationDetails;
import org.pitest.mutationtest.engine.Mutater;
import org.pitest.plugin.Feature;

/** Keeps only the mutants listed (class,method,line,mutator per line) in $PIT_ONLY_LISTED. */
public final class OnlyListed implements MutationInterceptorFactory {
  static String key(String cls, String method, int line, String mutator) {
    return cls + "|" + method + "|" + line + "|" + mutator;
  }

  @Override
  public String description() {
    return "keep only listed mutants";
  }

  @Override
  public Feature provides() {
    return Feature.named("ONLYLISTED").withOnByDefault(true).withDescription(description());
  }

  @Override
  public MutationInterceptor createInterceptor(InterceptorParameters params) {
    String f = System.getenv("PIT_ONLY_LISTED");
    Set<String> keep = new HashSet<>();
    if (f != null) {
      try {
        keep.addAll(Files.readAllLines(Paths.get(f)));
      } catch (IOException e) {
        throw new UncheckedIOException(e);
      }
    }
    return new MutationInterceptor() {
      @Override
      public InterceptorType type() {
        return InterceptorType.FILTER;
      }

      @Override
      public void begin(ClassTree clazz) {}

      @Override
      public Collection<MutationDetails> intercept(Collection<MutationDetails> mutations, Mutater m) {
        if (f == null) {
          return mutations;
        }
        return mutations.stream()
            .filter(d -> keep.contains(key(d.getClassName().asJavaName(), d.getMethod(), d.getLineNumber(), d.getMutator())))
            .collect(Collectors.toList());
      }

      @Override
      public void end() {}
    };
  }
}
