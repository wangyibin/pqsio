use super::*;
use hdf5::types::{TypeDescriptor, VarLenUnicode};
use hdf5::{Dataset, Group, H5Type, Location};

fn string_attr(location: &Location, name: &str, value: &str) -> Result<()> {
    location
        .new_attr::<VarLenUnicode>()
        .shape(())
        .create(name)?
        .write_scalar(&value.parse::<VarLenUnicode>()?)?;
    Ok(())
}
fn integer_attr(location: &Location, name: &str, value: i64) -> Result<()> {
    location
        .new_attr::<i64>()
        .shape(())
        .create(name)?
        .write_scalar(&value)?;
    Ok(())
}
fn column<T: H5Type>(group: &Group, name: &str, batch: usize) -> Result<Dataset> {
    Ok(group
        .new_dataset::<T>()
        .shape((0..,))
        .chunk((batch.clamp(1, 65_536),))
        .shuffle()
        .deflate(6)
        .create(name)?)
}
pub(super) fn append<T: H5Type>(dataset: &Dataset, values: &[T]) -> Result<()> {
    if values.is_empty() {
        return Ok(());
    }
    let old = dataset.size();
    let new = old
        .checked_add(values.len())
        .context("HDF5 dataset size overflow")?;
    dataset.resize((new,))?;
    dataset.write_slice(values, old..new)?;
    Ok(())
}

fn names(group: &Group, refs: &References) -> Result<()> {
    let width = refs
        .contigs
        .iter()
        .map(|c| c.name.len())
        .max()
        .unwrap()
        .max(1);
    let total = width
        .checked_mul(refs.contigs.len())
        .context("chromosome names exceed address space")?;
    let mut bytes = vec![0u8; total];
    for (i, contig) in refs.contigs.iter().enumerate() {
        bytes[i * width..i * width + contig.name.len()].copy_from_slice(contig.name.as_bytes());
    }
    let data = group
        .new_dataset_builder()
        .empty_as(&TypeDescriptor::FixedAscii(width))
        .shape((refs.contigs.len(),))
        .deflate(6)
        .create("name")?;
    let dtype = data.dtype()?;
    // The runtime string width cannot be expressed as FixedAscii<const N>.
    // Memory has exactly n * width initialized bytes, matching the validated
    // fixed ASCII datatype. Use the same HDF5 lock as the safe crate API.
    let status = hdf5::sync::sync(|| unsafe {
        hdf5_sys::h5d::H5Dwrite(
            data.id(),
            dtype.id(),
            hdf5_sys::h5s::H5S_ALL,
            hdf5_sys::h5s::H5S_ALL,
            hdf5_sys::h5p::H5P_DEFAULT,
            bytes.as_ptr().cast(),
        )
    });
    ensure!(status >= 0, "HDF5 failed writing chromosome names");
    Ok(())
}

fn bins(file: &hdf5::File, refs: &References, batch: usize) -> Result<()> {
    let chroms = file.create_group("chroms")?;
    names(&chroms, refs)?;
    let lengths: Vec<i64> = refs.contigs.iter().map(|c| c.length as i64).collect();
    chroms
        .new_dataset_builder()
        .with_data(&lengths)
        .deflate(6)
        .create("length")?;
    let bins = file.create_group("bins")?;
    let chrom = column::<i32>(&bins, "chrom", batch)?;
    let start = column::<i64>(&bins, "start", batch)?;
    let end = column::<i64>(&bins, "end", batch)?;
    let (mut ids, mut starts, mut ends) = (vec![], vec![], vec![]);
    let flush = |ids: &mut Vec<i32>, starts: &mut Vec<i64>, ends: &mut Vec<i64>| -> Result<()> {
        append(&chrom, ids)?;
        append(&start, starts)?;
        append(&end, ends)?;
        ids.clear();
        starts.clear();
        ends.clear();
        Ok(())
    };
    for (id, contig) in refs.contigs.iter().enumerate() {
        let mut position = 0;
        while position < contig.length {
            let next = position.saturating_add(refs.bin_size).min(contig.length);
            ids.push(id as i32);
            starts.push(position as i64);
            ends.push(next as i64);
            position = next;
            if ids.len() >= batch {
                flush(&mut ids, &mut starts, &mut ends)?;
            }
        }
    }
    flush(&mut ids, &mut starts, &mut ends)
}

