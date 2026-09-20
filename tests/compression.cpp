#include "pqsio.hpp"
#include <cassert>

int main(int argc, char **argv) {
    assert(argc == 3);
    const std::vector<pqsio_contig> contigs{{"chr1", 100}};
    const std::vector<pqsio_pair> rows{{"compressed", 0, 1, 0, 2, '+', '-', 60}};
    try {
        pqsio::Writer invalid(argv[1], pqsio::Kind::Pairs, contigs, pqsio::Compression{"gzip", 10});
        return 1;
    } catch (const std::runtime_error &) {}
    pqsio::Writer writer(argv[1], pqsio::Kind::Pairs, contigs, pqsio::Compression{"uncompressed"});
    writer.write_pairs(rows);
    writer.finish();
    try {
        pqsio::ParallelWriter invalid(argv[2], pqsio::Kind::Pairs, contigs,
                                      pqsio::Compression{"lz4", 1});
        return 1;
    } catch (const std::runtime_error &) {}
    pqsio::ParallelWriter parallel(argv[2], pqsio::Kind::Pairs, contigs, pqsio::Compression{"zstd", 6});
    auto producer = parallel.producer();
    producer.write_pairs(0, rows);
    parallel.finish();
}
