#include "pqsio.h"
#include <assert.h>
static int32_t receive(const pqsio_alignment *rows, size_t n, void *user) {
    size_t *count = user;
    assert(n > 0 && rows[0].read_idx > 0);
    *count += n;
    return 0;
}
int main(int argc, char **argv) {
    assert(argc == 2);
    for (uint32_t boundary = 0; boundary < 2; ++boundary) {
        for (uint32_t filter = 1; filter < 3; ++filter) {
            pqsio_stream *r = NULL;
            size_t count = 0;
            assert(pqsio_stream_open(argv[1], 30, 3, boundary, filter, &r) == 0);
            assert(pqsio_stream_kind(r) == 1);
            int32_t status;
            while ((status = pqsio_stream_next(r, NULL, receive, &count)) == 1) {}
            assert(status == 0 && count == (filter == 1 ? 3 : 8));
            assert(pqsio_stream_destroy(r) == 0);
            assert(pqsio_stream_open(argv[1], 30, 3, boundary, filter, &r) == 0);
            pqsio_column_batch *batch = NULL, *held = NULL;
            count = 0;
            while ((status = pqsio_stream_next_columns(r, &batch)) == 1) {
                pqsio_concat_columns v;
                assert(pqsio_column_batch_concat(batch, &v) == 0);
                assert(v.read_offsets.data[0] == 0);
                assert(v.read_offsets.data[v.read_offsets.len-1] == v.read_idx.len);
                assert(v.start.data[0] >= UINT64_C(4294967296));
                count += v.read_idx.len;
                if (!held) held = batch;
                else assert(pqsio_column_batch_destroy(batch) == 0);
            }
            assert(status == 0 && !batch && count == (filter == 1 ? 3 : 8));
            assert(pqsio_stream_destroy(r) == 0);
            pqsio_concat_columns v;
            assert(pqsio_column_batch_concat(held, &v) == 0 && v.read_idx.len > 0);
            assert(pqsio_column_batch_destroy(held) == 0);
        }
    }
    return 0;
}
