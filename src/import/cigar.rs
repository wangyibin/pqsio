//! CIGAR coordinates in original-read orientation, including hard clips.
use anyhow::{ensure, Context, Result};

pub(super) struct Coordinates {
    pub length: u32,
    pub start: u32,
    pub end: u32,
    pub reference: u64,
    pub identity: f32,
}

pub(super) fn coordinates(
    ops: &[(u32, u8)],
    reverse: bool,
    sequence_length: usize,
    nm: Option<i64>,
    need_identity: bool,
) -> Result<Coordinates> {
    ensure!(!ops.is_empty(), "invalid or missing CIGAR");
    ensure!(
        ops.iter()
            .all(|&(n, op)| n > 0 && b"MIDNSHP=X".contains(&op)),
        "invalid CIGAR operation or zero length"
    );
    let clip = |op: u8| op == b'S' || op == b'H';
    let left = ops.iter().take_while(|&&(_, op)| clip(op)).count();
    let right = ops.len() - ops.iter().rev().take_while(|&&(_, op)| clip(op)).count();
    ensure!(
        left < right
            && !ops[left..right].iter().any(|&(_, op)| clip(op))
            && !ops
                .iter()
                .enumerate()
                .any(|(i, &(_, op))| op == b'H' && i != 0 && i != ops.len() - 1),
        "CIGAR clipping must occur at alignment ends (H outside S)"
    );
    let sum = |letters: &[u8]| -> Result<u64> {
        ops.iter()
            .filter(|&&(_, op)| letters.contains(&op))
            .try_fold(0u64, |sum, &(n, _)| {
                sum.checked_add(n as u64).context("CIGAR length overflow")
            })
    };
    let query = sum(b"MIS=X")?;
    let length = query
        .checked_add(sum(b"H")?)
        .context("CIGAR length overflow")?;
    ensure!(
        length > 0 && length <= u32::MAX as u64,
        "CIGAR read length must fit UInt32"
    );
    ensure!(
        sequence_length == 0 || sequence_length as u64 == query,
        "CIGAR query length does not match SEQ"
    );
    let mut start: u64 = ops[..left].iter().map(|&(n, _)| n as u64).sum();
    let mut end = length - ops[right..].iter().map(|&(n, _)| n as u64).sum::<u64>();
    let reference = sum(b"MDN=X")?;
    ensure!(
        start < end && reference > 0,
        "CIGAR must consume both query and reference bases"
    );
    if reverse {
        (start, end) = (length - end, length - start);
    }
    let identity = if need_identity {
        let aligned = sum(b"M=X")?;
        let indels = sum(b"ID")?;
        let differences = sum(b"X")?;
        let ambiguous = sum(b"M")?;
        ensure!(nm.is_some() || ambiguous == 0,
            "BAM with M in CIGAR requires an NM tag to compute identity; add NM tags with samtools calmd using the reference FASTA");
        let mismatches = match nm {
            None => differences,
            Some(nm) => u64::try_from(nm)
                .ok()
                .and_then(|n| n.checked_sub(indels))
                .context("NM tag is inconsistent with CIGAR")?,
        };
        ensure!(
            mismatches >= differences && mismatches <= differences + ambiguous,
            "NM tag is inconsistent with CIGAR"
        );
        ((aligned - mismatches) as f64 / (aligned + indels) as f64) as f32
    } else {
        0.0
    };
    Ok(Coordinates {
        length: length as u32,
        start: start as u32,
        end: end as u32,
        reference,
        identity,
    })
}
