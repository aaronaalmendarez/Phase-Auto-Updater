use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::ErrorKind;
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tungstenite::handshake::server::ErrorResponse;
use tungstenite::{Message, accept_hdr};
use uuid::Uuid;

pub const VERSION: &str = "phase-companion/1";
pub const DEFAULT_PORT: u16 = 27730;
pub const DEFAULT_PATH: &str = "/phase-companion";

#[derive(Clone, Debug, Default)]
pub struct CompanionConfig {
    pub port: u16,
    pub path: String,
    pub token: String,
}

impl CompanionConfig {
    pub fn default_local() -> Self {
        Self {
            port: DEFAULT_PORT,
            path: DEFAULT_PATH.to_owned(),
            token: String::new(),
        }
    }

    pub fn url(&self) -> String {
        format!("ws://127.0.0.1:{}{}", self.port, clean_path(&self.path))
    }
}

#[derive(Clone, Debug)]
pub enum CompanionEvent {
    Listening { url: String },
    ClientConnected,
    ClientDisconnected,
    PacketReceived(CompanionPacket),
    PacketSent { op: String },
    SendFailed { op: String, message: String },
    Error(String),
    Stopped,
}

#[derive(Debug)]
#[allow(dead_code)]
enum CompanionCommand {
    Send {
        op: String,
        payload: Value,
        reply_to: Option<String>,
    },
    Stop,
}

pub struct CompanionBridge {
    command_tx: Sender<CompanionCommand>,
    event_rx: Receiver<CompanionEvent>,
}

