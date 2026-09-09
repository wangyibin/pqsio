# Rust DataFrame → 列式数组：基线测量

> 历史基线记录。测量程序现已扩展为三种实现交替比较，最新结果与复现方式见 [整数列转换实现对比](frame-columns-comparison.md)。

## 结论

80,000 行暖缓存合成数据中，`Reader::frame_columns` 占 `next_frame + frame_columns` 分段耗时约 61%–83%。这是值得优化的共享路径，但本次没有改动转换算法，也没有验证候选快速路径的加速效果。转换时间包含数值、类别代码、UTF-8 字符串和 read offsets，不能全部归因于数值列逐元素访问。

| 数据 | next_frame 中位数 ms | 紧接读取的转换 ms | 独立 next_columns ms | 转换占分段时间 |
|---|---:|---:|---:|---:|
| pairs / 短坐标 | 2.925 | 5.545 | 7.904 | 65.5% |
| pairs / 长坐标 | 2.932 | 4.632 | 7.543 | 61.2% |
| concat / 短坐标 | 1.374 | 6.482 | 7.953 | 82.5% |
| concat / 长坐标 | 1.372 | 6.024 | 7.370 | 81.4% |

占比按两个阶段各自中位数计算，非采样 profiler 的 CPU 占比。独立 next_columns 是另一次读取，不能要求其时间等于两个阶段中位数之和。Reader::open 在计时外，因此“完整读取”在此特指 next_columns 调用，不包括打开元数据、输出消费或 Python/C 接口。

同一预加载 DataFrame，合并为单 chunk 或切分并 vstack 成 8 个 chunk；构造和浅克隆均在计时外：

| 数据 | 单 chunk 转换 ms | 8 chunks 转换 ms |
|---|---:|---:|
| pairs / 短坐标 | 4.945 | 7.219 |
| pairs / 长坐标 | 4.720 | 6.473 |
| concat / 短坐标 | 7.779 | 8.695 |
| concat / 长坐标 | 6.125 | 8.401 |

缓存转换与紧接读取的转换有不同的缓冲区共享/释放和缓存状态，不应将其差值解释成某个单独操作的成本。多 chunk 样本说明值得测量按 chunk 遍历的候选实现，而不是证明某个实现必然更快。

## 方法与复现

新增 `src/frame_columns_bench.rs`，由 `src/lib.rs` 的 `cfg(test)` 模块入口加载。测试默认 ignored；不修改生产函数、公开接口和依赖。直接调用当前内部 `next_frame`、`frame_columns`、公开 `next_columns`，使用 black_box 防止结果被优化消除。

每种数据 80,000 行，64 个 contig，单 shard；concat 每 read 8 条 alignment。短坐标低于 2^32，长坐标大于 2^33，含中文字符串、零 MAPQ 和变化的坐标/类别字段。每个测量先预热一次，再保留七次；同进程按固定顺序运行，CPU 0–3，Polars/Rayon 各四线程，Pixi dev-release。完整行对象转换和与原始生成记录的全字段相等检查均在计时外执行，每个样本通过。临时数据限定在仓库 tests/output，结束后清理。

在 pqsio 目录执行固定 CPU 的版本：

```sh
pixi run python -c 'import os; cpus=sorted(os.sched_getaffinity(0))[:4]; os.sched_setaffinity(0, cpus); print("CPUS", cpus, flush=True); os.execvp("cargo", ["cargo", "test", "--locked", "--profile", "dev-release", "-j", "4", "--lib", "frame_columns_bench", "--", "--ignored", "--nocapture"])'
```

## 检查与边界

- 显式运行 ignored 测量测试：通过，全部字段一致。
- `pixi run cargo clippy --locked --profile dev-release --all-targets -j 4 -- -D warnings`：通过。
- `git diff --check`：通过。
- 仅新增测量代码，未重跑完整业务测试；已有优化保持原样。
- 未独占机器、未做样本随机化/独立进程隔离；毫秒级测量存在调度、缓存和分配器波动。
- 未测量真实基因组数据、冷盘、空值处理性能或峰值内存；不是整个流水线占比。

建议下一步在此基线下比较数值列按类型/按 chunk 转换的候选实现，同时保留空值和溢出检查，再判断实际整体收益。
