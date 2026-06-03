请读取 `.moju/ai/context.md`。

任务：
review flow，请改进

任务 ID：
flows.review_flow

当前视图：
Flows

当前选中：Flow `Orchestra.Run`

当前诊断：
- check info: OK - 121 structs, 13 flows, 19 modules, 8 verify cases

具体要求：
请读取 `.moju/ai/context.md`，review 当前 flow 的建模，并直接改进。

重点检查：
1. trigger / actor 是否合理。
2. step 划分是否清晰，是否过细或过粗。
3. step -> Event / step -> Event? 的事件生产语义是否正确。
4. produced event 是否有合理 consumer。
5. on handler / react block 是否表达了正确的异步或同步行为。
6. call / emit / create / ensure 是否符合业务语义。
7. goto / match 路由是否清晰，是否可以简化。
8. verify case 是否需要同步调整。

改进要求：
- 保持业务语义不变。
- 优先改 flow、event、verify，必要时调整 capability。
- 不要把技术实现细节塞进 flow。
- 修改后运行 `moju verify .` / `moju readiness .`，或项目已有验证命令。

通用约束：
1. 不要改无关文件。
2. 保持现有模型语义，不要做无关重构。
3. 如果必须修改项目实现代码，先说明原因，并保持改动最小。
4. 最后总结修改内容、验证命令和结果。
