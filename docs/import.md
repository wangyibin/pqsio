# Import BAM and PAF

`pqsio convert` uses one BAM input path for both Hi-C paired-end and long-read
data. Choose `bam2pairs` for pairs PQS or `bam2concat` for alignment-level concat
PQS; either mode accepts either BAM type. PAF imports support `paf2pairs` and
`paf2concat` for the corresponding output formats.
All record processing runs in Rust: BAM decoding via `rust-htslib`, PAF parsing,
external sorting, pair expansion and PQS writing. The Python API makes
one native call per conversion, releasing the GIL during that call. No Python
record iteration, SQLite database or `samtools` subprocess is used.
These modes are also available through `pqsio.convert` in Python and
`pqsio::import_alignments` with `ImportOptions` in Rust.
The obsolete `--samtools` / `samtools=` option reports an error asking you to
omit it. Older native libraries lacking `pqsio_import_json` must be rebuilt;
there is no fallback to Python record processing.

```sh
# Paired-end Hi-C BAM -> pairs PQS
pqsio convert hic.bam --mode bam2pairs -o hic.pairs.pqs --min-mapq 1 --threads 4

# Long-read / Pore-C BAM -> concat PQS
pqsio convert long_reads.bam --mode bam2concat -o long_reads.concat.pqs --threads 4

# The output mode is independent of the BAM sequencing type.
pqsio convert long_reads.bam --mode bam2pairs -o long_reads.pairs.pqs
pqsio convert hic.bam --mode bam2concat -o hic.concat.pqs

# PAF or gzip-compressed PAF -> concat PQS
pqsio convert reads.paf.gz --mode paf2concat -o reads.concat.pqs

# Direct PAF -> pairs PQS, without writing an intermediate concat dataset.
pqsio convert reads.paf.gz --mode paf2pairs -o reads.pairs.pqs --min-mapq 1

# Optional reference table includes contigs with no mappings; .fai is accepted.
pqsio convert reads.paf --mode paf2concat -o reads.concat.pqs --contigsizes reference.fa.fai

# Expand imported long-read contacts into pairs when needed.
pqsio convert reads.concat.pqs -o reads.pairs.pqs --min-order 2 --threads 4
```

For source checkout use, prefix the command with `pixi run`, for example
`pixi run pqsio convert hic.bam --mode bam2pairs -o hic.pqs`. See
[CLI installation](cli.md) for the standalone command and native library setup.

```python
import pqsio

result = pqsio.convert("reads.bam", "reads.concat.pqs", mode="bam2concat",
                       min_mapq=1, threads=4)
print(result.to_dict())
```

## Shared BAM parsing

Both BAM modes retain primary and supplementary alignments. Secondary alignments
are excluded unless `--include-secondary` is specified. Unmapped, duplicate and
QC-failed records are excluded, and `--min-mapq` applies to individual alignments.
Records are grouped by **read group (RG) and QNAME**, regardless of file order.
Paired records must identify READ1 or READ2; no proper-pair flag is required.
Interchromosomal contacts are retained. A QNAME/RG group mixing paired and
unpaired records is rejected as ambiguous.

## BAM to pairs

`bam2pairs` follows the general CPhasing BAM-to-pairs expansion: a retained group
of N alignments generates all `N*(N-1)/2` pairs. This applies to both paired-end
and single-end BAM. A simple Hi-C read pair yields one pair; supplementary
alignments can produce more contacts, including same-mate and same-contig pairs.
A group with fewer than two retained alignments produces no output. This is
alignment expansion, not restriction-fragment filtering or PCR deduplication.

Alignments are stably ordered by mate (unpaired=0, READ1=1, READ2=2), then original
query start. Pair IDs are `QNAME:left_index:right_index` using zero-based indices
in this order after filtering. IDs may repeat across read groups. Pair MAPQ is
the minimum of the two alignment MAPQs. Each output's endpoints are ordered by
BAM-header contig index and position.

