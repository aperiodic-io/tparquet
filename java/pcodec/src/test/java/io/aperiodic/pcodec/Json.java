package io.aperiodic.pcodec;

import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

/** A minimal JSON reader for the test manifests (objects, arrays, strings, numbers, literals). */
final class Json {
  private final String s;
  private int i;

  private Json(String s) {
    this.s = s;
  }

  static Object parse(String s) {
    Json j = new Json(s);
    Object v = j.value();
    j.ws();
    if (j.i != s.length()) {
      throw new IllegalArgumentException("trailing JSON at " + j.i);
    }
    return v;
  }

  private void ws() {
    while (i < s.length() && Character.isWhitespace(s.charAt(i))) {
      i++;
    }
  }

  private Object value() {
    ws();
    char c = s.charAt(i);
    if (c == '{') {
      Map<String, Object> m = new LinkedHashMap<>();
      i++;
      ws();
      if (s.charAt(i) == '}') {
        i++;
        return m;
      }
      while (true) {
        ws();
        String k = (String) value();
        ws();
        i++; // :
        m.put(k, value());
        ws();
        if (s.charAt(i++) == '}') {
          return m;
        }
      }
    }
    if (c == '[') {
      List<Object> l = new ArrayList<>();
      i++;
      ws();
      if (s.charAt(i) == ']') {
        i++;
        return l;
      }
      while (true) {
        l.add(value());
        ws();
        if (s.charAt(i++) == ']') {
          return l;
        }
      }
    }
    if (c == '"') {
      StringBuilder b = new StringBuilder();
      i++;
      while (s.charAt(i) != '"') {
        char ch = s.charAt(i++);
        if (ch == '\\') {
          char e = s.charAt(i++);
          switch (e) {
            case 'n':
              b.append('\n');
              break;
            case 't':
              b.append('\t');
              break;
            case 'u':
              b.append((char) Integer.parseInt(s.substring(i, i + 4), 16));
              i += 4;
              break;
            default:
              b.append(e);
          }
        } else {
          b.append(ch);
        }
      }
      i++;
      return b.toString();
    }
    if (s.startsWith("true", i)) {
      i += 4;
      return Boolean.TRUE;
    }
    if (s.startsWith("false", i)) {
      i += 5;
      return Boolean.FALSE;
    }
    if (s.startsWith("null", i)) {
      i += 4;
      return null;
    }
    int st = i;
    while (i < s.length() && "+-0123456789.eE".indexOf(s.charAt(i)) >= 0) {
      i++;
    }
    String num = s.substring(st, i);
    if (num.contains(".") || num.contains("e") || num.contains("E")) {
      return Double.parseDouble(num);
    }
    return new java.math.BigInteger(num);
  }
}
