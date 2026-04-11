<div align="center">

# GoWork

**Lightweight AI Terminal Assistant — Fast, Token-Efficient, Privacy-First**

[English](#english) | [中文](#中文) | [日本語](#日本語)

[![License: AGPL-3.0](https://img.shields.io/badge/License-AGPL%20v3-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Built%20with-Rust-orange.svg)](https://www.rust-lang.org/)

```
╔══════════════════════════════════════╗
║           GoWork v1.0.0              ║
╚══════════════════════════════════════╝
  Model: Qwen3.5-4B (MLX, localhost:8080)
  Tools: read_file, edit_file, bash, grep, glob, web_search...

>> summarize src/main.rs
```

</div>

---

<a id="english"></a>

## English

### What is GoWork?

A high-performance AI coding assistant CLI built in Rust. Works with any OpenAI-compatible LLM — local models (MLX/Ollama) or cloud APIs (OpenAI, DeepSeek, Claude).

**Not just a chat wrapper** — GoWork is a full agent with tool calling, file operations, web search, and batch processing.

### Features

- **30ms startup** — Single Rust binary, zero runtime dependencies
- **Save 90-99% Claude tokens** — Offload preprocessing to local models, Claude only reads summaries
- **Full Agent Loop** — LLM reasoning -> tool execution -> result feedback -> continue
- **9 Built-in Tools** — read_file, edit_file, bash, grep, glob, todo, web_search, web_fetch
- **MLX Native** — Optimized for Apple Silicon, 20-50% faster than Ollama
- **`--no-tools` Mode** — Pure chat, 35% faster for preprocessing tasks
- **`--batch` Mode** — Process multiple files in one command
- **`--cache` Mode** — Skip re-processing unchanged files
- **`--stats`** — Track token savings in real-time
- **Privacy-First** — Code stays on your machine by default

### Quick Start

```bash
# Build
git clone https://github.com/yourname/gowork.git
cd gowork
cargo build --release
cp target/release/gowork ~/.cargo/bin/

# Start MLX backend (Apple Silicon)
pip install mlx-lm
mlx_lm.server --model mlx-community/Qwen3.5-4B-OptiQ-4bit --port 8080

# Use
gowork                                    # Interactive mode
gowork -p "explain this code"             # One-shot
gowork --no-tools --file main.rs -p "list all functions"  # Preprocessing
```

### Usage Examples

```bash
# Quick Q&A
gowork -p "what does async move do in Rust?"

# Summarize a large file (saves Claude tokens)
gowork --no-tools --file large_report.md -p "summarize in 3 bullet points"

# Pipe stdin
grep "ERROR" app.log | gowork --no-tools -p "group by error type"

# Batch process
gowork --no-tools --batch "src/*.go" -p "one-line summary: {}"

# With caching (instant on repeat)
gowork --no-tools --cache --file data.csv -p "list column names"

# Check token savings
gowork --stats
```

### Configuration

```toml
# ~/.gowork/config.toml
base_url = "http://localhost:8080/v1"
model = "mlx-community/Qwen3.5-4B-OptiQ-4bit"
api_key = ""                    # Optional, for cloud APIs
searxng_url = "http://localhost:8888"  # Optional, for web search
```

Environment variables: `GOWORK_BASE_URL`, `GOWORK_MODEL`, `GOWORK_API_KEY`

CLI flags override env vars, which override config file.

### Supported Models

| Backend | Models | Notes |
|---------|--------|-------|
| **MLX** (recommended) | Qwen3.5-4B, Gemma4, Phi-4-mini | Apple Silicon, fastest |
| **Ollama** | Qwen, Llama, Mistral, DeepSeek | Cross-platform |
| **OpenAI** | GPT-4o, GPT-4o-mini | Cloud, needs API key |
| **DeepSeek** | DeepSeek-V3, DeepSeek-Coder | Cloud, affordable |
| **Any OpenAI-compatible** | vLLM, LM Studio, Groq | Self-hosted or cloud |

### CLI Reference

| Flag | Description |
|------|-------------|
| `-p "prompt"` | One-shot mode |
| `--no-tools` | Pure chat, no tool definitions (35% faster) |
| `--file path` | Read file into prompt |
| `--batch "glob"` | Process multiple files (`{}` = content placeholder) |
| `--cache` | Cache results by file+mtime+prompt |
| `--stats` | Show token savings statistics |
| `--stats-reset` | Reset statistics |
| `-m model` | Override model |
| `--base-url url` | Override API endpoint |
| `--api-key key` | Set API key |

### Architecture

```
src/
├── main.rs              # Entry: arg parsing, mode dispatch, batch, cache
├── cache.rs             # SHA256-based result caching
├── stats.rs             # Token savings tracking
├── config.rs            # TOML config, env resolution
├── cli/shell.rs         # Interactive REPL, slash commands
├── agent/loop_runner.rs # Agent loop: LLM <-> Tool orchestration
├── llm/
│   ├── types.rs         # OpenAI-compatible message/tool types
│   └── client.rs        # HTTP streaming + SSE parsing
└── tools/               # 9 tools: read, edit, bash, grep, glob, todo, web_fetch, web_search
```

### Extend: Add Custom Tools

```rust
#[async_trait]
impl Tool for MyTool {
    fn name(&self) -> &str { "my_tool" }
    fn definition(&self) -> ToolDefinition { /* JSON schema */ }
    async fn execute(&self, args: Value) -> Result<ToolResult> {
        Ok(ToolResult { success: true, output: "done".into() })
    }
}
// Register in main.rs:
registry.register(Box::new(MyTool));
```

---

<a id="中文"></a>

## 中文

### GoWork 是什么？

用 Rust 写的高性能 AI 终端助手。支持任意 OpenAI 兼容的 LLM -- 本地模型（MLX/Ollama）或云端 API（OpenAI、DeepSeek、Claude）。

**不只是聊天工具** -- GoWork 是完整的 Agent，支持工具调用、文件操作、网络搜索和批量处理。

### 功能亮点

- **30ms 启动** -- 单个 Rust 二进制，零运行时依赖
- **省 90-99% Claude Token** -- 预处理交给本地模型，Claude 只读摘要
- **完整 Agent Loop** -- LLM 推理 -> 工具执行 -> 结果回传 -> 继续
- **9 个内置工具** -- read_file, edit_file, bash, grep, glob, todo, web_search, web_fetch
- **MLX 原生支持** -- Apple Silicon 优化，比 Ollama 快 20-50%
- **`--no-tools` 模式** -- 纯对话，预处理快 35%
- **`--batch` 批量处理** -- 一条命令处理多个文件
- **`--cache` 缓存** -- 文件没改就不重复处理
- **`--stats` 统计** -- 实时查看省了多少 token
- **隐私优先** -- 默认本地运行，代码不出机器

### 快速开始

```bash
# 构建
git clone https://github.com/yourname/gowork.git
cd gowork && cargo build --release
cp target/release/gowork ~/.cargo/bin/

# 启动 MLX 后端（Apple Silicon）
pip install mlx-lm
mlx_lm.server --model mlx-community/Qwen3.5-4B-OptiQ-4bit --port 8080

# 使用
gowork                                    # 交互模式
gowork -p "解释这段代码"                    # 单次模式
gowork --no-tools --file main.rs -p "列出所有函数"  # 预处理
```

### 使用示例

```bash
# 简单问答
gowork -p "Rust 的 async move 是什么意思？"

# 长文本总结（省 Claude token）
gowork --no-tools --file large_report.md -p "3 个要点总结"

# 管道输入
grep "ERROR" app.log | gowork --no-tools -p "按错误类型分组"

# 批量处理
gowork --no-tools --batch "src/*.go" -p "一句话总结: {}"

# 带缓存（重复查询瞬间返回）
gowork --no-tools --cache --file data.csv -p "列出列名"

# 查看省了多少 token
gowork --stats

# 切换模型 / 清空记忆（交互模式下）
/model qwen3.5:4b
/clear
```

### 配置说明

```toml
# ~/.gowork/config.toml
base_url = "http://localhost:8080/v1"    # API 地址
model = "mlx-community/Qwen3.5-4B-OptiQ-4bit"  # 默认模型
api_key = ""                              # 可选，云端 API 需要
searxng_url = "http://localhost:8888"     # 可选，网络搜索
```

优先级：命令行参数 > 环境变量 > 配置文件 > 默认值

### 支持模型

| 后端 | 模型 | 说明 |
|------|------|------|
| **MLX**（推荐） | Qwen3.5-4B, Gemma4, Phi-4-mini | Apple Silicon，最快 |
| **Ollama** | Qwen, Llama, Mistral, DeepSeek | 跨平台 |
| **OpenAI** | GPT-4o, GPT-4o-mini | 云端，需 API key |
| **DeepSeek** | DeepSeek-V3, DeepSeek-Coder | 云端，便宜 |
| **任意 OpenAI 兼容** | vLLM, LM Studio, Groq | 自建或云端 |

---

<a id="日本語"></a>

## 日本語

### GoWork とは？

Rust で構築された高性能 AI ターミナルアシスタント。OpenAI 互換の任意の LLM に対応 -- ローカルモデル（MLX/Ollama）またはクラウド API（OpenAI、DeepSeek、Claude）。

**単なるチャットではありません** -- GoWork はツール呼び出し、ファイル操作、ウェブ検索、バッチ処理を備えた完全な Agent です。

### 特徴

- **30ms 起動** -- 単一の Rust バイナリ、ランタイム依存なし
- **Claude トークン 90-99% 節約** -- 前処理をローカルモデルに委任、Claude は要約のみ読み込み
- **完全な Agent Loop** -- LLM 推論 -> ツール実行 -> 結果フィードバック -> 継続
- **9つの組み込みツール** -- read_file, edit_file, bash, grep, glob, todo, web_search, web_fetch
- **MLX ネイティブ** -- Apple Silicon 最適化、Ollama より 20-50% 高速
- **`--no-tools` モード** -- 純粋なチャット、前処理が 35% 高速
- **`--batch` モード** -- 1コマンドで複数ファイルを処理
- **`--cache` モード** -- 未変更ファイルの再処理をスキップ
- **`--stats`** -- トークン節約量をリアルタイム表示
- **プライバシー優先** -- デフォルトでローカル実行、コードは外部に出ません

### クイックスタート

```bash
# ビルド
git clone https://github.com/yourname/gowork.git
cd gowork && cargo build --release
cp target/release/gowork ~/.cargo/bin/

# MLX バックエンド起動（Apple Silicon）
pip install mlx-lm
mlx_lm.server --model mlx-community/Qwen3.5-4B-OptiQ-4bit --port 8080

# 使用
gowork                                    # インタラクティブモード
gowork -p "このコードを説明して"              # ワンショット
gowork --no-tools --file main.rs -p "全関数をリスト"  # 前処理
```

### 使用例

```bash
# 簡単な質問
gowork -p "Rust の async move とは？"

# 大きなファイルの要約（Claude トークン節約）
gowork --no-tools --file large_report.md -p "3つのポイントで要約"

# パイプ入力
grep "ERROR" app.log | gowork --no-tools -p "エラータイプ別にグループ化"

# バッチ処理
gowork --no-tools --batch "src/*.go" -p "一行要約: {}"

# キャッシュ付き（繰り返しクエリは即座に返答）
gowork --no-tools --cache --file data.csv -p "列名をリスト"

# トークン節約量を確認
gowork --stats
```

### 設定

```toml
# ~/.gowork/config.toml
base_url = "http://localhost:8080/v1"
model = "mlx-community/Qwen3.5-4B-OptiQ-4bit"
api_key = ""
searxng_url = "http://localhost:8888"
```

優先順位：CLI フラグ > 環境変数 > 設定ファイル > デフォルト値

### 対応モデル

| バックエンド | モデル | 備考 |
|-------------|--------|------|
| **MLX**（推奨） | Qwen3.5-4B, Gemma4, Phi-4-mini | Apple Silicon、最速 |
| **Ollama** | Qwen, Llama, Mistral, DeepSeek | クロスプラットフォーム |
| **OpenAI** | GPT-4o, GPT-4o-mini | クラウド、API キー必要 |
| **DeepSeek** | DeepSeek-V3, DeepSeek-Coder | クラウド、低コスト |
| **任意の OpenAI 互換** | vLLM, LM Studio, Groq | セルフホストまたはクラウド |

---

## Performance

| Metric | GoWork | aider (Python) |
|--------|--------|----------------|
| Startup | ~30ms | 1-3s |
| Memory (idle) | ~10MB | ~80MB |
| Binary size | ~8MB | N/A |
| Dependencies | 0 | Python + dozens |

## Token Savings (Tested)

| Task | Claude Direct | GoWork + Claude | Savings |
|------|--------------|-----------------|---------|
| Summarize 500-line file | ~7,000 tokens | ~500 tokens | **93%** |
| Extract functions (1,957 lines) | 18,550 tokens (truncated!) | ~80 tokens | **99%** |
| Scan 10 files | ~10,000 tokens | ~1,000 tokens | **90%** |

## License

**AGPL-3.0** -- See [LICENSE](LICENSE)

### Why AGPL?

We chose AGPL-3.0 to protect this project from being cloned, rebranded, and sold as a closed-source product. Open source should benefit everyone, not just those who take without giving back.

**What this means:**
- Personal use, learning, open-source projects: **free, no restrictions**
- Modify and redistribute: **must open-source your changes under AGPL**
- Use in commercial SaaS/products: **must open-source your full service**, OR purchase a commercial license

This ensures the community benefits from all improvements, while preventing companies from simply copying the code and profiting without contributing.

**Commercial License:** If you need a proprietary license for enterprise use, contact: go7thxin@gmail.com

### 为什么选择 AGPL？

选择 AGPL-3.0 是为了防止项目被直接抄袭、换皮、闭源售卖。开源应该让所有人受益，而不是被白嫖。

- 个人使用、学习、开源项目：**免费，无限制**
- 修改后分发：**必须同样开源**
- 商业 SaaS/产品中使用：**必须开源你的服务**，或购买商用授权

**商用授权咨询：** go7thxin@gmail.com

### なぜ AGPL？

AGPL-3.0 を選択した理由は、プロジェクトがそのままコピーされ、リブランドされ、クローズドソース製品として販売されることを防ぐためです。

- 個人利用、学習、オープンソース：**無料、制限なし**
- 変更して再配布：**同じく AGPL でオープンソース化が必要**
- 商用 SaaS/製品：**サービス全体のオープンソース化が必要**、または商用ライセンスを購入

**商用ライセンスのお問い合わせ：** go7thxin@gmail.com

---

<div align="center">

**Built with Rust. Powered by local AI. Saves your tokens.**

[Report Bug](https://github.com/yourname/gowork/issues) | [Request Feature](https://github.com/yourname/gowork/issues)

</div>
