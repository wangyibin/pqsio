#include "pqsio.hpp"
#include <cassert>
static int32_t receive(const pqsio_alignment *rows, size_t n, void *user) {
    assert(n > 0 && rows[0].read_idx > 0);
    *static_cast<size_t *>(user) += n;
    return 0;
}
int main(int argc, char **argv) {
    assert(argc == 2);
    for (auto boundary : {pqsio::ReadBoundary::Rows, pqsio::ReadBoundary::CompleteReads}) {
        for (auto filter : {pqsio::ConcatFilter::MatchingAlignments, pqsio::ConcatFilter::CompleteReads}) {
            pqsio::StreamingReader r(argv[1], 30, 3, boundary, filter);
            size_t count = 0;
            while (r.next(nullptr, receive, &count)) {}
            assert(count == (filter == pqsio::ConcatFilter::MatchingAlignments ? 3 : 8));
            r.close();
            r.close();
            pqsio::ColumnBatch held;
            {
                pqsio::StreamingReader columns(argv[1], 30, 3, boundary, filter);
                count = 0;
                while (auto batch = columns.next_columns()) {
                    auto v = batch.concat();
                    count += v.read_idx.len;
                    assert(v.read_offsets.data[v.read_offsets.len-1] == v.read_idx.len);
                    held = std::move(batch);
                }
                assert(count == (filter == pqsio::ConcatFilter::MatchingAlignments ? 3 : 8));
                columns.close();
            }
            assert(held.concat().read_idx.len > 0);
        }
    }
}
