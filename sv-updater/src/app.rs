use std::{
    collections::{HashMap, HashSet},
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow, bail};
use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::{Html, IntoResponse},
    routing::{get, post},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use tokio::{
    fs,
    io::AsyncWriteExt,
    process::Command,
    sync::{RwLock, mpsc},
    time::sleep,
};
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status, transport::Server};
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::{
    config::{BuildTarget, ClientConfig, ServerConfig},
    generated::{
        ClientHello, ClientMessage, DeployCommand, DeployResult, Heartbeat, ServerMessage,
        agent_service_client::AgentServiceClient,
        agent_service_server::{AgentService, AgentServiceServer},
        client_message, server_message,
    },
    web::INDEX_HTML,
};

const MAX_MESSAGE_BYTES: usize = 128 * 1024 * 1024;

#[derive(Debug, Clone, Deserialize)]
struct DeployRequest {
    targets: Vec<String>,
    clients: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
struct DeployAccepted {
    deployment_id: String,
}

#[derive(Debug, Clone, Serialize)]
struct ApiState {
    build_targets: Vec<BuildTarget>,
    connected_clients: Vec<ClientSummary>,
    deployments: Vec<DeploymentRecord>,
}

#[derive(Debug, Clone, Serialize)]
struct ClientSummary {
    client_id: String,
    hostname: String,
    platform: String,
    labels: Vec<String>,
    root_dir: String,
    connected_at: DateTime<Utc>,
    last_seen: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
struct DeploymentRecord {
    deployment_id: String,
    status: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    requested_targets: Vec<String>,
    requested_clients: Vec<String>,
    entries: Vec<DeploymentEntry>,
    message: String,
}

#[derive(Debug, Clone, Serialize)]
struct DeploymentEntry {
    client_id: String,
    target_name: String,
    status: String,
    message: String,
    expected_md5: String,
    installed_md5: String,
    backup_path: String,
    updated_at: DateTime<Utc>,
}

#[derive(Debug)]
struct ConnectedClient {
    info: ClientHello,
    sender: mpsc::Sender<std::result::Result<ServerMessage, Status>>,
    connected_at: DateTime<Utc>,
    last_seen: RwLock<DateTime<Utc>>,
}

#[derive(Debug)]
struct ServerState {
    config: ServerConfig,
    clients: RwLock<HashMap<String, Arc<ConnectedClient>>>,
    deployments: RwLock<HashMap<String, DeploymentRecord>>,
}

#[derive(Debug)]
struct AgentRpc {
    state: Arc<ServerState>,
}

#[derive(Debug)]
struct AppError {
    status: StatusCode,
    message: String,
}

#[derive(Debug)]
struct BuiltArtifact {
    name: String,
    bytes: Vec<u8>,
    md5: String,
}

impl IntoResponse for AppError {
    fn into_response(self) -> axum::response::Response {
        (self.status, self.message).into_response()
    }
}

impl AppError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
        }
    }

    fn internal(error: anyhow::Error) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: error.to_string(),
        }
    }
}

impl From<anyhow::Error> for AppError {
    fn from(value: anyhow::Error) -> Self {
        AppError::internal(value)
    }
}

impl ServerState {
    fn new(config: ServerConfig) -> Self {
        Self {
            config,
            clients: RwLock::new(HashMap::new()),
            deployments: RwLock::new(HashMap::new()),
        }
    }

    async fn register_client(
        &self,
        hello: ClientHello,
        sender: mpsc::Sender<std::result::Result<ServerMessage, Status>>,
    ) {
        let now = Utc::now();
        let client = Arc::new(ConnectedClient {
            info: hello.clone(),
            sender,
            connected_at: now,
            last_seen: RwLock::new(Utc::now()),
        });

        self.clients
            .write()
            .await
            .insert(hello.client_id.clone(), client);
    }

    async fn touch_client(&self, client_id: &str) {
        let client = self.clients.read().await.get(client_id).cloned();
        if let Some(client) = client {
            *client.last_seen.write().await = Utc::now();
        }
    }