impl CompanionBridge {
    pub fn start(config: CompanionConfig) -> Self {
        let (command_tx, command_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        thread::spawn(move || run_server(config, command_rx, event_tx));
        Self {
            command_tx,
            event_rx,
        }
    }

    #[allow(dead_code)]
    pub fn send(&self, op: impl Into<String>, payload: Value, reply_to: Option<String>) {
        let _ = self.command_tx.send(CompanionCommand::Send {
            op: op.into(),
            payload,
            reply_to,
        });
    }

    pub fn poll(&self) -> Vec<CompanionEvent> {
        self.event_rx.try_iter().collect()
    }

    pub fn stop(&self) {
        let _ = self.command_tx.send(CompanionCommand::Stop);
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CompanionPacket {
    pub v: String,
    pub id: String,
    pub op: String,
    #[serde(default)]
    pub reply_to: Option<String>,
    #[serde(default)]
    pub token: String,
    #[serde(default)]
    pub sent_at: f64,
    #[serde(default)]
    pub payload: Value,
}

#[derive(Clone, Debug, Default)]
struct FaceCatalogCache {
    version: String,
    items: Vec<Value>,
    refreshed_at: f64,
}

pub fn capabilities_payload() -> Value {
    json!({
        "app": "Phase Companion",
        "protocol": VERSION,
        "platform": std::env::consts::OS,
        "capabilities": [
            {
                "id": "presence",
                "label": "Companion Detection",
                "status": "ready",
                "description": "Lets Studio detect the native companion app without blocking normal plugin use."
            },
            {
                "id": "perf-snapshot",
                "label": "Boost Doctor Snapshots",
                "status": "ready",
                "description": "Receives lightweight Studio performance snapshots so Companion can rank bottlenecks across sessions."
            },
            {
                "id": "local-cache",
                "label": "Native Local Cache",
                "status": "ready",
                "description": "Fast disk-backed catalogs, thumbnails, previews, and generated bake artifacts."
            },
            {
                "id": "face-catalog",
                "label": "Face Catalog Paging",
                "status": "ready",
                "description": "Caches Studio face catalog metadata locally and returns filtered visible pages."
            },
            {
                "id": "ik-prebake",
                "label": "IK Playback Cache",
                "status": "ready",
                "description": "Persists baked IK preview poses by signature so Studio can restore smooth playback without repeating matching bakes."
            },
            {
                "id": "timeline-index",
                "label": "Timeline Indexing",
                "status": "planned",
                "description": "Build compact indexes for heavy documents so plugin UI queries stay responsive."
            },
            {
                "id": "compression",
                "label": "Payload Compression",
                "status": "planned",
                "description": "Batch large timeline and rig payloads through compact JSON/binary chunks."
            },
            {
                "id": "video-reference",
                "label": "Video Reference",
                "status": "ready",
                "description": "Existing local video and YouTube reference bridge."
            }
        ]
    })
}

fn clean_path(path: &str) -> String {
    let mut value = path.trim().to_owned();
    if value.is_empty() {
        value = DEFAULT_PATH.to_owned();
    }
    if !value.starts_with('/') {
        value.insert(0, '/');
    }
    value
}

fn now_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs_f64())
        .unwrap_or_default()
}

fn make_packet(op: &str, payload: Value, token: &str, reply_to: Option<String>) -> CompanionPacket {
    CompanionPacket {
        v: VERSION.to_owned(),
        id: Uuid::new_v4().to_string(),
        op: op.to_owned(),
        reply_to,
        token: token.to_owned(),
        sent_at: now_seconds(),
        payload,
    }
}

fn run_server(
    config: CompanionConfig,
    command_rx: Receiver<CompanionCommand>,
    event_tx: Sender<CompanionEvent>,
) {
    let path = clean_path(&config.path);
    let token = config.token.trim().to_owned();
    let bind_addr = format!("127.0.0.1:{}", config.port);
    let listener = match TcpListener::bind(&bind_addr) {
        Ok(listener) => listener,
        Err(error) => {
            let _ = event_tx.send(CompanionEvent::Error(format!(
                "Could not start companion bridge on {bind_addr}: {error}"
            )));
            return;
        }
    };
    if let Err(error) = listener.set_nonblocking(true) {
        let _ = event_tx.send(CompanionEvent::Error(format!(
            "Could not configure companion bridge listener: {error}"
        )));
        return;
    }

    let _ = event_tx.send(CompanionEvent::Listening { url: config.url() });
    let mut clients = Vec::<tungstenite::WebSocket<TcpStream>>::new();
    let mut face_catalog = load_face_catalog_cache();

    loop {
        let mut stopping = false;
        for command in command_rx.try_iter() {
            match command {
                CompanionCommand::Send {
                    op,
                    payload,
                    reply_to,
                } => {
                    if clients.is_empty() {
                        let _ = event_tx.send(CompanionEvent::SendFailed {
                            op,
                            message: "Studio is not connected.".to_owned(),
                        });
                        continue;
                    }
                    let packet = make_packet(&op, payload, &token, reply_to);
                    let Ok(encoded) = serde_json::to_string(&packet) else {
                        let _ = event_tx.send(CompanionEvent::SendFailed {
                            op,
                            message: "Could not encode companion packet.".to_owned(),
                        });
                        continue;
                    };
                    send_to_clients(&mut clients, &event_tx, op, encoded);
                }
                CompanionCommand::Stop => stopping = true,
            }
        }
        if stopping {
            break;
        }

        loop {
            match listener.accept() {
                Ok((stream, _)) => match accept_client(stream, &path, &token) {
                    Ok(mut socket) => {
                        let _ = socket.get_mut().set_read_timeout(None);
                        let _ = socket.get_mut().set_write_timeout(None);
                        let _ = socket.get_mut().set_nonblocking(true);
                        clients.push(socket);
                        let _ = event_tx.send(CompanionEvent::ClientConnected);
                    }
                    Err(error) => {
                        let _ = event_tx.send(CompanionEvent::Error(error));
                    }
                },
                Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                Err(error) => {
                    let _ = event_tx.send(CompanionEvent::Error(format!(
                        "Companion bridge accept failed: {error}"
                    )));
                    break;
                }
            }
        }

        poll_clients(&mut clients, &event_tx, &token, &mut face_catalog);
        thread::sleep(Duration::from_millis(16));
    }

    for mut client in clients {
        let _ = client.close(None);
    }
    let _ = event_tx.send(CompanionEvent::Stopped);
}

fn accept_client(
    stream: TcpStream,
    expected_path: &str,
    expected_token: &str,
) -> Result<tungstenite::WebSocket<TcpStream>, String> {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
    let expected_path = expected_path.to_owned();
    let expected_token = expected_token.to_owned();

    accept_hdr(
        stream,
        move |request: &tungstenite::handshake::server::Request, response| {
            if request.uri().path() != expected_path {
                return Err(error_response(404, "Unknown Phase companion bridge path."));
            }
            if !expected_token.is_empty() {
                let request_token = request
                    .headers()
                    .get("X-Phase-Companion-Token")
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or("")
                    .trim();
                if request_token != expected_token {
                    return Err(error_response(401, "Invalid Phase companion bridge token."));
                }
            }
            Ok(response)
        },
    )
    .map_err(|error| format!("Companion bridge handshake failed: {error}"))
}

fn poll_clients(
    clients: &mut Vec<tungstenite::WebSocket<TcpStream>>,
    event_tx: &Sender<CompanionEvent>,
    token: &str,
    face_catalog: &mut FaceCatalogCache,
) {
    let mut index = 0;
    while index < clients.len() {
        let mut remove_client = false;
        loop {
            match clients[index].read() {
                Ok(Message::Text(text)) => match serde_json::from_str::<CompanionPacket>(&text) {
                    Ok(packet) if packet.v == VERSION => {
                        send_auto_reply(&mut clients[index], &packet, token, face_catalog);
                        let _ = event_tx.send(CompanionEvent::PacketReceived(packet));
                    }
                    Ok(packet) => {
                        let _ = event_tx.send(CompanionEvent::Error(format!(
                            "Unsupported companion protocol: {}",
                            packet.v
                        )));
                    }
                    Err(error) => {
                        let _ = event_tx.send(CompanionEvent::Error(format!(
                            "Invalid companion packet: {error}"
                        )));
                    }
                },
                Ok(Message::Ping(bytes)) => {
                    let _ = clients[index].send(Message::Pong(bytes));
                }
                Ok(Message::Close(_)) => {
                    remove_client = true;
                    break;
                }
                Ok(_) => {}
                Err(tungstenite::Error::Io(error)) if error.kind() == ErrorKind::WouldBlock => {
                    break;
                }
                Err(tungstenite::Error::ConnectionClosed)
                | Err(tungstenite::Error::AlreadyClosed) => {
                    remove_client = true;
                    break;
                }
                Err(error) => {
                    let _ = event_tx.send(CompanionEvent::Error(format!(
                        "Companion bridge client read failed: {error}"
                    )));
                    remove_client = true;
                    break;
                }
            }
        }

        if remove_client {
            clients.remove(index);
            let _ = event_tx.send(CompanionEvent::ClientDisconnected);
        } else {
            index += 1;
        }
    }
}

fn send_auto_reply(
    client: &mut tungstenite::WebSocket<TcpStream>,
    packet: &CompanionPacket,
    token: &str,
    face_catalog: &mut FaceCatalogCache,
) {
    let reply_op = match packet.op.as_str() {
        "hello" => "hello.ok",
        "ping" => "pong",
        "status.get" => "status.ok",
        "perf.snapshot" => "perf.snapshot.ok",
        "face.catalog.refresh" => "face.catalog.refresh.ok",
        "face.catalog.page" => "face.catalog.page.ok",
        "ik.cache.store" => "ik.cache.store.ok",
        "ik.cache.load" => "ik.cache.load.ok",
        "ik.solve.generic" => "ik.solve.generic.ok",
        _ => "ack",
    };
    let payload = match packet.op.as_str() {
        "hello" | "status.get" => capabilities_payload(),
        "ping" => json!({ "ok": true, "now": now_seconds() }),
        "perf.snapshot" => handle_perf_snapshot(&packet.payload),
        "face.catalog.refresh" => handle_face_catalog_refresh(&packet.payload, face_catalog),
        "face.catalog.page" => handle_face_catalog_page(&packet.payload, face_catalog),
        "ik.cache.store" => handle_ik_cache_store(&packet.payload),
        "ik.cache.load" => handle_ik_cache_load(&packet.payload),
        "ik.solve.generic" => handle_ik_solve_generic(&packet.payload),
        _ => json!({ "ok": true, "op": packet.op }),
    };
    let reply = make_packet(reply_op, payload, token, Some(packet.id.clone()));
    if let Ok(encoded) = serde_json::to_string(&reply) {
        let _ = client.send(Message::Text(encoded));
    }
}

fn send_to_clients(
    clients: &mut Vec<tungstenite::WebSocket<TcpStream>>,
    event_tx: &Sender<CompanionEvent>,
    op: String,
    encoded: String,
) {
    let mut index = 0;
    while index < clients.len() {
        match clients[index].send(Message::Text(encoded.clone())) {
            Ok(_) => {
                let _ = event_tx.send(CompanionEvent::PacketSent { op: op.clone() });
                index += 1;
            }
            Err(tungstenite::Error::Io(error)) if error.kind() == ErrorKind::WouldBlock => {
                let _ = event_tx.send(CompanionEvent::SendFailed {
                    op: op.clone(),
                    message: "Studio connection is busy; try again.".to_owned(),
                });
                index += 1;
            }
            Err(error) => {
                clients.remove(index);
                let _ = event_tx.send(CompanionEvent::ClientDisconnected);
                let _ = event_tx.send(CompanionEvent::SendFailed {
                    op: op.clone(),
                    message: format!("Studio send failed: {error}"),
                });
            }
        }
    }
}

fn companion_cache_dir() -> PathBuf {
    std::env::temp_dir().join("PhaseCompanion")
}

fn face_catalog_cache_path() -> PathBuf {
    companion_cache_dir().join("face-catalog-cache.json")
}

fn ik_cache_dir() -> PathBuf {
    companion_cache_dir().join("ik-playback-cache")
}

fn ik_cache_path(signature: &str) -> PathBuf {
    let mut hasher = Sha256::new();
    hasher.update(signature.as_bytes());
    let digest = hasher.finalize();
    let hash = digest
        .iter()
        .take(16)
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    ik_cache_dir().join(format!("{hash}.json"))
}

fn load_face_catalog_cache() -> FaceCatalogCache {
    let path = face_catalog_cache_path();
    let Ok(text) = fs::read_to_string(path) else {
        return FaceCatalogCache::default();
    };
    let Ok(value) = serde_json::from_str::<Value>(&text) else {
        return FaceCatalogCache::default();
    };
    FaceCatalogCache {
        version: value
            .get("catalogVersion")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned(),
        items: value
            .get("items")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
        refreshed_at: value
            .get("refreshedAt")
            .and_then(Value::as_f64)
            .unwrap_or_default(),
    }
}

fn store_face_catalog_cache(cache: &FaceCatalogCache) -> Result<(), String> {
    let dir = companion_cache_dir();
    fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    let payload = json!({
        "schema": 1,
        "catalogVersion": cache.version,
        "refreshedAt": cache.refreshed_at,
        "items": cache.items,
    });
    let bytes = serde_json::to_vec_pretty(&payload).map_err(|error| error.to_string())?;
    fs::write(face_catalog_cache_path(), bytes).map_err(|error| error.to_string())
}

fn snapshot_findings_count(payload: &Value) -> usize {
    payload
        .get("Findings")
        .and_then(Value::as_array)
        .or_else(|| payload.get("findings").and_then(Value::as_array))
        .map(|items| items.len())
        .unwrap_or(0)
}

fn snapshot_overview(payload: &Value) -> Value {
    payload
        .get("Stats")
        .and_then(|stats| stats.get("Overview"))
        .or_else(|| payload.get("stats").and_then(|stats| stats.get("overview")))
        .cloned()
        .unwrap_or_else(|| json!({}))
}

fn value_text(value: &Value, keys: &[&str]) -> String {
    for key in keys {
        if let Some(text) = value.get(*key).and_then(Value::as_str) {
            return text.to_owned();
        }
    }
    String::new()
}

fn value_bool(value: &Value, keys: &[&str]) -> bool {
    for key in keys {
        if let Some(flag) = value.get(*key).and_then(Value::as_bool) {
            return flag;
        }
    }
    false
}

fn face_item_category(item: &Value) -> String {
    value_text(item, &["Category", "category", "PartFilter", "partFilter"])
}

fn face_item_group(item: &Value) -> String {
    value_text(item, &["GroupId", "groupId"])
}

fn face_item_part_kind(item: &Value) -> String {
    let source = format!(
        "{} {}",
        value_text(item, &["EntryId", "entryId", "id"]),
        value_text(item, &["Name", "name"])
    )
    .to_lowercase();
    if source.contains("teeth") || source.contains("tooth") || source.contains("bite") {
        return "teeth".to_owned();
    }
    if source.contains("speech")
        || source.contains("phf")
        || source.contains("fb")
        || source.contains("chs")
        || source.contains("aei")
    {
        return "mouth-speech".to_owned();
    }
    if source.contains("mouth")
        || source.contains("mou")
        || source.contains("tongue")
        || source.contains("smile")
    {
        return "mouth-idle".to_owned();
    }
    if source.contains("leftbrow") || source.contains("ebl") || source.contains("elb") {
        return "brow-left".to_owned();
    }
    if source.contains("rightbrow") || source.contains("ebr") || source.contains("erb") {
        return "brow-right".to_owned();
    }
    if source.contains("brow") {
        return "brows".to_owned();
    }
    if source.contains("pupil") {
        return "pupils".to_owned();
    }
    if source.contains("lash") || source.contains("blank") {
        return "eye-details".to_owned();
    }
    if source.contains("eye") || source.contains("eyes") {
        return "eye-sets".to_owned();
    }
    if source.contains("blush")
        || source.contains("scar")
        || source.contains("freckle")
        || source.contains("misc")
        || source.contains("additional")
    {
        return "extras".to_owned();
    }
    "full".to_owned()
}

fn face_item_matches_filter(item: &Value, filter: &str) -> bool {
    if filter == "all" {
        return true;
    }
    let kind = face_item_part_kind(item);
    if kind == filter {
        return true;
    }
    match filter {
        "eyes" => kind == "eye-sets" || kind == "pupils" || kind == "eye-details",
        "brows" => kind == "brow-left" || kind == "brow-right" || kind == "brows",
        "mouth" => kind == "mouth-speech" || kind == "mouth-idle" || kind == "teeth",
        _ => false,
    }
}

fn face_item_matches(item: &Value, query: &str, filter: &str, group_id: &str) -> bool {
    if group_id == "favorites" && !value_bool(item, &["Favorite", "favorite"]) {
        return false;
    }
    if group_id == "ungrouped" && !face_item_group(item).trim().is_empty() {
        return false;
    }
    if group_id != "all"
        && group_id != "favorites"
        && group_id != "ungrouped"
        && face_item_group(item) != group_id
    {
        return false;
    }

    if !face_item_matches_filter(item, filter) {
        return false;
    }

    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return true;
    }
    let haystack = [
        value_text(item, &["EntryId", "entryId", "id"]),
        value_text(item, &["Name", "name"]),
        value_text(item, &["Image", "image", "assetId"]),
        face_item_category(item),
    ]
    .join(" ")
    .to_lowercase();
    haystack.contains(&query)
}

