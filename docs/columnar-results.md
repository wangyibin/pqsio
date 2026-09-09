# 列式接口验收与性能记录

已实现同步 pairs / concat 列式读写，保留原行式接口、C ABI v1 和磁盘 schema。
[使用示例与完整契约](columnar.md)包括 Rust、C、C++17、Python，类型、offsets、所有权及能力检测。
基础 Python 包没有新增依赖。以下结果来自最终代码的实际运行。

## 检查结果

- Pixi / dev-release 构建通过；Rust 1 个单元测试 + 15 个存储测试通过。
- Python 测试集 35 项通过，包括真正编译、链接并执行的 C11 / C++17 消费者。
- 两种格式的列写→行读、行写→列读，全字段、q0/q1、长/短坐标 schema 和计数对照通过。
- 覆盖空数据、UTF-8/空字符串、列长度、offsets、非法字段、完整/超大 read、跨调用 ID、
  原子校验失败后继续写入、I/O 失败后不可复用及 staging 清理。
- 覆盖旧版 shard-local ID 与 MAPQ、Python memoryview 生存期、C batch 独立于 reader、C++ 移动 RAII。
- 新 Python 包加载真实已归档 v0.0.1 原生库：旧行式功能可用，列式方法明确报错。
- 原 CPhasing Python reader / Rust writer 兼容性测试及原并行接口回归通过。
- `cargo clippy --profile dev-release --all-targets -- -D warnings` 通过；`git diff --check` 通过。
  新增 Rust 代码作了定向格式化，没有对仓库整体重排版。

从工作区根目录实际执行的主要命令：

```sh
pixi run --manifest-path cphasing-rs/pixi.toml cargo build --offline --locked --manifest-path pqsio/Cargo.toml --profile dev-release -j 4
pixi run --manifest-path cphasing-rs/pixi.toml cargo test --offline --locked --manifest-path pqsio/Cargo.toml --profile dev-release -j 4
pixi run --manifest-path cphasing-rs/pixi.toml cargo clippy --offline --locked --manifest-path pqsio/Cargo.toml --profile dev-release --all-targets -j 4 -- -D warnings
PQSIO_LIBRARY="$PWD/pqsio/target/dev-release/libpqsio.so" PYTHONPATH=pqsio/python:CPhasing pixi run --manifest-path cphasing-rs/pixi.toml python -m unittest discover -s pqsio/tests -p 'test_*.py' -v
pixi run --manifest-path cphasing-rs/pixi.toml python pqsio/scripts/columnar_bench.py --rows 80000 --repetitions 5
```

构建及检查过程中修复了新模块的可见性错误和 Clippy 冗余转换/对齐检查提示；最终上述检查通过。

## 性能方法

复用既有 `benchmarks/run_ffi.py` 的独立进程、固定线程/CPU、预热、交替顺序和写出内容校验方法。
比较**同一个最终原生库**的现有 Python 行式批量接口和新增列式接口，不是不同版本的 Rust 核心基准。
每种格式 80,000 条记录，64 个 contig；concat 每 read 8 条 alignment（10,000 reads）。
提交批次 4,096 条（最后一批较小），shard 20,000 条，两路径均产生相同边界。
使用同一 `ParquetWriter` 默认压缩配置，Polars/Rayon 各 4 线程，固定 4 个 CPU。
每个格式/阶段/接口组合先预热一次，再独立进程重复 5 次，交替行式/列式次序。

- **prepared**：计时前构建全部 Python dataclass 批次及 offsets，或全部 typed array 批次。
  计时包括 open、提交和 finish。行式 dataclass→ctypes→Rust 的封送是其接口成本，包含在计时内。
- **e2e**：包括上述输入构建，再加同样的写入过程；不包括解释器启动和动态库加载。
- **read**：所有读进程使用相同预生成数据集，计时包括 open、迭代、消费所有语义字段和 close。
  两路径都计算各数值字段总和、字符串字节/长度总和、记录/read 数；核对聚合结果一致。
  列式消费直接遍历 array，没有再构造行对象。该消费方式适合列式分析，不代表所有应用。
- 每次写入后，在计时和 RSS 快照之外，用独立生成器对所有记录的所有字段做 SHA-256 校验。
  所有写入结果一致；读取聚合结果跨接口/重复一致。小型正确性测试另做逐字段相等检查。
- RSS 是每个独立进程 `ru_maxrss` 的峰值（Linux KiB 转 MiB），写入快照在验证前采集；
  **prepared 的 RSS 仍包含输入构建和全部输入对象**。读取进程不运行 writer，避免写入峰值污染。

环境：Rust 1.91.1，Polars 0.49.1，Python 3.8.8，`Linux-5.4.278-1.el7.elrepo.x86_64-x86_64-with-glibc2.10`；CPU affinity [0, 1, 2, 3]。

时间单位秒；括号为 5 次的最小–最大值。RSS 为每次峰值的中位数（括号同为最小–最大）。

