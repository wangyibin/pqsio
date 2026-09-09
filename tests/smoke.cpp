#include "pqsio.hpp"
#include <cassert>
#include <cstring>
static int32_t read_pairs(const pqsio_pair *rows, size_t n, void *) {
    assert(n==1 && std::strcmp(rows[0].read_id,"cpp-read")==0); return 0;
}
static int32_t read_concat(const pqsio_alignment *rows, size_t n, void *) {
    assert(n==1 && rows[0].read_idx==9 && rows[0].identity==0.5f); return 0;
}
int main(int argc, char **argv) {
    assert(argc==3);
    pqsio::Writer w(argv[1],pqsio::Kind::Pairs,{{"chr1",100}});
    w.write_pairs({{"cpp-read",0,1,0,100,'+','-',60}}); w.finish();
    pqsio::Reader r(argv[1]); assert(r.kind()==pqsio::Kind::Pairs);
    assert(r.next(read_pairs,nullptr)); assert(!r.next(read_pairs,nullptr));
    pqsio::Writer c(argv[2],pqsio::Kind::Concat,{{"chr1",100}});
    c.write_read({{9,100,0,50,'+',0,0,50,1,0.5f,"pass"}}); c.finish();
    pqsio::Reader cr(argv[2]); assert(cr.next(nullptr,read_concat));
    return 0;
}
