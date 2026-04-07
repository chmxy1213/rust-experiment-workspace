use axum::{
    Router,
    body::Body,
    extract::{Request, State},
    http::StatusCode,
    response::Response,
    routing::any,
};
use reqwest::Client;
use tokio::net::TcpListener;

#[derive(Clone)]
struct AppState {
    client: Client,
}

async fn heartbeat() -> &'static str {
    "OK, from proxy"
}

async fn secvison_ping() -> &'static str {
    "OK, from proxy"
}

async fn proxy_handler(
    State(state): State<AppState>,
    req: Request,
) -> Result<Response, StatusCode> {
    let path_query = req
        .uri()
        .path_and_query()
        .map(|v| v.as_str())
        .unwrap_or(req.uri().path());
    let target_uri = format!("http://127.0.0.1:8080{}", path_query);

    let (parts, body) = req.into_parts();

    let body_bytes = match axum::body::to_bytes(body, usize::MAX).await {
        Ok(b) => b,
        Err(_) => return Err(StatusCode::BAD_REQUEST),
    };

    let mut out_req = state.client.request(parts.method, target_uri);

    for (name, value) in parts.headers.iter() {
        if name != axum::http::header::HOST {
            out_req = out_req.header(name.clone(), value.clone());
        }
    }

    let out_req = out_req.body(body_bytes);

    match out_req.send().await {
        Ok(res) => {
            let mut out_res = Response::builder().status(res.status());

            for (name, value) in res.headers().iter() {
                out_res = out_res.header(name.clone(), value.clone());
            }

            let out_body_bytes = res.bytes().await.unwrap_or_default();
            let response = out_res
                .body(Body::from(out_body_bytes))
                .unwrap_or_else(|_| {
                    Response::builder()
                        .status(StatusCode::INTERNAL_SERVER_ERROR)
                        .body(Body::from(""))
                        .unwrap()
                });

            Ok(response)
        }
        Err(e) => {
            eprintln!("Proxy error: {:?}", e);
            Err(StatusCode::BAD_GATEWAY)
        }
    }
}

async fn upstream_handler() -> &'static str {
    "from upstream"
}

#[tokio::main]
async fn main() {
    // 启动上游测试服务器 (127.0.0.1:8080)
    tokio::spawn(async {
        let upstream_app = Router::new().fallback(any(upstream_handler));
        let upstream_addr = "127.0.0.1:8080";
        println!("Upstream server listening on http://{}", upstream_addr);
        let upstream_listener = TcpListener::bind(upstream_addr).await.unwrap();
        axum::serve(upstream_listener, upstream_app).await.unwrap();
    });

    let client = Client::builder()
        .build()
        .expect("Failed to build reqwest client");

    let state = AppState { client };

    let app = Router::new()
        .route("/heartbeat.html", any(heartbeat))
        .route("/secvison_ping", any(secvison_ping))
        .fallback(any(proxy_handler))
        .with_state(state);

    let addr = "0.0.0.0:3000";
    println!("Proxy server listening on http://{}", addr);
    println!("- Intercepts: /heartbeat.html, /secvison_ping");
    println!("- Forwards all other traffic to: 127.0.0.1:8080");

    let listener = TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