    async fn unregister_client(&self, client_id: &str) {
        self.clients.write().await.remove(client_id);
    }

    async fn snapshot_clients(&self) -> Vec<ClientSummary> {
        let clients = self
            .clients
            .read()
            .await
            .values()
            .cloned()
            .collect::<Vec<_>>();
        let mut summaries = Vec::with_capacity(clients.len());

        for client in clients {
            summaries.push(ClientSummary {
                client_id: client.info.client_id.clone(),
                hostname: client.info.hostname.clone(),
                platform: client.info.platform.clone(),
                labels: client.info.labels.clone(),
                root_dir: client.info.root_dir.clone(),
                connected_at: client.connected_at,
                last_seen: client.last_seen.read().await.clone(),
            });
        }

        summaries.sort_by(|left, right| left.client_id.cmp(&right.client_id));
        summaries
    }

    async fn get_client_dispatch(
        &self,
        client_id: &str,
    ) -> Option<(
        ClientHello,
        mpsc::Sender<std::result::Result<ServerMessage, Status>>,
    )> {
        self.clients
            .read()
            .await
            .get(client_id)
            .map(|client| (client.info.clone(), client.sender.clone()))
    }

    async fn create_deployment(&self, request: &DeployRequest) -> String {
        let now = Utc::now();
        let deployment_id = Uuid::new_v4().to_string();
        let mut entries = Vec::new();

        for client_id in &request.clients {
            for target_name in &request.targets {
                entries.push(DeploymentEntry {
                    client_id: client_id.clone(),
                    target_name: target_name.clone(),
                    status: "queued".to_string(),
                    message: "等待构建".to_string(),
                    expected_md5: String::new(),
                    installed_md5: String::new(),
                    backup_path: String::new(),
                    updated_at: now,
                });
            }
        }

        let record = DeploymentRecord {
            deployment_id: deployment_id.clone(),
            status: "queued".to_string(),
            created_at: now,
            updated_at: Utc::now(),
            requested_targets: request.targets.clone(),
            requested_clients: request.clients.clone(),
            entries,
            message: "部署任务已创建".to_string(),
        };

        self.deployments
            .write()
            .await
            .insert(deployment_id.clone(), record);

        deployment_id
    }

    async fn update_entry(
        &self,
        deployment_id: &str,
        client_id: &str,
        target_name: &str,
        status: &str,
        message: impl Into<String>,
        expected_md5: Option<String>,
        installed_md5: Option<String>,
        backup_path: Option<String>,
    ) {
        let mut deployments = self.deployments.write().await;
        let Some(record) = deployments.get_mut(deployment_id) else {
            return;
        };

        if let Some(entry) = record
            .entries
            .iter_mut()
            .find(|entry| entry.client_id == client_id && entry.target_name == target_name)
        {
            entry.status = status.to_string();
            entry.message = message.into();
            if let Some(expected_md5) = expected_md5 {
                entry.expected_md5 = expected_md5;
            }
            if let Some(installed_md5) = installed_md5 {
                entry.installed_md5 = installed_md5;
            }
            if let Some(backup_path) = backup_path {
                entry.backup_path = backup_path;
            }
            entry.updated_at = Utc::now();
        }

        recalculate_record(record);
    }

    async fn fail_target_for_clients(&self, deployment_id: &str, target_name: &str, message: &str) {
        let mut deployments = self.deployments.write().await;
        let Some(record) = deployments.get_mut(deployment_id) else {
            return;
        };

        for entry in record
            .entries
            .iter_mut()
            .filter(|entry| entry.target_name == target_name && !is_terminal_status(&entry.status))
        {
            entry.status = "failed".to_string();
            entry.message = message.to_string();
            entry.updated_at = Utc::now();
        }

        recalculate_record(record);
    }

    async fn fail_pending_entries(&self, deployment_id: &str, message: &str) {
        let mut deployments = self.deployments.write().await;
        let Some(record) = deployments.get_mut(deployment_id) else {
            return;
        };

        for entry in record
            .entries
            .iter_mut()
            .filter(|entry| !is_terminal_status(&entry.status))
        {
            entry.status = "failed".to_string();
            entry.message = message.to_string();
            entry.updated_at = Utc::now();
        }

        record.message = message.to_string();
        recalculate_record(record);
    }

