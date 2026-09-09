# convert 列式转换与并行写入验收

此报告记录固定转换线程池之前的列式/并行写入版本。后续线程池复用改动及测试见 [固定线程池验收](convert-pool-acceptance.md)；下文性能数据保留为原轮次结果。

日期：2026-09-09。结论：本轮正确性、相关回归和合成性能验收通过；不代表真实基因组或冷盘吞吐量。

## 实现

- 输入直接消费 `ConcatColumns`，输出直接构造 `PairColumns`；不经过 Alignment/Pair 行对象。
- 每个保留片段只计算一次中点；pair ID 直接格式化到连续 UTF-8 缓冲区。
- 复用现有有界编码执行器，增加列 shard 支持；同步 Writer 的默认行为不变。
- `threads > 1` 时启动同等数量的持久编码线程，转换与 Parquet 编码/写入重叠；按源顺序编号 shard。
- 主线程仍顺序读取、接收有序输出和发布元数据；所有编码任务成功后才发布。失败时等待线程退出并清理 `.partial`。
- 无新依赖，Python/Rust/C 调用参数和默认值不变。

## 正确性与回归

| 检查 | 结果 |
|---|---|
| `pixi run test-convert` | 8 项通过 |
| `pixi run python -m unittest discover -s tests -p test_columns.py -v` | 10 项通过 |
| `pixi run python -m unittest discover -s tests -p test_parallel.py -v` | 4 项通过 |
| `pixi run cargo test --locked --profile dev-release -j 4 --lib` | 16 项通过；1 项独立性能测试按默认设置忽略 |
| `pixi run cargo test --locked --profile dev-release -j 4 --test storage` | 17 项通过 |
| `pixi run cargo clippy --locked --profile dev-release --lib -j 4 -- -D warnings` | 通过 |

共 55 项测试通过。包含不同线程数/批次大小/分块大小的一致性、MAPQ 和 order 过滤、空输出、UInt64 坐标、查询起点相同时的稳定排序、cn.info、完整格式校验、已有目标保护、阻塞中的转换线程取消，以及编码线程真实文件写入错误后的禁止发布和清理。

A/B 共运行 42 次独立转换（主基准 36 次，扩大输入基准 6 次）。每次独立校验解析公式得到的 reads、q0/q1 counts，并对 q0/q1 所有列进行保序 SHA-256 校验，涵盖 ID 字节及每个 ID 的长度；比较 schema、chunk size、contigs 和 cn.info。创建时间字段单独排除。所有对照通过，输出总字节数相同。

## 测量方法

- 环境：`Linux-5.4.278-1.el7.elrepo.x86_64-x86_64-with-glibc2.31`；可见逻辑 CPU 数 128。未绑核。
- 两个库均为 Pixi `dev-release` 构建，旧库在修改前保存；库 SHA-256 写入 JSON。
- 每次用新进程加载指定库；先加载库，再计时完整 convert（包括 finish/发布）。
- 墙钟与进程 CPU 时间在转换前后采样；Linux `/proc/self/status` 的 VmHWM 在验证扫描前采样，不含父进程构建 fixture 的峰值。
- 输入每个 shard 约 16,384 alignments；输出 `chunk_size=65536`，转换 `batch_rows=8192`；Polars/Rayon 内部线程设置均为 4。
- 每组重复 3 次、交替新旧执行顺序，下表为中位数；正式测量期间未同时运行本任务的编译或测试。
- 不清理系统页缓存，不调用 fsync；时间不代表断电持久化完成，也不代表冷盘性能。
- 数据为确定性、重复度较高的合成记录，压缩率较高。主基准每组约 80,000 alignments。
- `threads=4` 在旧版是最多 4 个转换线程，在新版是最多 4 个转换线程加 4 个编码线程；不是固定总 CPU 线程数的对比。`threads=1` 单独对比列式路径收益。

## 吞吐与内存

| 数据 | threads | pairs 数 | 旧版秒 | 新版秒 | 加速比 | 新版百万 pairs/s | 旧/新峰值 MiB |
|---|---:|---:|---:|---:|---:|---:|---:|
| 低阶 n=4 | 1 | 120,000 | 0.098 | 0.090 | 1.09× | 1.33 | 51.1 / 48.8 |
| 低阶 n=4 | 4 | 120,000 | 0.099 | 0.067 | 1.49× | 1.80 | 51.4 / 57.9 |
| 高阶 n=32 | 1 | 1,240,000 | 0.726 | 0.682 | 1.06× | 1.82 | 55.6 / 54.2 |
| 高阶 n=32 | 4 | 1,240,000 | 0.748 | 0.193 | 3.88× | 6.44 | 66.0 / 101.8 |
| 混合 n=2/4/16/64 | 1 | 1,993,117 | 1.154 | 1.089 | 1.06× | 1.83 | 57.5 / 54.3 |
| 混合 n=2/4/16/64 | 4 | 1,993,117 | 1.196 | 0.305 | 3.92× | 6.54 | 67.5 / 99.6 |

## 固定缓冲参数下扩大输入

将 n=32 的输入从 80,000 增至 320,000 alignments，输出从 1,240,000 增至 4,960,000 pairs：新版峰值内存从 101.8 MiB 变为 103.9 MiB，未随总输出量增加 4 倍。新版耗时中位数 0.626 s，旧版 2.824 s，加速 4.51×。这支持本轮固定分块配置的有界缓冲行为，不是所有输入的 RSS 上限证明。

## 代价与边界

- 并行编码使用额外内存：线程数为 N 时，最多 N 个在途 shard，外加当前 writer shard、转换队列、输入列和 Parquet 工作区。主基准高阶/混合峰值约 100 MiB，旧版约 66–68 MiB。
- 默认 chunk_size 为 1,000,000，高于本次实测值；不能将本次峰值直接用于默认配置。内存有限时可先使用本次的 65,536 / 8,192 参数。
- 转换线程仍按批创建；单条超长 read 不拆分；过滤和 pair ID/坐标语义保持原样。
- 未执行大型真实基因组、冷缓存、慢盘/网络文件系统、系统崩溃/断电、其他架构或 release 构建验收。

## 复现

在 pqsio 目录，先保留旧版 dev-release 共享库，再构建候选版本；本轮旧库保存在 tests/output/convert_acceptance/baseline.so。

```sh
pixi run build
pixi run python scripts/convert_bench.py --baseline tests/output/convert_acceptance/baseline.so --report docs/convert-benchmark.json
pixi run python scripts/convert_bench.py --baseline tests/output/convert_acceptance/baseline.so --report docs/convert-memory-benchmark.json --alignments 320000 --layouts high --threads 4
```

基准脚本自动清理合成输入和转换输出，仅保留 JSON 报告；旧库对照位于 tests/output，不作为源代码提交。

- [基准脚本](../scripts/convert_bench.py)

## 本轮修改文件

- `src/convert.rs`：列式展开、并行编码开关、故障测试。
- `src/columns.rs`：列 shard 交给共享编码执行器。
- `src/parallel.rs`：执行器支持列任务和独立 Writer 编码池。
- `src/lib.rs`：原行任务显式标记无列数据。
- `python/pqsio/convert.py`：并行语义说明。
- `tests/test_convert.py`：排序、元数据、完整验证覆盖。
- `scripts/convert_bench.py`：可复现 A/B 校验与性能测量。
- `README.md` 及本报告：使用说明和验收结论。
