//! Validated Parquet compression settings shared by all writer bindings.
use anyhow::{bail, ensure, Result};
use polars::prelude::{BrotliLevel, GzipLevel, ParquetCompression, ZstdLevel};

/// Compression for every q0/q1 shard. Omitted levels use the codec default.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Compression(pub(crate) ParquetCompression);

impl Compression {
    /// Codecs: uncompressed, zstd (1..=22), gzip (0..=9), brotli (0..=11),
    /// snappy and lz4 (Parquet LZ4_RAW). The last two and uncompressed reject levels.
    pub fn new(codec: &str, level: Option<i32>) -> Result<Self> {
        let range = match codec {
            "zstd" => Some((1, 22)),
            "gzip" => Some((0, 9)),
            "brotli" => Some((0, 11)),
            "uncompressed" | "snappy" | "lz4" => None,
            _ => bail!("unknown compression {codec:?}; choose uncompressed, zstd, gzip, brotli, snappy or lz4"),
        };
        if let Some(level) = level {
            let Some((min, max)) = range else {
                bail!("compression {codec:?} does not accept a level");
            };
            ensure!(
                (min..=max).contains(&level),
                "{codec} compression level must be in {min}..={max}, got {level}"
            );
        }
        Ok(Self(match codec {
            "uncompressed" => ParquetCompression::Uncompressed,
            "snappy" => ParquetCompression::Snappy,
            "lz4" => ParquetCompression::Lz4Raw,
            "zstd" => ParquetCompression::Zstd(level.map(ZstdLevel::try_new).transpose()?),
            "gzip" => {
                ParquetCompression::Gzip(level.map(|v| GzipLevel::try_new(v as u8)).transpose()?)
            }
            "brotli" => ParquetCompression::Brotli(
                level.map(|v| BrotliLevel::try_new(v as u32)).transpose()?,
            ),
            _ => unreachable!(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ColumnBatch, Contig, Kind, Pair, ParallelOptions, ParallelWriter, Reader, Writer};
    use polars::prelude::{ParquetReader, SerReader};
    use std::{fs, path::PathBuf, time::SystemTime};

    #[test]
    fn compression_levels_and_defaults() {
        assert_eq!(
            Compression::default(),
            Compression::new("zstd", None).unwrap()
        );
        for (codec, min, max) in [("zstd", 1, 22), ("gzip", 0, 9), ("brotli", 0, 11)] {
            assert!(Compression::new(codec, None).is_ok());
            assert!(Compression::new(codec, Some(min)).is_ok());
            assert!(Compression::new(codec, Some(max)).is_ok());
            for level in [i32::MIN, min - 1, max + 1, i32::MAX] {
                assert!(Compression::new(codec, Some(level)).is_err());
            }
        }
        for codec in ["uncompressed", "snappy", "lz4"] {
            assert!(Compression::new(codec, None).is_ok());
            assert!(Compression::new(codec, Some(0)).is_err());
        }
        assert!(Compression::new("lzo", None).is_err());
        assert!(Compression::new("", None).is_err());
    }

    struct Output(PathBuf);
    impl Output {
        fn new() -> Self {
            let id = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!(
                "tests/output/compression-{}-{id}",
                std::process::id()
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Output {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn compression_is_stored_in_every_q0_q1_column() -> Result<()> {
        let root = Output::new();
        let contigs = vec![Contig {
            name: "chr1".into(),
            length: 100,
        }];
        let rows: Vec<_> = [0, 1, 60]
            .into_iter()
            .map(|mapq| Pair {
                read_id: format!("r{mapq}"),
                chrom1: 0,
                pos1: 1,
                chrom2: 0,
                pos2: 2,
                strand1: b'+',
                strand2: b'-',
                mapq,
            })
            .collect();
        for (codec, level, expected) in [
            ("uncompressed", None, "Uncompressed"),
            ("zstd", Some(6), "Zstd"),
            ("gzip", Some(0), "Gzip"),
            ("brotli", Some(4), "Brotli"),
            ("snappy", None, "Snappy"),
            ("lz4", None, "Lz4Raw"),
        ] {
            let compression = Compression::new(codec, level)?;
            let row_path = root.0.join(format!("{codec}-rows"));
            let column_path = root.0.join(format!("{codec}-columns"));
            let parallel_path = root.0.join(format!("{codec}-parallel"));
            let mut writer = Writer::create_with_compression(
                &row_path,
                Kind::Pairs,
                contigs.clone(),
                2,
                compression,
            )?;
            writer.write_pairs(&rows)?;
            writer.finish()?;
            let mut reader = Reader::open(&row_path, 0)?;
            let mut writer = Writer::create_with_compression(
                &column_path,
                Kind::Pairs,
                contigs.clone(),
                2,
                compression,
            )?;
            while let Some(ColumnBatch::Pairs(batch)) = reader.next_columns()? {
                writer.write_pairs_columns(batch.as_view())?;
            }
            writer.finish()?;
            let mut writer = ParallelWriter::create_with_compression(
                &parallel_path,
                Kind::Pairs,
                contigs.clone(),
                2,
                ParallelOptions::default(),
                compression,
            )?;
            writer.producer().write_pairs(0, rows.clone())?;
            writer.finish()?;
            for path in [row_path, column_path, parallel_path] {
                for quality in ["q0", "q1"] {
                    let mut count = 0;
                    for entry in fs::read_dir(path.join(quality))? {
                        let mut reader = ParquetReader::new(fs::File::open(entry?.path())?);
                        for group in &reader.get_metadata()?.row_groups {
                            for column in group.parquet_columns() {
                                assert_eq!(
                                    format!("{:?}", column.compression()),
                                    expected,
                                    "{} {quality}",
                                    path.display()
                                );
                                count += 1;
                            }
                        }
                    }
                    assert!(count > 0);
                }
            }
        }
        Ok(())
    }
}