    async fn deployments_snapshot(&self) -> Vec<DeploymentRecord> {
        let mut items = self
            .deployments
            .read()
            .await
            .values()
            .cloned()
            .collect::<Vec<_>>();
        items.sort_by(|left, right| right.created_at.cmp(&left.created_at));
        items
    }
}

#[tonic::async_trait]
impl AgentService for AgentRpc {
    type OpenSessionStream = ReceiverStream<std::result::Result<ServerMessage, Status>>;

    async fn open_session(
        &self,
        request: Request<tonic::Streaming<ClientMessage>>,
    ) -> std::result::Result<Response<Self::OpenSessionStream>, Status> {
        let mut inbound = request.into_inner();
        let first_message = inbound
            .message()
            .await?
            .ok_or_else(|| Status::invalid_argument("missing hello message"))?;
        let hello = extract_hello(first_message)?;
        let client_id = hello.client_id.clone();
        let (sender, receiver) = mpsc::channel(32);

        self.state.register_client(hello.clone(), sender).await;
        info!(client_id = %client_id, hostname = %hello.hostname, "client connected");

        let state = self.state.clone();
        tokio::spawn(async move {
            loop {
                match inbound.message().await {
                    Ok(Some(message)) => {
                        if let Err(error) =
                            handle_client_message(state.clone(), &client_id, message).await
                        {
                            warn!(client_id = %client_id, error = %error, "failed to handle client message");
                        }
                    }
                    Ok(None) => break,
                    Err(error) => {
                        warn!(client_id = %client_id, error = %error, "client stream closed with error");
                        break;
                    }
                }
            }

            state.unregister_client(&client_id).await;
            info!(client_id = %client_id, "client disconnected");
        });

        Ok(Response::new(ReceiverStream::new(receiver)))
    }
}

pub async fn run_server(config: ServerConfig) -> Result<()> {
    if config.build_targets.is_empty() {
        bail!("server config must contain at least one build target");
    }

    let http_addr: SocketAddr = config
        .http_listen
        .parse()
        .with_context(|| format!("invalid http_listen: {}", config.http_listen))?;
    let grpc_addr: SocketAddr = config
        .grpc_listen
        .parse()
        .with_context(|| format!("invalid grpc_listen: {}", config.grpc_listen))?;

    let state = Arc::new(ServerState::new(config));
    let http_app = Router::new()
        .route("/", get(index_page))
        .route("/api/state", get(api_state))
        .route("/api/deploy", post(api_deploy))
        .with_state(state.clone());

    let grpc_service = AgentServiceServer::new(AgentRpc {
        state: state.clone(),
    })
    .max_decoding_message_size(MAX_MESSAGE_BYTES)
    .max_encoding_message_size(MAX_MESSAGE_BYTES);

    info!(http = %http_addr, grpc = %grpc_addr, "sv-updater server started");

    let grpc_server = async move {
        Server::builder()
            .add_service(grpc_service)
            .serve(grpc_addr)
            .await
            .context("gRPC server failed")
    };

    let http_server = async move {
        let listener = tokio::net::TcpListener::bind(http_addr)
            .await
            .context("failed to bind HTTP listener")?;
        axum::serve(listener, http_app)
            .await
            .context("HTTP server failed")
    };

    tokio::try_join!(grpc_server, http_server)?;
    Ok(())
}

pub async fn run_client(config: ClientConfig) -> Result<()> {
    loop {
        match connect_client_once(config.clone()).await {
            Ok(()) => warn!("server stream closed, reconnecting in 3 seconds"),
            Err(error) => warn!(error = %error, "client loop failed, reconnecting in 3 seconds"),
        }

        sleep(Duration::from_secs(3)).await;
    }
}

