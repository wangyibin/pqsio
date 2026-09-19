use super::*;
use std::io::BufRead;

struct Columns {
    chrom1: usize,
    pos1: usize,
    chrom2: usize,
    pos2: usize,
    mapq: Option<usize>,
    ends: Option<(usize, usize)>,
    width: usize,
}
impl Columns {
    fn declared(fields: &[&str]) -> Result<Self> {
        let mut index = HashMap::new();
        for (i, name) in fields.iter().enumerate() {
            ensure!(
                index.insert(*name, i).is_none(),
                "duplicate #columns entry {name}"
            );
        }
        let get = |name| {
            index
                .get(name)
                .copied()
                .with_context(|| format!("#columns is missing {name}"))
        };
        let mapq = index.get("mapq").copied();
        let ends = match (index.get("mapq1"), index.get("mapq2")) {
            (Some(&a), Some(&b)) => Some((a, b)),
            (None, None) => None,
            _ => bail!("#columns requires both mapq1 and mapq2"),
        };
        ensure!(
            mapq.is_none() || ends.is_none(),
            "declare mapq or mapq1/mapq2, not both"
        );
        Ok(Self {
            chrom1: get("chrom1")?,
            pos1: get("pos1")?,
            chrom2: get("chrom2")?,
            pos2: get("pos2")?,
            mapq,
            ends,
            width: fields.len(),
        })
    }
    fn bare(width: usize) -> Result<Self> {
        ensure!(
            matches!(width, 7 | 8),
            "without #columns, expected 7 pairs columns or 8 columns with MAPQ last"
        );
        Ok(Self {
            chrom1: 1,
            pos1: 2,
            chrom2: 3,
            pos2: 4,
            mapq: if width == 8 { Some(7) } else { None },
            ends: None,
            width,
        })
    }
}

pub(super) fn load(
    path: &Path,
    options: &CoolOptions,
    refs: &mut References,
    stats: &mut Statistics,
    sorter: &mut sort::Sorter,
) -> Result<()> {
    if let Some(sizes) = &options.contigsizes {
        for (number, line) in crate::io::common_reader(sizes, options.threads)?
            .lines()
            .enumerate()
        {
            let line = line?;
            invalid((|| {
                let fields: Vec<_> = line.split_whitespace().collect();
                if fields.is_empty() || line.starts_with('#') {
                    return Ok(());
                }
                ensure!(
                    fields.len() >= 2 && !refs.ids.contains_key(fields[0]),
                    "{}:{}: expected unique chromosome and length",
                    sizes.display(),
                    number + 1
                );
                refs.add(
                    fields[0],
                    fields[1].parse().context("invalid chromosome length")?,
                )
            })())?;
        }
    }
    let mut reader = crate::io::common_reader(path, options.threads)?;
    let mut line = String::new();
    let mut number = 0u64;
    let mut columns = None;
    let mut seen = false;
    let mut header_chroms = std::collections::HashSet::new();
    while reader
        .read_line(&mut line)
        .context("cannot decode pairs text")?
        != 0
    {
        number += 1;
        let pixel = invalid(
            (|| -> Result<Option<(i64, i64)>> {
                if line.trim().is_empty() {
                    return Ok(None);
                }
                if let Some(header) = line.strip_prefix("#chromsize:") {
                    ensure!(!seen, "#chromsize header occurs after data records");
                    let fields: Vec<_> = header.split_whitespace().collect();
                    ensure!(
                        fields.len() == 2 && header_chroms.insert(fields[0].to_string()),
                        "malformed or duplicate #chromsize header"
                    );
                    ensure!(
                        options.contigsizes.is_none() || refs.ids.contains_key(fields[0]),
                        "header chromosome {} is missing from --contigsizes",
                        fields[0]
                    );
                    refs.add(
                        fields[0],
                        fields[1].parse().context("invalid chromosome length")?,
                    )?;
                    return Ok(None);
                }
                if let Some(header) = line.strip_prefix("#columns:") {
                    ensure!(
                        !seen && columns.is_none(),
                        "duplicate or late #columns header"
                    );
                    columns = Some(Columns::declared(
                        &header.split_whitespace().collect::<Vec<_>>(),
                    )?);
                    return Ok(None);
                }
                if line.starts_with('#') {
                    return Ok(None);
                }
                seen = true;
                ensure!(
                    !refs.contigs.is_empty(),
                    "pairs text requires #chromsize headers or --contigsizes"
                );
                let fields: Vec<_> = line.trim_end_matches(['\r', '\n']).split('\t').collect();
                if columns.is_none() {
                    columns = Some(Columns::bare(fields.len())?);
                }
                let c = columns.as_ref().unwrap();
                ensure!(
                    fields.len() == c.width,
                    "record column count differs from #columns or first record"
                );
                stats.input_records += 1;
                if [fields[c.chrom1], fields[c.chrom2]]
                    .iter()
                    .any(|name| matches!(*name, "!" | "*"))
                {
                    stats.skipped_unmapped += 1;
                    return Ok(None);
                }
                let quality = |index: usize| {
                    fields[index]
                        .parse::<u8>()
                        .context("MAPQ must be an integer in 0..=255")
                };
                let mapq = if let Some(index) = c.mapq {
                    Some(quality(index)?)
                } else if let Some((a, b)) = c.ends {
                    Some(quality(a)?.min(quality(b)?))
                } else {
                    None
                };
                ensure!(
                    options.min_mapq == 0 || mapq.is_some(),
                    "--min-mapq requires mapq or mapq1/mapq2 columns in pairs text"
                );
                if mapq.is_some_and(|q| q < options.min_mapq) {
                    stats.skipped_mapq += 1;
                    return Ok(None);
                }
                let bin = |chrom: usize, position: usize| -> Result<i64> {
                    let id = *refs
                        .ids
                        .get(fields[chrom])
                        .with_context(|| format!("unknown chromosome {}", fields[chrom]))?;
                    refs.bin(
                        id,
                        fields[position]
                            .parse()
                            .context("position must be a positive integer")?,
                    )
                };
                Ok(Some((bin(c.chrom1, c.pos1)?, bin(c.chrom2, c.pos2)?)))
            })()
            .with_context(|| format!("pairs line {number}")),
        )?;
        if let Some((a, b)) = pixel {
            sorter.push(a, b)?;
            stats.contacts += 1;
        }
        line.clear();
    }
    Ok(())
}
