#include "pqsio.hpp"
#include <cassert>
#include <vector>
static int32_t collect(const pqsio_alignment *rows, size_t n, void *user) {
    auto &ids = *static_cast<std::vector<uint64_t> *>(user);
    for (size_t i=0; i<n; ++i) ids.push_back(rows[i].read_idx);
    return 0;
}
int main(int argc, char **argv) {
    assert(argc==2);
    pqsio::Writer w(argv[1],pqsio::Kind::Concat,{{"chr1",100}},2);
    std::vector<pqsio_alignment> rows={{1,100,0,50,'+',0,0,50,0,0.5f,"pass"},
        {1,100,50,100,'-',0,50,100,30,1.0f,"pass"},{2,100,0,50,'+',0,0,50,20,0.5f,"pass"}};
    bool rejected=false;
    try { w.write_reads(rows,{0,1,3}); } catch(const std::runtime_error &) { rejected=true; }
    assert(rejected);
    w.write_reads(rows,{0,2,3}); w.finish();
    pqsio::Reader r(argv[1]); std::vector<uint64_t> ids;
    while(r.next(nullptr,collect,&ids)) {}
    assert((ids==std::vector<uint64_t>{1,1,2}));
    return 0;
}