async fn connect_client_once(config: ClientConfig) -> Result<()> {
    let endpoint = format!("http://{}", config.server_addr);
    let channel = tonic::transport::Endpoint::from_shared(endpoint.clone())?
        .connect()
        .await
        .with_context(|| format!("failed to connect to server: {endpoint}"))?;

    let mut client = AgentServiceClient::new(channel)
        .max_decoding_message_size(MAX_MESSAGE_BYTES)
        .max_encoding_message_size(MAX_MESSAGE_BYTES);

    let (sender, receiver) = mpsc::channel(32);
    let hello = client_hello(&config);
    sender
        .send(ClientMessage {
            payload: Some(client_message::Payload::Hello(hello.clone())),
        })
        .await
        .map_err(|_| anyhow!("failed to enqueue hello message"))?;

    let heartbeat_tx = sender.clone();
    let heartbeat_seconds = config.heartbeat_seconds.max(1);
    let heartbeat_task = tokio::spawn(async move {
        loop {
            sleep(Duration::from_secs(heartbeat_seconds)).await;
            let heartbeat = ClientMessage {
                payload: Some(client_message::Payload::Heartbeat(Heartbeat {
                    unix_seconds: unix_seconds_now() as i64,
                })),
            };
            if heartbeat_tx.send(heartbeat).await.is_err() {
                break;
            }
        }
    });

    let response = client.open_session(ReceiverStream::new(receiver)).await?;
    let mut inbound = response.into_inner();
    info!(client_id = %hello.client_id, server = %config.server_addr, "client connected to updater server");

    while let Some(message) = inbound.message().await? {
        match message.payload {
            Some(server_message::Payload::Deploy(command)) => {
                let result = match apply_deploy(&config, &command).await {
                    Ok(result) => result,
                    Err(error) => DeployResult {
                        deployment_id: command.deployment_id.clone(),
                        target_name: command.target_name.clone(),
                        success: false,
                        message: error.to_string(),
                        installed_md5: String::new(),
                        backup_path: String::new(),
                    },
                };

                sender
                    .send(ClientMessage {
                        payload: Some(client_message::Payload::DeployResult(result)),
                    })
                    .await
                    .map_err(|_| anyhow!("failed to send deploy result back to server"))?;
            }
            None => warn!("received empty server message"),
        }
    }

    heartbeat_task.abort();
    Ok(())
}

async fn index_page() -> Html<&'static str> {
    Html(INDEX_HTML)
}

async fn api_state(State(state): State<Arc<ServerState>>) -> Json<ApiState> {
    let connected_clients = state.snapshot_clients().await;
    let deployments = state.deployments_snapshot().await;
    Json(ApiState {
        build_targets: state.config.build_targets.clone(),
        connected_clients,
        deployments,
    })
}

async fn api_deploy(
    State(state): State<Arc<ServerState>>,
    Json(request): Json<DeployRequest>,
) -> std::result::Result<Json<DeployAccepted>, AppError> {
    if request.targets.is_empty() {
        return Err(AppError::bad_request("at least one target is required"));
    }
    if request.clients.is_empty() {
        return Err(AppError::bad_request("at least one client is required"));
    }

    let known_targets = state
        .config
        .build_targets
        .iter()
        .map(|target| target.name.clone())
        .collect::<HashSet<_>>();
    for target in &request.targets {
        if !known_targets.contains(target) {
            return Err(AppError::bad_request(format!("unknown target: {target}")));
        }
    }

    let deployment_id = state.create_deployment(&request).await;
    let background_state = state.clone();
    let background_request = request.clone();
    let background_deployment_id = deployment_id.clone();
    tokio::spawn(async move {
        if let Err(error) = execute_deployment(
            background_state.clone(),
            &background_deployment_id,
            background_request,
        )
        .await
        {
            error!(deployment_id = %background_deployment_id, error = %error, "deployment task failed");
            background_state
                .fail_pending_entries(&background_deployment_id, &error.to_string())
                .await;
        }
    });

    Ok(Json(DeployAccepted { deployment_id }))
}

