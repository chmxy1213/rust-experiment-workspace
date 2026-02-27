use axum::{
    Json, Router,
    extract::Query,
    http::{StatusCode, Uri, header},
    response::IntoResponse,
    routing::{get, post},
};
use axum_extra::extract::Multipart;
use rust_embed::RustEmbed;
use serde::{Deserialize, Serialize};
use std::{net::SocketAddr, path::PathBuf};
use tokio::fs;
use tracing::{error, info};

#[derive(RustEmbed)]
#[folder = "static/"]
struct Asset;

#[derive(Serialize)]
struct LogFile {
    filename: String,
    path: String,
    agent_name: String,
}

#[derive(Deserialize)]
struct LogContentQuery {
    path: String,
}

const LOGS_DIR: &str = "logs";

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    // Ensure logs directory exists
    if let Err(e) = fs::create_dir_all(LOGS_DIR).await {
        error!("Failed to create logs directory: {}", e);
        return;
    }

    let app = Router::new()
        .route("/api/upload", post(upload_log))
        .route("/api/logs", get(list_logs))
        .route("/api/logs/content", get(get_log_content))
        .fallback(static_handler);

    let addr = SocketAddr::from(([0, 0, 0, 0], 8001));
    info!("Listening on {}", addr);
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

pub fn app() -> Router {
    Router::new()
        .route("/api/upload", post(upload_log))
        .route("/api/logs", get(list_logs))
        .route("/api/logs/content", get(get_log_content))
        .fallback(static_handler)
}

async fn static_handler(uri: Uri) -> impl IntoResponse {
    let mut path = uri.path().trim_start_matches('/').to_string();

    if path.is_empty() {
        path = "index.html".to_string();
    }

    match Asset::get(path.as_str()) {
        Some(content) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            ([(header::CONTENT_TYPE, mime.as_ref())], content.data).into_response()
        }
        None => {
            if uri.path() == "/" {
                return (StatusCode::NOT_FOUND, "404 Not Found").into_response();
            }
            // Fallback to index.html for SPA routing if needed
            match Asset::get("index.html") {
                Some(content) => {
                    let mime = mime_guess::from_path("index.html").first_or_octet_stream();
                    ([(header::CONTENT_TYPE, mime.as_ref())], content.data).into_response()
                }
                None => (StatusCode::NOT_FOUND, "404 Not Found").into_response(),
            }
        }
    }
}

async fn upload_log(mut multipart: Multipart) -> Result<impl IntoResponse, (StatusCode, String)> {
    let mut agent_name = String::new();
    let mut ip = String::new();
    let mut app = String::new();
    let mut task_id = String::new();
    let mut filename = String::new();
    let mut file_data = Vec::new();

    while let Some(field) = multipart.next_field().await.map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            format!("Failed to read multipart: {}", e),
        )
    })? {
        let name = field.name().unwrap_or("").to_string();

        if name == "file" {
            file_data = field
                .bytes()
                .await
                .map_err(|e| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("Failed to read file data: {}", e),
                    )
                })?
                .to_vec();
        } else {
            let text = field.text().await.map_err(|e| {
                (
                    StatusCode::BAD_REQUEST,
                    format!("Failed to read field text: {}", e),
                )
            })?;
            match name.as_str() {
                "agent_name" => agent_name = text,
                "ip" => ip = text,
                "app" => app = text,
                "task-id" => task_id = text,
                "filename" => filename = text,
                _ => {}
            }
        }
    }

    if ip.is_empty()
        || app.is_empty()
        || task_id.is_empty()
        || filename.is_empty()
        || file_data.is_empty()
    {
        return Err((
            StatusCode::BAD_REQUEST,
            "Missing required fields or file data".to_string(),
        ));
    }

    // Path: agent_name/ip/app/task-id/filename
    let mut save_path = PathBuf::from(LOGS_DIR);
    let agent_dir = if agent_name.is_empty() {
        "unknown_agent"
    } else {
        &agent_name
    };
    save_path.push(agent_dir);
    save_path.push(&ip);
    save_path.push(&app);
    save_path.push(&task_id);

    if let Err(e) = fs::create_dir_all(&save_path).await {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to create directories: {}", e),
        ));
    }

    save_path.push(&filename);

    if let Err(e) = fs::write(&save_path, file_data).await {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to save file: {}", e),
        ));
    }

    info!("Saved log file to {:?}", save_path);

    Ok((StatusCode::OK, "File uploaded successfully"))
}

async fn list_logs() -> Result<Json<Vec<LogFile>>, (StatusCode, String)> {
    let mut logs = Vec::new();
    let base_path = PathBuf::from(LOGS_DIR);

    if !base_path.exists() {
        return Ok(Json(logs));
    }

    let mut stack = vec![base_path.clone()];

    while let Some(dir) = stack.pop() {
        let mut entries = match fs::read_dir(&dir).await {
            Ok(e) => e,
            Err(_) => continue,
        };

        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.is_file() {
                if let Ok(rel_path) = path.strip_prefix(&base_path) {
                    let components: Vec<_> = rel_path.components().collect();
                    let agent_name = if components.len() > 0 {
                        components[0].as_os_str().to_string_lossy().into_owned()
                    } else {
                        "unknown".to_string()
                    };

                    logs.push(LogFile {
                        filename: path
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into_owned(),
                        path: rel_path.to_string_lossy().into_owned(),
                        agent_name,
                    });
                }
            }
        }
    }

    Ok(Json(logs))
}

async fn get_log_content(
    Query(query): Query<LogContentQuery>,
) -> Result<String, (StatusCode, String)> {
    // Prevent directory traversal
    if query.path.contains("..") {
        return Err((StatusCode::BAD_REQUEST, "Invalid path".to_string()));
    }

    let mut full_path = PathBuf::from(LOGS_DIR);
    full_path.push(&query.path);

    match fs::read_to_string(&full_path).await {
        Ok(content) => Ok(content),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to read file: {}", e),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum_test::TestServer;
    use axum_test::multipart::{MultipartForm, Part};

    #[tokio::test]
    async fn test_upload_log() {
        // Create a temporary directory for logs during testing
        let test_logs_dir = "test_logs";
        let _ = fs::create_dir_all(test_logs_dir).await;

        let app = app();
        let server = TestServer::new(app).unwrap();

        let multipart = MultipartForm::new()
            .add_text("agent_name", "test_agent")
            .add_text("ip", "127.0.0.1")
            .add_text("app", "test_app")
            .add_text("task-id", "task-123")
            .add_text("filename", "test.log")
            .add_part(
                "file",
                Part::bytes("test log content".as_bytes())
                    .file_name("test.log")
                    .mime_type("application/octet-stream"),
            );

        let response = server
            .post("/api/upload")
            .multipart(multipart)
            .await;

        response.assert_status_ok();

        // Verify the file was created
        let expected_path = PathBuf::from(LOGS_DIR)
            .join("test_agent")
            .join("127.0.0.1")
            .join("test_app")
            .join("task-123")
            .join("test.log");

        assert!(expected_path.exists());

        let content = fs::read_to_string(&expected_path).await.unwrap();
        assert_eq!(content, "test log content");

        // Clean up
        let _ = fs::remove_dir_all(LOGS_DIR).await;
    }
}
