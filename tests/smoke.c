#include "pqsio.h"
#include <assert.h>
#include <stdio.h>
#include <string.h>
static size_t seen = 0;
static int32_t pairs(const pqsio_pair *r, size_t n, void *unused) {
    (void)unused;
    assert(n == 1 && r[0].pos1 == 1 && r[0].mapq == 60);
    assert(strcmp(r[0].read_id, "c-read") == 0);
    seen += n; return 0;
}
static int32_t contigs(const pqsio_contig *c, size_t n, void *unused) {
    (void)unused; assert(n == 1 && c[0].length == 100); return 0;
}
static int32_t aligns(const pqsio_alignment *r, size_t n, void *unused) {
    (void)unused; assert(n == 2 && r[0].read_idx == 7 && r[1].end == 90); seen += n; return 0;
}
static void check(int32_t rc) { if (rc < 0) { fprintf(stderr, "%s\n", pqsio_last_error()); } assert(rc >= 0); }
int main(int argc, char **argv) {
    assert(argc == 3 && pqsio_abi_version() == 1);
    pqsio_contig cs[] = {{"chr1",100}};
    pqsio_writer *w = NULL;
    check(pqsio_writer_open(argv[1],0,cs,1,2,&w));
    pqsio_pair rs[] = {{"c-read",0,1,0,100,'+','-',60}};
    check(pqsio_write_pairs(w,rs,1)); check(pqsio_writer_finish(w)); check(pqsio_writer_destroy(w));
    pqsio_reader *r = NULL; check(pqsio_reader_open(argv[1],0,&r));
    check(pqsio_reader_contigs(r,contigs,NULL)); assert(pqsio_reader_next(r,pairs,NULL,NULL)==1);
    assert(pqsio_reader_next(r,pairs,NULL,NULL)==0 && seen==1); check(pqsio_reader_destroy(r));
    pqsio_alignment ar[] = {{7,100,0,40,'+',0,0,40,0,0.5f,"pass"},{7,100,40,80,'-',0,50,90,20,1.0f,"pass"}};
    check(pqsio_writer_open(argv[2],1,cs,1,1,&w)); check(pqsio_write_read(w,ar,2)); check(pqsio_writer_finish(w)); check(pqsio_writer_destroy(w));
    check(pqsio_reader_open(argv[2],0,&r)); assert(pqsio_reader_kind(r)==1);
    assert(pqsio_reader_next(r,NULL,aligns,NULL)==1 && seen==3); check(pqsio_reader_destroy(r));
    return 0;
}