async fn execute_deployment(
    state: Arc<ServerState>,
    deployment_id: &str,
    request: DeployRequest,
) -> Result<()> {
    for target_name in request.targets {
        let target = state
            .config
            .find_target(&target_name)
            .ok_or_else(|| anyhow!("unknown target: {target_name}"))?;

        let built = match build_target(&state.config, &target).await {
            Ok(built) => built,
            Err(error) => {
                state
                    .fail_target_for_clients(
                        deployment_id,
                        &target.name,
                        &format!("构建失败: {error}"),
                    )
                    .await;
                continue;
            }
        };

        info!(deployment_id = %deployment_id, target = %target.name, md5 = %built.md5, "target built successfully");

        for client_id in &request.clients {
            let Some((client_info, sender)) = state.get_client_dispatch(client_id).await else {
                state
                    .update_entry(
                        deployment_id,
                        client_id,
                        &target.name,
                        "failed",
                        "客户端未连接",
                        Some(built.md5.clone()),
                        None,
                        None,
                    )
                    .await;
                continue;
            };

            if !client_matches_target(&client_info, &target) {
                state
                    .update_entry(
                        deployment_id,
                        client_id,
                        &target.name,
                        "failed",
                        format!(
                            "客户端标签不匹配，要求 {:?}，当前 {:?}",
                            target.required_labels, client_info.labels
                        ),
                        Some(built.md5.clone()),
                        None,
                        None,
                    )
                    .await;
                continue;
            }

            let message = ServerMessage {
                payload: Some(server_message::Payload::Deploy(DeployCommand {
                    deployment_id: deployment_id.to_string(),
                    target_name: target.name.clone(),
                    artifact_name: built.name.clone(),
                    artifact_bytes: built.bytes.clone(),
                    destination_path: target.destination_path.clone(),
                    pre_hooks: target.pre_hooks.clone(),
                    post_hooks: target.post_hooks.clone(),
                    executable: target.executable,
                    backup_suffix: target.backup_suffix.clone(),
                    build_md5: built.md5.clone(),
                })),
            };

            match sender.send(Ok(message)).await {
                Ok(()) => {
                    state
                        .update_entry(
                            deployment_id,
                            client_id,
                            &target.name,
                            "dispatched",
                            "二进制已下发，等待客户端执行",
                            Some(built.md5.clone()),
                            None,
                            None,
                        )
                        .await;
                }
                Err(_) => {
                    state
                        .update_entry(
                            deployment_id,
                            client_id,
                            &target.name,
                            "failed",
                            "下发失败，客户端连接已断开",
                            Some(built.md5.clone()),
                            None,
                            None,
                        )
                        .await;
                }
            }
        }
    }

    Ok(())
}

async fn handle_client_message(
    state: Arc<ServerState>,
    client_id: &str,
    message: ClientMessage,
) -> Result<()> {
    match message.payload {
        Some(client_message::Payload::Heartbeat(_)) => {
            state.touch_client(client_id).await;
        }
        Some(client_message::Payload::DeployResult(result)) => {
            state.touch_client(client_id).await;
            let status = if result.success { "success" } else { "failed" };
            state
                .update_entry(
                    &result.deployment_id,
                    client_id,
                    &result.target_name,
                    status,
                    result.message,
                    None,
                    Some(result.installed_md5),
                    Some(result.backup_path),
                )
                .await;
        }
        Some(client_message::Payload::Hello(_)) => {}
        None => warn!(client_id = %client_id, "received empty client message"),
    }

    Ok(())
}

async fn build_target(config: &ServerConfig, target: &BuildTarget) -> Result<BuiltArtifact> {
    let workspace_dir = config.workspace_dir();
    run_shell_script(
        &target.build_script,
        &workspace_dir,
        &[("SV_TARGET_NAME", target.name.clone())],
    )
    .await
    .with_context(|| format!("build script failed for target {}", target.name))?;

    let artifact_path = resolve_path(&workspace_dir, &target.artifact_path);
    let artifact_bytes = fs::read(&artifact_path)
        .await
        .with_context(|| format!("failed to read artifact: {}", artifact_path.display()))?;
    let artifact_name = artifact_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow!("artifact_path must end with a file name"))?
        .to_string();

    Ok(BuiltArtifact {
        name: artifact_name,
        md5: md5_hex(&artifact_bytes),
        bytes: artifact_bytes,
    })
}

