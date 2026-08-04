# Kiro-rs 反代服务配置 SOP

> **适用范围**：macOS（含工程上云环境）
> **目的**：将 Kiro 订阅额度通过本地反代提供给 Claude Code / opencode 等 Anthropic API 兼容客户端使用
> **前提条件**：拥有有效的 Kiro 订阅（Pro/Pro+/Power）

---

## 一、概念说明

| 术语 | 说明 |
|------|------|
| **Kiro** | Amazon 开发的 AI 编程工具，提供 Claude 模型访问 |
| **Kiro-rs** | 开源 Anthropic API 兼容代理，将 Kiro 后端转换为标准 Claude API 格式 |
| **反代** | 在本地启动代理服务，将 Claude API 请求转发给 Kiro 后端 |
| **Claude Code** | Anthropic 官方 CLI 工具，支持通过 `ANTHROPIC_BASE_URL` 指向自定义端点 |
| **opencode** | 本 SOP 主要使用的客户端，支持多 provider 配置 |
| **IdC** | IAM Identity Center，企业 SSO 登录方式 |
| **Social** | 社交账号（Google/GitHub 等）登录方式 |

---

## 二、环境准备

### 2.1 安装依赖

```bash
# 1. 安装 Rust（如未安装）
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source $HOME/.cargo/env

# 2. 安装 Node.js + pnpm（用于构建前端）
# macOS 推荐用 Homebrew
brew install node pnpm

# 3. 安装 kiro-cli
curl -fsSL https://cli.kiro.dev/install | bash
```

### 2.2 登录 Kiro

```bash
kiro-cli login
# 按提示完成浏览器授权
```

验证登录状态：
```bash
kiro-cli whoami
# 预期输出：Logged in with IAM Identity Center / Social，以及 Email 和 Profile ARN
```

> **关键**：记录 `Profile:` 后面的 ARN（如 `arn:aws:codewhisperer:us-east-1:xxx:profile/xxx`），后续配置需要。

---

## 三、下载并编译 kiro-rs

### 3.1 克隆仓库

```bash
git clone https://github.com/hank9999/kiro.rs.git
cd kiro.rs
```

### 3.2 构建前端管理界面

```bash
cd admin-ui
pnpm install && pnpm build
cd ..
```

> 工程上云环境：直接用 `npm install && npm run build`

### 3.3 编译 Rust 后端

```bash
cargo build --release
```

编译产物位于 `./target/release/kiro-rs`

---

## 四、配置提取

### 4.1 自动提取脚本

创建 `extract_creds.py`（从 kiro-cli 数据库提取凭据）：

```python
#!/usr/bin/env python3
"""从 kiro-cli 的 SQLite 数据库生成 kiro-rs 配置文件"""

import sqlite3, json, os, secrets

os.umask(0o077)

# macOS 默认路径
DB_PATH = os.path.expanduser("~/Library/Application Support/kiro-cli/data.sqlite3")
# 工程上云路径：/home/docker/.local/share/kiro-cli/data.sqlite3
# Ubuntu 路径：/home/<user>/.local/share/kiro-cli/data.sqlite3

OUTPUT_DIR = os.path.expanduser("~/kiro-rs-config")
os.makedirs(OUTPUT_DIR, exist_ok=True)
os.chmod(OUTPUT_DIR, 0o700)

def write_private_json(filename, value):
    path = os.path.join(OUTPUT_DIR, filename)
    with open(path, "w") as f:
        json.dump(value, f, indent=2)
    os.chmod(path, 0o600)

conn = sqlite3.connect(DB_PATH)
cur = conn.cursor()

auth = {}
cur.execute("SELECT key, value FROM auth_kv")
for key, value in cur.fetchall():
    auth[key] = json.loads(value)
conn.close()

token = auth.get("kirocli:odic:token", {})
device = auth.get("kirocli:odic:device-registration", {})

# --- 生成 config.json ---
config = {
    "host": "127.0.0.1",
    "port": 8990,
    "apiKey": f"sk-kiro-rs-{secrets.token_hex(16)}",
    "region": device.get("region", "us-east-1"),
    "authRegion": device.get("region", "us-east-1"),
    "apiRegion": "us-east-1",        # 根据网络环境调整，公司内网通常用 us-east-1
    "tlsBackend": "native-tls",      # 如遇 TLS 错误可切换为 rustls
    "adminApiKey": f"sk-admin-{secrets.token_hex(16)}",
    "defaultEndpoint": "ide"
}

write_private_json("config.json", config)

# --- 生成 credentials.json ---
cred = {
    "accessToken": token.get("access_token", ""),
    "refreshToken": token.get("refresh_token", ""),
    "profileArn": "",  # 需要手动填入 kiro-cli whoami 显示的 ARN
    "expiresAt": token.get("expires_at", ""),
    "authMethod": "idc" if token.get("provider") else "social",
}

if cred["authMethod"] == "idc":
    cred["clientId"] = device.get("client_id", "")
    cred["clientSecret"] = device.get("client_secret", "")

write_private_json("credentials.json", cred)

print(f"✅ 配置已输出到: {OUTPUT_DIR}")
print("🔑 API Key 已写入 config.json，不在终端显示")
print(f"🌍 Region: {config['region']}")
```

