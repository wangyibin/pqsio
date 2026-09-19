use super::*;
use rust_htslib::bam::{self, record::Aux, Read};
use std::io::Read as IoRead;

pub(super) fn load(
    path: &Path,
    options: &ImportOptions,
    need_identity: bool,
    refs: &mut References,
    stats: &mut Statistics,
    sorter: &mut sort::Sorter,
) -> Result<()> {
    // Restrict this entry point to BAM; CRAM requires a separate reference API.
    let mut magic = [0; 4];
    crate::io::common_reader(path, 1)?
        .read_exact(&mut magic)
        .with_context(|| format!("cannot read BAM header: {}", path.display()))?;
    ensure!(
        &magic == b"BAM\x01",
        "input is not a BAM file: {}",
        path.display()
    );
    ensure!(
        options.threads <= i32::MAX as usize,
        "BAM threads must fit Int32"
    );
    let mut reader = bam::Reader::from_path(path)
        .with_context(|| format!("cannot open BAM: {}", path.display()))?;
    if options.threads > 1 {
        reader
            .set_threads(options.threads)
            .context("cannot set BAM decoder threads")?;
    }
    invalid((|| {
        for (tid, name) in reader.header().target_names().iter().enumerate() {
            let name = std::str::from_utf8(name).context("BAM reference name is not UTF-8")?;
            ensure!(
                !refs.ids.contains_key(name),
                "duplicate BAM reference: {name}"
            );
            refs.add(
                name,
                reader
                    .header()
                    .target_len(tid as u32)
                    .context("missing BAM reference length")?,
            )?;
        }
        Ok(())
    })())?;
    let mut record = bam::Record::new();
    while let Some(result) = reader.read(&mut record) {
        result.with_context(|| {
            format!("BAM decoding failed after {} records", stats.input_records)
        })?;
        stats.input_records += 1;
        let flag = record.flags();
        if flag & 4 != 0 {
            stats.skipped_unmapped += 1;
            continue;
        }
        if flag & (512 | 1024) != 0 {
            stats.skipped_duplicate_or_qcfail += 1;
            continue;
        }
        if flag & 256 != 0 && !options.include_secondary {
            stats.skipped_secondary += 1;
            continue;
        }
        if record.mapq() < options.min_mapq {
            stats.skipped_mapq += 1;
            continue;
        }
        let parsed = invalid(
            parse(&record, stats.input_records, refs, need_identity)
                .with_context(|| format!("BAM record {}", stats.input_records)),
        )?;
        sorter.push(parsed)?;
    }
    Ok(())
}

fn parse(
    record: &bam::Record,
    ordinal: u64,
    refs: &References,
    need_identity: bool,
) -> Result<Record> {
    let name = std::str::from_utf8(record.qname()).context("BAM QNAME is not UTF-8")?;
    ensure!(
        !name.is_empty() && name != "*",
        "mapped BAM record requires QNAME"
    );
    ensure!(!name.contains('\0'), "BAM QNAME contains NUL");
    let chrom = u32::try_from(record.tid()).context("mapped BAM record has no reference")?;
    let reference = refs
        .contigs
        .get(chrom as usize)
        .context("BAM reference is absent from @SQ")?;
    let flag = record.flags();
    let mate = if flag & 1 != 0 {
        ensure!(
            (flag & 64 != 0) != (flag & 128 != 0),
            "paired records require exactly one READ1/READ2 flag"
        );
        if flag & 64 != 0 {
            1
        } else {
            2
        }
    } else {
        0
    };
    let mut rg = "";
    let mut nm = None;
    let mut tags = HashSet::new();
    for item in record.aux_iter() {
        let (tag, value) = item.context("malformed BAM alignment tag")?;
        ensure!(tags.insert(tag), "duplicate BAM alignment tag");
        if tag == b"RG" {
            rg = match value {
                Aux::String(s) => s,
                _ => bail!("RG tag must have type Z"),
            };
            ensure!(!rg.contains('\0'), "RG contains NUL");
        } else if tag == b"NM" && need_identity {
            nm = Some(match value {
                Aux::I8(n) => n as i64,
                Aux::U8(n) => n as i64,
                Aux::I16(n) => n as i64,
                Aux::U16(n) => n as i64,
                Aux::I32(n) => n as i64,
                Aux::U32(n) => n as i64,
                _ => bail!("NM tag must have integer type"),
            });
        }
    }
    let ops: Vec<_> = record
        .cigar()
        .iter()
        .map(|op| (op.len(), op.char() as u8))
        .collect();
    let coords = cigar::coordinates(&ops, flag & 16 != 0, record.seq_len(), nm, need_identity)?;
    let start = u64::try_from(record.pos()).context("BAM POS must be nonnegative")?;
    let end = start
        .checked_add(coords.reference)
        .context("BAM reference end overflow")?;
    ensure!(
        end <= reference.length,
        "alignment exceeds contig {} length",
        reference.name
    );
    Ok(Record {
        name: name.into(),
        rg: rg.into(),
        ordinal,
        mate,
        row: Alignment {
            read_idx: 0,
            read_length: coords.length,
            read_start: coords.start,
            read_end: coords.end,
            strand: if flag & 16 != 0 { b'-' } else { b'+' },
            chrom,
            start,
            end,
            mapping_quality: record.mapq(),
            identity: coords.identity,
            filter_reason: "pass".into(),
        },
    })
}