async fn apply_deploy(config: &ClientConfig, command: &DeployCommand) -> Result<DeployResult> {
    let root_dir = resolve_path(Path::new("."), &config.root_dir);
    let staging_dir = root_dir
        .join("staging")
        .join(format!("{}-{}", command.deployment_id, command.target_name));
    fs::create_dir_all(&staging_dir).await.with_context(|| {
        format!(
            "failed to create staging directory: {}",
            staging_dir.display()
        )
    })?;

    let staged_artifact = staging_dir.join(&command.artifact_name);
    let mut file = fs::File::create(&staged_artifact).await.with_context(|| {
        format!(
            "failed to create staged artifact: {}",
            staged_artifact.display()
        )
    })?;
    file.write_all(&command.artifact_bytes).await?;
    file.flush().await?;

    let destination_path = resolve_path(&root_dir, &command.destination_path);
    if let Some(parent) = destination_path.parent() {
        fs::create_dir_all(parent).await.with_context(|| {
            format!("failed to create destination parent: {}", parent.display())
        })?;
    }

    let backup_path = if fs::try_exists(&destination_path).await.unwrap_or(false) {
        let backup = build_backup_path(
            &destination_path,
            &command.backup_suffix,
            &command.deployment_id,
        );
        fs::copy(&destination_path, &backup)
            .await
            .with_context(|| {
                format!(
                    "failed to backup destination {} to {}",
                    destination_path.display(),
                    backup.display()
                )
            })?;
        Some(backup)
    } else {
        None
    };

    let mut hook_env = vec![
        ("SV_DEPLOYMENT_ID", command.deployment_id.clone()),
        ("SV_TARGET_NAME", command.target_name.clone()),
        (
            "SV_ARTIFACT_PATH",
            staged_artifact.to_string_lossy().into_owned(),
        ),
        (
            "SV_DESTINATION_PATH",
            destination_path.to_string_lossy().into_owned(),
        ),
        ("SV_BUILD_MD5", command.build_md5.clone()),
    ];
    if let Some(path) = &backup_path {
        hook_env.push(("SV_BACKUP_PATH", path.to_string_lossy().into_owned()));
    }

    for hook in &command.pre_hooks {
        run_shell_script(hook, &root_dir, &hook_env)
            .await
            .with_context(|| format!("pre hook failed: {hook}"))?;
    }

    fs::copy(&staged_artifact, &destination_path)
        .await
        .with_context(|| {
            format!(
                "failed to copy artifact to destination: {}",
                destination_path.display()
            )
        })?;

    #[cfg(unix)]
    if command.executable {
        use std::os::unix::fs::PermissionsExt;

        let mut permissions = fs::metadata(&destination_path).await?.permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&destination_path, permissions).await?;
    }

    for hook in &command.post_hooks {
        run_shell_script(hook, &root_dir, &hook_env)
            .await
            .with_context(|| format!("post hook failed: {hook}"))?;
    }

    let installed_md5 = md5_hex(&fs::read(&destination_path).await?);
    let mut message = format!(
        "部署完成，目标文件 {}，服务端 MD5 {}，客户端 MD5 {}",
        destination_path.display(),
        command.build_md5,
        installed_md5
    );
    if installed_md5 != command.build_md5 {
        message.push_str("，警告：MD5 不一致");
    }

    Ok(DeployResult {
        deployment_id: command.deployment_id.clone(),
        target_name: command.target_name.clone(),
        success: installed_md5 == command.build_md5,
        message,
        installed_md5,
        backup_path: backup_path
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_default(),
    })
}

fn extract_hello(message: ClientMessage) -> std::result::Result<ClientHello, Status> {
    match message.payload {
        Some(client_message::Payload::Hello(hello)) => Ok(hello),
        _ => Err(Status::invalid_argument(
            "the first client message must be hello",
        )),
    }
}