运行脚本：
```bash
python3 extract_creds.py
```

### 4.2 手动补充 profileArn

```bash
kiro-cli whoami
```

输出示例：
```
Logged in with IAM Identity Center (https://d-9667a91ace.awsapps.com/start)
Email: xxx@example.com
Profile:
KiroProfile-us-east-1
arn:aws:codewhisperer:us-east-1:xxx:profile/xxx
```

将最后一行 ARN 填入 `credentials.json` 的 `profileArn` 字段。

> **注意**：如果 profileArn 留空，可能导致 API 请求返回 `400 Bad Request`

---

## 五、配置调优

### 5.1 配置文件详解

**`config.json`**：

```json
{
  "host": "127.0.0.1",
  "port": 8990,
  "apiKey": "sk-kiro-rs-xxx",
  "region": "us-east-1",
  "authRegion": "us-east-1",
  "apiRegion": "us-east-1",
  "tlsBackend": "native-tls",
  "adminApiKey": "sk-admin-请替换为独立强密钥",
  "defaultEndpoint": "ide"
}
```

| 字段 | 说明 | 建议值 |
|------|------|--------|
| `host` | 监听地址 | 默认使用 `127.0.0.1`；仅在配置访问控制后使用 `0.0.0.0` |
| `port` | 监听端口 | `8990` |
| `apiKey` | 自定义 API Key | 随机生成，客户端连接时使用 |
| `region` | 默认区域 | `us-east-1` |
| `authRegion` | Token 刷新区域 | 使用 Kiro 客户端注册设备记录中的 Region |
| `apiRegion` | API 请求区域 | `us-east-1`（公司内网通常只能用此） |
| `tlsBackend` | TLS 实现 | `native-tls`（macOS 推荐）/ `rustls` |
| `adminApiKey` | 管理后台密码 | 使用独立随机强密钥，不要复用示例值 |
| `proxyUrl` | 全局代理 | 如 `http://proxy:7890`（可选） |

> **Region 选择说明**：
> - 大多数公司内网只能解析 `*.us-east-1.amazonaws.com`
> - `authRegion` 应与 Kiro 客户端注册 IdC 设备时使用的 Region 一致
> - 如遇 `error sending request` 且 DNS 无法解析，说明 apiRegion 需改为 `us-east-1`

### 5.2 凭据文件详解

**`credentials.json`**（单凭据格式）：

```json
{
  "accessToken": "",
  "refreshToken": "",
  "profileArn": "arn:aws:codewhisperer:us-east-1:xxx:profile/xxx",
  "expiresAt": "2026-06-01T07:39:03.011578284Z",
  "authMethod": "idc",
  "clientId": "",
  "clientSecret": ""
}
```

| 字段 | 必填 | 说明 |
|------|------|------|
| `refreshToken` | ✅ | 刷新令牌，有效期 7-30 天 |
| `profileArn` | ✅ | AWS Profile ARN，从 `kiro-cli whoami` 获取 |
| `authMethod` | ✅ | `idc`（企业 SSO）或 `social`（社交账号） |
| `clientId` | IdC 必填 | IdC 客户端 ID |
| `clientSecret` | IdC 必填 | IdC 客户端密钥 |

---

## 六、启动服务

### 6.1 手动启动（调试/测试）

```bash
# 编译产物方式
./target/release/kiro-rs -c config.json --credentials credentials.json

# 或直接用 cargo
cargo run --release
```

### 6.2 后台常驻（推荐）

#### macOS - Launchd

创建 `~/Library/LaunchAgents/com.kiro.rs.plist`：

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.kiro.rs</string>
    <key>ProgramArguments</key>
    <array>
        <string>/path/to/kiro-rs/target/release/kiro-rs</string>
        <string>-c</string>
        <string>/path/to/config.json</string>
        <string>--credentials</string>
        <string>/path/to/credentials.json</string>
    </array>
    <key>WorkingDirectory</key>
    <string>/path/to/kiro.rs</string>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <dict>
        <key>SuccessfulExit</key>
        <false/>
        <key>Crashed</key>
        <true/>
    </dict>
