//! Owned column batches and borrowed, immutable submission views.
//! See docs/columnar.md for wire types, offsets and ownership contracts.
use crate::*;

fn strings<'a>(offsets: &[u64], bytes: &'a [u8], n: usize) -> Result<Vec<&'a str>> {
    ensure!(
        offsets.len() == n.checked_add(1).context("column too large")?,
        "string offsets length must be rows + 1"
    );
    ensure!(
        offsets.first() == Some(&0) && offsets.last() == Some(&(bytes.len() as u64)),
        "string offsets must span bytes"
    );
    offsets
        .windows(2)
        .map(|w| {
            ensure!(
                w[0] <= w[1] && w[1] <= bytes.len() as u64,
                "invalid string offsets"
            );
            Ok(std::str::from_utf8(&bytes[w[0] as usize..w[1] as usize])?)
        })
        .collect()
}
fn append_strings(
    dst_offsets: &mut Vec<u64>,
    dst_bytes: &mut Vec<u8>,
    offsets: &[u64],
    bytes: &[u8],
    a: usize,
    b: usize,
) {
    let base = dst_bytes.len() as u64;
    dst_offsets.extend(offsets[a + 1..=b].iter().map(|v| base + v - offsets[a]));
    dst_bytes.extend_from_slice(&bytes[offsets[a] as usize..offsets[b] as usize]);
}
#[derive(Debug, PartialEq)]
pub enum ColumnBatch {
    Pairs(PairColumns),
    Concat(ConcatColumns),
}
#[derive(Clone, Debug, PartialEq)]
pub struct PairColumns {
    pub read_id_offsets: Vec<u64>,
    pub read_id_bytes: Vec<u8>,
    pub chrom1: Vec<u32>,
    pub pos1: Vec<u64>,
    pub chrom2: Vec<u32>,
    pub pos2: Vec<u64>,
    pub strand1: Vec<u8>,
    pub strand2: Vec<u8>,
    pub mapq: Vec<u8>,
}
#[derive(Clone, Copy, Debug)]
pub struct PairColumnsView<'a> {
    pub read_id_offsets: &'a [u64],
    pub read_id_bytes: &'a [u8],
    pub chrom1: &'a [u32],
    pub pos1: &'a [u64],
    pub chrom2: &'a [u32],
    pub pos2: &'a [u64],
    pub strand1: &'a [u8],
    pub strand2: &'a [u8],
    pub mapq: &'a [u8],
}
impl Default for PairColumns {
    fn default() -> Self {
        Self {
            read_id_offsets: vec![0],
            read_id_bytes: vec![],
            chrom1: vec![],
            pos1: vec![],
            chrom2: vec![],
            pos2: vec![],
            strand1: vec![],
            strand2: vec![],
            mapq: vec![],
        }
    }
}
impl PairColumns {
    pub fn as_view(&self) -> PairColumnsView<'_> {
        PairColumnsView {
            read_id_offsets: &self.read_id_offsets,
            read_id_bytes: &self.read_id_bytes,
            chrom1: &self.chrom1,
            pos1: &self.pos1,
            chrom2: &self.chrom2,
            pos2: &self.pos2,
            strand1: &self.strand1,
            strand2: &self.strand2,
            mapq: &self.mapq,
        }
    }
    pub(crate) fn append(&mut self, v: PairColumnsView<'_>, a: usize, b: usize) {
        self.chrom1.extend_from_slice(&v.chrom1[a..b]);
        self.pos1.extend_from_slice(&v.pos1[a..b]);
        self.chrom2.extend_from_slice(&v.chrom2[a..b]);
        self.pos2.extend_from_slice(&v.pos2[a..b]);
        self.strand1.extend_from_slice(&v.strand1[a..b]);
        self.strand2.extend_from_slice(&v.strand2[a..b]);
        self.mapq.extend_from_slice(&v.mapq[a..b]);
        append_strings(
            &mut self.read_id_offsets,
            &mut self.read_id_bytes,
            v.read_id_offsets,
            v.read_id_bytes,
            a,
            b,
        );
    }
}
impl PairColumnsView<'_> {
    fn shape(&self) -> Result<Vec<&str>> {
        let n = self.pos1.len();
        ensure!(self.chrom1.len() == n, "chrom1 length must equal rows");
        ensure!(self.chrom2.len() == n, "chrom2 length must equal rows");
        ensure!(self.pos2.len() == n, "pos2 length must equal rows");
        ensure!(self.strand1.len() == n, "strand1 length must equal rows");
        ensure!(self.strand2.len() == n, "strand2 length must equal rows");
        ensure!(self.mapq.len() == n, "mapq length must equal rows");
        strings(self.read_id_offsets, self.read_id_bytes, n)
    }
    pub(crate) fn frame(&self, contigs: &[Contig]) -> Result<DataFrame> {
        let text = self.shape()?;
        storage_frame(
            Kind::Pairs,
            vec![
                Series::new("read_idx".into(), text).into(),
                Series::new(
                    "chrom1".into(),
                    self.chrom1
                        .iter()
                        .map(|&id| contigs[id as usize].name.as_str())
                        .collect::<Vec<_>>(),
                )
                .into(),
                Series::new("pos1".into(), self.pos1).into(),
                Series::new(
                    "chrom2".into(),
                    self.chrom2
                        .iter()
                        .map(|&id| contigs[id as usize].name.as_str())
                        .collect::<Vec<_>>(),
                )
                .into(),
                Series::new("pos2".into(), self.pos2).into(),
                Series::new(
                    "strand1".into(),
                    self.strand1
                        .iter()
                        .map(|&s| if s == b'+' { "+" } else { "-" })
                        .collect::<Vec<_>>(),
                )
                .into(),
                Series::new(
                    "strand2".into(),
                    self.strand2
                        .iter()
                        .map(|&s| if s == b'+' { "+" } else { "-" })
                        .collect::<Vec<_>>(),
                )
                .into(),
                Series::new("mapq".into(), self.mapq).into(),
            ],
            contigs,
        )
    }
}
#[derive(Clone, Debug, PartialEq)]
pub struct ConcatColumns {
    pub read_offsets: Vec<u64>,
    pub read_idx: Vec<u64>,
    pub read_length: Vec<u32>,
    pub read_start: Vec<u32>,
    pub read_end: Vec<u32>,
    pub strand: Vec<u8>,
    pub chrom: Vec<u32>,
    pub start: Vec<u64>,
    pub end: Vec<u64>,
    pub mapping_quality: Vec<u8>,
    pub identity: Vec<f32>,
    pub filter_reason_offsets: Vec<u64>,
    pub filter_reason_bytes: Vec<u8>,
}
#[derive(Clone, Copy, Debug)]
pub struct ConcatColumnsView<'a> {
    pub read_offsets: &'a [u64],
    pub read_idx: &'a [u64],
    pub read_length: &'a [u32],
    pub read_start: &'a [u32],
    pub read_end: &'a [u32],
    pub strand: &'a [u8],
    pub chrom: &'a [u32],
    pub start: &'a [u64],
    pub end: &'a [u64],
    pub mapping_quality: &'a [u8],
    pub identity: &'a [f32],
    pub filter_reason_offsets: &'a [u64],
    pub filter_reason_bytes: &'a [u8],
}
impl Default for ConcatColumns {
    fn default() -> Self {
        Self {
            read_offsets: vec![0],
            read_idx: vec![],
            read_length: vec![],
            read_start: vec![],
            read_end: vec![],
            strand: vec![],
            chrom: vec![],
            start: vec![],
            end: vec![],
            mapping_quality: vec![],
            identity: vec![],
            filter_reason_offsets: vec![0],
            filter_reason_bytes: vec![],
        }
    }
}
impl ConcatColumns {
    pub fn as_view(&self) -> ConcatColumnsView<'_> {
        ConcatColumnsView {
            read_offsets: &self.read_offsets,
            read_idx: &self.read_idx,
            read_length: &self.read_length,
            read_start: &self.read_start,
            read_end: &self.read_end,
            strand: &self.strand,
            chrom: &self.chrom,
            start: &self.start,
            end: &self.end,
            mapping_quality: &self.mapping_quality,
            identity: &self.identity,
            filter_reason_offsets: &self.filter_reason_offsets,
            filter_reason_bytes: &self.filter_reason_bytes,
        }
    }
    pub(crate) fn append(&mut self, v: ConcatColumnsView<'_>, a: usize, b: usize) {
        if a == b {
            return;
        }
        // Rebase offsets and merge a read continued from an earlier row group.
        let base = self.read_idx.len();
        let previous = self.read_idx.last().copied();
        self.read_offsets.pop();
        for i in a..b {
            let prev = if i == a {
                previous
            } else {
                Some(v.read_idx[i - 1])
            };
            if prev != Some(v.read_idx[i]) {
                self.read_offsets.push((base + i - a) as u64);
            }
        }
        self.read_idx.extend_from_slice(&v.read_idx[a..b]);
        self.read_length.extend_from_slice(&v.read_length[a..b]);
        self.read_start.extend_from_slice(&v.read_start[a..b]);
        self.read_end.extend_from_slice(&v.read_end[a..b]);
        self.strand.extend_from_slice(&v.strand[a..b]);
        self.chrom.extend_from_slice(&v.chrom[a..b]);
        self.start.extend_from_slice(&v.start[a..b]);
        self.end.extend_from_slice(&v.end[a..b]);
        self.mapping_quality
            .extend_from_slice(&v.mapping_quality[a..b]);
        self.identity.extend_from_slice(&v.identity[a..b]);
        append_strings(
            &mut self.filter_reason_offsets,
            &mut self.filter_reason_bytes,
            v.filter_reason_offsets,
            v.filter_reason_bytes,
            a,
            b,
        );
        self.read_offsets.push(self.read_idx.len() as u64);
    }
}
impl ConcatColumnsView<'_> {
    fn shape(&self) -> Result<Vec<&str>> {
        let n = self.read_idx.len();
        ensure!(
            self.read_length.len() == n,
            "read_length length must equal rows"
        );
        ensure!(
            self.read_start.len() == n,
            "read_start length must equal rows"
        );
        ensure!(self.read_end.len() == n, "read_end length must equal rows");
        ensure!(self.strand.len() == n, "strand length must equal rows");
        ensure!(self.chrom.len() == n, "chrom length must equal rows");
        ensure!(self.start.len() == n, "start length must equal rows");
        ensure!(self.end.len() == n, "end length must equal rows");
        ensure!(
            self.mapping_quality.len() == n,
            "mapping_quality length must equal rows"
        );
        ensure!(self.identity.len() == n, "identity length must equal rows");
        strings(self.filter_reason_offsets, self.filter_reason_bytes, n)
    }
    pub(crate) fn frame(&self, contigs: &[Contig]) -> Result<DataFrame> {
        let text = self.shape()?;
        storage_frame(
            Kind::Concat,
            vec![
                Series::new("read_idx".into(), self.read_idx).into(),
                Series::new("read_length".into(), self.read_length).into(),
                Series::new("read_start".into(), self.read_start).into(),
                Series::new("read_end".into(), self.read_end).into(),
                Series::new(
                    "strand".into(),
                    self.strand
                        .iter()
                        .map(|&s| if s == b'+' { "+" } else { "-" })
                        .collect::<Vec<_>>(),
                )
                .into(),
                Series::new(
                    "chrom".into(),
                    self.chrom
                        .iter()
                        .map(|&id| contigs[id as usize].name.as_str())
                        .collect::<Vec<_>>(),
                )
                .into(),
                Series::new("start".into(), self.start).into(),
                Series::new("end".into(), self.end).into(),
                Series::new("mapping_quality".into(), self.mapping_quality).into(),
                Series::new("identity".into(), self.identity).into(),
                Series::new("filter_reason".into(), text).into(),
            ],
            contigs,
        )
    }
}
impl Writer {
    // Shared by row and column entry points; no row object is needed.
    pub(crate) fn pair_fields(
        &self,
        chrom1: u32,
        pos1: u64,
        chrom2: u32,
        pos2: u64,
        strand1: u8,
        strand2: u8,
    ) -> Result<()> {
        self.position(chrom1, pos1)?;
        self.position(chrom2, pos2)?;
        ensure!(pos1 > 0 && pos2 > 0, "pairs positions must be 1-based");
        ensure!(
            strand_ok(strand1) && strand_ok(strand2),
            "strand must be + or -"
        );
        Ok(())
    }
    pub(crate) fn alignment_fields(
        &self,
        query: (u32, u32, u32),
        reference: (u32, u64, u64),
        strand: u8,
        identity: f32,
    ) -> Result<()> {
        let (length, start, end) = query;
        ensure!(start < end && end <= length, "invalid read interval");
        let (chrom, start, end) = reference;
        ensure!(start < end, "invalid reference interval");
        self.position(chrom, end)?;
        ensure!(
            strand_ok(strand) && identity.is_finite(),
            "invalid strand or non-finite identity"
        );
        Ok(())
    }
    pub(crate) fn flush_columns(&mut self) -> Result<()> {
        let Some(batch) = self.columns.take() else {
            return Ok(());
        };
        let result = (|| {
            let job = parallel::Shard {
                kind: self.kind, contigs: self.contigs.clone(),
                staging: self.staging.clone(), index: self.shard,
                columns: Some(batch), pairs: vec![], concat: vec![],
                concats: self.shard_concats,
            };
            if let Some(executor) = &mut self.executor {
                executor.submit(job, &mut self.counts)?;
            } else {
                parallel::add_counts(&mut self.counts, job.run()?);
            }
            self.shard_concats = [0, 0];
            self.shard += 1;
            Ok(())
        })();
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    pub fn write_pairs_columns(&mut self, v: PairColumnsView<'_>) -> Result<()> {
        self.active()?;
        ensure!(self.kind == Kind::Pairs, "requires pairs writer");
        v.shape()?;
        for i in 0..v.pos1.len() {
            self.pair_fields(
                v.chrom1[i],
                v.pos1[i],
                v.chrom2[i],
                v.pos2[i],
                v.strand1[i],
                v.strand2[i],
            )?;
        }
        if v.pos1.is_empty() {
            return Ok(());
        }
        self.flush()?;
        let mut a = 0;
        while a < v.pos1.len() {
            let batch = self
                .columns
                .get_or_insert_with(|| ColumnBatch::Pairs(PairColumns::default()));
            let ColumnBatch::Pairs(b) = batch else {
                unreachable!()
            };
            let end = (a + (self.chunk_size - b.pos1.len())).min(v.pos1.len());
            b.append(v, a, end);
            a = end;
            if b.pos1.len() == self.chunk_size {
                self.flush_columns()?;
            }
        }
        Ok(())
    }
    pub fn write_concat_columns(&mut self, v: ConcatColumnsView<'_>) -> Result<()> {
        self.active()?;
        ensure!(self.kind == Kind::Concat, "requires concat writer");
        v.shape()?;
        let n = v.read_idx.len() as u64;
        ensure!(
            v.read_offsets.first() == Some(&0) && v.read_offsets.last() == Some(&n),
            "read offsets must span alignments"
        );
        ensure!(
            v.read_offsets.windows(2).all(|w| w[0] < w[1] && w[1] <= n),
            "read offsets must be strictly increasing and within batch"
        );
        let mut previous = self.last_read;
        for w in v.read_offsets.windows(2) {
            let a = w[0] as usize;
            ensure!(
                previous.is_none_or(|id| v.read_idx[a] > id),
                "read IDs must be strictly increasing; submit each complete read once"
            );
            for i in a..w[1] as usize {
                ensure!(
                    v.read_idx[i] == v.read_idx[a] && v.read_length[i] == v.read_length[a],
                    "read ID and length must agree within a complete read"
                );
                self.alignment_fields(
                    (v.read_length[i], v.read_start[i], v.read_end[i]),
                    (v.chrom[i], v.start[i], v.end[i]),
                    v.strand[i],
                    v.identity[i],
                )?;
            }
            previous = Some(v.read_idx[a]);
        }
        if n == 0 {
            return Ok(());
        }
        self.flush()?;
        for w in v.read_offsets.windows(2) {
            let (a, end) = (w[0] as usize, w[1] as usize);
            let used = match &self.columns {
                Some(ColumnBatch::Concat(b)) => b.read_idx.len(),
                _ => 0,
            };
            if used > 0 && used.saturating_add(end - a) > self.chunk_size {
                self.flush_columns()?;
            }
            let batch = self
                .columns
                .get_or_insert_with(|| ColumnBatch::Concat(ConcatColumns::default()));
            let ColumnBatch::Concat(b) = batch else {
                unreachable!()
            };
            b.append(v, a, end);
            self.last_read = Some(v.read_idx[a]);
            self.shard_concats[0] += 1;
            self.shard_concats[1] += u64::from(v.mapping_quality[a..end].iter().any(|&q| q > 0));
            if b.read_idx.len() >= self.chunk_size {
                self.flush_columns()?;
            }
        }
        Ok(())
    }
}

// MODE 0/1 are benchmark-only alternatives; production uses chunk traversal.
macro_rules! integer_column {
    ($name:ident, $target:ty, $matching:ident) => {
        fn $name<const MODE: u8>(column: &Column) -> Result<Vec<$target>> {
            let numbers = Numbers::new(column)?;
            #[cfg(test)]
            if MODE == 0 {
                return (0..column.len())
                    .map(|i| Ok(<$target>::try_from(numbers.at(i)?)?))
                    .collect();
            }
            macro_rules! convert {
                ($c:expr) => {{
                    let c = $c;
                    // Preserve the original first-error order for malformed data.
                    if c.null_count() > 0 {
                        return (0..column.len())
                            .map(|i| Ok(<$target>::try_from(numbers.at(i)?)?))
                            .collect();
                    }
                    #[cfg(test)]
                    if MODE == 1 {
                        return c
                            .into_iter()
                            .map(|v| {
                                Ok(<$target>::try_from(u64::from(
                                    v.context("null integer in PQS")?,
                                ))?)
                            })
                            .collect();
                    }
                    let mut out = Vec::with_capacity(c.len());
                    for chunk in c.downcast_iter() {
                        for &v in chunk.values().as_slice() {
                            out.push(<$target>::try_from(u64::from(v))?);
                        }
                    }
                    Ok(out)
                }};
            }
            if let Numbers::$matching(c) = &numbers {
                if c.null_count() == 0 {
                    #[cfg(test)]
                    if MODE == 1 {
                        return c
                            .into_iter()
                            .map(|v| v.context("null integer in PQS"))
                            .collect();
                    }
                    let mut out = Vec::with_capacity(c.len());
                    for chunk in c.downcast_iter() {
                        out.extend_from_slice(chunk.values().as_slice());
                    }
                    return Ok(out);
                }
            }
            match &numbers {
                Numbers::U8(c) => convert!(c),
                Numbers::U32(c) => convert!(c),
                Numbers::U64(c) => convert!(c),
            }
        }
    };
}
integer_column!(integer_u64, u64, U64);
integer_column!(integer_u32, u32, U32);
integer_column!(integer_u8, u8, U8);

impl Reader {
    pub fn next_columns(&mut self) -> Result<Option<ColumnBatch>> {
        let Some(df) = self.next_frame()? else {
            return Ok(None);
        };
        Ok(Some(self.frame_columns(df)?))
    }
    pub(crate) fn frame_columns(&self, df: DataFrame) -> Result<ColumnBatch> {
        self.frame_columns_mode::<2>(df)
    }
    pub(crate) fn frame_columns_mode<const MODE: u8>(&self, df: DataFrame) -> Result<ColumnBatch> {
        let chromosome = |name: &str| {
            Codes::new(df.column(name)?, |s| {
                self.contig_ids
                    .get(s)
                    .copied()
                    .context("unknown contig in shard")
            })
        };
        let strand = |name: &str| {
            Codes::new(df.column(name)?, |s| {
                ensure!(s == "+" || s == "-", "invalid strand in PQS");
                Ok(s.as_bytes()[0])
            })
        };
        if self.kind == Kind::Pairs {
            let mut b = PairColumns::default();
            let c = chromosome("chrom1")?;
            b.chrom1 = (0..df.height())
                .map(|i| c.at(i))
                .collect::<Result<Vec<_>>>()?;
            b.pos1 = integer_u64::<MODE>(df.column("pos1")?)?;
            let c = chromosome("chrom2")?;
            b.chrom2 = (0..df.height())
                .map(|i| c.at(i))
                .collect::<Result<Vec<_>>>()?;
            b.pos2 = integer_u64::<MODE>(df.column("pos2")?)?;
            let c = strand("strand1")?;
            b.strand1 = (0..df.height())
                .map(|i| c.at(i))
                .collect::<Result<Vec<_>>>()?;
            let c = strand("strand2")?;
            b.strand2 = (0..df.height())
                .map(|i| c.at(i))
                .collect::<Result<Vec<_>>>()?;
            b.mapq = integer_u8::<MODE>(df.column("mapq")?)?;
            let text = df.column("read_idx")?.cast(&DataType::String)?;
            for s in text.str()?.into_iter() {
                b.read_id_bytes
                    .extend_from_slice(s.context("null string")?.as_bytes());
                b.read_id_offsets.push(b.read_id_bytes.len() as u64);
            }
            Ok(ColumnBatch::Pairs(b))
        } else {
            let mut b = ConcatColumns {
                read_idx: integer_u64::<MODE>(df.column("read_idx")?)?,
                read_length: integer_u32::<MODE>(df.column("read_length")?)?,
                read_start: integer_u32::<MODE>(df.column("read_start")?)?,
                read_end: integer_u32::<MODE>(df.column("read_end")?)?,
                ..Default::default()
            };
            let c = strand("strand")?;
            b.strand = (0..df.height())
                .map(|i| c.at(i))
                .collect::<Result<Vec<_>>>()?;
            let c = chromosome("chrom")?;
            b.chrom = (0..df.height())
                .map(|i| c.at(i))
                .collect::<Result<Vec<_>>>()?;
            b.start = integer_u64::<MODE>(df.column("start")?)?;
            b.end = integer_u64::<MODE>(df.column("end")?)?;
            b.mapping_quality = integer_u8::<MODE>(df.column("mapping_quality")?)?;
            let identity = df.column("identity")?.cast(&DataType::Float32)?;
            b.identity = identity
                .f32()?
                .into_iter()
                .map(|v| v.context("null identity"))
                .collect::<Result<Vec<_>>>()?;
            let text = df.column("filter_reason")?.cast(&DataType::String)?;
            for s in text.str()?.into_iter() {
                b.filter_reason_bytes
                    .extend_from_slice(s.context("null string")?.as_bytes());
                b.filter_reason_offsets
                    .push(b.filter_reason_bytes.len() as u64);
            }
            for i in 1..b.read_idx.len() {
                if b.read_idx[i] != b.read_idx[i - 1] {
                    b.read_offsets.push(i as u64);
                }
            }
            if !b.read_idx.is_empty() {
                b.read_offsets.push(b.read_idx.len() as u64);
            }
            Ok(ColumnBatch::Concat(b))
        }
    }
}

impl ColumnBatch {
    pub(crate) fn into_rows(self) -> Result<Batch> {
        match self {
            Self::Pairs(b) => {
                let text = strings(&b.read_id_offsets, &b.read_id_bytes, b.pos1.len())?;
                Ok(Batch::Pairs(
                    (0..b.pos1.len())
                        .map(|i| Pair {
                            read_id: text[i].to_owned(),
                            chrom1: b.chrom1[i],
                            pos1: b.pos1[i],
                            chrom2: b.chrom2[i],
                            pos2: b.pos2[i],
                            strand1: b.strand1[i],
                            strand2: b.strand2[i],
                            mapq: b.mapq[i],
                        })
                        .collect(),
                ))
            }
            Self::Concat(b) => {
                let text = strings(
                    &b.filter_reason_offsets,
                    &b.filter_reason_bytes,
                    b.read_idx.len(),
                )?;
                Ok(Batch::Concat(
                    (0..b.read_idx.len())
                        .map(|i| Alignment {
                            filter_reason: text[i].to_owned(),
                            read_idx: b.read_idx[i],
                            read_length: b.read_length[i],
                            read_start: b.read_start[i],
                            read_end: b.read_end[i],
                            strand: b.strand[i],
                            chrom: b.chrom[i],
                            start: b.start[i],
                            end: b.end[i],
                            mapping_quality: b.mapping_quality[i],
                            identity: b.identity[i],
                        })
                        .collect(),
                ))
            }
        }
    }
}

#[cfg(test)]
mod integer_tests {
    use super::*;

