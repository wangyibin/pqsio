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

/* Columnar extension version 1 (symbol presence + pqsio_columnar_version()).
 * Old ABI version and layouts are unchanged. All spans use element counts,
 * native endian, natural alignment and initialized contiguous typed storage.
 * Nonzero length requires non-NULL data; zero length permits NULL.
 * Caller guarantees actual extent, liveness, alignment of structs/handles,
 * and no concurrent mutation/access. Dangling pointers and false lengths
 * cannot be detected. Writes borrow immutable inputs until return.
 * Numeric columns have N elements. String offsets are uint64_t[N+1], start
 * at 0, nondecreasing, end at byte length; each slice is valid UTF-8.
 * Equal offsets encode empty strings; no NUL termination is required.
 * Concat read_offsets are uint64_t[R+1], start 0, strictly increase, end N;
 * each interval is a complete nonempty read with identical ID/read length.
 * IDs strictly increase within and across calls (including row calls).
 * Empty batches use string/read offsets {0}, all other spans length 0.
 * pairs positions are 1-based; concat intervals are 0-based half-open.
 * Invalid input accepts no part of a batch; storage errors poison writer.
 * Read handles own immutable buffers, independent of reader lifetime.
 * Views remain valid until batch_destroy; never free/reallocate view data.
 * next_columns: 1 batch (possibly empty after filtering), 0 EOF, -1 error.
 * Output handle must be writable, not overwrite a live owned handle.
 * View getters return 0/-1. destroy(NULL) succeeds. Not thread safe.
 */
uint32_t pqsio_columnar_version(void);
typedef struct pqsio_column_batch pqsio_column_batch;
typedef struct { const uint8_t *data; size_t len; } pqsio_span_u8;
typedef struct { const uint32_t *data; size_t len; } pqsio_span_u32;
typedef struct { const uint64_t *data; size_t len; } pqsio_span_u64;
typedef struct { const float *data; size_t len; } pqsio_span_f32;
typedef struct {
    pqsio_span_u64 read_id_offsets;
    pqsio_span_u8 read_id_bytes;
    pqsio_span_u32 chrom1;
    pqsio_span_u64 pos1;
    pqsio_span_u32 chrom2;
    pqsio_span_u64 pos2;
    pqsio_span_u8 strand1;
    pqsio_span_u8 strand2;
    pqsio_span_u8 mapq;
} pqsio_pairs_columns;
int32_t pqsio_write_pairs_columns(pqsio_writer *, const pqsio_pairs_columns *);
int32_t pqsio_column_batch_pairs(const pqsio_column_batch *, pqsio_pairs_columns *);
typedef struct {
    pqsio_span_u64 read_offsets;
    pqsio_span_u64 read_idx;
    pqsio_span_u32 read_length;
    pqsio_span_u32 read_start;
    pqsio_span_u32 read_end;
    pqsio_span_u8 strand;
    pqsio_span_u32 chrom;
    pqsio_span_u64 start;
    pqsio_span_u64 end;
    pqsio_span_u8 mapping_quality;
    pqsio_span_f32 identity;
    pqsio_span_u64 filter_reason_offsets;
    pqsio_span_u8 filter_reason_bytes;
} pqsio_concat_columns;
int32_t pqsio_write_concat_columns(pqsio_writer *, const pqsio_concat_columns *);
int32_t pqsio_column_batch_concat(const pqsio_column_batch *, pqsio_concat_columns *);
int32_t pqsio_reader_next_columns(pqsio_reader *, pqsio_column_batch **);
int32_t pqsio_column_batch_destroy(pqsio_column_batch *);
/* NEW: row-group streaming API; existing reader ABI is unchanged.
 * batch_rows: 1..UINT32_MAX. boundary: 0 rows, 1 complete_reads.
 * filter: 0 default, 1 matching_alignments, 2 complete_reads.
 * Positive MAPQ selects q1 for pairs/global-ID matching reads. MAPQ=0,
 * complete_reads filtering, and shard-local concat select q0. Boundary alone
 * does not affect source selection. Missing/unreadable source is an error.
 * Pairs requires boundary=0 and filter=0. Defaults: 65536, 0, 0.
 * next: 1 nonempty batch, 0 EOF, -1 terminal error (close/reopen).
 * Callbacks borrow arrays/strings only until return; return 0 on success.
 * No concurrent access, reentry, throwing, or destruction during callbacks. */