</dict>
</plist>
```

加载并启动：
```bash
launchctl load ~/Library/LaunchAgents/com.kiro.rs.plist
launchctl start com.kiro.rs
```

验证状态：
```bash
launchctl list | grep com.kiro.rs
lsof -i :8990
```

#### Linux - systemd（工程上云/服务器）

创建 `/etc/systemd/system/kiro-rs.service`：

```ini
[Unit]
Description=Kiro-rs Anthropic API Proxy
After=network.target

[Service]
Type=simple
User=your-user
WorkingDirectory=/path/to/kiro.rs
ExecStart=/path/to/kiro.rs/target/release/kiro-rs -c /path/to/config.json --credentials /path/to/credentials.json
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
```

启用并启动：
```bash
sudo systemctl daemon-reload
sudo systemctl enable kiro-rs
sudo systemctl start kiro-rs
sudo systemctl status kiro-rs
```

---

## 七、客户端配置

### 7.1 opencode 配置

编辑 `~/.config/opencode/opencode.jsonc`，添加 `Kiro-Local` provider：

```json
"Kiro-Local": {
  "npm": "@ai-sdk/anthropic",
  "name": "Kiro-Local",
    "options": {
      "baseURL": "http://127.0.0.1:8990/v1",
      "apiKey": "sk-kiro-rs-xxx"
    },
  "models": {
    "claude-sonnet-4.5": {
      "id": "claude-sonnet-4-5-20250929",
      "name": "Claude Sonnet 4.5",
      "reasoning": true,
      "tool_call": true,
      "temperature": true,
      "limit": { "context": 200000, "output": 128000 },
      "cost": { "input": 3, "cache_read": 0.3, "output": 15 },
      "variants": {
        "low": { "thinking": { "type": "enabled", "budgetTokens": 8000 } },
        "medium": { "thinking": { "type": "enabled", "budgetTokens": 16000 } },
        "high": { "thinking": { "type": "enabled", "budgetTokens": 24000 } },
        "max": { "thinking": { "type": "enabled", "budgetTokens": 24576 } }
      }
    },
    "claude-sonnet-4.6": {
      "id": "claude-sonnet-4-6",
      "name": "Claude Sonnet 4.6",
      "reasoning": true,
      "tool_call": true,
      "temperature": true,
      "limit": { "context": 1000000, "output": 128000 },
      "cost": { "input": 3, "cache_read": 0.3, "output": 15 },
      "variants": {
        "low": { "thinking": { "type": "adaptive" }, "effort": "low" },
        "medium": { "thinking": { "type": "adaptive" }, "effort": "medium" },
        "high": { "thinking": { "type": "adaptive" }, "effort": "high" },
        "max": { "thinking": { "type": "adaptive" }, "effort": "high" }
      }
    },
    "claude-sonnet-5": {
      "id": "claude-sonnet-5",
      "name": "Claude Sonnet 5",
      "reasoning": true,
      "tool_call": true,
      "temperature": true,
      "limit": { "context": 1000000, "output": 128000 },
      "cost": { "input": 3, "cache_read": 0.3, "output": 15 },
      "variants": {
        "low": { "thinking": { "type": "adaptive" }, "effort": "low" },
        "medium": { "thinking": { "type": "adaptive" }, "effort": "medium" },
        "high": { "thinking": { "type": "adaptive" }, "effort": "high" },
        "xhigh": { "thinking": { "type": "adaptive" }, "effort": "xhigh" },
        "max": { "thinking": { "type": "adaptive" }, "effort": "max" }
      }
    },
    "claude-opus-4.5": {
      "id": "claude-opus-4-5-20251101",
      "name": "Claude Opus 4.5",
      "reasoning": true,
      "tool_call": true,
      "temperature": true,
      "limit": { "context": 200000, "output": 128000 },
      "cost": { "input": 15, "cache_read": 1.5, "output": 75 },
      "variants": {
        "low": { "thinking": { "type": "enabled", "budgetTokens": 8000 } },
        "medium": { "thinking": { "type": "enabled", "budgetTokens": 16000 } },
        "high": { "thinking": { "type": "enabled", "budgetTokens": 24000 } },
        "max": { "thinking": { "type": "enabled", "budgetTokens": 24576 } }
      }
    },
    "claude-opus-4.6": {
      "id": "claude-opus-4-6",
      "name": "Claude Opus 4.6",
      "reasoning": true,
      "tool_call": true,
      "temperature": true,
      "limit": { "context": 1000000, "output": 128000 },
      "cost": { "input": 15, "cache_read": 1.5, "output": 75 },
      "variants": {
        "low": { "thinking": { "type": "adaptive" }, "effort": "low" },
        "medium": { "thinking": { "type": "adaptive" }, "effort": "medium" },
        "high": { "thinking": { "type": "adaptive" }, "effort": "high" },
        "max": { "thinking": { "type": "adaptive" }, "effort": "max" }
      }
    },
    "claude-opus-4.7": {
      "id": "claude-opus-4-7",
      "name": "Claude Opus 4.7",
      "reasoning": true,
      "tool_call": true,
      "temperature": true,
      "limit": { "context": 1000000, "output": 128000 },
      "cost": { "input": 15, "cache_read": 1.5, "output": 75 },
      "variants": {
        "low": { "thinking": { "type": "adaptive" }, "effort": "low" },
        "medium": { "thinking": { "type": "adaptive" }, "effort": "medium" },
        "high": { "thinking": { "type": "adaptive" }, "effort": "high" },
        "max": { "thinking": { "type": "adaptive" }, "effort": "max" }
      }
    },
    "claude-opus-4.8": {
      "id": "claude-opus-4-8",
      "name": "Claude Opus 4.8",
      "reasoning": true,
      "tool_call": true,
      "temperature": true,
      "limit": { "context": 1000000, "output": 128000 },
      "cost": { "input": 15, "cache_read": 1.5, "output": 75 },
      "variants": {
        "low": { "thinking": { "type": "adaptive" }, "effort": "low" },
        "medium": { "thinking": { "type": "adaptive" }, "effort": "medium" },
        "high": { "thinking": { "type": "adaptive" }, "effort": "high" },
        "xhigh": { "thinking": { "type": "adaptive" }, "effort": "xhigh" },
        "max": { "thinking": { "type": "adaptive" }, "effort": "max" }
      }
    }
  }
}
```

> **关键配置点**：
> - `baseURL` 配置为 `http://127.0.0.1:8990/v1`（标准端点，实时流式 SSE，首字延迟更低）
> - `apiKey` 与 `config.json` 中的 `apiKey` 保持一致
> - 模型 `id` 必须与实际返回的 ID 匹配（可通过 `curl http://127.0.0.1:8990/v1/models` 查看）
> - `/v1` 与 `/cc/v1` 均实时流式返回，`message_start` 中的 `input_tokens` 为估算值
> - 每个模型显式声明 `"reasoning": true`、`"tool_call": true`、`"temperature": true`，否则 opencode 不会把模型识别为支持 reasoning，variants 不会生效
> - 每个模型配置完整 `variants` 档位：`low` / `medium` / `high` / `max`，Opus 4.8 和 Sonnet 5 额外支持 `xhigh`
> - opencode 默认使用 `high` 档位，可按 `Ctrl+T` 循环切换
> - 4.5 系列用 `enabled` + `budgetTokens`；4.6/4.7/4.8/Sonnet 5 系列用 `adaptive` + `effort`
> - `budgetTokens` 上限为 24576，且必须小于该次请求的 `max_tokens`