The default `--pair-position leftmost` uses the **1-based leftmost aligned
position** on both strands, matching CPhasing's BAM-to-PQS coordinate convention.
Use `--pair-position five-prime` for the aligned 5′ end: `reference_start + 1` on
the forward strand and `reference_end` on the reverse strand. Clipped bases are
not extrapolated. BAM-to-pairs does not require identity or NM tags. Its positions
differ intentionally from the midpoint positions of native `concat2pairs`.

## BAM to concat

`bam2concat` stores each original read's alignment intervals. For paired-end
BAM, READ1 and READ2 are separate logical reads, each retaining its own length
and original query coordinates. `_import_read_names.jsonl` retains their common
QNAME/RG plus `mate: 1` or `mate: 2`; single-end reads use `mate: null`. No
synthetic concatenation or coordinate offset between mates is introduced.
Use `bam2pairs` directly to generate cross-mate Hi-C contacts; `concat2pairs`
operates within each logical concat read.

Read length includes hard-clipped bases. Query intervals include the leading
clipped offset, and reverse-strand query intervals are transformed back to the
original read orientation. Reference intervals are 0-based, half-open. Reference
skips (`N`) consume reference coordinates, while padding (`P`) does not.

Identity is `matches / (M + = + X + I + D)`. With an NM tag, matches are
`M + = + X - (NM - I - D)`. Without NM, an exact `=/X` CIGAR can determine
identity; a CIGAR containing `M` cannot. Such records produce an actionable
error suggesting `samtools calmd` with the reference FASTA. Unknown identity is
never silently replaced by 1.0. Reference skips, padding and clipping are
excluded from this identity denominator. Inconsistent NM/CIGAR data fail.

## PAF to pairs

`paf2pairs` reads PAF or gzip-compressed PAF and writes pairs PQS directly. It
uses disk-backed QNAME grouping and the same bounded pair expansion as
`bam2pairs`, without constructing a concat dataset or requiring samtools.
Each group of N retained alignments produces `N*(N-1)/2` pairs. Query lengths
within a retained group must agree. Singleton groups produce no pairs.

Alignments are stably sorted by query start; IDs are
`QNAME:left_index:right_index`. Endpoints are ordered by contig-table index and
position; pair MAPQ is the minimum of its two alignment MAPQs. Defaults match
`bam2pairs`: **1-based leftmost coordinates**, minimum order 2, minimum MAPQ 0.
`--pair-position five-prime` selects aligned 5′ ends instead. These coordinates
differ from the midpoints produced by `paf2concat` followed by `concat2pairs`.

The direct mode supports `--contigsizes`, `--include-secondary`, `--min-mapq`,
`--min-order`, `--max-order` (exclusive), `--batch-rows`, `--chunk-size`, and
`--tmpdir`. Order filtering follows alignment filtering. q0 contains every
output pair; q1 contains pairs with MAPQ >= 1. The JSON report and `_import.json`
record mode `paf2pairs`, pair counts and skip statistics.

## PAF and concat grouping

PAF accepts the standard twelve mandatory tab-separated fields, plus optional
tags. Gzip is detected by magic bytes. Intervals remain 0-based, half-open on
both strands. Identity is column 10 divided by column 11; for approximate
mappings this remains an approximation supplied by the mapper. `tp:A:S` records
are excluded by default and retained with `--include-secondary`. With no `tp`
tag, the importer does not infer secondary status. MAPQ 255 is preserved.

Contig lengths come from BAM `@SQ` headers or PAF target fields. All observed
PAF targets, including filtered records, contribute to the reference dictionary.
`--contigsizes` is PAF-only, accepts a two-column sizes table or `.fai`, and
requires every observed target and length to match it. An empty PAF needs this
option because it has no reference header. Conflicting lengths are errors.

