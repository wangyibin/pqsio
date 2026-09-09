#pragma once
#include "pqsio.h"
#include <stdexcept>
#include <string>
#include <vector>
namespace pqsio {
inline int check(int code) { if (code < 0) throw std::runtime_error(pqsio_last_error()); return code; }
enum class Kind : uint32_t { Pairs = 0, Concat = 1 };
class Writer {
    pqsio_writer *handle_ = nullptr;
public:
    Writer(const std::string &path, Kind kind, const std::vector<pqsio_contig> &contigs, size_t chunksize = 1000000) {
        check(pqsio_writer_open(path.c_str(), static_cast<uint32_t>(kind), contigs.data(), contigs.size(), chunksize, &handle_));
    }
    Writer(const Writer &) = delete;
    Writer &operator=(const Writer &) = delete;
    ~Writer() { pqsio_writer_destroy(handle_); }
    void write_pairs(const std::vector<pqsio_pair> &rows) { check(pqsio_write_pairs(handle_, rows.data(), rows.size())); }
    void write_read(const std::vector<pqsio_alignment> &rows) { check(pqsio_write_read(handle_, rows.data(), rows.size())); }
    void write_reads(const std::vector<pqsio_alignment> &rows, const std::vector<size_t> &offsets) {
        check(pqsio_write_reads(handle_, rows.data(), rows.size(), offsets.data(), offsets.size()));
    }
    void finish() { check(pqsio_writer_finish(handle_)); }
};
class Reader {
    pqsio_reader *handle_ = nullptr;
public:
    Reader(const std::string &path, uint8_t min_mapq = 0) { check(pqsio_reader_open(path.c_str(), min_mapq, &handle_)); }
    Reader(const Reader &) = delete;
    Reader &operator=(const Reader &) = delete;
    ~Reader() { pqsio_reader_destroy(handle_); }
    Kind kind() const { return static_cast<Kind>(check(pqsio_reader_kind(handle_))); }
    void contigs(pqsio_contigs_callback cb, void *user = nullptr) { check(pqsio_reader_contigs(handle_, cb, user)); }
    bool next(pqsio_pairs_callback pairs, pqsio_concat_callback concat, void *user = nullptr) {
        return check(pqsio_reader_next(handle_, pairs, concat, user)) == 1;
    }
};
} // namespace pqsio
