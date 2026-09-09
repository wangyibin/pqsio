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
        }
    }
    return 0;
}