For both concat imports, the default `--min-order` is **1**, retaining singleton
reads. For `bam2pairs` and `paf2pairs` it is **2**. MAPQ and secondary filtering precede order
filtering. `--max-order` is an exclusive upper bound. For pairs these bounds
apply to the QNAME/RG alignment group; for paired-end concat they apply per mate.
Within each retained concat read, alignments are stably sorted by query start; all
alignments must agree on original read length. `filter_reason` is `pass` for
retained alignments. No restriction-site, read-overlap or phasing filters run.

Input order may be arbitrary. Rust sorts binary scratch runs on disk
by `(RG, QNAME)` for BAM and QNAME for PAF. Groups are emitted in binary text
order by read group and name, not original file order. Concat output uses
increasing logical IDs starting at 1. `_import_read_names.jsonl` records the
mapping from those IDs to original names, read groups and mate identities. All modes write
`_import.json` with source, options, counts and skip statistics. These sidecars
are import provenance, not a new PQS schema; downstream operations do not
automatically propagate them. Arbitrary BAM/PAF tags and sequences are not stored.

## Resources, publication and verification

Text I/O follows CPhasing's `common_reader` / `common_writer` approach.
The native reader detects gzip by magic bytes regardless of the filename.
Ordinary gzip, concatenated gzip members and BGZF text use `flate2::MultiGzDecoder`.
Mgzip files with valid IG block framing use `gzp` parallel decoding when
`--threads > 1`; malformed/mixed framing uses the validating gzip decoder.
The Rust `common_writer` buffers plain text and uses parallel gzip-compatible
mgzip for `.gz` / `.mgz`, with explicit `finish()` error checking. PQS itself
remains a directory of compressed Parquet shards, not a gzip text file.
No external gzip or rapidgzip executable is invoked.

Scratch is created beside the output by default; `--tmpdir` selects an existing
directory with enough disk space. Rust forms sorting runs with a 32 MiB record
size target and merges at most 32 files at once, using additional passes when
needed. Scratch disk usage scales with selected input records, not the number
of expanded pairs. Run capacity, a complete QNAME group, reference dictionary,
output batches and native writer buffers add to peak memory; 32 MiB is not a
process memory cap. An oversized read may exceed batch/shard targets.

Pairs imports pack UTF-8 IDs and numeric fields directly into Rust column
batches. Both pair and concat output use native PQS validation and Parquet
writing. `--threads` controls BAM decoding, mgzip decoding and native Parquet
encoding workers. Ordinary gzip decoding, PAF parsing, sorting and pair
expansion currently run on the calling Rust thread. This is not a total CPU
cap: the encoding workers and Polars thread pool can run concurrently.
`concat2pairs` retains its existing native parallel conversion behavior.
See [the performance comparison](import-performance.md) for measurements and
reproduction instructions.

The entire input is decoded and grouped before PQS publication. A decoder
error, malformed record, conflicting group or write failure aborts the
operation. The native writer publishes only on success; normal failures remove
the operation's staging and scratch directories. Existing output/staging paths
are rejected, including dangling symlinks. A process crash can leave temporary
files, as with the underlying writer. Inputs must remain unchanged while reading.
These imports accept local files; stdin, CRAM reference configuration and text
`.pairs` import are not implemented here.

`pixi run test-import` tests tiny PAF/gzip and real BAM fixtures produced from
independent SAM text using samtools. BAM fixture tests skip explicitly when
samtools is unavailable. It also verifies BAM import with an empty `PATH`, no Python writers or decoder
subprocesses, compressed input, unsorted grouping, flags, clipping, reverse
coordinates, identity, endpoint modes, filtering, malformed input and cleanup.

The implementation references CPhasing's `bam2paf`, `get_query_start_end`,
general `bam2pairs`/`bam2pqs` expansion and PAF parsing, while using pqsio's current storage
contract. Field semantics follow the [SAM/BAM specification](https://samtools.github.io/hts-specs/SAMv1.pdf),
[SAM tag specification](https://samtools.github.io/hts-specs/SAMtags.pdf), and
[PAF specification](https://github.com/lh3/miniasm/blob/master/PAF.md).
