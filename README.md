# StandX Maker Bot (Rust)

高性能 Rust 做市机器人，用于在 StandX 永续合约 DEX 上获取 Maker Points 和 Uptime 奖励。

## 功能特性

- 🚀 **高性能** - Rust 实现，低延迟
- 📊 **双边挂单** - 在 mark price 上下自动挂单
- 🔄 **自动调整** - 价格波动时自动撤单重挂
- 📈 **波动率控制** - 高波动时暂停挂单
- 🛡️ **风险控制** - 仓位上限、止盈止损
- 🔌 **WebSocket** - 实时价格和订单更新
- 🐳 **Docker 支持** - 一键部署

## 快速开始

### 本地运行

```bash
# 1. 安装 Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# 2. 克隆并进入项目
cd standX-mm

# 3. 配置
cp config.example.yaml config.yaml
# 编辑 config.yaml，填入私钥

# 4. 运行
cargo run --release

# 或使用环境变量（更安全）
STANDX_PRIVATE_KEY=0x... cargo run --release
```

### Docker 部署

```bash
# 1. 配置
cp config.example.yaml config.yaml
# 编辑 config.yaml

# 2. 构建并运行
docker-compose up -d

# 3. 查看日志
docker-compose logs -f

# 4. 停止
docker-compose down
```

## 配置说明

```yaml
wallet:
  chain: bsc                    # 链类型：bsc
  private_key: ""               # 私钥（推荐用环境变量）

symbol: BTC-USD                 # 交易对

# 挂单参数
order_distance_bps: 10          # 挂单距离 (10 bps = 0.1%)
cancel_distance_bps: 5          # 太近撤单阈值
rebalance_distance_bps: 20      # 太远撤单阈值
order_size_btc: 0.01            # 单笔挂单大小

# 仓位控制
max_position_btc: 0.1           # 最大持仓

# 波动率控制
volatility_window_sec: 5        # 观察窗口
volatility_threshold_bps: 5     # 波动率阈值

# 止盈止损（可选）
take_profit_pct: 5.0            # 盈利 5% 平仓
stop_loss_pct: 5.0              # 亏损 5% 平仓

min_balance_usd: 100            # 最低余额要求
```

## 策略逻辑

1. **挂单策略**
   - 在 last_price ± order_distance_bps 处双边挂单
   - 订单停留 3 秒以上即可获得 Maker Points

2. **订单管理**
   - 价格靠近 < cancel_distance_bps → 撤单（避免成交）
   - 价格远离 > rebalance_distance_bps → 撤单重挂

3. **风险控制**
   - 仓位 >= max_position → 暂停做市
   - 仓位 > 70% 且盈利 → 减仓到 50%
   - 止盈/止损触发 → 市价平仓

4. **波动率控制**
   - 窗口内波动 > threshold → 暂停挂单

## 环境变量

| 变量 | 说明 |
|------|------|
| `STANDX_PRIVATE_KEY` | 钱包私钥（优先级高于配置文件） |
| `RUST_LOG` | 日志级别，默认 `info,standx_mm=debug` |

## 日志控制

使用 `RUST_LOG` 环境变量控制日志输出级别。

### 日志级别

从低到高：`trace` < `debug` < `info` < `warn` < `error`

### 默认配置

```bash
# 默认：全局 INFO，本项目 DEBUG
cargo run
# 等同于 RUST_LOG=info,standx_mm=debug cargo run
```

### 常用配置

```bash
# 只显示 INFO 及以上（减少输出）
RUST_LOG=info cargo run

# 显示所有 DEBUG（包括第三方库）
RUST_LOG=debug cargo run

# 只显示警告和错误
RUST_LOG=warn cargo run

# 只显示本项目的 DEBUG，其他 WARN
RUST_LOG=warn,standx_mm=debug cargo run

# 只显示特定模块的 DEBUG
RUST_LOG=info,standx_mm::ws_client=debug cargo run
RUST_LOG=info,standx_mm::maker=debug cargo run
RUST_LOG=info,standx_mm::http_client=debug cargo run
```

### 日志输出示例

```
# INFO 级别 - 关键操作
2026-01-14T03:00:24.550093Z  INFO standx_mm::maker: Placing sell order: 0.01 @ 95415.31
2026-01-14T03:00:24.549987Z  INFO standx_mm::ws_client: [Price] BTC-USD last=95319.99 mark=95326.32 latency=188ms
2026-01-14T03:00:24.690917Z  INFO standx_mm::http_client: [Latency] /api/new_order responded in 139ms

# DEBUG 级别 - 详细信息
2026-01-14T03:00:24.550093Z DEBUG standx_mm::maker: Tick: last_price=95319.99
2026-01-14T03:00:24.551014Z DEBUG standx_mm::http_client: POST /api/new_order (attempt 1): {...}
```

## 项目结构

```
src/
├── main.rs          # 入口，初始化和启动
├── config.rs        # 配置加载
├── auth.rs          # Ed25519 签名和钱包认证
├── http_client.rs   # HTTP API 客户端
├── ws_client.rs     # WebSocket 客户端
├── state.rs         # 状态管理
├── calculator.rs    # 价格计算
└── maker.rs         # 做市策略
```

## 测试

```bash
cargo test
```

## License

MIT