fn handle_face_catalog_refresh(payload: &Value, cache: &mut FaceCatalogCache) -> Value {
    let items = payload
        .get("items")
        .or_else(|| payload.get("Items"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    cache.version = value_text(payload, &["catalogVersion", "CatalogVersion"]);
    cache.items = items;
    cache.refreshed_at = now_seconds();
    let store_error = store_face_catalog_cache(cache).err().unwrap_or_default();
    json!({
        "ok": store_error.is_empty(),
        "schema": 1,
        "catalogVersion": cache.version,
        "total": cache.items.len(),
        "stored": store_error.is_empty(),
        "store_error": store_error,
        "stored_path": face_catalog_cache_path().to_string_lossy(),
        "refreshed_at": cache.refreshed_at,
    })
}

fn handle_face_catalog_page(payload: &Value, cache: &FaceCatalogCache) -> Value {
    let query = value_text(payload, &["query", "Query"]);
    let filter = {
        let text = value_text(payload, &["filter", "Filter"]);
        if text.trim().is_empty() {
            "all".to_owned()
        } else {
            text
        }
    };
    let group_id = {
        let text = value_text(payload, &["groupId", "GroupId"]);
        if text.trim().is_empty() {
            "all".to_owned()
        } else {
            text
        }
    };
    let offset = payload
        .get("offset")
        .or_else(|| payload.get("Offset"))
        .and_then(Value::as_u64)
        .unwrap_or(0) as usize;
    let limit = payload
        .get("limit")
        .or_else(|| payload.get("Limit"))
        .and_then(Value::as_u64)
        .unwrap_or(48)
        .clamp(1, 256) as usize;

    let matched = cache
        .items
        .iter()
        .filter(|item| face_item_matches(item, &query, &filter, &group_id))
        .cloned()
        .collect::<Vec<_>>();
    let page = matched
        .iter()
        .skip(offset)
        .take(limit)
        .cloned()
        .collect::<Vec<_>>();

    json!({
        "ok": true,
        "schema": 1,
        "catalogVersion": cache.version,
        "signature": format!("{}:{}:{}:{}", cache.version, query, filter, group_id),
        "offset": offset,
        "limit": limit,
        "total": matched.len(),
        "items": page,
        "cacheItems": cache.items.len(),
        "refreshed_at": cache.refreshed_at,
    })
}

fn ik_cache_frame_count(cache: &Value) -> usize {
    cache
        .get("FrameCount")
        .or_else(|| cache.get("frameCount"))
        .and_then(Value::as_u64)
        .or_else(|| {
            cache
                .get("Frames")
                .or_else(|| cache.get("frames"))
                .and_then(Value::as_array)
                .map(|items| items.len() as u64)
        })
        .unwrap_or(0) as usize
}

fn handle_ik_cache_store(payload: &Value) -> Value {
    let signature = value_text(payload, &["signature", "Signature"]);
    if signature.trim().is_empty() {
        return json!({
            "ok": false,
            "message": "Missing IK cache signature.",
        });
    }
    let cache = payload
        .get("cache")
        .or_else(|| payload.get("Cache"))
        .cloned()
        .unwrap_or_else(|| json!({}));
    if !cache.is_object() {
        return json!({
            "ok": false,
            "message": "Missing IK cache payload.",
        });
    }
    let dir = ik_cache_dir();
    let path = ik_cache_path(&signature);
    let mut stored = false;
    let mut store_error = String::new();
    if let Err(error) = fs::create_dir_all(&dir) {
        store_error = error.to_string();
    } else {
        let envelope = json!({
            "schema": 1,
            "signature": signature,
            "stored_at": now_seconds(),
            "metadata": payload.get("metadata").or_else(|| payload.get("Metadata")).cloned().unwrap_or_else(|| json!({})),
            "cache": cache,
        });
        match serde_json::to_vec(&envelope) {
            Ok(bytes) => match fs::write(&path, bytes) {
                Ok(_) => stored = true,
                Err(error) => store_error = error.to_string(),
            },
            Err(error) => store_error = error.to_string(),
        }
    }
    let frame_count = ik_cache_frame_count(
        payload
            .get("cache")
            .or_else(|| payload.get("Cache"))
            .unwrap_or(&Value::Null),
    );
    json!({
        "ok": stored,
        "schema": 1,
        "signature": signature,
        "stored": stored,
        "store_error": store_error,
        "stored_path": path.to_string_lossy(),
        "frame_count": frame_count,
    })
}

fn handle_ik_cache_load(payload: &Value) -> Value {
    let signature = value_text(payload, &["signature", "Signature"]);
    if signature.trim().is_empty() {
        return json!({
            "ok": false,
            "found": false,
            "message": "Missing IK cache signature.",
        });
    }
    let path = ik_cache_path(&signature);
    let Ok(text) = fs::read_to_string(&path) else {
        return json!({
            "ok": true,
            "found": false,
            "signature": signature,
        });
    };
    let Ok(envelope) = serde_json::from_str::<Value>(&text) else {
        return json!({
            "ok": false,
            "found": false,
            "signature": signature,
            "message": "Stored IK cache was not valid JSON.",
        });
    };
    let cache = envelope
        .get("cache")
        .or_else(|| envelope.get("Cache"))
        .cloned()
        .unwrap_or_else(|| json!({}));
    json!({
        "ok": true,
        "found": true,
        "schema": 1,
        "signature": signature,
        "cache": cache,
        "stored_at": envelope.get("stored_at").or_else(|| envelope.get("storedAt")).cloned().unwrap_or(Value::Null),
        "frame_count": ik_cache_frame_count(&cache),
    })
}

#[derive(Clone, Copy, Debug, Default)]
struct Vec3 {
    x: f64,
    y: f64,
    z: f64,
}

impl Vec3 {
    fn new(x: f64, y: f64, z: f64) -> Self {
        Self { x, y, z }
    }

    fn dot(self, other: Self) -> f64 {
        self.x * other.x + self.y * other.y + self.z * other.z
    }

    fn cross(self, other: Self) -> Self {
        Self::new(
            self.y * other.z - self.z * other.y,
            self.z * other.x - self.x * other.z,
            self.x * other.y - self.y * other.x,
        )
    }

    fn magnitude(self) -> f64 {
        self.dot(self).sqrt()
    }

    fn unit(self, fallback: Self) -> Self {
        let length = self.magnitude();
        if length > 1e-8 {
            self / length
        } else {
            let fallback_length = fallback.magnitude();
            if fallback_length > 1e-8 {
                fallback / fallback_length
            } else {
                Self::new(0.0, 1.0, 0.0)
            }
        }
    }

    fn lerp(self, other: Self, alpha: f64) -> Self {
        self + (other - self) * alpha.clamp(0.0, 1.0)
    }

    fn to_json(self) -> Value {
        json!([self.x, self.y, self.z])
    }
}

impl std::ops::Add for Vec3 {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        Self::new(self.x + rhs.x, self.y + rhs.y, self.z + rhs.z)
    }
}

impl std::ops::Sub for Vec3 {
    type Output = Self;

    fn sub(self, rhs: Self) -> Self::Output {
        Self::new(self.x - rhs.x, self.y - rhs.y, self.z - rhs.z)
    }
}

impl std::ops::Mul<f64> for Vec3 {
    type Output = Self;

    fn mul(self, rhs: f64) -> Self::Output {
        Self::new(self.x * rhs, self.y * rhs, self.z * rhs)
    }
}

impl std::ops::Div<f64> for Vec3 {
    type Output = Self;

    fn div(self, rhs: f64) -> Self::Output {
        Self::new(self.x / rhs, self.y / rhs, self.z / rhs)
    }
}

fn value_vec3(value: &Value) -> Option<Vec3> {
    let items = value.as_array()?;
    if items.len() < 3 {
        return None;
    }
    Some(Vec3::new(
        items.first()?.as_f64()?,
        items.get(1)?.as_f64()?,
        items.get(2)?.as_f64()?,
    ))
}

fn perpendicular_to(vector: Vec3) -> Vec3 {
    let direction = vector.unit(Vec3::new(0.0, 1.0, 0.0));
    let axis = if direction.dot(Vec3::new(0.0, 1.0, 0.0)).abs() < 0.92 {
        Vec3::new(0.0, 1.0, 0.0)
    } else {
        Vec3::new(1.0, 0.0, 0.0)
    };
    let perpendicular = direction.cross(axis);
    if perpendicular.magnitude() > 1e-8 {
        perpendicular.unit(Vec3::new(1.0, 0.0, 0.0))
    } else {
        direction
            .cross(Vec3::new(0.0, 0.0, 1.0))
            .unit(Vec3::new(1.0, 0.0, 0.0))
    }
}

fn rotate_vector_axis_angle(value: Vec3, axis: Vec3, angle: f64) -> Vec3 {
    let axis = axis.unit(Vec3::new(0.0, 1.0, 0.0));
    let cos = angle.cos();
    let sin = angle.sin();
    value * cos + axis.cross(value) * sin + axis * (axis.dot(value) * (1.0 - cos))
}

fn rotation_between_apply(from: Vec3, to: Vec3, value: Vec3, max_angle: Option<f64>) -> Vec3 {
    let from_unit = from.unit(Vec3::new(0.0, 1.0, 0.0));
    let to_unit = to.unit(from_unit);
    let dot = from_unit.dot(to_unit).clamp(-1.0, 1.0);
    if dot > 0.9999 {
        return value;
    }
    let mut axis = from_unit.cross(to_unit);
    if axis.magnitude() <= 1e-8 {
        axis = perpendicular_to(from_unit);
    } else {
        axis = axis.unit(Vec3::new(1.0, 0.0, 0.0));
    }
    let mut angle = dot.acos();
    if let Some(limit) = max_angle {
        if limit > 0.0 {
            angle = angle.min(limit);
        }
    }
    rotate_vector_axis_angle(value, axis, angle)
}

fn segment_lengths(positions: &[Vec3]) -> (Vec<f64>, f64) {
    let mut lengths = Vec::new();
    let mut total = 0.0;
    for pair in positions.windows(2) {
        let length = (pair[1] - pair[0]).magnitude().max(1e-8);
        lengths.push(length);
        total += length;
    }
    (lengths, total)
}

fn pole_side(
    root: Vec3,
    target: Vec3,
    current_mid: Vec3,
    pole: Option<Vec3>,
    fallback: Vec3,
) -> Vec3 {
    let direction = (target - root).unit(fallback);
    let mut side = pole.map(|value| value - root).unwrap_or(current_mid - root);
    side = side - direction * side.dot(direction);
    if side.magnitude() <= 1e-8 {
        side = (current_mid - root) - direction * (current_mid - root).dot(direction);
    }
    if side.magnitude() <= 1e-8 {
        side = perpendicular_to(direction);
    }
    side.unit(perpendicular_to(direction))
}

fn solve_distance_soft(distance: f64, max_reach: f64, soft_enabled: bool, soft_amount: f64) -> f64 {
    let amount = if soft_enabled {
        soft_amount.clamp(0.0, 1.0)
    } else {
        0.0
    };
    if amount <= 1e-8 || distance <= max_reach {
        return distance.min(max_reach);
    }
    let soft_start = (max_reach * (1.0 - amount)).max(0.0);
    let softness = (max_reach - soft_start).max(1e-8);
    soft_start + softness * (1.0 - (-(distance - soft_start) / softness).exp())
}

fn solve_two_bone_positions(
    positions: &[Vec3],
    target: Vec3,
    pole: Option<Vec3>,
    options: &Value,
) -> (Vec<Vec3>, Value) {
    if positions.len() < 3 {
        return (
            positions.to_vec(),
            json!({"warning": "Two-bone solve requires at least two segments."}),
        );
    }
    let root = positions[0];
    let mid = positions[1];
    let end_position = *positions.last().unwrap_or(&mid);
    let upper_length = (mid - root).magnitude().max(1e-8);
    let lower_length = positions[1..]
        .windows(2)
        .map(|pair| (pair[1] - pair[0]).magnitude().max(1e-8))
        .sum::<f64>();
    let target_vector = target - root;
    let current_vector = end_position - root;
    let distance = target_vector.magnitude();
    let direction = target_vector.unit(current_vector);
    let min_reach = (upper_length - lower_length).abs().max(0.001) + 0.001;
    let max_reach = min_reach.max(upper_length + lower_length - 0.001);
    let stretch = value_bool(options, &["StretchEnabled", "stretch"]);
    let soft = value_bool(options, &["SoftIKEnabled", "softIK"]);
    let soft_amount = options
        .get("SoftIKAmount")
        .or_else(|| options.get("softIKAmount"))
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let solved_distance = if stretch {
        distance.max(min_reach)
    } else {
        solve_distance_soft(
            distance.clamp(min_reach, distance),
            max_reach,
            soft,
            soft_amount,
        )
        .clamp(min_reach, max_reach)
    };
    let pole_angle = options
        .get("PoleAngle")
        .or_else(|| options.get("poleAngle"))
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let mut side = pole_side(
        root,
        root + direction * solved_distance,
        mid,
        pole,
        current_vector,
    );
    if pole_angle.abs() > 1e-8 {
        side = rotate_vector_axis_angle(side, direction, pole_angle);
    }
    let x = ((upper_length * upper_length + solved_distance * solved_distance
        - lower_length * lower_length)
        / (2.0 * solved_distance.max(1e-8)))
    .clamp(0.0, upper_length);
    let y = (upper_length * upper_length - x * x).max(0.0).sqrt();
    let solved_mid = root + direction * x + side * y;
    let mut solved_end = root + direction * solved_distance;
    if stretch && distance > max_reach {
        solved_end = target;
    }
    let mut result = positions.to_vec();
    result[0] = root;
    result[1] = solved_mid;
    if positions.len() == 3 {
        result[2] = solved_end;
    } else {
        let lower_direction =
            (solved_end - solved_mid).unit(positions[positions.len() - 1] - positions[1]);
        let mut cursor = solved_mid;
        for index in 2..positions.len() {
            let segment = (positions[index] - positions[index - 1]).magnitude();
            if index == positions.len() - 1 {
                result[index] = solved_end;
            } else {
                cursor = cursor + lower_direction * segment;
                result[index] = cursor;
            }
        }
    }
    (
        result,
        json!({
            "unreachable": distance > max_reach + 1e-8,
            "stretch_amount": if stretch { (distance - max_reach).max(0.0) } else { 0.0 },
            "iterations": 1
        }),
    )
}

fn solve_fabrik_positions(
    positions: &[Vec3],
    target: Vec3,
    pole: Option<Vec3>,
    options: &Value,
) -> (Vec<Vec3>, Value) {
    let mut solved = positions.to_vec();
    if solved.len() < 2 {
        return (solved, json!({"iterations": 0}));
    }
    let (lengths, total_length) = segment_lengths(positions);
    let root = solved[0];
    let tolerance = options
        .get("Tolerance")
        .or_else(|| options.get("tolerance"))
        .and_then(Value::as_f64)
        .unwrap_or(0.002)
        .max(1e-8);
    let iterations = options
        .get("Iterations")
        .or_else(|| options.get("iterations"))
        .and_then(Value::as_u64)
        .unwrap_or(16)
        .clamp(1, 128) as usize;
    let target_distance = (target - root).magnitude();
    let stretch = value_bool(options, &["StretchEnabled", "stretch"]);
    let unreachable = target_distance > total_length + 1e-8;
    let mut used_iterations = iterations;
    if unreachable && !stretch {
        let direction = (target - root).unit(*positions.last().unwrap_or(&root) - root);
        for index in 1..solved.len() {
            solved[index] = solved[index - 1] + direction * lengths[index - 1];
        }
    } else {
        for iteration in 1..=iterations {
            let last = solved.len() - 1;
            solved[last] = target;
            for index in (0..last).rev() {
                let direction = (solved[index] - solved[index + 1])
                    .unit(positions[index] - positions[index + 1]);
                solved[index] = solved[index + 1] + direction * lengths[index];
            }
            solved[0] = root;
            for index in 1..solved.len() {
                let direction = (solved[index] - solved[index - 1])
                    .unit(positions[index] - positions[index - 1]);
                solved[index] = solved[index - 1] + direction * lengths[index - 1];
            }
            if (solved[last] - target).magnitude() <= tolerance {
                used_iterations = iteration;
                break;
            }
        }
    }
    if let Some(pole_value) = pole {
        if solved.len() >= 3 {
            let direction = (solved[solved.len() - 1] - solved[0])
                .unit(positions[positions.len() - 1] - positions[0]);
            let side = pole_side(
                solved[0],
                solved[solved.len() - 1],
                solved[1],
                Some(pole_value),
                direction,
            );
            let pole_weight = options
                .get("PoleWeight")
                .or_else(|| options.get("poleWeight"))
                .and_then(Value::as_f64)
                .unwrap_or(1.0)
                .clamp(0.0, 1.0);
            for index in 1..solved.len() - 1 {
                let along = solved[0] + direction * (solved[index] - solved[0]).dot(direction);
                let distance_from_axis = (solved[index] - along).magnitude();
                if distance_from_axis > 1e-8 {
                    solved[index] =
                        solved[index].lerp(along + side * distance_from_axis, pole_weight);
                }
            }
        }
    }
    (
        solved,
        json!({
            "iterations": used_iterations,
            "unreachable": unreachable,
            "stretch_amount": if stretch { (target_distance - total_length).max(0.0) } else { 0.0 },
        }),
    )
}

fn solve_ccd_positions(
    positions: &[Vec3],
    target: Vec3,
    pole: Option<Vec3>,
    options: &Value,
) -> (Vec<Vec3>, Value) {
    let mut solved = positions.to_vec();
    if solved.len() < 2 {
        return (solved, json!({"iterations": 0}));
    }
    let tolerance = options
        .get("Tolerance")
        .or_else(|| options.get("tolerance"))
        .and_then(Value::as_f64)
        .unwrap_or(0.002)
        .max(1e-8);
    let iterations = options
        .get("Iterations")
        .or_else(|| options.get("iterations"))
        .and_then(Value::as_u64)
        .unwrap_or(16)
        .clamp(1, 128) as usize;
    let mut max_angle = options
        .get("MaxAnglePerJoint")
        .or_else(|| options.get("maxAnglePerJoint"))
        .and_then(Value::as_f64);
    if let Some(angle) = max_angle {
        if angle > std::f64::consts::PI * 2.0 {
            max_angle = Some(angle.to_radians());
        }
    }
    let mut used_iterations = iterations;
    for iteration in 1..=iterations {
        for index in (0..solved.len() - 1).rev() {
            let pivot = solved[index];
            let end = *solved.last().unwrap_or(&pivot);
            let to_end = end - pivot;
            let to_target = target - pivot;
            if to_end.magnitude() > 1e-8 && to_target.magnitude() > 1e-8 {
                for item in solved.iter_mut().skip(index + 1) {
                    *item =
                        pivot + rotation_between_apply(to_end, to_target, *item - pivot, max_angle);
                }
            }
        }
        if (*solved.last().unwrap_or(&target) - target).magnitude() <= tolerance {
            used_iterations = iteration;
            break;
        }
    }
    if let Some(pole_value) = pole {
        if solved.len() >= 3 {
            let root = solved[0];
            let direction = (*solved.last().unwrap_or(&root) - root)
                .unit(*positions.last().unwrap_or(&root) - positions[0]);
            let side = pole_side(
                root,
                *solved.last().unwrap_or(&root),
                solved[1],
                Some(pole_value),
                direction,
            );
            let current_side = (solved[1] - root) - direction * (solved[1] - root).dot(direction);
            if current_side.magnitude() > 1e-8 {
                for item in solved.iter_mut().skip(1) {
                    *item = root
                        + rotation_between_apply(
                            current_side,
                            side * current_side.magnitude(),
                            *item - root,
                            None,
                        );
                }
            }
        }
    }
    (
        solved,
        json!({
            "iterations": used_iterations,
            "unreachable": false,
            "stretch_amount": 0.0,
        }),
    )
}

fn handle_ik_solve_generic(payload: &Value) -> Value {
    let positions = payload
        .get("Positions")
        .or_else(|| payload.get("positions"))
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(value_vec3).collect::<Vec<_>>())
        .unwrap_or_default();
    let target = payload
        .get("Target")
        .or_else(|| payload.get("target"))
        .and_then(value_vec3);
    if positions.len() < 2 || target.is_none() {
        return json!({
            "ok": false,
            "success": false,
            "message": "Missing generic IK positions or target.",
        });
    }
    let target = target.unwrap();
    let pole = payload
        .get("Pole")
        .or_else(|| payload.get("pole"))
        .and_then(value_vec3);
    let options = payload
        .get("Options")
        .or_else(|| payload.get("options"))
        .unwrap_or(&Value::Null);
    let requested = value_text(payload, &["SolverType", "solverType"]).to_lowercase();
    let fallback = {
        let text = value_text(payload, &["FallbackSolver", "fallbackSolver"]);
        if text.trim().is_empty() {
            "ccd".to_owned()
        } else {
            text.to_lowercase()
        }
    };
    let joint_count = positions.len().saturating_sub(1);
    let (solved, meta, solver_type, fallback_used) = if requested == "two-bone"
        || requested == "twobone"
        || (requested == "auto" && (2..=3).contains(&joint_count))
    {
        let (solved, meta) = solve_two_bone_positions(&positions, target, pole, options);
        (solved, meta, "two-bone", false)
    } else if requested == "fabrik" || requested == "single" || requested == "auto" {
        let (solved, meta) = solve_fabrik_positions(&positions, target, pole, options);
        (solved, meta, "FABRIK", false)
    } else if requested == "ccd" {
        let (solved, meta) = solve_ccd_positions(&positions, target, pole, options);
        (solved, meta, "CCD", false)
    } else if fallback == "fabrik" {
        let (solved, meta) = solve_fabrik_positions(&positions, target, pole, options);
        (solved, meta, "FABRIK", true)
    } else {
        let (solved, meta) = solve_ccd_positions(&positions, target, pole, options);
        (solved, meta, "CCD", true)
    };
    let error_distance = solved
        .last()
        .map(|end| (*end - target).magnitude())
        .unwrap_or_default();
    json!({
        "ok": true,
        "success": true,
        "schema": 1,
        "solver_type": solver_type,
        "fallback_used": fallback_used,
        "joint_count": joint_count,
        "positions": solved.into_iter().map(Vec3::to_json).collect::<Vec<_>>(),
        "iterations": meta.get("iterations").and_then(Value::as_u64).unwrap_or(1),
        "unreachable": meta.get("unreachable").and_then(Value::as_bool).unwrap_or(false),
        "stretch_amount": meta.get("stretch_amount").and_then(Value::as_f64).unwrap_or(0.0),
        "error_distance": error_distance,
        "warnings": [],
    })
}