typedef struct pqsio_stream pqsio_stream;
int32_t pqsio_stream_open(const char *, uint8_t, uint64_t, uint32_t, uint32_t, pqsio_stream **);
int32_t pqsio_stream_next(pqsio_stream *, pqsio_pairs_callback, pqsio_concat_callback, void *);
/* Additive capability: resolve this symbol before use with older libraries.
 * Same cursor/options as stream_next: 1 nonempty batch, 0 EOF, -1 terminal
 * error. Sets *out=NULL on EOF/error. out must be writable, naturally aligned,
 * and must not overwrite a live batch. NULL out also poisons a live stream.
 * Batch owns its buffers independently of the stream: use existing
 * column_batch_pairs/concat getters and column_batch_destroy exactly once.
 * In rows mode, concat read_offsets delimit retained fragments in this batch;
 * only complete_reads boundary keeps each returned read together. Filtering
 * matching_alignments returns retained alignments, not the original full read.
 * No concurrent access/reentry. Previously returned batches remain valid. */
int32_t pqsio_stream_next_columns(pqsio_stream *, pqsio_column_batch **);
int32_t pqsio_stream_kind(const pqsio_stream *);
int32_t pqsio_stream_contigs(const pqsio_stream *, pqsio_contigs_callback, void *);
int32_t pqsio_stream_destroy(pqsio_stream *);
/* Read-only metadata/validation extension; resolve symbols for old libraries.
 * JSON UTF-8 bytes are valid only during the callback; copy to retain them.
 * Callback returns 0, must not throw/unwind or reenter the API.
 * level: 0 quick, 1 full; max_issues > 0 caps stored examples, not scanning.
 * Return 0 for a delivered report (including invalid/incomplete), -1 for an
 * invocation error. Report status must be examined separately. */
typedef int32_t (*pqsio_json_callback)(const uint8_t *, size_t, void *);
int32_t pqsio_inspect_json(const char *, pqsio_json_callback, void *);
int32_t pqsio_validate_json(const char *, uint32_t, size_t, pqsio_json_callback, void *);
/* Additive ABI v1 merge capability. Inputs retain caller order, provenance is
 * 0/1. Return 0 on success, -1 on error. Callback follows JSON contract above.
 * Callback failure is reported AFTER publication; valid output remains.
 * Inputs must stay unchanged throughout the call. See docs/merge.md. */
int32_t pqsio_merge_json(const char *const *, size_t, const char *, size_t,
                         size_t, uint32_t, pqsio_json_callback, void *);
/* Optional region query extension. No change to ABI v1 stream handles.
 * Positive-MAPQ matching queries use q1; all index modes target the selected quality.
 * Complete reads and shard-local IDs always use q0. JSON stats source_quality
 * identifies the partition counted by the row/group statistics.
 * Coordinates: 0-based half-open. index: 0 auto, 1 off, 2 require;
 * pairs_mode: 0 either, 1 both; boundary/filter follow stream_open.
 * Query handles use stream_next[_columns], kind, contigs and destroy.
 * stats are partial until complete=true. No source file is modified. */
typedef struct { const char *contig; uint64_t start; uint64_t end; } pqsio_region;
int32_t pqsio_build_index(const char *, uint32_t rebuild);
/* quality: 0 q0, 1 q1; additive symbol, old build_index still selects q0. */
int32_t pqsio_build_index_quality(const char *, uint32_t quality, uint32_t rebuild);
int32_t pqsio_query_open(const char *, const pqsio_region *, size_t,
    uint8_t, uint32_t, uint32_t, uint64_t, uint32_t, uint32_t, pqsio_stream **);
int32_t pqsio_query_stats_json(const pqsio_stream *, pqsio_json_callback, void *);
#ifdef __cplusplus
}
#endif
#endif
