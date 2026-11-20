package io.aperiodic.pcodec;

import java.util.Arrays;

/**
 * How numbers are split into pages ({@code pco::PagingSpec}); in the standalone format, into
 * chunks of one page each.
 */
public final class PagingSpec {
  public static final int DEFAULT_MAX_PAGE_N = 1 << 18;

  private final long equalPagesUpTo;
  private final long[] exact;

  private PagingSpec(long equalPagesUpTo, long[] exact) {
    this.equalPagesUpTo = equalPagesUpTo;
    this.exact = exact;
  }

  public static PagingSpec equalPagesUpTo(long maxPageN) {
    return new PagingSpec(maxPageN, null);
  }

  public static PagingSpec exact(long... nPerPage) {
    return new PagingSpec(0, nPerPage.clone());
  }

  public static final PagingSpec DEFAULT = equalPagesUpTo(DEFAULT_MAX_PAGE_N);

  /** {@code n_per_page} */
  public long[] nPerPage(long n) {
    long[] res;
    if (exact == null) {
      if (n == 0) {
        return new long[0];
      }
      if (equalPagesUpTo == 0) {
        throw new ArithmeticException("attempt to divide by zero");
      }
      long nPages = (n + equalPagesUpTo - 1) / equalPagesUpTo;
      long low = n / nPages;
      long r = n % nPages;
      res = new long[(int) nPages];
      Arrays.fill(res, low);
      for (int i = 0; i < r; i++) {
        res[i] = low + 1;
      }
    } else {
      res = exact.clone();
    }
    long sum = 0;
    for (long x : res) {
      sum += x;
    }
    if (sum != n) {
      throw PcoException.invalidArgument(
          "paging spec suggests " + sum + " numbers but " + n + " were given");
    }
    for (long x : res) {
      if (x == 0) {
        throw PcoException.invalidArgument("cannot write data page of 0 numbers");
      }
    }
    return res;
  }

  @Override
  public String toString() {
    return exact == null ? "EqualPagesUpTo(" + equalPagesUpTo + ")" : "Exact" + Arrays.toString(exact);
  }
}
