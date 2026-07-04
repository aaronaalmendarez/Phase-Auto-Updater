use discord_rich_presence::{DiscordIpc, DiscordIpcClient, activity};
use serde_json::{Value, json};
use std::time::{SystemTime, UNIX_EPOCH};

const DEFAULT_LARGE_IMAGE: &str = "";
const DEFAULT_SMALL_IMAGE: &str = "";
const DEFAULT_CLIENT_ID: &str = "1522757647873343538";
const MAX_TEXT: usize = 120;

pub struct DiscordPresenceManager {
    client: Option<DiscordIpcClient>,
    client_id: String,
    last_signature: String,
    last_error: String,
    connected: bool,
}

impl DiscordPresenceManager {
    pub fn new() -> Self {
        Self {
            client: None,
            client_id: String::new(),
            last_signature: String::new(),
            last_error: String::new(),
            connected: false,
        }
    }

    pub fn update(&mut self, payload: &Value) -> Value {
        if !payload_bool(payload, &["enabled", "Enabled"]) {
            return self.clear();
        }

        let client_id = presence_client_id(payload);
        if client_id.is_empty() {
            self.last_error =
                "Discord presence is missing PHASE_DISCORD_CLIENT_ID or payload.client_id."
                    .to_owned();
            return self.status(false);
        }

        let signature = stable_signature(payload);
        if self.connected && self.last_signature == signature {
            return self.status(true);
        }

        if self.client_id != client_id {
            self.disconnect();
            self.client_id = client_id.clone();
        }

        if let Err(error) = self.ensure_connected(&client_id) {
            self.last_error = error;
            return self.status(false);
        }

        let activity = build_activity(payload);
        let result = self
            .client
            .as_mut()
            .ok_or_else(|| "Discord IPC client is not connected.".to_owned())
            .and_then(|client| {
                client.set_activity(activity).map_err(|error| {
                    self.connected = false;
                    format!("Discord presence update failed: {error}")
                })
            });

        match result {
            Ok(_) => {
                self.last_signature = signature;
                self.last_error.clear();
                self.status(true)
            }
            Err(error) => {
                self.last_error = error;
                self.status(false)
            }
        }
    }

    pub fn clear(&mut self) -> Value {
        if let Some(client) = self.client.as_mut() {
            let _ = client.clear_activity();
        }
        self.last_signature.clear();
        self.status(self.connected)
    }

    pub fn disconnect(&mut self) {
        if let Some(client) = self.client.as_mut() {
            let _ = client.clear_activity();
            let _ = client.close();
        }
        self.client = None;
        self.connected = false;
        self.last_signature.clear();
    }

    fn ensure_connected(&mut self, client_id: &str) -> Result<(), String> {
        if self.connected && self.client.is_some() {
            return Ok(());
        }

        let mut client = DiscordIpcClient::new(client_id);
        client
            .connect()
            .map_err(|error| format!("Discord IPC connection failed: {error}"))?;
        self.client = Some(client);
        self.connected = true;
        Ok(())
    }

    fn status(&self, ok: bool) -> Value {
        json!({
            "ok": ok,
            "connected": self.connected,
            "configured": !self.client_id.trim().is_empty() || !env_client_id().is_empty() || !DEFAULT_CLIENT_ID.is_empty(),
            "last_error": self.last_error,
            "updated_at": now_seconds(),
        })
    }
}

fn env_client_id() -> String {
    std::env::var("PHASE_DISCORD_CLIENT_ID")
        .ok()
        .or_else(|| option_env!("PHASE_DISCORD_CLIENT_ID").map(str::to_owned))
        .unwrap_or_else(|| DEFAULT_CLIENT_ID.to_owned())
        .trim()
        .to_owned()
}

fn presence_client_id(payload: &Value) -> String {
    value_text(payload, &["client_id", "clientId", "ClientId"])
        .or_else(|| {
            let text = env_client_id();
            if text.is_empty() { None } else { Some(text) }
        })
        .unwrap_or_default()
}

fn build_activity(payload: &Value) -> activity::Activity<'static> {
    let document = payload.get("document").unwrap_or(&Value::Null);
    let options = payload.get("options").unwrap_or(&Value::Null);
    let details = value_text(
        document,
        &["details", "animation_name", "animationName", "name"],
    )
    .unwrap_or_else(|| "Animating in Phase Animator".to_owned());
    let state = value_text(document, &["state", "summary"])
        .unwrap_or_else(|| "Creating animation".to_owned());

    let mut presence = activity::Activity::new()
        .details(truncate_text(details, MAX_TEXT))
        .state(truncate_text(state, MAX_TEXT))
        .timestamps(
            activity::Timestamps::new().start(
                payload
                    .get("session_started_at")
                    .or_else(|| payload.get("sessionStartedAt"))
                    .and_then(Value::as_i64)
                    .unwrap_or_else(now_seconds_i64),
            ),
        );

    let large_image = env_or_payload(
        payload,
        &["large_image", "largeImage"],
        "PHASE_DISCORD_LARGE_IMAGE",
        DEFAULT_LARGE_IMAGE,
    );
    let small_image = env_or_payload(
        payload,
        &["small_image", "smallImage"],
        "PHASE_DISCORD_SMALL_IMAGE",
        DEFAULT_SMALL_IMAGE,
    );
    let large_text = value_text(document, &["large_text", "largeText"])
        .unwrap_or_else(|| "Phase Animator".to_owned());
    let small_text = value_text(document, &["small_text", "smallText"])
        .unwrap_or_else(|| "Roblox Studio".to_owned());
    let has_large_image = !large_image.trim().is_empty();
    let has_small_image = !small_image.trim().is_empty();
    if has_large_image || has_small_image {
        let mut assets = activity::Assets::new();
        if has_large_image {
            assets = assets
                .large_image(large_image)
                .large_text(truncate_text(large_text, MAX_TEXT));
        }
        if has_small_image {
            assets = assets
                .small_image(small_image)
                .small_text(truncate_text(small_text, MAX_TEXT));
        }
        presence = presence.assets(assets);
    }

    if payload_bool(options, &["show_buttons", "showButtons"]) {
        presence = presence.buttons(vec![activity::Button::new(
            "Phase Animator",
            "https://phase.motioncore.xyz",
        )]);
    }

    presence
}

fn stable_signature(payload: &Value) -> String {
    let mut clone = payload.clone();
    if let Some(object) = clone.as_object_mut() {
        object.remove("sent_at");
        object.remove("sentAt");
    }
    serde_json::to_string(&clone).unwrap_or_default()
}

fn env_or_payload(payload: &Value, keys: &[&str], env_key: &str, fallback: &str) -> String {
    value_text(payload, keys)
        .or_else(|| std::env::var(env_key).ok())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| fallback.to_owned())
}

fn value_text(value: &Value, keys: &[&str]) -> Option<String> {
    for key in keys {
        let text = value.get(*key).and_then(Value::as_str).unwrap_or("").trim();
        if !text.is_empty() {
            return Some(text.to_owned());
        }
    }
    None
}

fn payload_bool(value: &Value, keys: &[&str]) -> bool {
    for key in keys {
        if let Some(flag) = value.get(*key).and_then(Value::as_bool) {
            return flag;
        }
    }
    false
}

fn truncate_text(value: String, max: usize) -> String {
    let trimmed = value.trim();
    if trimmed.chars().count() <= max {
        return trimmed.to_owned();
    }
    let mut output = trimmed
        .chars()
        .take(max.saturating_sub(1))
        .collect::<String>();
    output.push('…');
    output
}

fn now_seconds_i64() -> i64 {
    now_seconds() as i64
}

fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}
