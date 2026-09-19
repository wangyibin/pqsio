//! One-pass q0 quality summaries. No Python rows or per-pair/read-ID set.
use crate::metadata::{obj, Value};
use crate::*;
use std::collections::BTreeMap;

fn ratio(n: u64, d: u64) -> Value {
    if d == 0 {
        Value::Null
    } else {
        Value::Number((n as f64 / d as f64).to_string())
    }
}
fn histogram(values: &[u64]) -> Value {
    Value::List(values.iter().copied().map(Value::from).collect())
}

/// Scan q0 once; threshold filters summary records, never double-counting q1.
pub fn stats(path: impl AsRef<Path>, min_mapq: u8) -> Result<Value> {
    let path = path.as_ref();
    let mut reader = StreamingReader::open(path, 0, ReadOptions::default())?;
    let kind = reader.kind();
    let contigs = reader.contigs().to_vec();
    let mut per_contig = vec![0u64; contigs.len()];
    let (mut scanned, mut kept, mut cis, mut trans) = (0u64, 0u64, 0u64, 0u64);
    let mut mapq = [0u64; 256];
    let mut distances = [0u64; 5];
    let mut strands = BTreeMap::<String, u64>::new();
    let mut reasons = BTreeMap::<String, u64>::new();
    let mut orders = BTreeMap::<u64, u64>::new();
    let (mut previous, mut order, mut scanned_reads, mut reads) = (None, 0u64, 0u64, 0u64);
    let (mut read_length, mut read_bases, mut ref_bases) = (0u64, 0u128, 0u128);
    let mut chromosomes = HashSet::new();
    let mut multi_contig_reads = 0u64;
    let mut identity_sum = 0f64;
    let mut finite_identities = 0u64;
    let mut finish_read = |order: u64, length: u64, chromosome_count: usize| {
        if order > 0 {
            reads += 1;
            *orders.entry(order).or_default() += 1;
            read_bases += length as u128;
            multi_contig_reads += u64::from(chromosome_count > 1);
        }
    };
    progress::emit("Scanning q0 quality statistics", 0, 0);
    while let Some(batch) = reader.next_batch()? {
        match batch {
            Batch::Pairs(rows) => {
                for r in rows {
                    scanned += 1;
                    if r.mapq < min_mapq {
                        continue;
                    }
                    kept += 1;
                    mapq[r.mapq as usize] += 1;
                    per_contig[r.chrom1 as usize] += 1;
                    per_contig[r.chrom2 as usize] += 1;
                    *strands
                        .entry(format!("{}{}", r.strand1 as char, r.strand2 as char))
                        .or_default() += 1;
                    if r.chrom1 == r.chrom2 {
                        cis += 1;
                        let distance = r.pos1.abs_diff(r.pos2);
                        let bucket = [1_000, 10_000, 100_000, 1_000_000]
                            .partition_point(|edge| distance >= *edge);
                        distances[bucket] += 1;
                    } else {
                        trans += 1;
                    }
                }
            }
            Batch::Concat(rows) => {
                for r in rows {
                    scanned += 1;
                    if previous != Some(r.read_idx) {
                        finish_read(order, read_length, chromosomes.len());
                        previous = Some(r.read_idx);
                        scanned_reads += 1;
                        order = 0;
                        chromosomes.clear();
                        read_length = r.read_length as u64;
                    }
                    if r.mapping_quality < min_mapq {
                        continue;
                    }
                    kept += 1;
                    order += 1;
                    mapq[r.mapping_quality as usize] += 1;
                    per_contig[r.chrom as usize] += 1;
                    chromosomes.insert(r.chrom);
                    *reasons.entry(r.filter_reason).or_default() += 1;
                    ref_bases += r
                        .end
                        .checked_sub(r.start)
                        .context("invalid concat reference interval")?
                        as u128;
                    if r.identity.is_finite() {
                        identity_sum += r.identity as f64;
                        finite_identities += 1;
                    }
                }
            }
        }
        progress::emit("Scanning q0 quality statistics", scanned, 0);
    }
    finish_read(order, read_length, chromosomes.len());
    let mut fields = vec![
        ("path", Value::from(path.to_string_lossy().as_ref())),
        (
            "format",
            Value::from(if kind == Kind::Pairs {
                "pairs"
            } else {
                "concat"
            }),
        ),
        ("source_quality", Value::from("q0")),
        ("min_mapq", Value::from(min_mapq as u64)),
        ("scanned_records", scanned.into()),
        ("selected_records", kept.into()),
        ("filtered_records", (scanned - kept).into()),
        ("selected_fraction", ratio(kept, scanned)),
        ("mapq_histogram", histogram(&mapq)),
        ("mapq_zero_fraction", ratio(mapq[0], kept)),
        ("mapq_ge30_fraction", ratio(mapq[30..].iter().sum(), kept)),
        (
            "per_contig",
            Value::List(
                contigs
                    .iter()
                    .zip(per_contig)
                    .map(|(c, n)| {
                        obj([
                            ("contig", Value::from(c.name.as_str())),
                            ("count", n.into()),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "per_contig_unit",
            Value::from(if kind == Kind::Pairs {
                "pair_endpoints"
            } else {
                "alignments"
            }),
        ),
    ];
    if kind == Kind::Pairs {
        fields.extend([
            ("cis_records", cis.into()),
            ("trans_records", trans.into()),
            ("cis_fraction", ratio(cis, kept)),
            ("trans_fraction", ratio(trans, kept)),
            (
                "cis_distance_bp",
                obj([
                    "0-999",
                    "1000-9999",
                    "10000-99999",
                    "100000-999999",
                    "1000000+",
                ]
                .into_iter()
                .zip(distances.map(Value::from))),
            ),
            (
                "strand_pairs",
                obj(strands.into_iter().map(|(s, n)| (s, n.into()))),
            ),
        ]);
    } else {
        fields.extend([
            ("scanned_reads", scanned_reads.into()),
            ("selected_reads", reads.into()),
            (
                "read_order_histogram",
                obj(orders
                    .into_iter()
                    .map(|(n, count)| (n.to_string(), count.into()))),
            ),
            ("mean_read_order", ratio(kept, reads)),
            (
                "mean_read_length",
                if reads == 0 {
                    Value::Null
                } else {
                    Value::Number((read_bases as f64 / reads as f64).to_string())
                },
            ),
            ("multi_contig_reads", multi_contig_reads.into()),
            (
                "filter_reasons",
                obj(reasons.into_iter().map(|(s, n)| (s, n.into()))),
            ),
            (
                "aligned_reference_bases",
                Value::Number(ref_bases.to_string()),
            ),
            (
                "mean_identity",
                if finite_identities == 0 {
                    Value::Null
                } else {
                    Value::Number((identity_sum / finite_identities as f64).to_string())
                },
            ),
            (
                "nonfinite_identity_records",
                (kept - finite_identities).into(),
            ),
        ]);
    }
    Ok(obj(fields))
}
