#include "pqsio.hpp"
#include <thread>
#include <cassert>
int main(int argc, char **argv) {
    assert(argc == 2);
    pqsio::ParallelWriter writer(argv[1], pqsio::Kind::Pairs, {{"chr1", 100}}, 1, 2, 1, 4096);
    auto producer = writer.producer();
    const std::vector<pqsio_pair> row{{"r",0,1,0,2,'+','-',20}};
    producer.write_pairs(1, row);
    std::thread future([&] { producer.write_pairs(2, row); });
    producer.write_pairs(0, row);
    future.join();
    writer.finish();
    try { producer.write_pairs(3, row); return 1; } catch (const std::runtime_error &) {}
}
