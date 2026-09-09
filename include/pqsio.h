#ifndef PQSIO_H
#define PQSIO_H
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
/* ABI v1. UTF-8 strings; contig IDs index the supplied ordered contig array.
 * Handles are not thread safe. All pointers must be valid for the call.
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
uint32_t pqsio_abi_version(void);
const char *pqsio_last_error(void);
/* kind: 0 pairs, 1 concat. Existing output/.partial paths are rejected. */
int32_t pqsio_writer_open(const char *, uint32_t, const pqsio_contig *, size_t, size_t, pqsio_writer **);
int32_t pqsio_write_pairs(pqsio_writer *, const pqsio_pair *, size_t);
/* Exactly one complete read per call, strictly increasing read_idx. */
int32_t pqsio_write_read(pqsio_writer *, const pqsio_alignment *, size_t);
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
