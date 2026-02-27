# 日志收集Web服务

## 功能


1. 日志保存

很多多客户端定时将指定日志文件通过web接上传到服务端。

客户端上传日志文件需要包含如下信息：

```json
{
    "agent_name": "xxx",
    "ip": "xxx",
    "app": "pcli",
    "task-id": "111-222",
    "filename": "pcli.log"
}
```

日志文件报错路径为: ip/app/task-id/filename

2. 日志查看

可通过 web 页面查看所有日志列表，以及查看任意日志文件的日志内容，丰富日志格式显示（内有INFO, ERROR, TRACE, WARN日志等级）


## 技术选择

服务端: 使用 rust + axum 实现
前端: Vue

## 发布

构建时将整个前端资产静态打包到二进制中