fn client_matches_target(client: &ClientHello, target: &BuildTarget) -> bool {
    if target.required_labels.is_empty() {
        return true;
    }

    let labels = client.labels.iter().cloned().collect::<HashSet<_>>();
    target
        .required_labels
        .iter()
        .all(|required| labels.contains(required))
}

fn recalculate_record(record: &mut DeploymentRecord) {
    let now = Utc::now();
    record.updated_at = now;

    if record.entries.is_empty() {
        record.status = "empty".to_string();
        record.message = "没有部署条目".to_string();
        return;
    }

    let all_terminal = record
        .entries
        .iter()
        .all(|entry| is_terminal_status(&entry.status));
    let has_failure = record.entries.iter().any(|entry| entry.status == "failed");
    let success_count = record
        .entries
        .iter()
        .filter(|entry| entry.status == "success")
        .count();

    record.status = if all_terminal {
        if has_failure {
            "completed_with_failures".to_string()
        } else {
            "completed".to_string()
        }
    } else if record.entries.iter().any(|entry| entry.status != "queued") {
        "running".to_string()
    } else {
        "queued".to_string()
    };

    record.message = format!(
        "{} / {} 条目成功，{} 个条目总计",
        success_count,
        record.entries.len(),
        record.entries.len()
    );
}

fn is_terminal_status(status: &str) -> bool {
    matches!(status, "success" | "failed")
}

async fn run_shell_script(
    script: &str,
    working_dir: &Path,
    envs: &[(&str, String)],
) -> Result<String> {
    let mut command = if cfg!(windows) {
        let mut command = Command::new("cmd");
        command.args(["/C", script]);
        command
    } else {
        let mut command = Command::new("sh");
        command.args(["-lc", script]);
        command
    };

    command.current_dir(working_dir);
    for (key, value) in envs {
        command.env(key, value);
    }

    let output = command.output().await.with_context(|| {
        format!(
            "failed to execute script in {}: {script}",
            working_dir.display()
        )
    })?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{}{}", stdout, stderr);

    if !output.status.success() {
        bail!(
            "script exited with status {}\n{}",
            output.status,
            combined.trim()
        );
    }

    Ok(combined)
}

pub async fn load_toml_config<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let content = fs::read_to_string(path)
        .await
        .with_context(|| format!("failed to read config: {}", path.display()))?;
    toml::from_str(&content).with_context(|| format!("invalid TOML config: {}", path.display()))
}

fn client_hello(config: &ClientConfig) -> ClientHello {
    let detected_hostname = detect_hostname();
    let hostname = config
        .hostname
        .clone()
        .unwrap_or_else(|| detected_hostname.clone());
    let client_id = config.client_id.clone().unwrap_or_else(|| hostname.clone());

    ClientHello {
        client_id,
        hostname,
        platform: format!("{}/{}", std::env::consts::OS, std::env::consts::ARCH),
        labels: config.labels.clone(),
        root_dir: resolve_path(Path::new("."), &config.root_dir)
            .to_string_lossy()
            .into_owned(),
    }
}

fn resolve_path(base_dir: &Path, raw_path: &str) -> PathBuf {
    let path = PathBuf::from(raw_path);
    if path.is_absolute() {
        path
    } else {
        base_dir.join(path)
    }
}

fn build_backup_path(destination: &Path, suffix: &str, deployment_id: &str) -> PathBuf {
    let backup_suffix = if suffix.trim().is_empty() {
        format!("bak.{deployment_id}")
    } else {
        format!(
            "{}.{}",
            suffix.trim().trim_start_matches('.'),
            deployment_id
        )
    };
    PathBuf::from(format!("{}.{}", destination.display(), backup_suffix))
}

fn md5_hex(bytes: &[u8]) -> String {
    format!("{:x}", md5::compute(bytes))
}

pub fn init_tracing() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "sv_updater=info,info".into()),
        )
        .with_target(false)
        .compact()
        .init();
}

fn detect_hostname() -> String {
    hostname::get()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "unknown-host".to_string())
}

fn unix_seconds_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}