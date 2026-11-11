// The conformance vectors of testdata/: every file read through the fork's
// parquet::arrow::FileReader, through tparquet::ReadTable and through the C stream reader
// (rust/cread) gives exactly expected/<name>.jsonl; the footer, codecs, physical types and page
// types agree with files/manifest.json.
#include <cstring>
#include <map>
#include <set>

#include <arrow/c/bridge.h>
#include <parquet/column_page.h>
#include <parquet/column_reader.h>
#include <rapidjson/document.h>

#include "common.h"

#ifdef TPARQUET_HAVE_READ
#include "tparquet_read.h"
#endif

namespace tparquet::test {
namespace {

std::string Expected(const std::filesystem::path& file) {
  return ReadFile(file.parent_path().parent_path() / "expected" /
                  (file.stem().string() + ".jsonl"));
}

TEST(Conformance, ThereAreVectors) {
  auto files = VectorFiles();
  EXPECT_GE(files.size(), 24u);
  EXPECT_EQ(files.front().filename(), "all_null_pages.parquet");
}

TEST(Conformance, TheForkReadsEveryVector) {
  for (const auto& file : VectorFiles()) {
    ASSERT_OK_AND_ASSIGN(auto in, arrow::io::ReadableFile::Open(file.string()));
    ASSERT_OK_AND_ASSIGN(auto reader, parquet::arrow::OpenFile(in, arrow::default_memory_pool()));
    ASSERT_OK_AND_ASSIGN(auto table, reader->ReadTable());
    EXPECT_TRUE(Jsonl(*table) == Expected(file)) << file.filename() << ": rows differ";
  }
}

TEST(Conformance, TheForkReadsEveryVectorBatchByBatch) {
  for (const auto& file : VectorFiles()) {
    ASSERT_OK_AND_ASSIGN(auto in, arrow::io::ReadableFile::Open(file.string()));
    parquet::ArrowReaderProperties props;
    props.set_batch_size(100);
    parquet::arrow::FileReaderBuilder builder;
    ASSERT_OK(builder.Open(in));
    builder.properties(props);
    ASSERT_OK_AND_ASSIGN(auto reader, builder.Build());
    ASSERT_OK_AND_ASSIGN(auto batches, reader->GetRecordBatchReader());
    std::string got;
    for (auto b : *batches) {
      ASSERT_OK(b.status());
      EXPECT_LE((*b)->num_rows(), 100);
      ASSERT_OK_AND_ASSIGN(auto lines, CanonicalJsonLines(**b));
      for (const auto& l : lines) got += l + "\n";
    }
    EXPECT_TRUE(got == Expected(file)) << file.filename();
  }
}

TEST(Conformance, ReadTableReadsEveryVector) {
  for (const auto& file : VectorFiles()) {
    ASSERT_OK_AND_ASSIGN(auto table, ReadTable(file.string()));
    EXPECT_TRUE(Jsonl(*table) == Expected(file)) << file.filename();
  }
}

std::string CodecName(arrow::Compression::type c) {
  std::string name = arrow::util::Codec::GetCodecAsString(c);
  for (auto& ch : name) ch = static_cast<char>(std::toupper(static_cast<unsigned char>(ch)));
  return name;
}

std::string PageTypeName(parquet::PageType::type t) {
  switch (t) {
    case parquet::PageType::DATA_PAGE:
      return "DATA_PAGE";
    case parquet::PageType::DATA_PAGE_V2:
      return "DATA_PAGE_V2";
    case parquet::PageType::DICTIONARY_PAGE:
      return "DICTIONARY_PAGE";
    default:
      return "OTHER";
  }
}

TEST(Conformance, FooterCodecsAndPagesMatchTheManifest) {
  rapidjson::Document doc;
  doc.Parse(ReadFile(Testdata() / "files" / "manifest.json").c_str());
  ASSERT_FALSE(doc.HasParseError());
  int files = 0, pcodec_chunks = 0;
  for (const auto& entry : doc["files"].GetArray()) {
    const auto path = Testdata() / entry["file"].GetString();
    SCOPED_TRACE(path.filename().string());
    auto reader = parquet::ParquetFileReader::OpenFile(path.string());
    auto md = reader->metadata();
    EXPECT_EQ(md->num_rows(), entry["rows"].GetInt64());
    EXPECT_EQ(md->num_row_groups(), entry["row_groups"].GetInt());
    const auto& version = entry["pcodec_version"];
    EXPECT_EQ(FooterValue(*md, kPcodecVersionKey), version.IsNull() ? "" : version.GetString());
    const auto& columns = entry["columns"].GetArray();
    ASSERT_EQ(md->num_columns(), static_cast<int>(columns.Size()));
    for (int c = 0; c < md->num_columns(); ++c) {
      const auto& col = columns[static_cast<rapidjson::SizeType>(c)];
      const auto* descr = md->schema()->Column(c);
      EXPECT_EQ(descr->path()->ToDotString(), col["path"].GetString());
      EXPECT_EQ(parquet::TypeToString(descr->physical_type()), col["physical_type"].GetString());
      std::set<std::string> codecs;
      std::map<std::string, int> pages;
      for (int rg = 0; rg < md->num_row_groups(); ++rg) {
        auto chunk = md->RowGroup(rg)->ColumnChunk(c);
        codecs.insert(CodecName(chunk->compression()));
        pcodec_chunks += chunk->compression() == arrow::Compression::PCODEC;
        EXPECT_TRUE(chunk->can_decompress());
        // every page decodes through the fork's page reader
        auto page_reader = reader->RowGroup(rg)->GetColumnPageReader(c);
        while (auto page = page_reader->NextPage()) pages[PageTypeName(page->type())]++;
      }
      std::set<std::string> want_codecs;
      for (const auto& w : col["codecs"].GetArray()) want_codecs.insert(w.GetString());
      EXPECT_EQ(codecs, want_codecs) << descr->path()->ToDotString();
      std::map<std::string, int> want_pages;
      for (const auto& p : col["pages"].GetObject()) want_pages[p.name.GetString()] = p.value.GetInt();
      // parquet-cpp's page reader stops after a chunk's num_values, so it never reaches the
      // dictionary page of a chunk without values (empty.parquet)
      if (md->num_rows() > 0) {
        EXPECT_EQ(pages, want_pages) << descr->path()->ToDotString();
      }
    }
    files++;
  }
  EXPECT_EQ(files, static_cast<int>(VectorFiles().size()));
  EXPECT_GE(pcodec_chunks, 51);
}

TEST(Conformance, OtherCodecValuesAreStillRefused) {
  // the fork accepts exactly 20547 beyond the standard range: in the footer of mixed.parquet,
  // replace the codec values (thrift compact: zigzag varints, 20547 is 86 c1 02) by 20548
  // (88 c1 02)
  const std::string file = ReadFile(Testdata() / "files" / "mixed.parquet");
  uint32_t footer_len;
  std::memcpy(&footer_len, file.data() + file.size() - 8, 4);
  const size_t footer = file.size() - 8 - footer_len;
  const std::string pcodec("\x86\xc1\x02", 3);
  for (const auto& [other, value] : {std::pair{std::string("\x88\xc1\x02", 3), 20548}}) {
    std::string bad = file;
    int replaced = 0;
    for (size_t at = bad.find(pcodec, footer); at != std::string::npos; at = bad.find(pcodec, at + 3)) {
      bad.replace(at, 3, other);
      replaced++;
    }
    ASSERT_EQ(replaced, 3) << "ts, price, qty";
    auto table = ReadBuffer(arrow::Buffer::FromString(bad));
    ASSERT_FALSE(table.ok());
    EXPECT_NE(table.status().message().find("Unsupported compression codec value: " + std::to_string(value)),
              std::string::npos)
        << table.status().ToString();
  }
}

#ifdef TPARQUET_HAVE_READ

/// The file read through the C stream reader, imported into Arrow C++.
arrow::Result<std::shared_ptr<arrow::Table>> ReadThroughCStream(const std::filesystem::path& file,
                                                                int64_t batch_size = 0) {
  struct ArrowArrayStream stream;
  if (tpr_open(file.c_str(), nullptr, 0, batch_size, &stream) != TPR_OK) {
    char msg[512];
    tpr_last_error(msg, sizeof msg);
    return arrow::Status::IOError(msg);
  }
  ARROW_ASSIGN_OR_RAISE(auto reader, arrow::ImportRecordBatchReader(&stream));
  return reader->ToTable();
}

TEST(Conformance, TheCStreamReaderReadsEveryVectorAsTheForkDoes) {
  for (const auto& file : VectorFiles()) {
    ASSERT_OK_AND_ASSIGN(auto from_c, ReadThroughCStream(file, 777));
    ASSERT_OK_AND_ASSIGN(auto from_fork, ReadTable(file.string()));
    const std::string c_rows = Jsonl(*from_c);
    EXPECT_TRUE(c_rows == Expected(file)) << file.filename() << ": C stream rows differ";
    EXPECT_TRUE(c_rows == Jsonl(*from_fork)) << file.filename() << ": the two readers disagree";
    EXPECT_EQ(from_c->num_columns(), from_fork->num_columns());
    for (int i = 0; i < from_c->num_columns(); ++i) {
      EXPECT_EQ(from_c->field(i)->name(), from_fork->field(i)->name());
    }
  }
}

#endif

}  // namespace
}  // namespace tparquet::test
