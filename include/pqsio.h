#ifndef PQSIO_H
#define PQSIO_H
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
/* ABI v1. UTF-8 strings; contig IDs index the supplied ordered contig array.
 * Only producer submissions are thread safe; other handles are not. All pointers must be valid for the call.
 * Write calls copy/consume inputs before returning. Callback arrays/strings
 * are borrowed for the callback duration only. Never throw across callbacks.
 * Return: 0 success / -1 error unless otherwise specified. Read last_error
 * before another API call; its pointer is thread-local and transient.
 */
typedef struct pqsio_writer pqsio_writer;
typedef struct pqsio_reader pqsio_reader;
typedef struct { const char *name; uint64_t length; } pqsio_contig;
typedef struct {
    const char *read_id;
    uint32_t chrom1; uint64_t pos1; uint32_t chrom2; uint64_t pos2;
    uint8_t strand1, strand2, mapq;
} pqsio_pair;
typedef struct {
    uint64_t read_idx; uint32_t read_length, read_start, read_end;
    uint8_t strand; uint32_t chrom; uint64_t start, end;
    uint8_t mapping_quality; float identity; const char *filter_reason;
} pqsio_alignment;
/* Parallel extension: sequences are unique, contiguous from 0. Submission
 * may block; keep the next expected batch schedulable. Queue reserves one
 * extra slot for it. max_batch_bytes bounds owned input, not Parquet/RSS.
 * Each batch is a shard boundary; concat reads must be complete and ordered.
 * Queued success is asynchronous: finish after all submissions to observe
 * validation/I/O errors. Missing sequence gaps fail at finish.
 * Producers may outlive their writer and then return errors. Concurrent
 * submissions on one producer are allowed; destroy requires no active calls.
 * Input memory must remain valid until the submission call returns.
 */
typedef struct pqsio_parallel_writer pqsio_parallel_writer;
typedef struct pqsio_producer pqsio_producer;
int32_t pqsio_parallel_open(const char *, uint32_t, const pqsio_contig *, size_t,
    size_t chunk_size, size_t workers, size_t queue_capacity, size_t max_batch_bytes, pqsio_parallel_writer **);
int32_t pqsio_parallel_producer(const pqsio_parallel_writer *, pqsio_producer **);
int32_t pqsio_producer_pairs(const pqsio_producer *, uint64_t sequence, const pqsio_pair *, size_t);
int32_t pqsio_producer_reads(const pqsio_producer *, uint64_t sequence, const pqsio_alignment *, size_t, const size_t *, size_t);
int32_t pqsio_parallel_finish(pqsio_parallel_writer *);
int32_t pqsio_parallel_destroy(pqsio_parallel_writer *);
int32_t pqsio_producer_destroy(pqsio_producer *);
uint32_t pqsio_abi_version(void);
const char *pqsio_last_error(void);
/* kind: 0 pairs, 1 concat. Existing output/.partial paths are rejected. */
int32_t pqsio_writer_open(const char *, uint32_t, const pqsio_contig *, size_t, size_t, pqsio_writer **);
int32_t pqsio_write_pairs(pqsio_writer *, const pqsio_pair *, size_t);
/* Exactly one complete read per call, strictly increasing read_idx. */
int32_t pqsio_write_read(pqsio_writer *, const pqsio_alignment *, size_t);
/* Added in pqsio 0.0.2; ABI v1 layouts unchanged. Offsets have read_count+1
 * elements, start at 0, end at n, and are strictly increasing. Each interval
 * is one complete read; IDs increase across all calls. Empty batch: n=0,
 * offsets={0}. Invalid input is rejected before any record is accepted. */
int32_t pqsio_write_reads(pqsio_writer *, const pqsio_alignment *, size_t n, const size_t *offsets, size_t offset_count);
int32_t pqsio_writer_finish(pqsio_writer *);
/* Destroy aborts an unfinished writer; finish does not destroy the handle. */
int32_t pqsio_writer_destroy(pqsio_writer *);
int32_t pqsio_reader_open(const char *, uint8_t, pqsio_reader **);
int32_t pqsio_reader_kind(const pqsio_reader *); /* 0 pairs, 1 concat, -1 error */
typedef int32_t (*pqsio_contigs_callback)(const pqsio_contig *, size_t, void *);
typedef int32_t (*pqsio_pairs_callback)(const pqsio_pair *, size_t, void *);
typedef int32_t (*pqsio_concat_callback)(const pqsio_alignment *, size_t, void *);
int32_t pqsio_reader_contigs(const pqsio_reader *, pqsio_contigs_callback, void *);
/* 1 delivered shard, 0 EOF, -1 error. Consumes shard even on callback failure. */
int32_t pqsio_reader_next(pqsio_reader *, pqsio_pairs_callback, pqsio_concat_callback, void *);
int32_t pqsio_reader_destroy(pqsio_reader *);
#ifdef __cplusplus
}
#endif
#endif
