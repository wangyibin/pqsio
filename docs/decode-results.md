# Python 行式读取解码优化

将 Pair / Alignment 的逐记录 dataclass 字段反射、类型判断和中间字典替换为固定 ABI 字段读取及位置参数构造。其他类型保留原回退逻辑；UTF-8 解码、空字符串、NULL 字符串、整数宽度及 strand 转换语义保持一致。不改 Rust、C ABI、依赖或存储格式。

## 可复现测量

在 pqsio 目录运行：

```sh
pixi run python scripts/decode_bench.py > docs/decode-results.json
```

脚本在同一个进程、同一个原生库、同一份合成数据上交替切换原反射实现与当前实现。每种格式 80,000 条记录，shard 20,000；concat 每 read 8 条 alignment。包含中文字符串，每次读取后在计时外逐记录比较所有字段。每条路径预热一次，再测量五次；固定 CPU 0–3，Polars/Rayon 各四线程。最终测量未与本任务测试同时运行。

计时包含 Reader 打开、Parquet 读取、原生行转换、Python 解码、结果列表构建和关闭，不包含数据生成、写入、首次动态库加载或结果比较。

| 格式 | 优化前中位数 | 优化后中位数 | 吞吐提升 |
|---|---:|---:|---:|
| pairs | 0.400695 s | 0.133390 s | 3.00 倍 |
| concat | 0.497503 s | 0.148217 s | 3.36 倍 |

该结果是暖缓存、单进程内交替测量；没有独占机器、隔离每个样本进程或测量峰值内存。不能外推为 Rust 核心、列式读取、写入或全基因组流水线同等加速。

## 验证

- `pixi run python -m unittest discover -s tests -p 'test_*.py' -q`：111 项，109 项通过，两项 CPhasing reader 兼容性测试因未找到 cphasing 失败。
- 加入相邻 `../CPhasing` 导入路径后重跑 `test_compatibility.py` 和 `test_decode.py`：11 项，9 项通过；上述两项因环境缺少 click 仍未执行成功。未安装额外依赖。
- 新增解码测试覆盖所有字段、UTF-8/空/NULL 字符串、无效 UTF-8、64 位及 32 位边界。
- 基准全部读取结果与原始记录一致；`git diff --check` 通过。
- 使用现有 Pixi dev-release 原生库，未修改或重新编译 Rust，未跑大规模数据或端到端基因组流程。
