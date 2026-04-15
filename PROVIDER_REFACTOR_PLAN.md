# Provider 层重构执行方案

> 记录时间：2026-04-12
> 目标：把 `src/llm/` 重写为统一的 async `Provider` 层，支持 CLI/HTTP/未来 WS，按 3 个 PR 推进。

---

## 总体原则

- **async `Stream` 为核心抽象**，不走同步 `Iterator`。
- **Provider = 哑管道**（纯 IO），**Router = 所有策略**（重试/路由/预算），两层严格分离。
- **不引入新依赖**：tokio / reqwest(stream) / async-trait / futures 已在 `Cargo.toml`。
- 分 3 个 PR，PR1 老代码零改动，PR2 才切换并删旧，PR3 叠加高级能力。

---

## PR1 范围（下次开工从这里开始）

### 新建目录
```
src/provider/
├── mod.rs           # trait + Request + StreamEvent + Error
├── http.rs          # HttpProvider（仅 OpenAI dialect）
├── claude_cli.rs    # ClaudeCliProvider（路线 B）
└── dialect/
    └── openai.rs    # SSE parser，覆盖 GLM/Ollama/DeepSeek
```

### 核心类型（PR1 最小集）

```rust
// 只做这些，不要多写
pub struct Request {
    pub messages: Vec<Message>,
    pub system: Option<String>,
    pub model: Option<String>,
    pub params: SamplingParams,          // temperature / top_p / max_tokens
    pub cancel: Option<CancellationToken>,
}

pub enum StreamEvent {
    Delta(String),                       // PR1 用 String，PR3 再换 Bytes
    Done { reason: StopReason },
    Error(ProviderError),
}

#[async_trait]
pub trait LLMProvider: Send + Sync {
    fn name(&self) -> &str;
    async fn stream(&self, req: Request)
        -> Result<Pin<Box<dyn Stream<Item = StreamEvent> + Send>>>;

    // 默认实现，聚合 stream
    async fn call(&self, req: Request) -> Result<String> { ... }
}
```

### PR1 明确**不做**的事

- ❌ `Capabilities` 结构体（PR3 做路由时再加）
- ❌ Router 层（PR2 做）
- ❌ 重试 / 竞速 / budget（PR3）
- ❌ `ToolCall` / `Usage` 事件（PR3 接 budget 时一起上）
- ❌ `Bytes` 零拷贝（PR3 性能优化时再换）
- ❌ Anthropic HTTP / Qwen / DashScope dialect（MVP 只 OpenAI）
- ❌ Claude 长连接版路线 A（永久不做，需要时再说）
- ❌ WebSocket / SessionProvider（PR3）
- ❌ 接线到 `main.rs` / `agent/`（PR2 才接）

### PR1 必须**做**的事

- ✅ `CancellationToken` 进 `Request`——补加成本后远大于现在加
- ✅ Claude 路线 B：`claude -p --output-format stream-json --session-id <uuid> [--resume]`
- ✅ 老 `src/llm/` **一行不动**，两套并存
- ✅ PR1 结束时新代码要能 `cargo build` 通过
- ✅ 至少一个 smoke test（可以是 `#[cfg(test)]` mock SSE）

### PR1 开工前先做

1. 读 `src/llm/types.rs`（204 行）理解现有 Message/请求结构
2. 读 `src/llm/client.rs`（724 行）摸清上游怎么调
3. 读 `src/agent/loop_runner.rs` + `src/agent/mod.rs` + `src/session.rs`
4. 判断 `loop_runner` 是**轻依赖**（调入口函数）还是**重依赖**（match 老 struct）
   - 如果重依赖：PR1 里顺手在 `provider/mod.rs` 定义适配层类型（不实现），让 PR2 有明确锚点
   - 如果轻依赖：PR2 改 20–50 行就完事

### PR1 预计行数
**500–600 行新增 + 0 行改动**

---

## PR2 范围

- 新建 `src/provider/router.rs`：先只做 fallback（A 失败走 B），不做竞速
- 把 `agent/loop_runner.rs` 和 `session.rs` 的调用点切到新 `LLMProvider`
- **一次性删 `src/llm/` 整个目录**（724+204 行）
- 不留 "legacy fallback" 开关，git revert 就是最好的回滚
- 预计 ~600 行净变更

---

## PR3 范围

- `SessionProvider` trait + `WsSession`（reader task + turn router + 心跳）
- `Capabilities` 结构体 + 按能力路由
- 竞速策略（`tokio::select!` 多 provider 抢首 token）
- `Usage` 事件 + budget 模块（成本路由）
- `StreamEvent::Delta` 换 `Bytes` 零拷贝
- `ToolCall` 事件统一
- 预计 ~800 行

---

## 已拍板的决定（不要再讨论）

1. **Claude 走路线 B**，长连接版永久不做
2. **MVP 只做 OpenAI dialect**，GLM/Ollama/DeepSeek 都走它
3. **老 `llm/` 在 PR2 一次性删**，不留并存
4. **`CancellationToken` PR1 就进**
5. **Router 在 PR2 才出现**，PR1 Provider 被直接调用

## 仍待确认的点

- `loop_runner.rs` 的耦合程度——PR1 开工第一件事就是扫这个
- 是否需要保留一个 `--legacy-llm` 临时开关过渡（倾向：不要）

---

## 下次开工第一句话

> "按 PROVIDER_REFACTOR_PLAN.md 的 PR1 范围开始，先扫 llm/ 和 agent/loop_runner.rs 判断耦合程度再动手。"
