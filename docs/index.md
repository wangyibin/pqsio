# pqsio

Read, write and convert genomic contacts. Use the native CLI or the
Python, Rust and C/C++ APIs.

## Why PQS?

- **Faster repeated analysis.** Read the columns you need and use quality
  partitions for filtering, reducing repeated text parsing.
- **Keep multi-fragment reads.** Store alignments together in concat, select
  complete reads, and expand pairs with different thresholds when needed.
- **Process data in batches.** Streaming and parallel I/O support large
  datasets without requiring a full in-memory record table.
- **Reuse one dataset across languages.** Native CLI, Python, Rust and C/C++
  APIs support BAM/PAF import, queries and Cooler output.

In one-million-record tests, default PQS counted MAPQ >= 30 records
**20.87–33.50× faster** and wrote **4.71–6.25× faster** than gzip level 6 text.
These are operation timings; see [benchmark methods and results](pqs-format.md#performance-benchmark).

| Dataset | Stores |
| --- | --- |
| **pairs** | Contacts between two genomic positions |
| **concat** | Alignments grouped by read, retaining multiple fragments |

## Start here

1. [Install pqsio](installation.md).
2. Choose [CLI](cli.md), [Python API](python.md) or [Rust API](rust.md).
3. For multi-fragment reads, follow the [concat workflow](concat-workflow.md).

## A typical CLI workflow

```sh
pqsio bam2pairs hic.bam -o hic.pairs.pqs --min-mapq 30
pqsio info hic.pairs.pqs
pqsio pairs2cool hic.pairs.pqs -o hic.10k.cool --bin-size 10k
```

Replace `hic.bam` with your input. PQS is a directory; outputs must be new paths.
Use `pqsio COMMAND --help` for command options.

Find format details, advanced APIs and benchmarks in the [reference](reference.md).