| 格式 | 阶段 | 接口 | 时间中位数（范围） | 吞吐：万条/秒 | 峰值 RSS MiB（范围） |
|---|---|---|---:|---:|---:|
| pairs | prepared | row | 0.4126 (0.4093–0.4201) | 19.39 | 62.5 (60.1–64.1) |
| pairs | prepared | column | 0.0451 (0.0447–0.0522) | 177.57 | 35.3 (35.1–36.7) |
| pairs | e2e | row | 0.5298 (0.5275–0.5424) | 15.10 | 60.9 (60.4–62.9) |
| pairs | e2e | column | 0.1191 (0.1189–0.1257) | 67.19 | 35.4 (34.4–37.1) |
| pairs | read | row | 0.5670 (0.5642–0.5902) | 14.11 | 44.0 (43.7–46.0) |
| pairs | read | column | 0.0335 (0.0334–0.0383) | 238.51 | 29.8 (28.9–30.5) |
| concat | prepared | row | 0.5491 (0.5441–0.5576) | 14.57 | 70.6 (69.7–71.6) |
| concat | prepared | column | 0.0572 (0.0525–0.0594) | 139.88 | 38.1 (37.4–39.4) |
| concat | e2e | row | 0.6684 (0.6678–0.6944) | 11.97 | 70.9 (70.1–72.8) |
| concat | e2e | column | 0.1460 (0.1449–0.1567) | 54.80 | 37.9 (37.3–38.7) |
| concat | read | row | 0.7778 (0.7587–0.7848) | 10.29 | 56.1 (54.5–57.3) |
| concat | read | column | 0.0363 (0.0356–0.0365) | 220.29 | 31.6 (31.4–32.7) |

本次所有测量路径的列式接口均更快且峰值内存更低；没有观测到列式退化的测量项。
收益主要来自避开 Python 行 dataclass 的封送/反射、C 行数组、Rust 行字符串对象；
这不证明 Parquet 编解码本身提速，也不证明 C/Rust 原生调用具有同样倍数的收益。
输入构建占比增加后，端到端收益明显小于 prepared；列式提交仍需缓冲复制、字符串/类别转换和编码。
不做“端到端零拷贝”声明。

原始 60 个有效样本与汇总见 [columnar-results.json](columnar-results.json)。
这是单机小型合成数据、暖缓存、无 fsync 的结果，未独占整台机器；范围不是置信区间。
没有把结果外推到全基因组、冷盘、网络存储、超大字符串或长时间运行的内存行为。

## 生命周期、复制和未验证事项

- Rust 读结果拥有 Vec；C/C++ 使用独立原生 batch，视图必须短于 batch 生命周期。
  Python 每列一次批量复制到自有 array，原生内存随即释放；memoryview 保持 array 存活。
- 写入复制输入列到 writer 缓冲；Polars Series、字符串/类别化、q1 过滤、Parquet 编码继续分配。
  读取先构建 DataFrame，再形成连续 Vec；Python 额外进行一次批量复制和目标数组初始化。
- 行/列非空提交混用会结束当前缓冲 shard，可能改变分片边界，但不拆完整 read、不中断 ID 契约。
- 原始指针的真实长度、存活和并发修改是调用者责任；不会尝试验证悬空指针。
- 测试实际解释器为 Python 3.8.8（现有命令环境），包声明为 ≥3.9；**未覆盖声明支持的版本矩阵**。
  未验证 Windows/macOS、32 位、大端、Miri/sanitizer、故意悬空指针或独立 C/Rust 性能。
- 不新增异步列式写入、Arrow/NumPy、索引、CLI、云存储；没有执行大型基因组流程或下载数据。

## 修改文件

- `src/columns.rs`：typed owned/view 批次、列式校验/缓存/读写。
- `src/lib.rs`：接入 Writer/Reader，复用字段校验、磁盘 dtype/categorical 规则和读取帧处理。
- `src/parallel.rs`：提取共用 Parquet shard 写入/过滤/计数函数，原并行协议不变。
- `src/ffi.rs`、`include/pqsio.h`：新增 span、能力检测、列式读写、独立 batch 所有权函数。
- `include/pqsio.hpp`：C++17 move-only batch RAII 与 writer/reader 方法。
- `python/pqsio/columns.py`、`python/pqsio/__init__.py`：列批次、UTF-8 打包、缓冲验证、批量复制和迭代器。
- `tests/storage.rs`、`tests/test_columns.py`：Rust/Python 交叉路径及边界测试。
- `tests/columns.c`、`tests/columns.cpp`、`tests/test_native.py`：实际 C/C++ 编译和调用测试。
- `scripts/columnar_bench.py`：可重复的中等规模基准与一致性检查。
- `README.md`、`docs/columnar.md`、`docs/columnar-results.md`、`docs/columnar-results.json`：使用文档与实测记录。

未修改依赖、锁文件、旧 C 结构体布局或现有函数签名。
