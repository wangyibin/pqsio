#include "pqsio.hpp"
#include <cassert>
#include <utility>
int main(int argc, char **argv) {
    assert(argc==3);
    std::vector<pqsio_contig> contigs{{"chr1", 5000000000ULL}};
    uint64_t offsets[]{0,0}, pos[]{4294967297ULL};
    uint32_t chrom[]{0}; uint8_t strand[]{'+'}, quality[]{60};
    pqsio_pairs_columns p{{offsets,2},{nullptr,0},{chrom,1},{pos,1},{chrom,1},{pos,1},{strand,1},{strand,1},{quality,1}};
    {
        pqsio::Writer w(argv[1],pqsio::Kind::Pairs,contigs,2);
        w.write_pairs_columns(p); w.finish();
    }
    pqsio::ColumnBatch owner;
    {
        pqsio::Reader r(argv[1]); owner=r.next_columns(); assert(!r.next_columns());
    }
    auto moved=std::move(owner); assert(!owner);
    assert(moved.pairs().pos1.data[0]==pos[0]);
    uint64_t reads[]{0,1}, ids[]{7}, start[]{0}, end[]{50};
    uint32_t length[]{100}, qs[]{0}, qe[]{50}; float identity[]{0.5f};
    pqsio_concat_columns c{{reads,2},{ids,1},{length,1},{qs,1},{qe,1},{strand,1},{chrom,1},{start,1},{end,1},{quality,1},{identity,1},{offsets,2},{nullptr,0}};
    {
        pqsio::Writer w(argv[2],pqsio::Kind::Concat,contigs,1);
        w.write_concat_columns(c); w.finish();
    }
    {
        pqsio::Reader r(argv[2]); owner=r.next_columns();
    }
    assert(owner.concat().read_idx.data[0]==7);
    return 0;
}
