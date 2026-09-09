# concat 完整 read 读取：省去无过滤时的重复复制

## 修改

`src/streaming.rs` 的完整 read 组装路径，仅在 `min_mapq > 0` 或存在区域查询时构造匹配过滤缓冲区。无查询且阈值为 0 时直接保留已经组装好的列。CompleteReads 过滤分支保持原逻辑；Python/C/Rust 接口、read 边界、磁盘格式和依赖不变。

新增 `tests/test_streaming.py` 测试：默认/显式 matching_alignments、不同批次大小、跨 row group/shard read、超批次 read、零 MAPQ、空输入及全字段行列一致性。`tests/test_query.py` 补充零 MAPQ 下区域查询和空区域仍正确过滤的回归。

## 测量

使用 `scripts/streaming_copy_bench.py`，优化前后分别构建 dev-release，再运行：

```sh
pixi run cargo build --locked --profile dev-release -j 4
pixi run python scripts/streaming_copy_bench.py > docs/streaming-copy-before.json
# 应用优化并重新执行上述构建后：
pixi run python scripts/streaming_copy_bench.py > docs/streaming-copy-after.json
```

每个布局 80,000 条合成 alignment，含 UTF-8 字符串、两种 strand、0–60 MAPQ。常规布局每 read 8 条，另一布局单 read 80,000 条。输出批次 4096、writer chunk_size 20000（不拆 read）。完整 read 边界，默认 matching_alignments，MAPQ=0，无查询。每个样本独立 Python 子进程，预热一次、保留五次；CPU 0–3，Polars/Rayon 各四线程。计时包含读取、列式 Python 复制及所有输出列的 SHA-256 消费，不包含首次加载动态库、启动解释器和数据生成。前后全部样本记录数及校验值一致。

| 布局 | 优化前中位数 | 优化后中位数 | 吞吐比（后/前） |
|---|---:|---:|---:|
| 每 read 8 条 | 25.91 ms | 21.19 ms | 1.223 |
| 单 read 80,000 条 | 17.87 ms | 19.09 ms | 0.936 |

常规短 read 场景吞吐增加约 22.3%，耗时减少约 18.2%。超长 read 场景未测得收益，耗时增加约 1.22 ms（6.8%）。测量前后分阶段进行、未交替版本、未独占机器，不能认定该小幅变化的原因，也不保证其他数据布局提速。

原始结果保留 `ru_maxrss`，但子进程指标可能受父进程生成数据时的峰值继承影响，本次不以该数值证明内存下降。代码层面省去一次过滤缓冲区构造，不等于已证明进程峰值下降。未做真实基因组数据、冷缓存或端到端流水线测量。

## 检查

- Pixi dev-release 构建通过。
- `pixi run cargo test --locked --profile dev-release -j 4`：33 项通过。
- Pixi Python unittest：`test_streaming.py` 与 `test_query.py` 原运行 38 项通过；补充 `-k zero_mapq` 单项通过，共 39 项。
- `pixi run cargo clippy --locked --profile dev-release --all-targets -j 4 -- -D warnings` 通过。
- 前后基准所有列校验值一致；`git diff --check` 通过。

本次保留此前 Python 解码优化的已有修改，未提交 Git commit，也未更改锁文件。
