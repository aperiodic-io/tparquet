// The fork's codec (cpp/arrow/FORK.md) through Arrow's Codec API and Parquet's GetCodec:
// every payload vector of testdata/payloads decodes (or fails) as its manifest says, and the
// compression enums, names, levels and type hint behave as the fork documents.
#include <cmath>
#include <cstring>
#include <random>

#include <arrow/util/compression.h>
#include <parquet/types.h>
#include <rapidjson/document.h>

#include "common.h"

namespace tparquet::test {
namespace {

using arrow::Compression;
using arrow::util::Codec;
using arrow::util::PcodecCodecOptions;

struct PayloadVector {
  std::string name;
  int64_t expected_len;
  bool valid;
  std::string payload;
  std::string decoded;
  std::string mode;
};

std::vector<PayloadVector> PayloadVectors() {
  const auto dir = Testdata() / "payloads";
  rapidjson::Document doc;
  doc.Parse(ReadFile(dir / "manifest.json").c_str());
  EXPECT_FALSE(doc.HasParseError());
  EXPECT_EQ(doc["format_version"].GetInt(), 1);
  std::vector<PayloadVector> out;
  for (const auto& v : doc["vectors"].GetArray()) {
    PayloadVector p;
    p.name = v["name"].GetString();
    p.expected_len = v["expected_len"].GetInt64();
    p.valid = v["valid"].GetBool();
    p.payload = ReadFile(dir / v["payload"].GetString());
    if (p.valid) {
      p.decoded = ReadFile(dir / v["decoded"].GetString());
      p.mode = v["mode"].GetString();
    }
    out.push_back(std::move(p));
  }
  return out;
}

std::unique_ptr<Codec> MakeCodec(const arrow::util::CodecOptions& options = PcodecCodecOptions{}) {
  auto codec = Codec::Create(Compression::PCODEC, options);
  EXPECT_TRUE(codec.ok()) << codec.status().ToString();
  return std::move(codec).ValueOrDie();
}

/// Decodes `payload` into exactly `size` bytes.
arrow::Result<std::string> Decompress(Codec& codec, const std::string& payload, int64_t size) {
  std::string out(static_cast<size_t>(size), '\0');
  ARROW_ASSIGN_OR_RAISE(
      int64_t n, codec.Decompress(static_cast<int64_t>(payload.size()),
                                  reinterpret_cast<const uint8_t*>(payload.data()), size,
                                  reinterpret_cast<uint8_t*>(out.data())));
  if (n != size) return arrow::Status::Invalid("decoded ", n, " bytes");
  return out;
}

std::string Compress(Codec& codec, const std::string& src) {
  const auto* in = reinterpret_cast<const uint8_t*>(src.data());
  const int64_t cap = codec.MaxCompressedLen(static_cast<int64_t>(src.size()), in);
  std::string out(static_cast<size_t>(cap), '\0');
  auto n = codec.Compress(static_cast<int64_t>(src.size()), in, cap,
                          reinterpret_cast<uint8_t*>(out.data()));
  EXPECT_TRUE(n.ok()) << n.status().ToString();
  out.resize(static_cast<size_t>(n.ValueOr(0)));
  return out;
}

TEST(Codec, EveryPayloadVectorDecodesAsListed) {
  const auto vectors = PayloadVectors();
  ASSERT_GE(vectors.size(), 44u);
  auto codec = MakeCodec();
  // the Parquet layer's factory returns the same codec
  auto parquet_codec = parquet::GetCodec(Compression::PCODEC);
  ASSERT_NE(parquet_codec, nullptr);
  int valid = 0, invalid = 0;
  for (const auto& v : vectors) {
    for (Codec* c : {codec.get(), parquet_codec.get()}) {
      auto got = Decompress(*c, v.payload, v.expected_len);
      if (v.valid) {
        ASSERT_TRUE(got.ok()) << v.name << ": " << got.status().ToString();
        EXPECT_EQ(*got, v.decoded) << v.name;
      } else {
        ASSERT_FALSE(got.ok()) << v.name << " decoded";
        EXPECT_TRUE(got.status().IsIOError()) << v.name << ": " << got.status().ToString();
        EXPECT_NE(got.status().message().find("pcodec decompress failed: pcodec payload:"),
                  std::string::npos)
            << got.status().ToString();
      }
    }
    (v.valid ? valid : invalid)++;
  }
  EXPECT_EQ(valid, 18);
  EXPECT_EQ(invalid, 26);
}

TEST(Codec, ValidPayloadsNeedTheirExactSize) {
  auto codec = MakeCodec();
  for (const auto& v : PayloadVectors()) {
    if (!v.valid) continue;
    EXPECT_FALSE(Decompress(*codec, v.payload, v.expected_len + 1).ok()) << v.name;
    if (v.expected_len > 0) {
      EXPECT_FALSE(Decompress(*codec, v.payload, v.expected_len - 1).ok()) << v.name;
    }
  }
}

TEST(Codec, NamesLevelsAndAvailability) {
  EXPECT_EQ(Codec::GetCodecAsString(Compression::PCODEC), "pcodec");
  ASSERT_OK_AND_ASSIGN(auto type, Codec::GetCompressionType("pcodec"));
  EXPECT_EQ(type, Compression::PCODEC);
  EXPECT_FALSE(Codec::GetCompressionType("PCODEC").ok());  // names are lower case, as upstream
  EXPECT_TRUE(Codec::IsAvailable(Compression::PCODEC));
  EXPECT_TRUE(Codec::SupportsCompressionLevel(Compression::PCODEC));
  EXPECT_TRUE(parquet::IsCodecSupported(Compression::PCODEC));
  EXPECT_EQ(Codec::MinimumCompressionLevel(Compression::PCODEC).ValueOr(-1), 0);
  EXPECT_EQ(Codec::MaximumCompressionLevel(Compression::PCODEC).ValueOr(-1), 12);
  EXPECT_EQ(Codec::DefaultCompressionLevel(Compression::PCODEC).ValueOr(-1), 8);
  // the upstream codecs keep their names and values
  EXPECT_EQ(Codec::GetCodecAsString(Compression::ZSTD), "zstd");
  EXPECT_EQ(Codec::GetCodecAsString(Compression::BZ2), "bz2");
  EXPECT_EQ(static_cast<int>(Compression::PCODEC), static_cast<int>(Compression::LZ4_HADOOP) + 1);

  auto codec = MakeCodec();
  EXPECT_EQ(codec->compression_type(), Compression::PCODEC);
  EXPECT_EQ(codec->name(), "pcodec");
  EXPECT_EQ(codec->compression_level(), 8);
  EXPECT_EQ(codec->minimum_compression_level(), 0);
  EXPECT_EQ(codec->maximum_compression_level(), 12);
  EXPECT_EQ(codec->default_compression_level(), 8);
  for (int level : {0, 5, 12}) {
    ASSERT_OK_AND_ASSIGN(auto c, Codec::Create(Compression::PCODEC, level));
    EXPECT_EQ(c->compression_level(), level);
  }
  for (int level : {-1, 13, 100}) {
    auto c = Codec::Create(Compression::PCODEC, level);
    ASSERT_FALSE(c.ok()) << level;
    EXPECT_TRUE(c.status().IsInvalid());
    EXPECT_EQ(c.status().message(), "pcodec compression level " + std::to_string(level) + " not in 0..12");
  }
  EXPECT_EQ(codec->MaxCompressedLen(0, nullptr), 2);
  EXPECT_EQ(codec->MaxCompressedLen(1000, nullptr), 1002);
  EXPECT_TRUE(codec->MakeCompressor().status().IsNotImplemented());
  EXPECT_TRUE(codec->MakeDecompressor().status().IsNotImplemented());
}

std::string Doubles(int n) {
  std::string out(static_cast<size_t>(n) * 8, '\0');
  for (int i = 0; i < n; ++i) {
    const double v = (10000 + i % 300) / 100.0;
    std::memcpy(&out[static_cast<size_t>(i) * 8], &v, 8);
  }
  return out;
}

/// payload byte 1 is the mode, byte 2 the dtype (spec section 4)
std::pair<int, int> ModeAndDtype(const std::string& payload) {
  return {static_cast<uint8_t>(payload[1]), payload.size() > 2 ? static_cast<uint8_t>(payload[2]) : -1};
}

TEST(Codec, TheTypeHintAndDecimalFlagReachTheEncoder) {
  const std::string src = Doubles(10000);
  struct Case {
    int physical_type;
    bool decimal;
    int mode;
    int dtype;
  };
  // DOUBLE: DECIMAL (2) when tried, else PCO (1); INT64 reads the same bytes as integers; no
  // hint (or a type pcodec does not model): ZSTD (3)
  for (const Case& c : {Case{5, true, 2, 5}, Case{5, false, 1, 5}, Case{2, true, 1, 2},
                        Case{1, true, 1, 1}, Case{4, false, 1, 4}, Case{-1, true, 3, 0x28},
                        Case{6, true, 3, 0x28}, Case{0, true, 3, 0x28}}) {
    PcodecCodecOptions options;
    options.physical_type = c.physical_type;
    options.decimal = c.decimal;
    auto codec = MakeCodec(options);
    const std::string payload = Compress(*codec, src);
    ASSERT_GE(payload.size(), 3u);
    EXPECT_EQ(payload[0], 1) << "version";
    EXPECT_EQ(ModeAndDtype(payload), std::make_pair(c.mode, c.dtype)) << c.physical_type;
    ASSERT_OK_AND_ASSIGN(auto back, Decompress(*codec, payload, static_cast<int64_t>(src.size())));
    EXPECT_EQ(back, src);
  }
  // plain CodecOptions: no hint, decimal on; the level is kept
  arrow::util::CodecOptions plain(3);
  auto codec = MakeCodec(plain);
  EXPECT_EQ(codec->compression_level(), 3);
  EXPECT_EQ(ModeAndDtype(Compress(*codec, src)).first, 3);
  // level 0 and 12 both round trip and differ
  PcodecCodecOptions low, high;
  low.physical_type = high.physical_type = 2;
  low.compression_level = 0;
  high.compression_level = 12;
  const auto a = Compress(*MakeCodec(low), src), b = Compress(*MakeCodec(high), src);
  EXPECT_NE(a, b);
}

TEST(Codec, ErrorsAreStatusesNotCrashes) {
  auto codec = MakeCodec();
  uint8_t out[16];
  // a payload too small for its header, a negative size, a negative input length
  EXPECT_TRUE(codec->Decompress(1, reinterpret_cast<const uint8_t*>("\x01"), 16, out).status().IsIOError());
  // (the codec refuses a negative length itself, before the cast to size_t; the C ABI would
  // refuse the huge size_t as an argument error too, so the message tells the two apart)
  const auto negative = [](const arrow::Status& s, const std::string& message) {
    EXPECT_TRUE(s.IsInvalid()) << s.ToString();
    EXPECT_EQ(s.message(), message) << s.ToString();
  };
  negative(codec->Decompress(2, reinterpret_cast<const uint8_t*>("\x01\x00"), -1, out).status(),
           "pcodec decompress: negative length");
  negative(codec->Decompress(-1, out, 16, out).status(), "pcodec decompress: negative length");
  negative(codec->Compress(-1, out, 16, out).status(), "pcodec compress: negative length");
  negative(codec->Compress(4, out, -1, out).status(), "pcodec compress: negative length");
  // an output buffer below MaxCompressedLen is an argument error (Invalid)
  const std::string src = Doubles(100);
  std::string small(src.size(), '\0');
  auto r = codec->Compress(static_cast<int64_t>(src.size()), reinterpret_cast<const uint8_t*>(src.data()),
                           static_cast<int64_t>(small.size()), reinterpret_cast<uint8_t*>(small.data()));
  ASSERT_FALSE(r.ok());
  EXPECT_TRUE(r.status().IsInvalid()) << r.status().ToString();
  EXPECT_NE(r.status().message().find("pcodec compress failed: dst_cap"), std::string::npos);
  // empty input: a RAW payload of two bytes, decoded back to nothing
  const std::string empty_payload = Compress(*codec, "");
  EXPECT_EQ(empty_payload, std::string("\x01\x00", 2));
  ASSERT_OK_AND_ASSIGN(auto none, Decompress(*codec, empty_payload, 0));
  EXPECT_EQ(none, "");
}

TEST(Codec, RandomPayloadsFailCleanly) {
  auto codec = MakeCodec();
  std::mt19937_64 rng(20547);
  int ok = 0;
  for (int i = 0; i < 3000; ++i) {
    std::string payload(rng() % 64, '\0');
    for (auto& c : payload) c = static_cast<char>(rng());
    if (payload.size() >= 2 && rng() % 2) {
      payload[0] = 1;                                // a valid version
      payload[1] = static_cast<char>(rng() % 4);     // a valid mode
      if (payload.size() >= 3) payload[2] = "\x01\x02\x04\x05"[rng() % 4];  // a valid dtype
    }
    const int64_t size = static_cast<int64_t>(rng() % 256);
    ok += Decompress(*codec, payload, size).ok();
  }
  EXPECT_LT(ok, 300);  // a few RAW payloads of the right length decode
}

}  // namespace
}  // namespace tparquet::test
