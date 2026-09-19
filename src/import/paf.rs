use super::*;
use std::io::BufRead;

fn number<T: std::str::FromStr>(value: &str, field: &str) -> Result<T> {
    value
        .parse()
        .map_err(|_| anyhow::anyhow!("{field} must be a nonnegative integer in range"))
}

pub(super) fn load(
    path: &Path,
    options: &ImportOptions,
    refs: &mut References,
    stats: &mut Statistics,
    sorter: &mut sort::Sorter,
) -> Result<()> {
    if let Some(path) = &options.contigsizes {
        let mut reader = crate::io::common_reader(path, options.threads)?;
        let mut line = String::new();
        let mut number = 0;
        while reader.read_line(&mut line)? != 0 {
            number += 1;
            invalid((|| {
                let fields: Vec<_> = line.split_whitespace().collect();
                if fields.is_empty() || line.starts_with('#') {
                    return Ok(());
                }
                ensure!(
                    fields.len() >= 2 && !refs.ids.contains_key(fields[0]),
                    "{}:{number}: expected a unique contig and length",
                    path.display()
                );
                refs.add(
                    fields[0],
                    fields[1].parse().context("invalid contig length")?,
                )?;
                Ok(())
            })())?;
            line.clear();
        }
    }
    let mut reader = crate::io::common_reader(path, options.threads)?;
    let mut line = String::new();
    let mut line_number = 0u64;
    while reader
        .read_line(&mut line)
        .with_context(|| format!("cannot read PAF {}", path.display()))?
        != 0
    {
        line_number += 1;
        let parsed = invalid(
            parse(&line, line_number, options, refs, stats)
                .with_context(|| format!("PAF line {line_number}")),
        )?;
        if let Some(record) = parsed {
            sorter.push(record)?;
        }
        line.clear();
    }
    Ok(())
}

fn parse(
    line: &str,
    ordinal: u64,
    options: &ImportOptions,
    refs: &mut References,
    stats: &mut Statistics,
) -> Result<Option<Record>> {
    if line.trim().is_empty() || line.starts_with('#') {
        return Ok(None);
    }
    let fields: Vec<_> = line.trim_end_matches(['\r', '\n']).split('\t').collect();
    ensure!(
        fields.len() >= 12,
        "PAF requires at least 12 tab-separated fields"
    );
    let name = fields[0];
    ensure!(
        !name.is_empty() && name != "*",
        "PAF query name must be present"
    );
    ensure!(!name.contains('\0'), "query name contains NUL");
    let length: u32 = number(fields[1], "query length")?;
    let qstart: u32 = number(fields[2], "query start")?;
    let qend: u32 = number(fields[3], "query end")?;
    ensure!(
        matches!(fields[4], "+" | "-") && qstart < qend && qend <= length,
        "invalid query interval or strand"
    );
    let tlen: u64 = number(fields[6], "target length")?;
    let start: u64 = number(fields[7], "target start")?;
    let end: u64 = number(fields[8], "target end")?;
    let matches: u64 = number(fields[9], "matches")?;
    let block: u64 = number(fields[10], "block length")?;
    let mapq: u8 = number(fields[11], "MAPQ")?;
    ensure!(
        start < end && end <= tlen && matches <= block && block > 0,
        "invalid target interval or matches/block length"
    );
    ensure!(
        options.contigsizes.is_none() || refs.ids.contains_key(fields[5]),
        "contig {:?} is missing from --contigsizes",
        fields[5]
    );
    let chrom = refs.add(fields[5], tlen)?;
    let mut tags = HashSet::new();
    let mut secondary = false;
    for field in &fields[12..] {
        let mut parts = field.splitn(3, ':');
        let tag = parts.next().unwrap();
        let kind = parts.next().context("malformed alignment tag")?;
        let value = parts.next().context("malformed alignment tag")?;
        ensure!(
            tag.len() == 2 && kind.len() == 1,
            "malformed alignment tag: {field:?}"
        );
        ensure!(tags.insert(tag), "duplicate alignment tag: {tag}");
        if tag == "tp" {
            ensure!(kind == "A", "tp tag must have type A");
            secondary = value == "S";
        }
    }
    stats.input_records += 1;
    if secondary && !options.include_secondary {
        stats.skipped_secondary += 1;
        return Ok(None);
    }
    if mapq < options.min_mapq {
        stats.skipped_mapq += 1;
        return Ok(None);
    }
    Ok(Some(Record {
        rg: String::new(),
        name: name.into(),
        ordinal,
        mate: 0,
        row: Alignment {
            read_idx: 0,
            read_length: length,
            read_start: qstart,
            read_end: qend,
            strand: fields[4].as_bytes()[0],
            chrom,
            start,
            end,
            mapping_quality: mapq,
            identity: (matches as f64 / block as f64) as f32,
            filter_reason: "pass".into(),
        },
    }))
}
