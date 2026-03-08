# Rust + tracing + OTLP + SigNoz 多机日志追踪 DEMO

这个 DEMO 提供 3 个 Rust 应用：
- `gateway`：入口服务
- `inventory`：库存服务
- `payment`：支付服务

3 个服务会通过 W3C Trace Context 透传 trace，统一上报到 SigNoz（OTLP gRPC）。

## 1. 架构

- 观测节点（Machine A）：运行 SigNoz
- 业务节点（Machine B/C/...）：运行 `gateway/inventory/payment`
- 用户请求只打到 `gateway`，但在 SigNoz 里可单击看到跨应用链路

示意链路：
- `gateway /checkout/{item}` -> `inventory /reserve/{item}`
- `gateway /checkout/{item}` -> `payment /pay/{item}`

## 2. 一键启动

### 2.1 Machine A 启动 SigNoz

```bash
cd signoz-demo
chmod +x scripts/start-observability-node.sh
./scripts/start-observability-node.sh
```

启动后：
- SigNoz UI: `http://<machine-a-ip>:3301`
- OTLP gRPC: `http://<machine-a-ip>:4317`

### 2.2 Machine B/C 启动应用服务

在工作区根目录执行：

```bash
chmod +x signoz-demo/scripts/start-app-node.sh
```

启动 inventory:

```bash
signoz-demo/scripts/start-app-node.sh inventory http://<machine-a-ip>:4317 0.0.0.0:7002 node-inventory
```

启动 payment:

```bash
signoz-demo/scripts/start-app-node.sh payment http://<machine-a-ip>:4317 0.0.0.0:7003 node-payment
```

启动 gateway（并指向远端 inventory/payment）:

```bash
INVENTORY_URL=http://<inventory-ip>:7002 PAYMENT_URL=http://<payment-ip>:7003 \
signoz-demo/scripts/start-app-node.sh gateway http://<machine-a-ip>:4317 0.0.0.0:7001 node-gateway
```

### 2.3 触发调用

```bash
curl http://<gateway-ip>:7001/checkout/book
curl http://<gateway-ip>:7001/checkout/fail
```

然后在 SigNoz 中查看：
- `Services` 页面中的 `gateway-service`, `inventory-service`, `payment-service`
- 单个 Trace 里能看到 3 个服务 span 串联
- 每个 span 内包含 `tracing` 事件日志

## 3. 本机一键演示

如果你只想快速本机跑通：

```bash
cd signoz-demo
chmod +x scripts/start-local-all.sh scripts/stop-local-all.sh
./scripts/start-local-all.sh http://127.0.0.1:4317
curl http://127.0.0.1:7001/checkout/book
./scripts/stop-local-all.sh
```

## 4. 环境变量

- `OTEL_EXPORTER_OTLP_ENDPOINT`：OTLP gRPC 地址，默认 `http://127.0.0.1:4317`
- `LISTEN_ADDR`：服务监听地址
- `MACHINE_ID`：机器标识，会作为资源属性上报
- `INVENTORY_URL` / `PAYMENT_URL`：仅 `gateway` 需要
- `RUST_LOG`：日志级别，默认 `info`

## 5. 运行二进制

你也可以手动启动：

```bash
cargo run -p signoz-demo --bin inventory
cargo run -p signoz-demo --bin payment
cargo run -p signoz-demo --bin gateway
```