### 7.2 Claude Code 配置

编辑 `~/.claude/settings.json`：

```json
{
  "env": {
    "ANTHROPIC_BASE_URL": "http://localhost:8990/cc",
    "ANTHROPIC_AUTH_TOKEN": "sk-kiro-rs-xxx"
  }
}
```

> 注意：Claude Code 使用 `/cc` 端点（非 `/cc/v1`），因为 Claude Code 会自行追加 `/v1`

---

## 八、验证测试

### 8.1 服务健康检查

```bash
# 查看模型列表
curl http://127.0.0.1:8990/v1/models \
  -H "x-api-key: sk-kiro-rs-xxx"

# 测试对话（非流式）
curl http://127.0.0.1:8990/v1/messages \
  -H "Content-Type: application/json" \
  -H "x-api-key: sk-kiro-rs-xxx" \
  -d '{
    "model": "claude-sonnet-4-5-20250929",
    "max_tokens": 1024,
    "stream": false,
    "messages": [{"role": "user", "content": "Hello"}]
  }'

# 测试对话（流式）
curl http://127.0.0.1:8990/v1/messages \
  -H "Content-Type: application/json" \
  -H "x-api-key: sk-kiro-rs-xxx" \
  -d '{
    "model": "claude-sonnet-4-5-20250929",
    "max_tokens": 1024,
    "stream": true,
    "messages": [{"role": "user", "content": "Hello"}]
  }'
```

