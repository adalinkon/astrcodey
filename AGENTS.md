## 总原则

不要把项目搞得一堆石山，把代码放在该放的位置，保持清晰的模块边界和职责划分。

## 修改前

- 先读所属模块、调用点、测试和现有命名风格。
- 默认不要新增文件、trait、DTO、依赖、配置项或公开 API。

但如果新增内容能明确改善边界、复用、测试隔离或依赖方向，可以新增。
新增前必须说明：
- 为什么现有位置不合适；
- 新增抽象服务的边界是什么；
- 谁依赖它，谁不应该依赖它；
- 是否可以先用更小改动解决。

## 架构边界

./PROJECT_ARCHITECTURE.md 描述了 astrcode 的目标架构和设计原则。

## DTO 规则

只有数据跨边界时才创建 DTO：
内部业务逻辑使用领域类型 / value object，不使用 DTO 命名。

- HTTP 请求 / 响应
- SSE / 事件流载荷
- 前端线缆契约
- 插件 / MCP / 外部进程边界
- 明确需要版本化的持久化格式

不要为内部函数调用创建 DTO。

新增结构前，先检查现有 request / response / payload 是否已经拥有这个契约。

## 映射规则

- 在边界做映射，不要在核心逻辑里映射。
- 需要上下文的转换，用显式映射函数。
- 只有明显、无损、无需上下文的转换才用 `From`。
- 不要为了“未来可能用”添加 `Option<T>` 字段。但是可以留下TODO注释说明未来可能添加。
- 不要把内部 enum 直接暴露成线缆契约，除非它本来就是稳定协议。
- ``serde(rename_all = "camelCase")` 只应出现在外部契约类型中，例如：
protocol / wire DTO
持久化格式
配置文件格式
插件 / MCP / 外部进程协议
LLM tool call 参数类型

不要随意加到纯内部领域结构体上。

## Rust 实现

- 函数保持小而直白。
- 优先使用清晰的领域命名，不要滥用 `utils`、`helper`、`manager`。
- 避免过宽的 `pub`。
- 避免不必要的 `clone`、`unwrap`、`expect`、`panic`。
- 不要在 `.await` 时持有锁。
- 不要启动无生命周期、无错误处理、无 tracing 的后台任务。

## 验证

优先运行最小相关检查：

```bash
cargo fmt --check
cargo test -p <crate> <test_name>
cargo clippy -p <crate> --all-targets -- -D warnings
```

大范围改动再运行：

```bash
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
```

## 回复要求

每次完成修改后，回复末尾必须附带：
- **下一步建议**：基于当前改动，接下来最值得做的事情（按优先级排列）。
- **剩余风险**：当前改动中已知或潜在的隐患、未覆盖的边界情况。

## 重要

  必须遵守：
- 没有遇见bug不准写测试，非复杂逻辑不写测试
- 项目代码都在crates里面，外置代码不必理会
- 集单元测试写在被测模块同文件底部的 `#[cfg(test)] mod tests` 中。集成测试放在对应 crate 的 `tests/` 目录下。

