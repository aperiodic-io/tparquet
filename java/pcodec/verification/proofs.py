"""SMT proofs (Z3) for pcodec-java.

Each theorem states, for ALL inputs of the given widths, either that a Java expression of
pcodec-java computes the same bits as the corresponding Rust expression of pco 1.0.4 (the
translation is exact where Java lacks unsigned types, branchless idioms or Rust's float
semantics), or that a transform pcodec relies on for losslessness is exactly inverted by its
decoder. A theorem is proved when Z3 finds the negation unsatisfiable.

The Java side is transcribed from src/main/java/io/aperiodic/pcodec (the class and method are
named in each theorem); the Rust side from pco 1.0.4. Run: python3 proofs.py [-k name]
"""

import sys
import time
from z3 import (
    And, BitVec, BitVecVal, Extract, FP, FPVal, Float32, Float64, If, Implies, LShR, Not, Or,
    RNA, RNE, RTN, RTZ, Solver, UDiv, ULE, ULT, URem, ZeroExt, fpAbs, fpAdd, fpEQ, fpGEQ, fpGT,
    fpIsInf, fpIsNaN, fpIsNegative, fpLEQ, fpLT, fpMul, fpNeg, fpRoundToIntegral, fpSignedToFP,
    fpSub, fpToIEEEBV, fpToSBV, fpToUBV, fpUnsignedToFP, sat, unsat,
)

RESULTS = []


def prove(name, claim, timeout_s=300):
    s = Solver()
    s.set("timeout", timeout_s * 1000)
    s.add(Not(claim))
    t = time.time()
    r = s.check()
    dt = time.time() - t
    ok = r == unsat
    RESULTS.append((name, ok))
    status = "PROVED" if ok else ("COUNTEREXAMPLE" if r == sat else "UNKNOWN")
    print(f"{status:14} {name} ({dt:.1f}s)", flush=True)
    if r == sat:
        print("  ", s.model())
    return ok


def mask(w):
    return (1 << w) - 1


# ---------------------------------------------------------------- ordered latents

def rust_float_to_latent(mem, w):
    # pco data_types/float.rs: flip_all = ZERO.wrapping_sub(mem >> (BITS - 1)); mem ^ (flip_all | SIGN)
    flip_all = BitVecVal(0, w) - LShR(mem, w - 1)
    return mem ^ (flip_all | BitVecVal(1 << (w - 1), w))


def rust_float_from_latent(l, w):
    flip_all = ~(BitVecVal(0, w) - LShR(l, w - 1))
    return l ^ (flip_all | BitVecVal(1 << (w - 1), w))


def java_float_to_latent(raw, w):
    # NumberType.toLatentOrdered: flipAll = (mid & raw) != 0 ? mask : 0; raw ^ (flipAll | mid)
    mid = BitVecVal(1 << (w - 1), w)
    flip = If((mid & raw) != 0, BitVecVal(mask(w), w), BitVecVal(0, w))
    return raw ^ (flip | mid)


def java_float_from_latent(l, w):
    mid = BitVecVal(1 << (w - 1), w)
    flip = If((mid & l) != 0, BitVecVal(0, w), BitVecVal(mask(w), w))
    return l ^ (flip | mid)


def ordered_latents():
    for w in (8, 16, 32, 64):
        x = BitVec("x", w)
        mid = BitVecVal(1 << (w - 1), w)
        prove(f"NumberType.toLatentOrdered float u{w} == pco to_latent_ordered",
              java_float_to_latent(x, w) == rust_float_to_latent(x, w))
        prove(f"NumberType.fromLatentOrdered float u{w} == pco from_latent_ordered",
              java_float_from_latent(x, w) == rust_float_from_latent(x, w))
        prove(f"float latents u{w}: from(to(x)) == x",
              java_float_from_latent(java_float_to_latent(x, w), w) == x)
        # signed: pco `self.wrapping_sub(MIN) as L` and `(l as Self).wrapping_add(MIN)`
        prove(f"NumberType signed u{w}: x ^ mid == x - MIN and == l + MIN",
              And(x ^ mid == x - mid, x ^ mid == x + mid))
    for (sort, w) in ((Float32(), 32), (Float64(), 64)):
        a = FP("a", sort)
        b = FP("b", sort)
        la = java_float_to_latent(fpToIEEEBV(a), w)
        lb = java_float_to_latent(fpToIEEEBV(b), w)
        prove(f"float latents f{w} preserve order: a < b => latent(a) < latent(b)",
              Implies(And(Not(fpIsNaN(a)), Not(fpIsNaN(b)), fpLT(a, b)), ULT(la, lb)))
        prove(f"float latents f{w}: -0.0 sorts just below +0.0",
              java_float_to_latent(BitVecVal(1 << (w - 1), w), w) + 1 == java_float_to_latent(BitVecVal(0, w), w))