fn handle_perf_snapshot(payload: &Value) -> Value {
    let cache_dir = companion_cache_dir();
    let mut stored = false;
    let mut store_error = String::new();
    if let Err(error) = fs::create_dir_all(&cache_dir) {
        store_error = error.to_string();
    } else {
        let path = cache_dir.join("latest-perf-snapshot.json");
        match serde_json::to_vec_pretty(payload) {
            Ok(bytes) => match fs::write(path, bytes) {
                Ok(_) => stored = true,
                Err(error) => store_error = error.to_string(),
            },
            Err(error) => store_error = error.to_string(),
        }
    }

    json!({
        "ok": true,
        "stored": stored,
        "store_error": store_error,
        "stored_path": cache_dir.join("latest-perf-snapshot.json").to_string_lossy(),
        "received_at": now_seconds(),
        "finding_count": snapshot_findings_count(payload),
        "overview": snapshot_overview(payload)
    })
}

fn error_response(status: u16, message: &str) -> ErrorResponse {
    tungstenite::handshake::server::Response::builder()
        .status(status)
        .body(Some(message.to_owned()))
        .unwrap_or_else(|_| {
            tungstenite::handshake::server::Response::builder()
                .status(500)
                .body(Some("Companion bridge error.".to_owned()))
                .expect("fallback response")
        })
}
