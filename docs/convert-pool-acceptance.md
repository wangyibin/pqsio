# convert 固定线程池复用验收

## 改动

`threads > 1` 时，转换线程池只在一次 convert 调用开始时创建，各输入批次复用相同线程。每个任务通过 Arc 共享输入列，不复制列缓冲；每个线程的任务队列和输出队列容量均为 1。每批结果按源顺序消费，不累计多个未消费的输入批次。

线程池部分启动失败时关闭队列并回收已启动线程；任务或写入失败时先释放所有输出接收端，让阻塞的生产线程退出，再回收线程。失败的池禁止继续提交。编码池继续沿用前一轮的持久线程池；公共 API、输出顺序、ID 和过滤语义保持不变。

## 验证

- `pixi run test-convert`：8 项通过。
- `pixi run cargo test --locked --profile dev-release -j 4 --lib convert::tests`：3 项通过。新增测试确认同一组存活线程连续处理 20 个批次，并验证输出顺序和 q0/q1 计数；保留转换线程阻塞时取消、编码错误清理测试。
- `pixi run cargo clippy --locked --profile dev-release --lib -j 4 -- -D warnings`：通过。

## 小批次 A/B

对照库为本轮改动前的列式转换＋并行编码版本，双方都使用最多 4 个转换线程和 4 个编码线程。80,000 条合成 alignments，每条 read 4 个片段，输出 120,000 pairs；`threads=4, batch_rows=64, chunk_size=65536`，Polars/Rayon 各设 4。每组独立进程重复 3 次，交替新旧顺序，报告中位数。测量期间未并行执行本任务的构建或测试。

| 指标 | 每批创建线程 | 固定线程池 |
|---|---:|---:|
| 墙钟时间 | 0.293 s | 0.139 s |
| pairs/s | 408,975 | 864,248 |
| CPU 时间 | 0.697 s | 0.269 s |
| 峰值 RSS | 51.9 MiB | 55.2 MiB |

本组墙钟加速 **2.11×**。6 次转换的 q0/q1 所有列保序哈希、计数、schema、contigs、cn.info 均一致；元数据仅排除创建时间。输出大小相同。

复现（在 pqsio 目录，需保留改动前的库）：

```sh
pixi run python scripts/convert_bench.py --baseline tests/output/convert_acceptance/before-pool.so --report docs/convert-pool-benchmark.json --layouts low --threads 4 --batch-rows 64
```

本次特意使用小批次检验线程创建开销，不能把 2.11× 外推到默认批次或真实基因组；未重复大型数据测试。读取仍顺序进行，单条超长 read 不拆分。线程池按 convert 调用复用，不跨调用全局共享。

本轮修改文件：`src/convert.rs`（实现与 Rust 测试）、`README.md`、前轮报告的版本说明和本报告。
