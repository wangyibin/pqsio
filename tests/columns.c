#include "pqsio.h"
#include <assert.h>
#include <string.h>

int main(int argc, char **argv) {
    assert(argc == 3);
    assert(pqsio_abi_version() == 1 && pqsio_columnar_version() == 1);
    pqsio_contig ctg = {"chr1", UINT64_C(5000000000)};
    uint64_t so[] = {0,0,3}, pos[] = {UINT64_C(4294967297),2};
    uint8_t text[] = {0xe8,0xaf,0xbb}, plus[] = {'+','+'}, minus[] = {'-','-'}, mq[] = {0,60};
    uint32_t chrom[] = {0,0};
    pqsio_pairs_columns v = {
        {so,3}, {text,3}, {chrom,2}, {pos,2}, {chrom,2}, {pos,2}, {plus,2}, {minus,2}, {mq,2}
    };
    pqsio_writer *w = NULL;
    assert(pqsio_writer_open(argv[1],0,&ctg,1,1,&w)==0);
    v.pos2.len=1;
    assert(pqsio_write_pairs_columns(w,&v)==-1);
    v.pos2.len=2;
    v.pos2.data=NULL;
    assert(pqsio_write_pairs_columns(w,&v)==-1);
    /* Deliberate misalignment within a live, sufficiently large allocation. */
    unsigned char raw[32];
    uintptr_t aligned=((uintptr_t)raw+7)&~(uintptr_t)7;
    v.pos2.data=(const uint64_t *)(aligned+1);
    assert(pqsio_write_pairs_columns(w,&v)==-1);
    v.pos2.data=pos;
    assert(pqsio_write_pairs_columns(w,&v)==0);
    uint64_t zero=0;
    pqsio_pairs_columns empty={0};
    empty.read_id_offsets.data=&zero; empty.read_id_offsets.len=1;
    assert(pqsio_write_pairs_columns(w,&empty)==0);
    assert(pqsio_writer_finish(w)==0);
    assert(pqsio_writer_destroy(w)==0);
    pqsio_reader *r=NULL;
    assert(pqsio_reader_open(argv[1],0,&r)==0);
    pqsio_column_batch *b=NULL;
    assert(pqsio_reader_next_columns(r,&b)==1);
    assert(pqsio_reader_destroy(r)==0);
    pqsio_pairs_columns result;
    assert(pqsio_column_batch_pairs(b,&result)==0);
    assert(result.pos1.len==1 && result.pos1.data[0]==pos[0]);
    assert(result.read_id_offsets.len==2 && result.read_id_offsets.data[1]==0);
    pqsio_concat_columns wrong;
    assert(pqsio_column_batch_concat(b,&wrong)==-1);
    assert(pqsio_column_batch_destroy(b)==0);
    assert(pqsio_column_batch_destroy(NULL)==0);

    uint64_t ro[]={0,2}, ids[]={1,1}, start[]={0,UINT64_C(4294967296)}, end[]={50,UINT64_C(4294967346)};
    uint32_t length[]={100,100}, qs[]={0,50}, qe[]={50,100};
    float identity[]={0.5f,1.0f};
    pqsio_concat_columns c = {
        {ro,2},{ids,2},{length,2},{qs,2},{qe,2},{plus,2},{chrom,2},
        {start,2},{end,2},{mq,2},{identity,2},{so,3},{text,3}
    };
    assert(pqsio_writer_open(argv[2],1,&ctg,1,1,&w)==0);
    assert(pqsio_write_concat_columns(w,&c)==0);
    assert(pqsio_write_concat_columns(w,&c)==-1); /* duplicate complete read */
    assert(pqsio_writer_finish(w)==0);
    assert(pqsio_writer_destroy(w)==0);
    assert(pqsio_reader_open(argv[2],1,&r)==0);
    assert(pqsio_reader_next_columns(r,&b)==1);
    assert(pqsio_column_batch_concat(b,&c)==0);
    assert(c.read_idx.len==1 && c.read_idx.data[0]==1);
    assert(c.start.data[0]==start[1] && c.identity.data[0]==1.0f);
    assert(c.read_offsets.len==2 && c.read_offsets.data[1]==1);
    assert(c.filter_reason_bytes.len==3 && memcmp(c.filter_reason_bytes.data,text,3)==0);
    assert(pqsio_reader_destroy(r)==0);
    assert(c.end.data[0]==end[1]);
    assert(pqsio_column_batch_destroy(b)==0);
    return 0;
}
