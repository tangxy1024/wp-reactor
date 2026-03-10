# file_input 示例

这个示例演示 `wfusion` 的 `file` 输入源：启动后自动读取 `data/port_scan.ndjson`，并把数据按 `stream=netflow` 注入运行时。

## 文件说明

- `wfusion.toml`：启用 `[[sources]] type = "file"`，并复用 `../distinct` 的 schema/rule。
- `data/port_scan.ndjson`：10 条 `syn` 事件，触发 `port_scan` 规则 1 次告警。

## 验证步骤

在仓库根目录执行：

```bash
rm -f examples/file_input/alerts/all.jsonl

cargo run -p wf-engine -- run --config examples/file_input/wfusion.toml &
PID=$!
sleep 2
kill -INT "$PID"
wait "$PID"

wc -l examples/file_input/alerts/all.jsonl
cat examples/file_input/alerts/all.jsonl
```

预期：

- `examples/file_input/alerts/all.jsonl` 存在；
- 行数为 `1`；
- `rule_name` 为 `port_scan`，`entity_id` 为 `10.0.0.1`。
