package io.aperiodic.pcodec;

import java.util.Arrays;

/**
 * An open-addressing set of {@code long} keys that numbers them in order of first insertion:
 * key {@code i} is the {@code i}-th distinct key added. Callers keep their values in arrays
 * indexed by that number, so iterating {@code 0 .. size()} is a {@code LinkedHashMap}'s order.
 */
final class LongIndexMap {
  private static final long PHI = 0x9E3779B97F4A7C15L;

  private long[] keys;
  /** 0 for an empty slot, else the key's index + 1 */
  private int[] slots;
  private int shift;
  private int size;

  LongIndexMap(int expected) {
    int cap = 16;
    while (cap < 2L * expected && cap < (1 << 30)) {
      cap <<= 1;
    }
    slots = new int[cap];
    shift = 64 - Integer.numberOfTrailingZeros(cap);
    keys = new long[Math.max(cap >>> 1, 1)];
  }

  int size() {
    return size;
  }

  long key(int index) {
    return keys[index];
  }

  /** the key's index, or -1 */
  int get(long key) {
    int mask = slots.length - 1;
    for (int s = (int) ((key * PHI) >>> shift); ; s = (s + 1) & mask) {
      int v = slots[s];
      if (v == 0) {
        return -1;
      }
      if (keys[v - 1] == key) {
        return v - 1;
      }
    }
  }

  /** the key's index, adding it if absent */
  int add(long key) {
    int mask = slots.length - 1;
    int s = (int) ((key * PHI) >>> shift);
    while (true) {
      int v = slots[s];
      if (v == 0) {
        break;
      }
      if (keys[v - 1] == key) {
        return v - 1;
      }
      s = (s + 1) & mask;
    }
    int index = size++;
    if (index == keys.length) {
      keys = Arrays.copyOf(keys, keys.length * 2);
    }
    keys[index] = key;
    slots[s] = index + 1;
    if (2L * size > slots.length) {
      rehash();
    }
    return index;
  }

  private void rehash() {
    int cap = slots.length * 2;
    slots = new int[cap];
    shift = 64 - Integer.numberOfTrailingZeros(cap);
    int mask = cap - 1;
    for (int i = 0; i < size; i++) {
      int s = (int) ((keys[i] * PHI) >>> shift);
      while (slots[s] != 0) {
        s = (s + 1) & mask;
      }
      slots[s] = i + 1;
    }
  }
}