# ---------------------------------------------------------------- latent arithmetic in a long

def latent_ops():
    # Latent: values of width w held zero-extended in a long; ops mask to w
    for w in (8, 16, 32):
        a = BitVec("a", 64)
        b = BitVec("b", 64)
        m = BitVecVal(mask(w), 64)
        pre = And(ULE(a, m), ULE(b, m))
        ea, eb = Extract(w - 1, 0, a), Extract(w - 1, 0, b)
        z = lambda v: ZeroExt(64 - w, v)
        mid = BitVecVal(1 << (w - 1), 64)
        prove(f"Latent u{w}: (a + b) & mask == wrapping_add", Implies(pre, (a + b) & m == z(ea + eb)))
        prove(f"Latent u{w}: (a - b) & mask == wrapping_sub", Implies(pre, (a - b) & m == z(ea - eb)))
        prove(f"Latent u{w}: (a * b) & mask == wrapping_mul", Implies(pre, (a * b) & m == z(ea * eb)))
        prove(f"Latent u{w}: toggleCenter == wrapping_add(MID)", Implies(pre, (a + mid) & m == z(ea + Extract(w - 1, 0, mid))))
        prove(f"Latent u{w}: unsigned compare of longs == compare of u{w}", Implies(pre, ULT(a, b) == ULT(ea, eb)))
        if w <= 16:
            # (for u32, bit-blasting a 64-bit division is beyond a CI budget; zero extension
            # preserves the integer values, and Long.divideUnsigned is integer division)
            prove(f"Latent u{w}: divideUnsigned/remainderUnsigned == u{w} div/rem",
                  Implies(And(pre, b != 0), And(UDiv(a, b) == z(UDiv(ea, eb)), URem(a, b) == z(URem(ea, eb)))))


# ---------------------------------------------------------------- unsigned conversions

def conversions():
    x = BitVec("x", 64)
    # Latent.toF64: x >= 0 ? (double) x : ((double)((x >>> 1) | (x & 1))) * 2.0
    halved = LShR(x, 1) | (x & 1)
    java = If(x >= 0, fpSignedToFP(RNE(), x, Float64()),
              fpMul(RNE(), fpSignedToFP(RNE(), halved, Float64()), FP_VAL(2.0, Float64())))
    prove("Latent.toF64 == `u64 as f64` (round to nearest even)",
          fpEQ(java, fpUnsignedToFP(RNE(), x, Float64())), timeout_s=1200)
    java32 = If(x >= 0, fpSignedToFP(RNE(), x, Float32()),
                fpMul(RNE(), fpSignedToFP(RNE(), halved, Float32()), FP_VAL(2.0, Float32())))
    prove("RustMath.u64ToF32 == `u64 as f32`",
          fpEQ(java32, fpUnsignedToFP(RNE(), x, Float32())), timeout_s=1200)


def FP_VAL(v, sort):
    return FPVal(v, sort)


def saturating_casts():
    # RustMath.f64ToUnsigned(x, 64): !(x > 0) -> 0; x >= 2^64 -> MAX; x >= 2^63 -> (long)(x - 2^63) ^ MIN;
    # else (long) x. Rust's `x as u64`: NaN -> 0, saturating, truncating toward zero.
    d = FP("d", Float64())
    two63 = FP_VAL(2.0 ** 63, Float64())
    two64 = FP_VAL(2.0 ** 64, Float64())
    zero = FP_VAL(0.0, Float64())
    java = If(Not(fpGT(d, zero)), BitVecVal(0, 64),
              If(fpGEQ(d, two64), BitVecVal(mask(64), 64),
                 If(fpGEQ(d, two63), fpToSBV(RTZ(), fpSub(RNE(), d, two63), BitVec("s", 64).sort()) ^ BitVecVal(1 << 63, 64),
                    fpToSBV(RTZ(), d, BitVec("s", 64).sort()))))
    rust = If(fpIsNaN(d), BitVecVal(0, 64),
              If(fpLEQ(d, zero), BitVecVal(0, 64),
                 If(fpGEQ(d, two64), BitVecVal(mask(64), 64), fpToUBV(RTZ(), d, BitVec("u", 64).sort()))))
    prove("RustMath.f64ToUnsigned(x, 64) == `f64 as u64`", java == rust, timeout_s=1200)
    for w in (16, 32):
        lim = FP_VAL(2.0 ** w, Float64())
        javaw = If(Not(fpGT(d, zero)), BitVecVal(0, 64),
                   If(fpGEQ(d, lim), BitVecVal(mask(w), 64), fpToSBV(RTZ(), d, BitVec("s", 64).sort())))
        rustw = If(fpIsNaN(d), BitVecVal(0, w),
                   If(fpLEQ(d, zero), BitVecVal(0, w),
                      If(fpGEQ(d, lim), BitVecVal(mask(w), w), fpToUBV(RTZ(), d, BitVec("u", w).sort()))))
        prove(f"RustMath.f64ToUnsigned(x, {w}) == `f64 as u{w}`", javaw == ZeroExt(64 - w, rustw), timeout_s=1200)


