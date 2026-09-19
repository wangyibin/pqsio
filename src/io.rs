//! Buffered text I/O following CPhasing's common_reader/common_writer.
//! Gzip is detected by magic bytes, mgzip by its IG block headers. PQS Parquet
//! keeps its own compression; these helpers handle text inputs and sidecars.
use anyhow::{ensure, Context, Result};
use flate2::read::MultiGzDecoder;
use gzp::deflate::Mgzip;
use gzp::par::compress::{ParCompress, ParCompressBuilder};
use gzp::par::decompress::ParDecompressBuilder;
use gzp::ZWriter;
use std::fs::File;
use std::io::{self, BufRead, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;

const BUFFER_SIZE: usize = 256 * 1024;

// gzp 2.0.1 treats a short final block header as EOF. Check every framing
// boundary before enabling its parallel reader; otherwise MultiGzDecoder
// validates the stream. This also handles mixed gzip/mgzip members and BGZF.
fn mgzip_framing(file: &mut File) -> io::Result<bool> {
    let length = file.metadata()?.len();
    let mut offset = 0u64;
    while offset < length {
        if length - offset < 28 {
            return Ok(false);
        }
        file.seek(SeekFrom::Start(offset))?;
        let mut header = [0; 20];
        file.read_exact(&mut header)?;
        if header[..4] != [31, 139, 8, 4] || header[10..16] != [8, 0, b'I', b'G', 4, 0] {
            return Ok(false);
        }
        let size = u32::from_le_bytes(header[16..20].try_into().unwrap()) as u64;
        if size < 28 || size > length - offset {
            return Ok(false);
        }
        offset += size;
    }
    Ok(offset > 0)
}

/// Read a plain, gzip, multi-member gzip, BGZF or mgzip local text file.
/// threads controls mgzip workers; ordinary gzip decoding is serial.
pub fn common_reader(path: impl AsRef<Path>, threads: usize) -> Result<Box<dyn BufRead + Send>> {
    ensure!(threads > 0, "I/O threads must be positive");
    let path = path.as_ref();
    let mut file = File::open(path).with_context(|| format!("cannot open {}", path.display()))?;
    let mut magic = [0; 2];
    let gzip = file.read(&mut magic)? == 2 && magic == [31, 139];
    if !gzip {
        file.rewind()?;
        return Ok(Box::new(BufReader::with_capacity(BUFFER_SIZE, file)));
    }
    let parallel = threads > 1 && mgzip_framing(&mut file)?;
    file.rewind()?;
    if parallel {
        let reader = ParDecompressBuilder::<Mgzip>::new()
            .num_threads(threads)?
            .from_reader(BufReader::with_capacity(BUFFER_SIZE, file));
        Ok(Box::new(BufReader::with_capacity(BUFFER_SIZE, reader)))
    } else {
        Ok(Box::new(BufReader::with_capacity(
            BUFFER_SIZE,
            MultiGzDecoder::new(file),
        )))
    }
}

enum Output {
    Plain(BufWriter<File>),
    Mgzip(ParCompress<'static, Mgzip, BufWriter<File>>),
}

/// Explicit finish propagates compressor/footer/flush failures before publication.
pub struct CommonWriter {
    inner: Option<Output>,
}
impl Write for CommonWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        match self.inner.as_mut().unwrap() {
            Output::Plain(w) => w.write(buffer),
            Output::Mgzip(w) => w.write(buffer),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        match self.inner.as_mut().unwrap() {
            Output::Plain(w) => w.flush(),
            Output::Mgzip(w) => w.flush(),
        }
    }
}
impl CommonWriter {
    pub fn finish(mut self) -> Result<()> {
        let inner = self.inner.take().unwrap();
        catch_unwind(AssertUnwindSafe(|| -> Result<()> {
            match inner {
                Output::Plain(mut w) => w.flush()?,
                Output::Mgzip(mut w) => w.finish()?.flush()?,
            }
            Ok(())
        }))
        .map_err(|_| anyhow::anyhow!("compression worker failed while finishing output"))?
    }
}
impl Drop for CommonWriter {
    fn drop(&mut self) {
        // Upstream's compressed writer can panic from Drop on I/O errors.
        // Preserve a caller's original failure while joining its workers.
        if let Some(inner) = self.inner.take() {
            let _ = catch_unwind(AssertUnwindSafe(|| drop(inner)));
        }
    }
}

/// Write plain text, or parallel gzip-compatible mgzip for .gz/.mgz suffixes.
/// Call finish() and check its result; dropping alone cannot report I/O errors.
pub fn common_writer(path: impl AsRef<Path>, threads: usize) -> Result<CommonWriter> {
    ensure!(threads > 0, "I/O threads must be positive");
    let path = path.as_ref();
    let compressed = matches!(
        path.extension().and_then(|s| s.to_str()),
        Some("gz" | "mgz")
    );
    writer_from_file(File::create(path)?, compressed, threads)
}

pub(crate) fn writer_from_file(file: File, compressed: bool, threads: usize) -> Result<CommonWriter> {
    ensure!(threads > 0, "I/O threads must be positive");
    let writer = BufWriter::with_capacity(BUFFER_SIZE, file);
    let inner = if compressed {
        Output::Mgzip(
            ParCompressBuilder::<Mgzip>::new()
                .num_threads(threads)?
                .buffer_size(BUFFER_SIZE)?
                .from_writer(writer),
        )
    } else {
        Output::Plain(writer)
    };
    Ok(CommonWriter { inner: Some(inner) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};
    static SERIAL: AtomicU64 = AtomicU64::new(0);
    struct Temp(std::path::PathBuf);
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn directory() -> Result<Temp> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/output")
            .join(format!(
                "io-{}-{}",
                std::process::id(),
                SERIAL.fetch_add(1, Ordering::Relaxed)
            ));
        fs::create_dir_all(&path)?;
        Ok(Temp(path))
    }
    #[test]
    fn mgzip_roundtrip_plain_gzip_compatibility_and_errors() -> Result<()> {
        let root = directory()?;
        let data = "read\t100\t0\t10\t+\ta\t100\t0\t10\t9\t10\t30\n".repeat(20000);
        for extension in ["txt", "gz", "mgz"] {
            let path = root.0.join(format!("data.{extension}"));
            let mut writer = common_writer(&path, 2)?;
            writer.write_all(data.as_bytes())?;
            writer.finish()?;
            for threads in [1, 2] {
                let mut actual = String::new();
                common_reader(&path, threads)?.read_to_string(&mut actual)?;
                assert_eq!(actual, data);
            }
            if extension == "txt" {
                continue;
            }
            assert!(mgzip_framing(&mut File::open(&path)?)?);
            let mut actual = String::new();
            MultiGzDecoder::new(File::open(&path)?).read_to_string(&mut actual)?;
            assert_eq!(actual, data);
            let bytes = fs::read(&path)?;
            for cut in [bytes.len() - 1, bytes.len() - 8, 11] {
                let broken = root.0.join("broken.gz");
                fs::write(&broken, &bytes[..cut])?;
                assert!(common_reader(&broken, 2)?
                    .read_to_end(&mut Vec::new())
                    .is_err());
            }
            let mut bad_crc = bytes.clone();
            let last = bad_crc.len() - 8;
            bad_crc[last] ^= 1;
            let corrupt = root.0.join("crc.gz");
            fs::write(&corrupt, bad_crc)?;
            assert!(common_reader(&corrupt, 2)?
                .read_to_end(&mut Vec::new())
                .is_err());
        }
        Ok(())
    }
}
