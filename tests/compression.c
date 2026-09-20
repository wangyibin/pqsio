#include "pqsio.h"
#include <assert.h>
#include <string.h>

int main(int argc, char **argv) {
    assert(argc == 3 && pqsio_abi_version() == 1);
    pqsio_contig contigs[] = {{"chr1", 100}};
    pqsio_pair pair[] = {{"compressed", 0, 1, 0, 2, '+', '-', 60}};
    pqsio_writer *writer = NULL;
    int32_t level = 23;
    assert(pqsio_writer_open_with_compression(argv[1], 0, contigs, 1, 10,
                                             "zstd", &level, &writer) == -1);
    assert(writer == NULL && strstr(pqsio_last_error(), "1..=22"));
    assert(pqsio_writer_open_with_compression(argv[1], 0, contigs, 1, 10,
                                             NULL, NULL, &writer) == -1);
    assert(writer == NULL);
    assert(pqsio_writer_open_with_compression(argv[1], 0, contigs, 1, 10,
                                             "uncompressed", NULL, &writer) == 0);
    assert(pqsio_write_pairs(writer, pair, 1) == 0);
    assert(pqsio_writer_finish(writer) == 0);
    assert(pqsio_writer_destroy(writer) == 0);

    pqsio_parallel_writer *parallel = NULL;
    level = 0; /* Must not be interpreted as an omitted level. */
    assert(pqsio_parallel_open_with_compression(argv[2], 0, contigs, 1, 10, 2, 4, 4096,
                                               "snappy", &level, &parallel) == -1);
    assert(parallel == NULL && strstr(pqsio_last_error(), "does not accept"));
    assert(pqsio_parallel_open_with_compression(argv[2], 0, contigs, 1, 10, 2, 4, 4096,
                                               "gzip", &level, &parallel) == 0);
    pqsio_producer *producer = NULL;
    assert(pqsio_parallel_producer(parallel, &producer) == 0);
    assert(pqsio_producer_pairs(producer, 0, pair, 1) == 0);
    assert(pqsio_parallel_finish(parallel) == 0);
    assert(pqsio_producer_destroy(producer) == 0);
    assert(pqsio_parallel_destroy(parallel) == 0);
    return 0;
}