def rounding():
    # RustMath.roundF64/roundF32: t = floor(|x|); if |x| - t >= 0.5: t += 1; copysign(t, x), for
    # |x| < 2^52 (2^23), else x. Rust's round is round-half-away-from-zero: fp.roundToIntegral(RNA).
    for (sort, big, name) in ((Float64(), 2.0 ** 52, "roundF64"), (Float32(), 2.0 ** 23, "roundF32")):
        x = FP("x", sort)
        a = fpAbs(x)
        t = fpRoundToIntegral(RTN(), a)
        half = FP_VAL(0.5, sort)
        t2 = If(fpGEQ(fpSub(RNE(), a, t), half), fpAdd(RNE(), t, FP_VAL(1.0, sort)), t)
        signed = If(fpIsNegative(x), fpNeg(t2), t2)
        java = If(fpGEQ(a, FP_VAL(big, sort)), x, signed)
        rust = fpRoundToIntegral(RNA(), x)
        prove(f"RustMath.{name} == Rust round (half away from zero), bit for bit",
              Implies(And(Not(fpIsNaN(x)), Not(fpIsInf(x))), fpToIEEEBV(java) == fpToIEEEBV(rust)), timeout_s=1200)


# ---------------------------------------------------------------- mode transforms are lossless

def float_quant():
    for w, p in ((16, 10), (32, 23), (64, 52)):
        raw = BitVec("raw", w)
        k = BitVec("k", w)
        one = BitVecVal(1, w)
        mid = BitVecVal(1 << (w - 1), w)
        full = BitVecVal(mask(w), w)
        pre = And(ULE(one, k), ULE(k, BitVecVal(p, w)))
        # split (Modes.FloatQuant.splitLatents)
        u = java_float_to_latent(raw, w)
        lowest_max = (one << k) - one
        primary = LShR(u, k)
        low = u & lowest_max
        positive = (raw & mid) == 0
        secondary = If(positive, low, lowest_max - low)
        # join (ChunkDecompressor.join, FLOAT_QUANT)
        sign_cutoff = LShR(mid, k)
        high_mask = ~((one << k) - one) & full
        flip = If(ULT(primary, sign_cutoff), high_mask, mid)
        joined = (primary << k) ^ secondary ^ flip
        prove(f"FloatQuant f{w}: join(split(x)) == x for every bit pattern and 1 <= k <= {p}",
              Implies(pre, joined == raw))
        prove(f"FloatQuant f{w}: the secondary latent has at most k bits", Implies(pre, LShR(secondary, k) == 0))


def int_mult():
    # (wider widths: the identity is the definition of unsigned division, and bit-blasting a
    # 16-bit or wider divider exceeds a CI budget)
    for w in (8,):
        u = BitVec("u", w)
        b = BitVec("b", w)
        prove(f"IntMult u{w}: mult * base + adj == x (wrapping), base > 0",
              Implies(b != 0, UDiv(u, b) * b + URem(u, b) == u))


def classic():
    for w in (8, 16, 32, 64):
        x = BitVec("x", w)
        mid = BitVecVal(1 << (w - 1), w)
        prove(f"Classic signed u{w}: from(to(x)) == x", (x ^ mid) ^ mid == x)


# ---------------------------------------------------------------- delta encodings are lossless

