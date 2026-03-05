use reqwest::multipart;
use tokio::fs;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. 准备要上传的文件
    let file_path = "test_client.log";
    fs::write(
        file_path,
        "This is a test log from Rust client.\nINFO: Everything is fine.",
    )
    .await?;

    // 2. 读取文件内容
    let file_content = fs::read(file_path).await?;

    // 3. 构造 multipart 表单
    // 使用 reqwest::multipart::Form::new() 创建表单
    // 使用 .text() 添加普通文本字段
    // 使用 .part() 添加文件字段
    let form = multipart::Form::new()
        .text("agent_name", "rust-client-01")
        .text("ip", "192.168.1.100")
        .text("app", "my-rust-app")
        .text("task-id", "task-999")
        .text("filename", "test_client.log")
        .part(
            "file",
            multipart::Part::bytes(file_content)
                .file_name("test_client.log")
                .mime_str("text/plain")?,
        );

    // 4. 发送请求
    let client = reqwest::Client::new();
    let response = client
        .post("http://127.0.0.1:8001/api/upload")
        .multipart(form)
        .send()
        .await?;

    // 5. 处理响应
    println!("Status: {}", response.status());
    let body = response.text().await?;
    println!("Response: {}", body);

    // 清理测试文件
    fs::remove_file(file_path).await?;

    Ok(())
}