    #[test]
    fn conversion_modes_preserve_values_and_errors() -> Result<()> {
        let mut chunked = Series::new(
            "x".into(),
            &[999u64, 0, 255, 256, u32::MAX as u64, u64::MAX],
        );
        chunked = chunked.slice(1, 5);
        chunked.append(&Series::new("x".into(), &[42u64]))?;
        let cases: Vec<Column> = vec![
            chunked.into(),
            Series::new("x".into(), &[0u8, 255]).into(),
            Series::new("x".into(), &[0u32, 255, 256, u32::MAX]).into(),
            Series::new("x".into(), Vec::<u64>::new()).into(),
            Series::new("x".into(), &[Some(0u64), None, Some(u64::MAX)]).into(),
            Series::new("x".into(), &[Some(u64::MAX), None]).into(),
            Series::new("x".into(), &[None, Some(1u8)]).into(),
            Series::new("x".into(), &[Some(256u32), None]).into(),
            Series::new("x".into(), &[0i64, 42, -1]).into(),
            Series::new("x".into(), &[0.0f64, 42.0, 256.0]).into(),
        ];
        for column in &cases {
            macro_rules! compare {
                ($f:ident) => {{
                    let reference = $f::<0>(column).map_err(|e| e.to_string());
                    assert_eq!($f::<1>(column).map_err(|e| e.to_string()), reference);
                    assert_eq!($f::<2>(column).map_err(|e| e.to_string()), reference);
                }};
            }
            compare!(integer_u64);
            compare!(integer_u32);
            compare!(integer_u8);
        }
        assert_eq!(integer_u64::<2>(&cases[1])?, vec![0, 255]);
        assert!(integer_u8::<2>(&cases[2]).is_err());
        assert_eq!(integer_u32::<2>(&cases[3])?, Vec::<u32>::new());
        Ok(())
    }
}