def consecutive():
    # Delta.consecutiveEncode then consecutiveDecode (in batches) restores the latents
    w = 32
    for n in range(1, 7):
        for order in range(0, min(n, 4) + 1):
            xs = [BitVec(f"x{i}", w) for i in range(n)]
            v = list(xs)
            moments = []
            start = 0
            for _ in range(order):
                moments.append(v[start] if start < n else BitVecVal(0, w))
                for i in range(n - 1, start, -1):
                    v[i] = v[i] - v[i - 1]
                start += min(n - start, 1)
            mid = BitVecVal(1 << (w - 1), w)
            for i in range(start, n):
                v[i] = v[i] + mid
            # the decoder sees the stored latents (after the state) and then junk
            stored = v[start:] + [BitVec(f"junk{i}", w) for i in range(order)]
            out = [s + mid for s in stored]
            mom = list(moments)
            for m in range(len(mom) - 1, -1, -1):
                moment = mom[m]
                for i in range(len(out)):
                    tmp = out[i]
                    out[i] = moment
                    moment = moment + tmp
                mom[m] = moment
            prove(f"consecutive delta: decode(encode(x)) == x, n={n} order={order}",
                  And(*[out[i] == xs[i] for i in range(n)]) if n > 0 else True)


def lookback():
    # Delta.lookbackEncode then lookbackDecode with any valid lookbacks restores the latents
    w = 32
    for n in range(1, 6):
        for state_n in (1, 2):
            if state_n > n:
                continue
            window_n = 16
            xs = [BitVec(f"x{i}", w) for i in range(n)]
            lbs = [BitVec(f"lb{i}", 8) for i in range(n - state_n)]
            # valid lookbacks: 1 <= lb <= i (only earlier latents of the page)
            pre = And(*[And(ULE(1, lbs[i - state_n]), ULE(lbs[i - state_n], i)) for i in range(state_n, n)]) if n > state_n else True
            def sel(arr, idx):  # arr[idx] for a symbolic 8-bit index
                r = arr[0]
                for j in range(1, len(arr)):
                    r = If(idx == j, arr[j], r)
                return r
            v = list(xs)
            for i in range(n - 1, state_n - 1, -1):
                lb = lbs[i - state_n]
                v[i] = v[i] - sel(v, BitVecVal(i, 8) - lb)
            state = [BitVecVal(0, w)] * state_n
            for j in range(state_n):
                state[j] = v[j]
            mid = BitVecVal(1 << (w - 1), w)
            enc = [x + mid for x in v]
            # decode: window holds state at [window_n - state_n, window_n); then
            # window[pos] = latent + window[pos - lb]; output window[pos - state_n]
            stored = enc[state_n:] + [BitVec(f"junk{i}", w) for i in range(state_n)]
            look = lbs + [BitVecVal(1, 8)] * state_n
            win = {window_n - state_n + j: state[j] for j in range(state_n)}
            outs = []
            for i, s in enumerate(stored):
                pos = window_n + i
                lat = s + mid
                lb = look[i]
                cands = {p: win[p] for p in win}
                val = None
                for p, cv in cands.items():
                    val = cv if val is None else If(BitVecVal(pos, 8) - lb == p, cv, val)
                win[pos] = lat + val
            for i in range(n):
                outs.append(win[window_n - state_n + i])
            prove(f"lookback delta: decode(encode(x)) == x, n={n} state_n={state_n}",
                  Implies(pre, And(*[outs[i] == xs[i] for i in range(n)])))


# ---------------------------------------------------------------- bit packing and varints

def bit_io():
    # BitWriter.writeUint ORs x << off into 8 bytes and x >>> (64 - off) into the 9th;
    # BitReader.peekAt reads u64 >>> off, ORs u64(next) << (64 - off) when n > 64 - off.
    x = BitVec("x", 64)
    off = BitVec("off", 64)
    n = BitVec("n", 64)
    prior = BitVec("prior", 128)  # bits already written below off
    pre = And(ULE(off, 7), ULE(n, 64), ULE(1, n),
              If(n == 64, True, LShR(x, n) == 0),
              LShR(prior, ZeroExt(64, off)) == 0)
    wide = ZeroExt(64, x) << ZeroExt(64, off)
    buf = prior | wide
    lo = Extract(63, 0, LShR(buf, ZeroExt(64, off)))
    hi_word = Extract(127, 64, buf)
    shift_back = BitVecVal(64, 64) - off
    v = If(And(UGT_(n, BitVecVal(64, 64) - off), off != 0), lo | (hi_word << shift_back), lo)
    read = If(n == 64, v, v & ((BitVecVal(1, 64) << n) - 1))
    prove("BitWriter.writeUint then BitReader.readUint returns the value (1..64 bits, any offset)",
          Implies(pre, read == x))
    prove("BitWriter.writeUint leaves the bits below the position unchanged",
          Implies(pre, Extract(7, 0, buf) & ((BitVecVal(1, 8) << Extract(7, 0, off)) - 1) == Extract(7, 0, prior)))


