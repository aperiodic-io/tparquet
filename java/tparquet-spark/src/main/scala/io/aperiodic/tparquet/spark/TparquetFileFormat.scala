package io.aperiodic.tparquet.spark

import io.aperiodic.tparquet.{Pcodec, TparquetWriter}
import java.util.Locale
import org.apache.hadoop.conf.Configuration
import org.apache.hadoop.fs.Path
import org.apache.hadoop.mapreduce.{Job, TaskAttemptContext}
import org.apache.parquet.conf.HadoopParquetConfiguration
import org.apache.parquet.hadoop.ParquetWriter
import org.apache.parquet.hadoop.util.HadoopOutputFile
import org.apache.spark.sql.SparkSession
import org.apache.spark.sql.catalyst.InternalRow
import org.apache.spark.sql.execution.datasources.{OutputWriter, OutputWriterFactory}
import org.apache.spark.sql.execution.datasources.parquet.{ParquetFileFormat, ParquetWriteSupport}
import org.apache.spark.sql.internal.SQLConf
import org.apache.spark.sql.sources.DataSourceRegister
import org.apache.spark.sql.types.StructType

/**
 * The `tparquet` data source: Spark's Parquet source, writing with PCODEC as spec/FORMAT.md
 * section 3 recommends (`io.aperiodic.tparquet.TparquetWriter`: V2 pages, PLAIN numbers without a
 * dictionary). Reading is Spark's Parquet reader, which reads PCODEC through the parquet-java
 * fork (`java/parquet-java`) in place of Spark's parquet jars; `spark.read.parquet` reads the same
 * files.
 *
 * Write options: `level` (pcodec level 0..12, default 8), `decimal` (DECIMAL mode for floats,
 * default true), `rowGroupSize` and `pageSize` in bytes (default `parquet.block.size` and
 * `parquet.page.size`, else 128 MiB and 1 MiB), `outputTimestampType` (`INT96`,
 * `TIMESTAMP_MICROS` or `TIMESTAMP_MILLIS`). `compression` may only be `pcodec`.
 *
 * Timestamps are written as `format("parquet")` writes them: as the session's
 * `spark.sql.parquet.outputTimestampType` says (Spark's default is INT96), with Spark's INT96
 * rebase (`spark.sql.parquet.int96RebaseModeInWrite`), unless the `outputTimestampType` option
 * overrides it. PCODEC does not model INT96, so INT96 columns are the payload's ZSTD fallback;
 * `outputTimestampType=TIMESTAMP_MICROS` (or the session setting) makes them INT64, which PCODEC
 * compresses.
 */
class TparquetFileFormat extends ParquetFileFormat with DataSourceRegister {
  override def shortName(): String = "tparquet"

  override def toString: String = "Tparquet"

  override def hashCode(): Int = getClass.hashCode()

  override def equals(other: Any): Boolean = other.isInstanceOf[TparquetFileFormat]

  override def prepareWrite(
      sparkSession: SparkSession,
      job: Job,
      options: Map[String, String],
      dataSchema: StructType): OutputWriterFactory = {
    val writerOptions = TparquetFileFormat.options(options, job.getConfiguration)
    // Spark's Parquet source puts the write support's settings (the row schema, the timestamp
    // and decimal formats) in the job's configuration; its codec and writer are not used. The
    // timestamp type is the session's spark.sql.parquet.outputTimestampType, as for `format("parquet")`;
    // the outputTimestampType option overrides it
    super.prepareWrite(sparkSession, job, options - "compression", dataSchema)
    TparquetFileFormat.option(options, "outputTimestampType").foreach { t =>
      job.getConfiguration.set(SQLConf.PARQUET_OUTPUT_TIMESTAMP_TYPE.key, t)
    }
    new TparquetOutputWriterFactory(writerOptions)
  }
}

object TparquetFileFormat {
  private[spark] def option(options: Map[String, String], key: String): Option[String] =
    options.collectFirst { case (k, v) if k.equalsIgnoreCase(key) => v }

  /** The writer settings of a write's options (see the class documentation). */
  def options(options: Map[String, String], conf: Configuration): TparquetWriter.Options = {
    option(options, "compression").map(_.toLowerCase(Locale.ROOT)) match {
      case None | Some("pcodec") =>
      case Some(other) =>
        throw new IllegalArgumentException(s"tparquet writes PCODEC: compression=$other is not supported")
    }
    var o = TparquetWriter.Options.defaults()
    o = o.withLevel(option(options, "level").map(_.toInt).getOrElse(Pcodec.DEFAULT_LEVEL))
    o = o.withDecimal(option(options, "decimal").forall(_.toBoolean))
    o = o.withRowGroupSize(option(options, "rowGroupSize").map(_.toLong).getOrElse(conf.getLong("parquet.block.size", 128L << 20)))
    o.withPageSize(option(options, "pageSize").map(_.toInt).getOrElse(conf.getInt("parquet.page.size", 1 << 20)))
  }
}

/** Creates a [[TparquetOutputWriter]] per task output file. */
class TparquetOutputWriterFactory(options: TparquetWriter.Options) extends OutputWriterFactory {
  override def getFileExtension(context: TaskAttemptContext): String = ".pcodec.parquet"

  override def newInstance(path: String, dataSchema: StructType, context: TaskAttemptContext): OutputWriter =
    new TparquetOutputWriter(path, context, options)
}

/** Writes Spark's rows with Spark's [[ParquetWriteSupport]] through [[TparquetWriter]]. */
class TparquetOutputWriter(val path: String, context: TaskAttemptContext, options: TparquetWriter.Options)
    extends OutputWriter {
  private val writer: ParquetWriter[InternalRow] = {
    val conf = context.getConfiguration
    val writeSupport = new ParquetWriteSupport()
    val schema = writeSupport.init(conf).getSchema
    TparquetWriter.create(
      HadoopOutputFile.fromPath(new Path(path), conf),
      writeSupport,
      schema,
      new HadoopParquetConfiguration(conf),
      options)
  }

  override def write(row: InternalRow): Unit = writer.write(row)

  override def close(): Unit = writer.close()
}
