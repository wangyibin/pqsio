# 整数列转换实现对比

采用按 chunk 转换作为生产路径。仅改变 DataFrame 到列式批次的整数列转换；类别、字符串和 identity 浮点转换保持原逻辑。默认实现仍通过原 `frame_columns` / `next_columns` 入口调用，公开 API、C ABI、依赖与数据格式不变。

## 三种实现

- baseline：保留原逐下标 `Numbers::at` 与 checked conversion，在测试构建中使用。
- typed：每列分派一次整数类型，然后使用有空值检查的 typed iterator；仅在测试构建中使用。
- chunk：每列分派一次整数类型。无空值且类型匹配时，预分配 Vec 并逐 chunk `extend_from_slice`；不同整数宽度逐 chunk 转换，窄化保留 TryFrom 检查。含空值时回退原路径，维持空值/溢出的首个错误顺序。非标准 dtype 沿用原 Numbers::new cast 规则。

## 实测

每格单位 ms，为九次测量中位数。加速比为 baseline/chunk 的吞吐比，不是耗时降低百分比。

| 数据 | 阶段 | chunks | baseline | typed | chunk | chunk 加速比 |
|---|---|---:|---:|---:|---:|---:|
| pairs_u32 | 纯转换 | 1 | 4.642 | 4.343 | 3.914 | 1.186× |
| pairs_u32 | 纯转换 | 8 | 6.394 | 5.357 | 4.826 | 1.325× |
| pairs_u32 | 读取+转换 | 1 | 8.023 | 8.168 | 7.085 | 1.132× |
| pairs_u64 | 纯转换 | 1 | 4.659 | 4.362 | 3.573 | 1.304× |
| pairs_u64 | 纯转换 | 8 | 6.421 | 5.435 | 4.471 | 1.436× |
| pairs_u64 | 读取+转换 | 1 | 7.574 | 7.271 | 6.487 | 1.168× |
| concat_u32 | 纯转换 | 1 | 5.801 | 5.455 | 3.370 | 1.721× |
| concat_u32 | 纯转换 | 8 | 7.826 | 5.804 | 3.724 | 2.102× |
| concat_u32 | 读取+转换 | 1 | 7.332 | 6.937 | 4.778 | 1.535× |
| concat_u64 | 纯转换 | 1 | 5.728 | 4.984 | 3.079 | 1.860× |
| concat_u64 | 纯转换 | 8 | 7.694 | 5.306 | 3.421 | 2.249× |
| concat_u64 | 读取+转换 | 1 | 7.111 | 6.434 | 4.454 | 1.597× |

chunk 在本次全部测量组合中均快于 baseline 和 typed。完整读取吞吐 pairs 增加约 13%–17%，concat 增加约 54%–60%；纯转换加速 1.19–2.25 倍。typed 对 pairs 短坐标完整读取略慢，未选用。这里只证明本次合成数据收益，不代表所有真实数据或整个 CPhasing 流水线。

## 测量方法

沿用 80,000 行、64 contig、单 shard、concat 每 read 八条的确定性生成器，包含中文/空字符串、变化的类别及 MAPQ。短坐标使用 UInt32，长坐标大于 2^33。预加载数据的纯转换分别使用 1/8 个 chunk，切片/vstack/浅克隆在计时外。完整读取包含 next_frame + frame_columns（选用版本调用真实 next_columns），不包含 Reader::open、Python 封送或消费结果。

同一测试二进制中的三种实现每轮轮换顺序，每种路径预热三次，再测九次；每种实现同等次数处于第一、第二和第三顺位。固定 CPU 0–3，Polars/Rayon 四线程，Pixi dev-release；测量时未与本任务的测试或编译并行。每次将输出转换回行对象并与原始记录全字段相等检查，全部通过；验证和释放输出在计时外。

依赖编译器针对测试构建的优化，非三个独立发布二进制对比。暖缓存、同进程、不独占机器，无置信区间；未测真实数据、冷盘、峰值 RSS 或生产规模流水线。不使用本结果宣称端到端零拷贝。

复现（在 pqsio 目录）：

```sh
pixi run python -c 'import os; cpus=sorted(os.sched_getaffinity(0))[:4]; os.sched_setaffinity(0, cpus); print("CPUS", cpus, flush=True); os.execvp("cargo", ["cargo", "test", "--locked", "--profile", "dev-release", "-j", "4", "--lib", "frame_columns_bench", "--", "--ignored", "--nocapture"])'
```

## 正确性与检查

- `pixi run cargo test --locked --profile dev-release -j 4`：34 项通过，性能测试默认 ignored。
- 显式性能测试：36 组测量，每组九个有效样本，全部字段一致。
- 新整数转换测试比较全部三种实现的值和错误信息，覆盖 UInt8/32/64、空列、切片偏移、多 chunk、窄化溢出、空值先后顺序、signed/float fallback。
- `pixi run cargo build --locked --profile dev-release -j 4`：通过，更新 Python 使用的共享库。
- Pixi Python unittest：test_columns.py、test_streaming.py、test_query.py 共 49 项通过。
- `pixi run cargo clippy --locked --profile dev-release --all-targets -j 4 -- -D warnings`：通过；初次 field_reassign_with_default 提示已按建议修正为结构体初始化。
- `git diff --check`：通过。

修改文件：src/columns.rs（实现与边界测试）、src/frame_columns_bench.rs（三路比较）；本报告及原始结果。历史基线报告保留并标注测量程序已更新。没有改动锁文件或提交 Git commit。