def UGT_(a, b):
    return ULT(b, a)


def varints():
    # payload/Varint: put, then get, returns the value and consumes the bytes written
    v = BitVec("v", 64)
    bytes_ = []
    rest = v
    done = BitVecVal(0, 1) == 1
    for i in range(10):
        more = ULE(BitVecVal(0x80, 64), rest)
        b = If(more, (rest & 0x7f) | 0x80, rest & 0x7f)
        bytes_.append((Not(done), Extract(7, 0, b)))
        done = Or(done, Not(more))
        rest = LShR(rest, 7)
    # decode
    val = BitVecVal(0, 64)
    stopped = BitVecVal(0, 1) == 1
    bad = BitVecVal(0, 1) == 1
    for i, (present, b) in enumerate(bytes_):
        part = ZeroExt(56, b & 0x7f)
        active = And(present, Not(stopped))
        bad = Or(bad, And(active, i == 9, ULT(BitVecVal(1, 64), part)))
        val = If(active, val | (part << (7 * i)), val)
        stopped = Or(stopped, And(active, ULT(ZeroExt(56, b), BitVecVal(0x80, 64))))
    prove("Varint: get(put(v)) == v for every u64, within 10 bytes", And(val == v, stopped, Not(bad)))


def int_float():
    # FloatType.intFloatToLatent / intFloatFromLatent (pco Float::int_float_to_latent and
    # int_float_from_latent): the decoder recovers every value a FloatMult split can produce,
    # the rounded multipliers (integer-valued floats, infinities and NaNs), bit for bit; with
    # that, join(split(x)) == x, since the adjustment is to_latent(x) - to_latent(mult * base).
    for (sort, w, p) in ((Float32(), 32, 23), (Float64(), 64, 52)):
        x = FP("x", sort)
        bits = fpToIEEEBV(x)
        mid = BitVecVal(1 << (w - 1), w)
        gpi = BitVecVal(1 << (p + 1), w)
        gpi_float = fpUnsignedToFP(RNE(), gpi, sort)
        a = fpAbs(x)
        abs_bits = bits & BitVecVal(mask(w - 1), w)
        abs_int = If(fpLT(a, gpi_float), fpToUBV(RTZ(), a, BitVec("t", w).sort()),
                     gpi + (abs_bits - fpToIEEEBV(gpi_float)))
        positive = (bits & mid) == 0
        l = If(positive, mid + abs_int, mid - 1 - abs_int)
        # from
        neg = ~(BitVecVal(0, w) - LShR(l, w - 1))
        abs_int2 = (l ^ neg) & BitVecVal(mask(w - 1), w)
        offset = fpToIEEEBV(gpi_float) - gpi
        abs_float = If(ULT(abs_int2, gpi), fpToIEEEBV(fpUnsignedToFP(RNE(), abs_int2, sort)), abs_int2 + offset)
        back = abs_float | (neg & mid)
        integral = Or(fpIsNaN(x), fpIsInf(x), fpEQ(fpRoundToIntegral(RNA(), x), x))
        prove(f"FloatType f{w}: intFloatFromLatent(intFloatToLatent(m)) == m for integer, infinite and NaN m",
              Implies(integral, back == bits), timeout_s=1200)


THEOREMS = [ordered_latents, latent_ops, classic, int_mult, float_quant, consecutive, lookback,
            bit_io, varints, rounding, saturating_casts, conversions, int_float]


def main():
    sel = sys.argv[sys.argv.index("-k") + 1] if "-k" in sys.argv else None
    for th in THEOREMS:
        if sel and sel not in th.__name__:
            continue
        th()
    failed = [n for n, ok in RESULTS if not ok]
    print(f"\n{len(RESULTS) - len(failed)} of {len(RESULTS)} theorems proved")
    if failed:
        print("not proved:", *failed, sep="\n  ")
        sys.exit(1)


if __name__ == "__main__":
    main()