fn pixels(
    file: &hdf5::File,
    refs: &References,
    rows: &mut sort::Merged,
    batch: usize,
    threads: usize,
) -> Result<u64> {
    let group = file.create_group("pixels")?;
    let a = column::<i64>(&group, "bin1_id", batch)?;
    let b = column::<i64>(&group, "bin2_id", batch)?;
    let count = column::<i64>(&group, "count", batch)?;
    let chunk = batch.clamp(1, 65_536);
    let flush_rows = if threads > 1 { chunk } else { batch };
    let mut writer = compressed::Writer::new([a, b, count], chunk, threads)?;
    let indexes = file.create_group("indexes")?;
    indexes
        .new_dataset_builder()
        .with_data(&refs.offsets)
        .deflate(6)
        .create("chrom_offset")?;
    let offsets = column::<i64>(&indexes, "bin1_offset", batch)?;
    let mut columns: compressed::Columns = Default::default();
    let mut index = vec![];
    let (mut next_bin, mut nnz) = (0i64, 0i64);
    while let Some(row) = rows.next()? {
        while next_bin <= row.bin1 {
            index.push(nnz);
            next_bin += 1;
            if index.len() >= batch {
                append(&offsets, &index)?;
                index.clear();
            }
        }
        columns[0].push(row.bin1);
        columns[1].push(row.bin2);
        columns[2].push(row.count);
        nnz = nnz.checked_add(1).context("pixel count exceeds Int64")?;
        if columns[0].len() >= flush_rows {
            writer.append(&mut columns)?;
            progress::emit("Merging, compressing and writing Cooler", nnz as u64, 0);
        }
    }
    writer.append(&mut columns)?;
    writer.finish()?;
    let nbins = *refs.offsets.last().unwrap();
    while next_bin <= nbins {
        index.push(nnz);
        if index.len() >= batch {
            append(&offsets, &index)?;
            index.clear();
        }
        if next_bin == nbins {
            break;
        }
        next_bin += 1;
    }
    append(&offsets, &index)?;
    Ok(nnz as u64)
}

pub(super) fn cooler(
    path: &Path,
    source: &Path,
    refs: &References,
    rows: &mut sort::Merged,
    stats: &Statistics,
    options: &CoolOptions,
) -> Result<u64> {
    let file = hdf5::File::create(path)?;
    bins(&file, refs, options.batch_rows)?;
    let nnz = pixels(&file, refs, rows, options.batch_rows, options.threads)?;
    string_attr(&file, "format", "HDF5::Cooler")?;
    integer_attr(&file, "format-version", 3)?;
    string_attr(&file, "bin-type", "fixed")?;
    integer_attr(&file, "bin-size", options.bin_size as i64)?;
    string_attr(&file, "storage-mode", "symmetric-upper")?;
    integer_attr(&file, "nchroms", refs.contigs.len() as i64)?;
    integer_attr(&file, "nbins", *refs.offsets.last().unwrap())?;
    integer_attr(&file, "nnz", i64::try_from(nnz)?)?;
    integer_attr(
        &file,
        "sum",
        i64::try_from(stats.contacts).context("total contact count exceeds Int64")?,
    )?;
    string_attr(
        &file,
        "generated-by",
        concat!("pqsio-", env!("CARGO_PKG_VERSION")),
    )?;
    string_attr(&file, "creation-date", &chrono::Utc::now().to_rfc3339())?;
    string_attr(
        &file,
        "metadata",
        &obj([
            ("source", Value::from(source.to_string_lossy().as_ref())),
            ("min_mapq", Value::from(options.min_mapq as u64)),
        ])
        .json(),
    )?;
    file.flush()?;
    file.close()?;
    Ok(nnz)
}
