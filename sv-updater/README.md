# SV Updater

## 概述

SV Updater 是一个面向开发和测试环境的轻量更新器，解决以下重复动作：

改代码 -> 编译二进制 -> 上传机器 -> 替换文件 -> 执行重启/校验脚本 -> 检查结果

当前实现已经覆盖 README 里的核心需求：

1. 服务端提供前端控制页
2. 可配置多个构建目标，每个目标对应独立构建脚本、产物路径、目标落盘路径、前后置钩子
3. 页面上可一次选择多个构建目标和多个在线机器
4. 客户端启动后会自动连接服务端并注册自身信息
5. 服务端构建完成后会通过双向 gRPC 直接把二进制推送给客户端
6. 客户端执行备份、替换、钩子脚本，并回传安装结果与 MD5

## 架构

程序是一个单独的二进制，通过子命令区分角色：

1. server 模式
    负责 HTTP 控制台、部署 API、构建执行、维护在线客户端连接、下发更新任务
2. client 模式
    负责注册、心跳、接收部署命令、落盘替换、执行钩子、回传结果

通信模型使用双向 gRPC 流：

1. client 建立长连接并发送 hello + heartbeat
2. server 通过同一条连接把 DeployCommand 发给 client
3. client 执行完后把 DeployResult 发回 server

## 已实现功能

### 服务端

1. 配置驱动的多目标构建
2. 前端页面选择多个 target 和多个 client
3. 对每个 target 只构建一次，然后复用构建产物分发给多台机器
4. 按客户端标签过滤目标，例如 linux/prod/macos
5. 维护部署状态面板，展示每个 机器 x 目标 的执行结果

### 客户端

1. 自动注册 client_id、hostname、platform、labels、root_dir
2. 接收二进制字节流，先写到 staging 目录
3. 目标文件存在时自动备份
4. 支持前置和后置钩子脚本
5. 部署后计算 MD5 并回传服务端

## 配置

### 服务端配置

可参考 [examples/server.toml](examples/server.toml)。

```toml
http_listen = "0.0.0.0:8088"
grpc_listen = "0.0.0.0:50061"
workspace_dir = "."

[[build_targets]]
name = "remote-shell-linux"
build_script = "cargo build --release -p remote-shell --target x86_64-unknown-linux-musl"
artifact_path = "target/x86_64-unknown-linux-musl/release/remote-shell"
destination_path = "/opt/remote-shell/bin/remote-shell"
required_labels = ["linux", "prod"]
pre_hooks = ["echo preparing $SV_TARGET_NAME"]
post_hooks = ["systemctl restart remote-shell"]
executable = true
backup_suffix = "bak"
```

字段说明：

1. `build_script`: 服务端执行的构建脚本
2. `artifact_path`: 构建完成后读取的产物路径，基于 `workspace_dir`
3. `destination_path`: 客户端最终替换的目标路径
4. `required_labels`: 只有标签全部匹配的客户端才会接收该目标
5. `pre_hooks` / `post_hooks`: 在客户端执行的命令
6. `backup_suffix`: 备份文件后缀，最终会追加 deployment_id 防止覆盖

### 客户端配置

可参考 [examples/client.toml](examples/client.toml)。

```toml
server_addr = "127.0.0.1:50061"
client_id = "node-a"
hostname = "node-a"
labels = ["linux", "prod"]
root_dir = "/tmp/sv-updater-client"
heartbeat_seconds = 5
```

字段说明：

1. `server_addr`: 服务端 gRPC 地址
2. `client_id`: 机器唯一标识，不填时默认使用主机名
3. `labels`: 用于和服务端的 target 做匹配
4. `root_dir`: staging 目录和相对目标路径的基准目录

## 运行方式

### 1. 启动服务端

```bash
cargo run -p sv-updater -- server --config sv-updater/examples/server.toml
```

启动后访问：

1. HTTP 控制台: http://127.0.0.1:8088
2. gRPC 地址: 127.0.0.1:50061

### 2. 启动客户端

```bash
cargo run -p sv-updater -- client --config sv-updater/examples/client.toml
```

客户端会自动重连，并持续发送心跳。

### 3. 页面操作

1. 选中一个或多个构建目标
2. 选中一个或多个在线客户端
3. 点击“开始部署”
4. 页面会展示每个目标在每台机器上的状态、MD5 和备份路径

## 客户端环境变量

执行前后置钩子时会注入以下环境变量：

1. `SV_DEPLOYMENT_ID`
2. `SV_TARGET_NAME`
3. `SV_ARTIFACT_PATH`
4. `SV_DESTINATION_PATH`
5. `SV_BUILD_MD5`
6. `SV_BACKUP_PATH`，当目标文件原本存在时提供

## 当前边界

这是一个可运行的 MVP，重点解决“多目标构建 + 多机器更新”的主链路。当前没有覆盖以下高级能力：

1. 权限控制和鉴权
2. 断点续传和大文件分块传输
3. 灰度发布、批次回滚
4. 历史部署持久化存储
5. Windows 专用 shell 适配细化

如果后续需要，可以继续往这几个方向扩展。