### 8.2 opencode 验证

```bash
# 查看模型列表
opencode models Kiro-Local

# 测试对话
opencode run -m Kiro-Local/claude-sonnet-4.5 "Hello, who are you?"
```

---

## 九、常见问题排查

### 9.1 error sending request / DNS 解析失败

**现象**：`error sending request for url (https://q.ap-southeast-1.amazonaws.com/...)`
**原因**：公司内网 DNS 无法解析 `ap-southeast-1` 的 AWS 域名

**解决**：
```json
// config.json
{
  "region": "us-east-1",
  "authRegion": "us-east-1",
  "apiRegion": "us-east-1"
}
```

验证 DNS：
```bash
dig q.us-east-1.amazonaws.com +short    # 应返回 IP
dig q.ap-southeast-1.amazonaws.com +short  # 可能为空
```

### 9.2 400 Bad Request

**现象**：`{"message":"Improperly formed request.","reason":null}`
**原因**：`profileArn` 为空或格式错误

**解决**：
1. 运行 `kiro-cli whoami` 获取完整 ARN
2. 填入 `credentials.json` 的 `profileArn` 字段

### 9.3 Not Found（opencode 连接时）

**现象**：opencode 返回 `Error: Not Found`
**原因**：`baseURL` 配置错误，路径不匹配

**解决**：
```json
// 正确
"baseURL": "http://127.0.0.1:8990/v1"

// 错误示例
"baseURL": "http://127.0.0.1:8990"      // 缺少 /v1
"baseURL": "http://127.0.0.1:8990/cc"   // 错误路径，Claude Code 才用 /cc
```

### 9.4 TLS / 证书错误

**现象**：请求失败，提示 TLS 相关错误
**解决**：切换 `tlsBackend`：

```json
// 默认 rustls，如遇证书问题改为 native-tls
"tlsBackend": "native-tls"
```

### 9.5 服务端口冲突

**现象**：`Address already in use (os error 48)`
**解决**：
```bash
# 查找占用端口的进程
lsof -i :8990
# 或
killall kiro-rs
```

### 9.6 Token 过期

**现象**：API 调用突然失败，提示认证错误
**原因**：`accessToken` 过期（通常 1 小时）

**解决**：kiro-rs 会自动刷新 Token，无需手动干预。如刷新失败：
1. 检查 `refreshToken` 是否有效
2. 检查网络是否可访问 `authRegion` 的 IdC 服务
3. 按 6.1 节以前台方式运行 `RUST_LOG=debug ./target/release/kiro-rs -c config.json --credentials credentials.json`，在当前终端查看刷新日志

---

## 十、管理后台

服务启动后，访问管理界面：

```
http://127.0.0.1:8990/admin
```

使用 `config.json` 中配置的 `adminApiKey` 进行认证。

管理后台功能：
- 查看凭据状态（Token 有效期、余额、失败次数）
- 添加/删除凭据
- 调整凭据优先级
- 查看实时日志

---

## 十一、附录

### 11.1 模型倍率参考

| 模型 | 倍率 | 上下文 | 特点 |
|------|------|--------|------|
| Claude Opus 4.8 | 2.2x | 1M | 最强推理，最贵 |
| Claude Opus 4.7 | 2.2x | 1M | 强推理 |
| Claude Opus 4.6 | 2.2x | 1M | 强推理 |
| Claude Opus 4.5 | 2.2x | 200K | 平衡型 |
| Claude Sonnet 4.6 | 1.3x | 1M | 快速响应 |
| Claude Sonnet 4.5 | 1.3x | 200K | 性价比之选 |
| Claude Haiku 4.5 | 0.25x | 200K | 最快最便宜 |

> **使用经验**：Kiro 订阅比 Claude 订阅更耐用。3 天 Opus 4.8 消耗约 4000 credits（等效约 $2000 的 API 费用）。

### 11.2 相关资源

- **Kiro-rs GitHub**: https://github.com/hank9999/kiro.rs
- **Kiro 订阅管理**: https://app.kiro.dev/settings/account
- **Claude Code 文档**: https://docs.anthropic.com/en/docs/claude-code
- **opencode 文档**: https://opencode.ai

### 11.3 免责声明

> 本 SOP 仅供技术研究使用。使用本项目导致的任何后果由使用人自行承担，与项目作者无关。本项目与 AWS/KIRO/Anthropic/Claude 等官方无关，不代表官方立场。
