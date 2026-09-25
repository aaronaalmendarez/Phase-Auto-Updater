use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fs;
use std::io::ErrorKind;
use std::io::Read;
use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tungstenite::handshake::server::ErrorResponse;
use tungstenite::{Message, accept_hdr};
use uuid::Uuid;

pub const VERSION: &str = "phase-video-reference/1";
pub const DEFAULT_PORT: u16 = 27731;
pub const DEFAULT_PATH: &str = "/phase-video-reference";
const POPUP_COMMAND_PATH: &str = "/phase-video-popup-command";
const POPUP_PAGE_PATH: &str = "/phase-video-reference-player";
const YOUTUBE_EMBED_ORIGIN_PLACEHOLDER: &str = "__PHASE_YOUTUBE_ORIGIN__";

#[derive(Clone, Debug, Default)]
pub struct BridgeConfig {
    pub port: u16,
    pub path: String,
    pub token: String,
}

impl BridgeConfig {
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
pub enum BridgeEvent {
    Listening { url: String },
    ClientConnected,
    ClientDisconnected,
    PacketReceived(VideoPacket),
    PacketSent { op: String },
    SendFailed { op: String, message: String },
    Error(String),
    Stopped,
}

#[derive(Debug)]
enum BridgeCommand {
    Send {
        op: String,
        payload: Value,
        reply_to: Option<String>,
    },
    Stop,
}

pub struct VideoReferenceBridge {
    command_tx: Sender<BridgeCommand>,
    event_rx: Receiver<BridgeEvent>,
}

impl VideoReferenceBridge {
    pub fn start(config: BridgeConfig) -> Self {
        let (command_tx, command_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        thread::spawn(move || run_server(config, command_rx, event_tx));
        Self {
            command_tx,
            event_rx,
        }
    }

    pub fn send(&self, op: impl Into<String>, payload: Value, reply_to: Option<String>) {
        let _ = self.command_tx.send(BridgeCommand::Send {
            op: op.into(),
            payload,
            reply_to,
        });
    }

    pub fn poll(&self) -> Vec<BridgeEvent> {
        self.event_rx.try_iter().collect()
    }

    pub fn stop(&self) {
        let _ = self.command_tx.send(BridgeCommand::Stop);
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct VideoPacket {
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReferenceKind {
    Youtube,
    LocalFile,
    External,
}

impl ReferenceKind {
    pub fn as_protocol_str(&self) -> &'static str {
        match self {
            Self::Youtube => "YouTube",
            Self::LocalFile => "LocalFile",
            Self::External => "External",
        }
    }
}

fn media_kind_label(kind: &ReferenceKind) -> &'static str {
    match kind {
        ReferenceKind::Youtube => "YouTube",
        ReferenceKind::LocalFile => "Local file",
        ReferenceKind::External => "External",
    }
}

#[derive(Clone, Debug)]
pub struct ReferenceDraft {
    pub source_kind: ReferenceKind,
    pub source: String,
    pub title: String,
    pub duration_seconds: f64,
    pub fps: f64,
    pub start_frame: i64,
    pub offset_seconds: f64,
    pub playback_rate: f64,
}

impl ReferenceDraft {
    pub fn payload(&self) -> Value {
        json!({
            "source_kind": self.source_kind.as_protocol_str(),
            "source": self.source,
            "title": self.title,
            "duration_seconds": self.duration_seconds.max(0.0),
            "fps": self.fps.max(1.0),
            "start_frame": self.start_frame.max(0),
            "offset_seconds": self.offset_seconds,
            "playback_rate": self.playback_rate.clamp(0.05, 8.0),
        })
    }
}

pub fn open_reference_popup(draft: &ReferenceDraft) -> Result<(), String> {
    let html = render_player_html(draft)?;
    let dir = std::env::temp_dir().join("PhaseAnimatorVideoReference");
    fs::create_dir_all(&dir).map_err(|error| format!("Could not prepare video popup: {error}"))?;
    let html_path = dir.join("player.html");
    fs::write(&html_path, html).map_err(|error| format!("Could not write video popup: {error}"))?;
    append_popup_log(format!(
        "launch requested: kind={} source={} html={}",
        draft.source_kind.as_protocol_str(),
        draft.source,
        html_path.display()
    ));
    open_html_popup(&html_path)
}

pub fn source_kind_for(source: &str) -> ReferenceKind {
    let trimmed = source.trim();
    let lower = trimmed.to_ascii_lowercase();
    if lower.contains("youtube.com/") || lower.contains("youtu.be/") {
        ReferenceKind::Youtube
    } else if lower.ends_with(".mp4")
        || lower.ends_with(".mov")
        || lower.ends_with(".m4v")
        || lower.ends_with(".webm")
        || Path::new(trimmed).exists()
    {
        ReferenceKind::LocalFile
    } else {
        ReferenceKind::External
    }
}

fn render_player_html(draft: &ReferenceDraft) -> Result<String, String> {
    let title = if draft.title.trim().is_empty() {
        default_title_for(&draft.source)
    } else {
        draft.title.trim().to_owned()
    };
    let source = draft.source.trim();
    if source.is_empty() {
        return Err("Choose a YouTube URL or local MP4 first.".to_owned());
    }

    let media = match draft.source_kind {
        ReferenceKind::Youtube => render_youtube_media_html(draft, &title, source)?,
        ReferenceKind::LocalFile => {
            let src = if is_http_url(source) {
                source.to_owned()
            } else {
                file_url(Path::new(source))
            };
            format!(
                r#"<section class="video-wrap">
  <video id="phase-media" src="{src}" playsinline preload="metadata"></video>
  <video id="phase-preview-source" class="preview-source" src="{src}" muted preload="metadata"></video>
  <div class="video-shade"></div>
  <div class="phase-controls" role="group" aria-label="Video controls">
    <div class="scrub-row">
      <span id="phase-current" class="timecode">0:00</span>
      <div class="scrub-field">
        <input id="phase-scrub" type="range" min="0" max="1000" value="0" step="1" aria-label="Scrub video">
        <div id="phase-preview" class="scrub-preview" aria-hidden="true">
          <canvas id="phase-preview-canvas" width="160" height="90"></canvas>
          <span id="phase-preview-time">0:00</span>
        </div>
      </div>
      <span id="phase-duration" class="timecode">0:00</span>
    </div>
    <div class="button-row control-strip">
      <div class="control-group start-group">
        <div id="phase-rate" class="rate-control custom-rate">
          <span>Rate</span>
          <button id="phase-rate-button" class="rate-button" type="button" aria-haspopup="listbox" aria-expanded="false" title="Playback rate">1x</button>
          <div id="phase-rate-menu" class="rate-menu" role="listbox" aria-label="Playback rate">
            <button class="rate-option" type="button" role="option" data-rate="0.25">0.25x</button>
            <button class="rate-option" type="button" role="option" data-rate="0.5">0.5x</button>
            <button class="rate-option selected" type="button" role="option" data-rate="1" aria-selected="true">1x</button>
            <button class="rate-option" type="button" role="option" data-rate="1.5">1.5x</button>
            <button class="rate-option" type="button" role="option" data-rate="2">2x</button>
          </div>
        </div>
        <div id="phase-volume" class="volume-control">
          <button id="phase-mute" class="icon-button volume-mute" type="button" aria-pressed="false" aria-label="Mute" title="Mute"><svg class="icon-sound" viewBox="0 0 24 24" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M4 9.5h3.2L12 5.6v12.8l-4.8-3.9H4z" fill="currentColor" stroke="none"/><path d="M15.5 9.2a4 4 0 0 1 0 5.6"/><path d="M18.2 6.6a7.6 7.6 0 0 1 0 10.8"/></svg><svg class="icon-muted" viewBox="0 0 24 24" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M4 9.5h3.2L12 5.6v12.8l-4.8-3.9H4z" fill="currentColor" stroke="none"/><path d="M16 9.5l5 5"/><path d="M21 9.5l-5 5"/></svg></button>
          <input id="phase-volume-slider" type="range" min="0" max="100" value="100" step="1" aria-label="Volume">
          <span id="phase-volume-value">100%</span>
        </div>
      </div>
      <div class="control-group frame-group">
        <button id="phase-back" class="icon-button control-button frame-button" type="button" aria-label="Back one frame" title="Back one frame"><svg viewBox="0 0 24 24" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M6 5.5v13"/><path d="M18 6.2v11.6a.7.7 0 0 1-1.1.6L9.4 12.6a.7.7 0 0 1 0-1.2l7.5-5.8a.7.7 0 0 1 1.1.6Z" fill="currentColor" stroke="none"/></svg></button>
        <button id="phase-play" class="icon-button control-button primary play-button" type="button" aria-label="Play" title="Play"><svg class="icon-play" viewBox="0 0 24 24" aria-hidden="true" fill="currentColor"><path d="M8 5.2v13.6a.8.8 0 0 0 1.2.7l10.6-6.8a.8.8 0 0 0 0-1.4L9.2 4.5A.8.8 0 0 0 8 5.2Z"/></svg><svg class="icon-pause" viewBox="0 0 24 24" aria-hidden="true" fill="currentColor"><rect x="6.5" y="5" width="4" height="14" rx="1.2"/><rect x="13.5" y="5" width="4" height="14" rx="1.2"/></svg></button>
        <button id="phase-forward" class="icon-button control-button frame-button" type="button" aria-label="Forward one frame" title="Forward one frame"><svg viewBox="0 0 24 24" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M18 5.5v13"/><path d="M6 6.2v11.6a.7.7 0 0 0 1.1.6l7.5-5.8a.7.7 0 0 0 0-1.2L7.1 5.6a.7.7 0 0 0-1.1.6Z" fill="currentColor" stroke="none"/></svg></button>
      </div>
      <div class="control-group utility-group">
        <button id="phase-loop" class="icon-button control-button utility-button" type="button" aria-pressed="false" aria-label="Loop" title="Loop"><svg viewBox="0 0 24 24" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M17 3l3 3-3 3"/><path d="M4 11V9.5A3.5 3.5 0 0 1 7.5 6H20"/><path d="M7 21l-3-3 3-3"/><path d="M20 13v1.5a3.5 3.5 0 0 1-3.5 3.5H4"/></svg></button>
        <button id="phase-fit" class="icon-button control-button utility-button" type="button" aria-label="Fit" title="Fit or fill the window"><svg class="icon-fit" viewBox="0 0 24 24" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="5" width="18" height="14" rx="2.5"/><rect x="7.5" y="8.5" width="9" height="7" rx="1"/></svg><svg class="icon-fill" viewBox="0 0 24 24" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="5" width="18" height="14" rx="2.5"/><rect x="5.5" y="7.5" width="13" height="9" rx="1" fill="currentColor" stroke="none"/></svg></button>
        <button id="phase-fullscreen" class="icon-button control-button utility-button" type="button" aria-label="Full screen" title="Full screen"><svg viewBox="0 0 24 24" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M4 9V5.5A1.5 1.5 0 0 1 5.5 4H9"/><path d="M15 4h3.5A1.5 1.5 0 0 1 20 5.5V9"/><path d="M20 15v3.5a1.5 1.5 0 0 1-1.5 1.5H15"/><path d="M9 20H5.5A1.5 1.5 0 0 1 4 18.5V15"/></svg></button>
      </div>
    </div>
  </div>
</section>
<script>
(() => {{
  const video = document.getElementById('phase-media');
  const play = document.getElementById('phase-play');
  const back = document.getElementById('phase-back');
  const forward = document.getElementById('phase-forward');
  const loop = document.getElementById('phase-loop');
  const mute = document.getElementById('phase-mute');
  const volumeSlider = document.getElementById('phase-volume-slider');
  const volumeValue = document.getElementById('phase-volume-value');
  const fit = document.getElementById('phase-fit');
  const full = document.getElementById('phase-fullscreen');
  const rateButton = document.getElementById('phase-rate-button');
  const rateMenu = document.getElementById('phase-rate-menu');
  const rateOptions = Array.from(document.querySelectorAll('.rate-option'));
  const scrub = document.getElementById('phase-scrub');
  const current = document.getElementById('phase-current');
  const duration = document.getElementById('phase-duration');
  const scrubPreview = document.getElementById('phase-preview');
  const previewCanvas = document.getElementById('phase-preview-canvas');
  const previewTime = document.getElementById('phase-preview-time');
  const previewVideo = document.getElementById('phase-preview-source');
  const headerBar = document.querySelector('header');
  const controls = document.querySelector('.phase-controls');
  const mediaInfo = document.querySelector('.media-info');
  const mediaPopover = document.querySelector('.media-popover');
  const mediaKind = document.getElementById('phase-kind');
  const titleLabel = document.getElementById('phase-title');
  const pathLabel = document.getElementById('phase-path');
  const showFolder = document.getElementById('phase-show-folder');
  const swap = document.getElementById('phase-swap');
  const frameStep = 1 / {fps};
  const bridgeUrl = 'ws://127.0.0.1:{bridge_port}{bridge_path}';
  const sourceFps = {fps};
  const startFrame = {start_frame};
  const offsetSeconds = {offset_seconds};
  const initialPlaybackRate = {playback_rate};
  let referencePayload = {reference_json};
  let currentSource = {source_json};
  let contain = true;
  let bridge;
  let seq = 0;
  let chromeTimer;
  let swapTimer;
  let commandBeacons = [];
  let previewTarget = 0;
  let previewPending = false;
  let suppressLocalSyncUntil = 0;
  let desiredPlaying = false;
  let phaseDrivenPlayback = false;
  let playPromise = null;
  function setChromeVisible(visible) {{
    document.body.classList.toggle('controls-visible', visible);
  }}
  function clearChromeTimer() {{
    if (chromeTimer) clearTimeout(chromeTimer);
    chromeTimer = undefined;
  }}
  function hideChrome() {{
    clearChromeTimer();
    if (controls?.contains(document.activeElement)) return;
    setChromeVisible(false);
  }}
  function scheduleChromeHide() {{
    clearChromeTimer();
    chromeTimer = setTimeout(hideChrome, 1250);
  }}
  function revealChrome() {{
    setChromeVisible(true);
    scheduleChromeHide();
  }}
  function positionMediaPopover() {{
    if (!mediaPopover || !mediaKind) return;
    const margin = window.innerWidth < 420 ? 8 : 10;
    const width = Math.max(168, Math.min(430, window.innerWidth - (margin * 2)));
    const pillRect = mediaKind.getBoundingClientRect();
    const headerRect = headerBar?.getBoundingClientRect();
    const maxLeft = Math.max(margin, window.innerWidth - width - margin);
    const left = Math.max(margin, Math.min(pillRect.left, maxLeft));
    const top = Math.max(margin, (headerRect?.bottom || pillRect.bottom) + 8);
    mediaPopover.style.setProperty('--popover-left', `${{left}}px`);
    mediaPopover.style.setProperty('--popover-top', `${{top}}px`);
    mediaPopover.style.setProperty('--popover-width', `${{width}}px`);
  }}
  function setPopoverPinned(pinned) {{
    positionMediaPopover();
    document.body.classList.toggle('media-popover-pinned', pinned);
    mediaKind?.setAttribute('aria-expanded', String(pinned));
    mediaKind?.setAttribute('aria-pressed', String(pinned));
    mediaKind?.setAttribute(
      'aria-label',
      pinned ? 'Media details pinned open' : 'Open media details'
    );
  }}
  function popupCommand(op, payload) {{
    const url = window.__PHASE_POPUP_COMMAND_URL;
    if (!url) return;
    const params = new URLSearchParams();
    params.set('op', op);
    for (const [key, value] of Object.entries(payload || {{}})) {{
      params.set(key, String(value ?? ''));
    }}
    params.set('_', String(Date.now()));
    const commandUrl = `${{url}}?${{params.toString()}}`;
    try {{
      const beacon = new Image();
      commandBeacons.push(beacon);
      beacon.onload = beacon.onerror = () => {{
        commandBeacons = commandBeacons.filter((item) => item !== beacon);
      }};
      beacon.src = commandUrl;
    }} catch (_) {{
      fetch(commandUrl, {{ method: 'GET', mode: 'no-cors', keepalive: true }}).catch(() => {{}});
    }}
  }}
  function fmt(seconds) {{
    if (!Number.isFinite(seconds)) return '0:00';
    const whole = Math.max(0, Math.floor(seconds));
    const m = Math.floor(whole / 60);
    const s = String(whole % 60).padStart(2, '0');
    return `${{m}}:${{s}}`;
  }}
  function refresh() {{
    play.setAttribute('aria-label', video.paused ? 'Play' : 'Pause');
    play.title = video.paused ? 'Play' : 'Pause';
    play.classList.toggle('is-playing', !video.paused);
    const silent = video.muted || video.volume === 0;
    mute.setAttribute('aria-label', silent ? 'Unmute' : 'Mute');
    mute.title = silent ? 'Unmute' : 'Mute';
    mute.classList.toggle('is-muted', silent);
    paintSlider(volumeSlider);
    mute.setAttribute('aria-pressed', String(video.muted || video.volume === 0));
    if (volumeSlider) volumeSlider.value = String(Math.round(video.volume * 100));
    if (volumeValue) volumeValue.textContent = `${{Math.round(video.volume * 100)}}%`;
    current.textContent = fmt(video.currentTime);
    duration.textContent = fmt(video.duration);
    if (Number.isFinite(video.duration) && video.duration > 0) {{
      scrub.value = String(Math.round((video.currentTime / video.duration) * 1000));
    }}
    paintSlider(scrub);
  }}
  function paintSlider(slider) {{
    if (!slider) return;
    const min = Number(slider.min) || 0;
    const max = Number(slider.max) || 100;
    const pct = ((Number(slider.value) - min) / Math.max(1, max - min)) * 100;
    slider.style.setProperty('--fill', `${{pct}}%`);
  }}
  scrub?.addEventListener('input', () => paintSlider(scrub));
  volumeSlider?.addEventListener('input', () => paintSlider(volumeSlider));
  function connectBridge() {{
    try {{
      bridge = new WebSocket(bridgeUrl);
      bridge.onopen = () => {{
        sendBridge('hello', {{ side: 'phase-video-popup', source: currentSource }});
        sendBridge('reference.set', referencePayload);
      }};
      bridge.onclose = () => setTimeout(connectBridge, 1200);
      bridge.onerror = () => {{}};
      bridge.onmessage = (event) => {{
        try {{
          const packet = JSON.parse(event.data);
          if (packet.op === 'sync.timeline' || packet.op === 'sync.seek') {{
            const payload = packet.payload || {{}};
            const remoteRate = playbackRateFromPayload(payload);
            if (remoteRate !== undefined) setRate(remoteRate, {{ send: false }});
            applyRemoteSeconds(payload.seconds, packet.op === 'sync.seek');
          }}
          if (packet.op === 'sync.playback') {{
            const payload = packet.payload || {{}};
            const remoteRate = playbackRateFromPayload(payload);
            if (remoteRate !== undefined) setRate(remoteRate, {{ send: false }});
            suppressLocalSyncFor(520);
            phaseDrivenPlayback = payload.playing === true;
            if (payload.playing === false) requestPlayback(false);
            applyRemoteSeconds(payload.seconds, true);
            if (payload.playing === true) requestPlayback(true);
          }}
        }} catch (_) {{}}
      }};
    }} catch (_) {{}}
  }}
  function sendBridge(op, payload) {{
    if (!bridge || bridge.readyState !== WebSocket.OPEN) return;
    bridge.send(JSON.stringify({{
      v: 'phase-video-reference/1',
      id: crypto.randomUUID ? crypto.randomUUID() : `popup-${{Date.now()}}-${{++seq}}`,
      op,
      reply_to: null,
      token: '',
      sent_at: Date.now() / 1000,
      payload: payload || {{}}
    }}));
  }}
  function playbackRateFromPayload(payload) {{
    const raw = payload?.playback_rate ?? payload?.PlaybackRate ?? payload?.rate;
    const rate = Number(raw);
    if (!Number.isFinite(rate)) return undefined;
    return Math.max(0.05, Math.min(8, rate));
  }}
  function canSendLocalSync() {{
    return Date.now() >= suppressLocalSyncUntil;
  }}
  function suppressLocalSyncFor(ms) {{
    suppressLocalSyncUntil = Math.max(suppressLocalSyncUntil, Date.now() + ms);
  }}
  function applyRemoteSeconds(seconds, force) {{
    if (typeof seconds !== 'number' || !Number.isFinite(seconds)) return;
    const target = Math.max(0, seconds);
    const currentSeconds = Number.isFinite(video.currentTime) ? video.currentTime : 0;
    const drift = Math.abs(currentSeconds - target);
    const correctionThreshold = video.paused
      ? Math.max(0.045, Math.min(0.12, frameStep * 1.5))
      : Math.max(0.12, Math.min(0.24, frameStep * 12));
    if (force || video.paused || drift > correctionThreshold) {{
      suppressLocalSyncFor(220);
      video.currentTime = target;
    }}
  }}
  function requestPlayback(shouldPlay) {{
    desiredPlaying = shouldPlay === true;
    if (!desiredPlaying) {{
      try {{ video.pause(); }} catch (_) {{}}
      refresh();
      return;
    }}
    try {{
      const result = video.play();
      if (result && typeof result.then === 'function') {{
        playPromise = result
          .catch(() => {{}})
          .then(() => {{
            playPromise = null;
            if (!desiredPlaying) {{
              try {{ video.pause(); }} catch (_) {{}}
            }}
            refresh();
          }});
      }}
    }} catch (_) {{
      playPromise = null;
    }}
  }}
  function timelinePayload(extra) {{
    const seconds = Number.isFinite(video.currentTime) ? video.currentTime : 0;
    return Object.assign({{
      seq: ++seq,
      seconds,
      fps: sourceFps,
      frame: Math.max(0, Math.round(startFrame + ((seconds - offsetSeconds) * sourceFps))),
      playing: !video.paused,
      playback_rate: video.playbackRate
    }}, extra || {{}});
  }}
  function seekTo(seconds) {{
    phaseDrivenPlayback = false;
    const max = Number.isFinite(video.duration) && video.duration > 0 ? video.duration : seconds;
    video.currentTime = Math.max(0, Math.min(max, seconds));
    sendBridge('sync.seek', timelinePayload());
  }}
  function setRateMenuOpen(open) {{
    rateMenu?.classList.toggle('open', open);
    rateButton?.setAttribute('aria-expanded', String(open));
  }}
  function setRate(value, options) {{
    options = options || {{}};
    const nextRate = Math.max(0.05, Math.min(8, Number(value) || 1));
    video.playbackRate = nextRate;
    if (previewVideo) previewVideo.playbackRate = nextRate;
    if (rateButton) rateButton.textContent = `${{nextRate}}x`;
    referencePayload = Object.assign({{}}, referencePayload, {{ playback_rate: nextRate }});
    rateOptions.forEach((option) => {{
      const selected = Number(option.dataset.rate) === nextRate;
      option.classList.toggle('selected', selected);
      option.setAttribute('aria-selected', String(selected));
    }});
    setRateMenuOpen(false);
    if (options.send !== false && canSendLocalSync()) {{
      sendBridge('sync.timeline', timelinePayload({{ reason: 'rate_change', playback_rate: nextRate }}));
      sendBridge('reference.set', referencePayload);
    }}
  }}
  function setVolume(value) {{
    const nextVolume = Math.max(0, Math.min(1, Number(value) || 0));
    video.volume = nextVolume;
    if (nextVolume > 0 && video.muted) video.muted = false;
    refresh();
  }}
  function previewSecondsFromClientX(clientX) {{
    if (!Number.isFinite(video.duration) || video.duration <= 0) return 0;
    const rect = scrub.getBoundingClientRect();
    const ratio = Math.max(0, Math.min(1, (clientX - rect.left) / rect.width));
    return ratio * video.duration;
  }}
  function placePreview(clientX, seconds) {{
    if (!scrubPreview || !previewTime) return;
    const rect = scrub.getBoundingClientRect();
    const ratio = Math.max(0, Math.min(1, (clientX - rect.left) / rect.width));
    scrubPreview.style.left = `${{ratio * 100}}%`;
    previewTime.textContent = fmt(seconds);
    scrubPreview.classList.add('visible');
  }}
  function drawPreviewFrame() {{
    if (!previewCanvas || !previewVideo || previewVideo.readyState < 2) return;
    const context = previewCanvas.getContext('2d');
    if (!context) return;
    context.clearRect(0, 0, previewCanvas.width, previewCanvas.height);
    context.drawImage(previewVideo, 0, 0, previewCanvas.width, previewCanvas.height);
  }}
  function requestPreviewFrame(seconds) {{
    if (!previewVideo || !Number.isFinite(seconds)) return;
    previewTarget = Math.max(0, Math.min(video.duration || seconds, seconds));
    if (previewPending && Math.abs(previewVideo.currentTime - previewTarget) < 0.08) return;
    previewPending = true;
    try {{
      if (Math.abs(previewVideo.currentTime - previewTarget) > 0.05) {{
        previewVideo.currentTime = previewTarget;
      }} else {{
        drawPreviewFrame();
        previewPending = false;
      }}
    }} catch (_) {{
      previewPending = false;
    }}
  }}
  function showScrubPreview(event) {{
    const seconds = previewSecondsFromClientX(event.clientX);
    placePreview(event.clientX, seconds);
    requestPreviewFrame(seconds);
  }}
  function hideScrubPreview() {{
    scrubPreview?.classList.remove('visible');
  }}
  window.phaseVideoReferenceSwap = (next) => {{
    if (swapTimer) clearTimeout(swapTimer);
    swapTimer = undefined;
    document.body.classList.remove('is-swapping');
    revealChrome();
    if (!next || !next.src) return;
    video.pause();
    video.src = next.src;
    if (previewVideo) previewVideo.src = next.src;
    video.load();
    previewVideo?.load();
    currentSource = next.source || next.src;
    referencePayload = Object.assign({{}}, referencePayload, {{
      source_kind: 'LocalFile',
      source: currentSource,
      title: next.title || 'Video Reference',
      duration_seconds: 0
    }});
    if (titleLabel) titleLabel.textContent = referencePayload.title;
    if (pathLabel) pathLabel.textContent = currentSource;
    sendBridge('reference.set', referencePayload);
    refresh();
  }};
  video.addEventListener('loadedmetadata', refresh);
  video.addEventListener('timeupdate', () => {{
    refresh();
    if (!video.paused && !phaseDrivenPlayback && canSendLocalSync()) sendBridge('sync.timeline', timelinePayload());
  }});
  video.addEventListener('play', () => {{
    desiredPlaying = true;
    refresh();
    if (canSendLocalSync()) sendBridge('sync.playback', timelinePayload({{ playing: true }}));
  }});
  video.addEventListener('pause', () => {{
    desiredPlaying = false;
    phaseDrivenPlayback = false;
    refresh();
    if (canSendLocalSync()) sendBridge('sync.playback', timelinePayload({{ playing: false }}));
  }});
  video.addEventListener('error', () => {{
    document.body.classList.add('load-error');
    document.getElementById('phase-error').textContent = 'Could not load this MP4. Check the path or codec.';
  }});
  play.addEventListener('click', () => {{
    phaseDrivenPlayback = false;
    suppressLocalSyncUntil = 0;
    requestPlayback(video.paused && !desiredPlaying);
  }});
  back.addEventListener('click', () => seekTo(video.currentTime - frameStep));
  forward.addEventListener('click', () => seekTo(video.currentTime + frameStep));
  loop.addEventListener('click', () => {{
    video.loop = !video.loop;
    loop.setAttribute('aria-pressed', String(video.loop));
    loop.classList.toggle('active', video.loop);
  }});
  mute.addEventListener('click', () => {{
    if (video.muted || video.volume === 0) {{
      video.muted = false;
      if (video.volume === 0) video.volume = 1;
    }} else {{
      video.muted = true;
    }}
    refresh();
  }});
  volumeSlider?.addEventListener('input', () => {{
    revealChrome();
    setVolume(Number(volumeSlider.value) / 100);
  }});
  previewVideo?.addEventListener('seeked', () => {{
    previewPending = false;
    drawPreviewFrame();
    if (Math.abs(previewVideo.currentTime - previewTarget) > 0.08) {{
      requestPreviewFrame(previewTarget);
    }}
  }});
  rateButton?.addEventListener('click', (event) => {{
    event.stopPropagation();
    revealChrome();
    setRateMenuOpen(!rateMenu?.classList.contains('open'));
  }});
  rateOptions.forEach((option) => option.addEventListener('click', (event) => {{
    event.stopPropagation();
    revealChrome();
    setRate(option.dataset.rate);
  }}));
  fit.addEventListener('click', () => {{
    contain = !contain;
    video.style.objectFit = contain ? 'contain' : 'cover';
    fit.setAttribute('aria-label', contain ? 'Fit' : 'Fill');
    fit.classList.toggle('is-fill', !contain);
  }});
  full.addEventListener('click', () => document.documentElement.requestFullscreen?.());
  showFolder?.addEventListener('click', () => {{
    revealChrome();
    popupCommand('show-folder', {{ source: currentSource }});
  }});
  swap?.addEventListener('click', () => {{
    revealChrome();
    document.body.classList.add('is-swapping');
    popupCommand('swap-video', {{ source: currentSource }});
    if (swapTimer) clearTimeout(swapTimer);
    swapTimer = setTimeout(() => document.body.classList.remove('is-swapping'), 9000);
  }});
  scrub.addEventListener('input', () => {{
    if (Number.isFinite(video.duration) && video.duration > 0) {{
      seekTo((Number(scrub.value) / 1000) * video.duration);
      const rect = scrub.getBoundingClientRect();
      const clientX = rect.left + (Number(scrub.value) / 1000) * rect.width;
      placePreview(clientX, video.currentTime);
      requestPreviewFrame(video.currentTime);
    }}
  }});
  scrub.addEventListener('pointermove', showScrubPreview);
  scrub.addEventListener('pointerdown', showScrubPreview);
  scrub.addEventListener('pointerleave', hideScrubPreview);
  window.addEventListener('keydown', (event) => {{
    revealChrome();
    if (event.code === 'Escape') setPopoverPinned(false);
    if (event.code === 'Space') {{ event.preventDefault(); play.click(); }}
    if (event.code === 'ArrowLeft') {{ event.preventDefault(); back.click(); }}
    if (event.code === 'ArrowRight') {{ event.preventDefault(); forward.click(); }}
  }});
  mediaKind?.addEventListener('click', (event) => {{
    event.stopPropagation();
    revealChrome();
    positionMediaPopover();
    setPopoverPinned(!document.body.classList.contains('media-popover-pinned'));
  }});
  mediaInfo?.addEventListener('pointerenter', positionMediaPopover);
  mediaInfo?.addEventListener('focusin', positionMediaPopover);
  document.addEventListener('click', (event) => {{
    if (!mediaInfo?.contains(event.target)) setPopoverPinned(false);
    if (!rateMenu?.contains(event.target) && event.target !== rateButton) setRateMenuOpen(false);
  }});
  window.addEventListener('resize', positionMediaPopover);
  window.addEventListener('pointerenter', revealChrome);
  window.addEventListener('pointermove', revealChrome);
  window.addEventListener('pointerleave', hideChrome);
  window.addEventListener('blur', hideChrome);
  document.documentElement.addEventListener('mouseleave', hideChrome);
  document.addEventListener('mouseout', (event) => {{
    if (!event.relatedTarget && !event.toElement) hideChrome();
  }});
  document.addEventListener('focusin', revealChrome);
  document.addEventListener('focusout', scheduleChromeHide);
  document.addEventListener('visibilitychange', () => {{
    if (document.hidden) hideChrome();
  }});
  connectBridge();
  setRate(initialPlaybackRate, {{ send: false }});
  setVolume(1);
  refresh();
}})();
</script>"#,
                src = html_escape_attr(&src),
                fps = draft.fps.max(1.0),
                start_frame = draft.start_frame.max(0),
                offset_seconds = draft.offset_seconds,
                playback_rate = draft.playback_rate.clamp(0.05, 8.0),
                bridge_port = DEFAULT_PORT,
                bridge_path = DEFAULT_PATH,
                source_json =
                    serde_json::to_string(&draft.source).unwrap_or_else(|_| "\"\"".to_owned()),
                reference_json =
                    serde_json::to_string(&draft.payload()).unwrap_or_else(|_| "{}".to_owned()),
            )
        }
        ReferenceKind::External => {
            format!(
                r#"<iframe id="phase-media" src="{src}" title="{title}" allowfullscreen></iframe>"#,
                src = html_escape_attr(source),
                title = html_escape_attr(&title),
            )
        }
    };

    let safe_title = html_escape_text(&title);
    let source_display = html_escape_text(source);
    let folder_display = html_escape_text(
        Path::new(source)
            .parent()
            .and_then(|path| path.to_str())
            .unwrap_or("No local folder available."),
    );
    let can_show_folder = draft.source_kind == ReferenceKind::LocalFile
        && !is_http_url(source)
        && Path::new(source).parent().is_some();
    let show_folder_disabled = if can_show_folder { "" } else { " disabled" };
    let can_swap_video = draft.source_kind == ReferenceKind::LocalFile;
    let swap_disabled = if can_swap_video { "" } else { " disabled" };
    let kind_label = media_kind_label(&draft.source_kind);
    Ok(format!(
        r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>{safe_title}</title>
  <style>
    :root {{
      color-scheme: dark;
      --bar: rgba(24, 24, 28, .94);
      --menu: #1d1d22;
      --hairline: rgba(255, 255, 255, .09);
      --text: #f5f5f7;
      --muted: rgba(245, 245, 247, .6);
      --hover: rgba(255, 255, 255, .1);
      --track: rgba(255, 255, 255, .22);
    }}
    * {{
      box-sizing: border-box;
    }}
    html, body {{
      height: 100%;
      margin: 0;
      background: #000;
      color: var(--text);
      font: 13px/1.35 "Segoe UI Variable Text", "Segoe UI", system-ui, sans-serif;
      overflow: hidden;
      -webkit-font-smoothing: antialiased;
      user-select: none;
    }}
    button {{
      font: inherit;
      color: inherit;
    }}
    :focus {{
      outline: none;
    }}
    :focus-visible {{
      outline: 2px solid rgba(255, 255, 255, .85);
      outline-offset: 2px;
    }}
    .shell {{
      height: 100%;
      position: relative;
      background: #000;
    }}
    main,
    .video-wrap {{
      position: absolute;
      inset: 0;
    }}
    #phase-media {{
      display: block;
      width: 100%;
      height: 100%;
      border: 0;
      background: #000;
      object-fit: contain;
    }}
    .preview-source {{
      display: none;
    }}
    body.is-swapping #phase-media {{
      opacity: .35;
      transition: opacity 180ms ease;
    }}

    /* Soft scrims keep the title and bar legible over bright footage. */
    .video-shade {{
      position: absolute;
      inset: 0;
      pointer-events: none;
      opacity: 0;
      transition: opacity 180ms ease;
      background:
        linear-gradient(to bottom, rgba(0, 0, 0, .55), transparent 90px),
        linear-gradient(to top, rgba(0, 0, 0, .5), transparent 140px);
    }}
    body.controls-visible .video-shade {{
      opacity: 1;
    }}

    header {{
      position: absolute;
      top: 0;
      left: 0;
      right: 0;
      z-index: 70;
      display: flex;
      align-items: center;
      gap: 12px;
      padding: 12px 14px;
      opacity: 0;
      pointer-events: none;
      overflow: visible;
      transition: opacity 180ms ease;
    }}
    body.controls-visible header,
    body.media-popover-pinned header,
    .youtube-page header {{
      opacity: 1;
      pointer-events: auto;
    }}
    .top-left {{
      flex: 1;
      min-width: 0;
    }}
    .title {{
      display: block;
      overflow: hidden;
      font-size: 14px;
      font-weight: 600;
      text-overflow: ellipsis;
      white-space: nowrap;
      text-shadow: 0 1px 2px rgba(0, 0, 0, .6);
    }}
    .top-actions {{
      display: flex;
      align-items: center;
      gap: 6px;
    }}

    .icon-button {{
      display: inline-flex;
      align-items: center;
      justify-content: center;
      width: 34px;
      height: 34px;
      padding: 0;
      border: 0;
      border-radius: 8px;
      background: transparent;
      color: var(--text);
      cursor: pointer;
      transition: background 120ms ease, color 120ms ease, transform 120ms ease;
    }}
    .icon-button:hover {{
      background: var(--hover);
    }}
    .icon-button:active {{
      transform: scale(.92);
    }}
    .icon-button:disabled {{
      opacity: .35;
      cursor: default;
      background: transparent;
    }}
    .icon-button svg {{
      display: block;
      width: 20px;
      height: 20px;
    }}
    header .icon-button {{
      background: rgba(0, 0, 0, .45);
    }}
    header .icon-button:hover,
    .media-pill[aria-pressed="true"] {{
      background: rgba(0, 0, 0, .7);
    }}

    /* File details */
    .media-info {{
      position: relative;
    }}
    .media-popover {{
      position: fixed;
      left: var(--popover-left, 10px);
      top: var(--popover-top, 56px);
      width: var(--popover-width, 320px);
      z-index: 80;
      display: grid;
      gap: 4px;
      padding: 14px;
      border: 1px solid var(--hairline);
      border-radius: 12px;
      background: var(--menu);
      opacity: 0;
      pointer-events: none;
      transform: translateY(-4px);
      transition: opacity 140ms ease, transform 140ms ease;
      user-select: text;
    }}
    .media-info:hover .media-popover,
    .media-info:focus-within .media-popover,
    body.media-popover-pinned .media-popover {{
      opacity: 1;
      pointer-events: auto;
      transform: none;
    }}
    .popover-head {{
      display: flex;
      align-items: baseline;
      justify-content: space-between;
      gap: 12px;
      margin-bottom: 8px;
    }}
    .popover-head strong {{
      overflow: hidden;
      font-size: 14px;
      text-overflow: ellipsis;
      white-space: nowrap;
    }}
    .popover-kind,
    .popover-label,
    .popover-hint {{
      color: var(--muted);
      font-size: 12px;
    }}
    .popover-stats {{
      display: flex;
      gap: 18px;
      margin-bottom: 8px;
      font-variant-numeric: tabular-nums;
    }}
    .popover-stats span {{
      color: var(--muted);
    }}
    .popover-label {{
      margin-top: 4px;
    }}
    .media-popover code {{
      font: 12px/1.4 "Cascadia Mono", Consolas, monospace;
      color: var(--text);
      word-break: break-all;
    }}
    .popover-foot {{
      display: flex;
      align-items: center;
      justify-content: space-between;
      gap: 12px;
      margin-top: 10px;
    }}
    .pin-state-pinned,
    body.media-popover-pinned .pin-state-default {{
      display: none;
    }}
    body.media-popover-pinned .pin-state-pinned {{
      display: inline;
    }}
    .top-action {{
      flex: 0 0 auto;
      padding: 6px 12px;
      border: 1px solid var(--hairline);
      border-radius: 8px;
      background: var(--hover);
      cursor: pointer;
    }}
    .top-action:hover {{
      background: rgba(255, 255, 255, .16);
    }}
    .top-action:disabled {{
      opacity: .4;
      cursor: default;
    }}

    #phase-error {{
      position: absolute;
      top: 58px;
      left: 50%;
      display: none;
      max-width: calc(100% - 32px);
      padding: 8px 12px;
      border-radius: 8px;
      background: #3a1618;
      color: #ffc9c9;
      font-size: 12.5px;
      transform: translateX(-50%);
    }}
    #phase-error:not(:empty) {{
      display: block;
    }}

    /* Transport bar */
    .phase-controls {{
      position: absolute;
      left: 50%;
      bottom: 16px;
      z-index: 60;
      width: min(720px, calc(100% - 32px));
      padding: 10px 14px 8px;
      border: 1px solid var(--hairline);
      border-radius: 14px;
      background: var(--bar);
      opacity: 0;
      pointer-events: none;
      overflow: visible;
      transform: translate(-50%, 6px);
      transition: opacity 180ms ease, transform 180ms ease;
    }}
    body.controls-visible .phase-controls {{
      opacity: 1;
      pointer-events: auto;
      transform: translate(-50%, 0);
    }}
    .scrub-row {{
      display: flex;
      align-items: center;
      gap: 10px;
    }}
    .timecode {{
      min-width: 38px;
      color: var(--muted);
      font-size: 12px;
      font-variant-numeric: tabular-nums;
    }}
    #phase-duration {{
      text-align: right;
    }}
    .scrub-field {{
      position: relative;
      display: flex;
      flex: 1;
      align-items: center;
      height: 18px;
    }}
    #phase-scrub,
    #phase-volume-slider {{
      width: 100%;
      height: 18px;
      margin: 0;
      background: transparent;
      cursor: pointer;
      appearance: none;
      -webkit-appearance: none;
    }}
    #phase-scrub::-webkit-slider-runnable-track,
    #phase-volume-slider::-webkit-slider-runnable-track {{
      height: 4px;
      border-radius: 2px;
      background: linear-gradient(to right, var(--text) var(--fill, 0%), var(--track) var(--fill, 0%));
    }}
    #phase-scrub::-webkit-slider-thumb,
    #phase-volume-slider::-webkit-slider-thumb {{
      width: 12px;
      height: 12px;
      margin-top: -4px;
      border-radius: 50%;
      background: var(--text);
      -webkit-appearance: none;
      transition: transform 120ms ease;
      transform: scale(0);
    }}
    .scrub-field:hover #phase-scrub::-webkit-slider-thumb,
    #phase-scrub:active::-webkit-slider-thumb,
    #phase-volume-slider::-webkit-slider-thumb {{
      transform: scale(1);
    }}
    .scrub-preview {{
      position: absolute;
      bottom: 26px;
      left: 0;
      display: grid;
      gap: 4px;
      padding: 4px;
      border: 1px solid var(--hairline);
      border-radius: 8px;
      background: var(--menu);
      opacity: 0;
      pointer-events: none;
      transform: translateX(-50%);
      transition: opacity 120ms ease;
    }}
    .scrub-preview.visible {{
      opacity: 1;
    }}
    #phase-preview-canvas {{
      display: block;
      width: 160px;
      height: 90px;
      border-radius: 5px;
      background: #000;
    }}
    #phase-preview-time {{
      font-size: 11.5px;
      font-variant-numeric: tabular-nums;
      text-align: center;
    }}

    .control-strip {{
      position: relative;
      display: flex;
      align-items: center;
      justify-content: center;
      min-height: 46px;
      margin-top: 4px;
    }}
    .control-group {{
      display: flex;
      align-items: center;
      gap: 2px;
    }}
    .frame-group {{
      gap: 10px;
    }}
    .start-group,
    .utility-group {{
      position: absolute;
      top: 50%;
      transform: translateY(-50%);
    }}
    .start-group {{
      left: 0;
    }}
    .utility-group {{
      right: 0;
    }}
    .play-button {{
      width: 44px;
      height: 44px;
      border-radius: 50%;
      background: var(--text);
      color: #111114;
    }}
    .play-button:hover {{
      background: #fff;
    }}
    .play-button svg {{
      width: 22px;
      height: 22px;
    }}
    .icon-pause,
    .play-button.is-playing .icon-play,
    .icon-muted,
    #phase-mute.is-muted .icon-sound,
    .icon-fill,
    #phase-fit.is-fill .icon-fit {{
      display: none !important;
    }}
    .play-button.is-playing .icon-pause,
    #phase-mute.is-muted .icon-muted,
    #phase-fit.is-fill .icon-fill {{
      display: block !important;
    }}
    #phase-loop {{
      color: var(--muted);
    }}
    #phase-loop.active {{
      color: var(--text);
      background: var(--hover);
    }}

    .custom-rate {{
      position: relative;
    }}
    .custom-rate > span {{
      display: none;
    }}
    .rate-button {{
      min-width: 44px;
      height: 30px;
      padding: 0 10px;
      border: 0;
      border-radius: 8px;
      background: transparent;
      font-size: 12.5px;
      font-weight: 600;
      font-variant-numeric: tabular-nums;
      cursor: pointer;
    }}
    .rate-button:hover,
    .rate-button[aria-expanded="true"] {{
      background: var(--hover);
    }}
    .rate-menu {{
      position: absolute;
      bottom: calc(100% + 10px);
      left: 0;
      z-index: 70;
      display: none;
      min-width: 128px;
      padding: 4px;
      border: 1px solid var(--hairline);
      border-radius: 10px;
      background: var(--menu);
    }}
    .rate-menu.open {{
      display: grid;
    }}
    .rate-option {{
      display: flex;
      justify-content: space-between;
      padding: 7px 10px;
      border: 0;
      border-radius: 6px;
      background: transparent;
      font-variant-numeric: tabular-nums;
      text-align: left;
      cursor: pointer;
    }}
    .rate-option:hover {{
      background: var(--hover);
    }}
    .rate-option.selected {{
      font-weight: 600;
    }}
    .rate-option.selected::after {{
      content: "✓";
    }}

    .volume-control {{
      display: flex;
      align-items: center;
    }}
    .volume-control #phase-volume-slider {{
      width: 0;
      opacity: 0;
      transition: width 160ms ease, opacity 160ms ease, margin 160ms ease;
    }}
    .volume-control:hover #phase-volume-slider,
    .volume-control:focus-within #phase-volume-slider {{
      width: 76px;
      margin: 0 6px 0 2px;
      opacity: 1;
    }}
    #phase-volume-value {{
      display: none;
    }}

    @media (max-width: 560px) {{
      .volume-control,
      #phase-fit {{
        display: none;
      }}
      .phase-controls {{
        bottom: 8px;
        width: calc(100% - 16px);
        padding: 8px 10px 6px;
      }}
    }}
    @media (max-width: 400px) {{
      .start-group,
      .utility-group {{
        display: none;
      }}
    }}
    @media (max-height: 300px) {{
      header {{
        display: none;
      }}
    }}
  </style>
</head>
<body>
  <div class="shell">
    <header>
      <div class="top-left">
        <span id="phase-title" class="title">{safe_title}</span>
      </div>
      <div class="top-actions">
        <div class="media-info">
          <button id="phase-kind" class="icon-button media-pill" type="button" aria-expanded="false" aria-pressed="false" aria-label="Open media details" title="Details"><svg viewBox="0 0 24 24" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="8.5"/><path d="M12 11v5"/><path d="M12 7.6v.2"/></svg></button>
          <div class="media-popover" role="tooltip">
            <div class="popover-head">
              <strong>{safe_title}</strong>
              <span class="popover-kind">{kind_label}</span>
            </div>
            <div class="popover-stats">
              <div>{fps} <span>fps</span></div>
              <div>{offset}s <span>offset</span></div>
            </div>
            <span class="popover-label">File</span>
            <code id="phase-path">{source_display}</code>
            <span class="popover-label">Folder</span>
            <code>{folder_display}</code>
            <div class="popover-foot">
              <span class="popover-hint"><span class="pin-state-default">Click the info button to pin this open.</span><span class="pin-state-pinned">Pinned open.</span> Click outside to close.</span>
              <button id="phase-show-folder" class="top-action ghost" type="button"{show_folder_disabled}>Show folder</button>
            </div>
          </div>
        </div>
        <button id="phase-swap" class="icon-button" type="button" aria-label="Swap video" title="Swap video"{swap_disabled}><svg viewBox="0 0 24 24" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M16 3.5l4 4-4 4"/><path d="M20 7.5H5"/><path d="M8 20.5l-4-4 4-4"/><path d="M4 16.5h15"/></svg></button>
      </div>
      <span id="phase-error"></span>
    </header>
    <main>{media}</main>
  </div>
</body>
</html>"#,
        safe_title = safe_title,
        kind_label = kind_label,
        source_display = source_display,
        folder_display = folder_display,
        show_folder_disabled = show_folder_disabled,
        swap_disabled = swap_disabled,
        media = media,
        fps = draft.fps.max(1.0),
        offset = draft.offset_seconds,
    ))
}

fn open_html_popup(html_path: &Path) -> Result<(), String> {
    let exe = std::env::current_exe()
        .map_err(|error| format!("Could not locate Phase app executable: {error}"))?;
    append_popup_log(format!(
        "spawning popup process: exe={} html={}",
        exe.display(),
        html_path.display()
    ));
    Command::new(exe)
        .arg("--video-popup")
        .arg(html_path)
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("Could not launch embedded video popup: {error}"))
}

fn render_youtube_media_html(
    draft: &ReferenceDraft,
    title: &str,
    source: &str,
) -> Result<String, String> {
    let embed = youtube_embed_url(source)
        .ok_or_else(|| "Could not read the YouTube video ID from that URL.".to_owned())?;
    Ok(format!(
        r#"<section class="video-wrap youtube-wrap">
  <iframe id="phase-media" src="{embed}" title="{title}" referrerpolicy="origin" allow="accelerometer; autoplay; clipboard-write; encrypted-media; gyroscope; picture-in-picture; web-share" allowfullscreen></iframe>
</section>
<script src="https://www.youtube.com/iframe_api"></script>
<script>
(() => {{
  const headerBar = document.querySelector('header');
  const mediaInfo = document.querySelector('.media-info');
  const mediaPopover = document.querySelector('.media-popover');
  const mediaKind = document.getElementById('phase-kind');
  const titleLabel = document.getElementById('phase-title');
  const pathLabel = document.getElementById('phase-path');
  const swap = document.getElementById('phase-swap');
  const frameStep = 1 / {fps};
  const bridgeUrl = 'ws://127.0.0.1:{bridge_port}{bridge_path}';
  const sourceFps = {fps};
  const startFrame = {start_frame};
  const offsetSeconds = {offset_seconds};
  const initialPlaybackRate = {playback_rate};
  let referencePayload = {reference_json};
  let currentSource = {source_json};
  let player;
  let playerReady = false;
  let playing = false;
  let bridge;
  let seq = 0;
  let chromeTimer;
  let suppressLocalSyncUntil = 0;
  let lastTimelineSent = 0;
  let lastObservedSeconds = 0;
  let lastRemoteCorrectionAt = 0;
  let phaseDrivenPlayback = false;
  let desiredPlaying = false;
  let pendingRemotePlayback = null;
  function setChromeVisible(visible) {{
    document.body.classList.toggle('controls-visible', visible);
  }}
  function clearChromeTimer() {{
    if (chromeTimer) clearTimeout(chromeTimer);
    chromeTimer = undefined;
  }}
  function hideChrome() {{
    clearChromeTimer();
    setChromeVisible(false);
  }}
  function scheduleChromeHide() {{
    clearChromeTimer();
    chromeTimer = setTimeout(hideChrome, 1250);
  }}
  function revealChrome() {{
    setChromeVisible(true);
    scheduleChromeHide();
  }}
  function positionMediaPopover() {{
    if (!mediaPopover || !mediaKind) return;
    const margin = window.innerWidth < 420 ? 8 : 10;
    const width = Math.max(168, Math.min(430, window.innerWidth - (margin * 2)));
    const pillRect = mediaKind.getBoundingClientRect();
    const headerRect = headerBar?.getBoundingClientRect();
    const maxLeft = Math.max(margin, window.innerWidth - width - margin);
    const left = Math.max(margin, Math.min(pillRect.left, maxLeft));
    const top = Math.max(margin, (headerRect?.bottom || pillRect.bottom) + 8);
    mediaPopover.style.setProperty('--popover-left', `${{left}}px`);
    mediaPopover.style.setProperty('--popover-top', `${{top}}px`);
    mediaPopover.style.setProperty('--popover-width', `${{width}}px`);
  }}
  function setPopoverPinned(pinned) {{
    positionMediaPopover();
    document.body.classList.toggle('media-popover-pinned', pinned);
    mediaKind?.setAttribute('aria-expanded', String(pinned));
    mediaKind?.setAttribute('aria-pressed', String(pinned));
    mediaKind?.setAttribute(
      'aria-label',
      pinned ? 'Media details pinned open' : 'Open media details'
    );
  }}
  function playerState() {{
    if (!playerReady || !player?.getPlayerState) return -1;
    try {{ return player.getPlayerState(); }} catch (_) {{ return -1; }}
  }}
  function secondsNow() {{
    if (!playerReady || !player?.getCurrentTime) return 0;
    try {{
      const seconds = Number(player.getCurrentTime());
      return Number.isFinite(seconds) ? Math.max(0, seconds) : 0;
    }} catch (_) {{
      return 0;
    }}
  }}
  function refresh() {{
    playing = playerState() === 1;
  }}
  function connectBridge() {{
    try {{
      bridge = new WebSocket(bridgeUrl);
      bridge.onopen = () => {{
        sendBridge('hello', {{ side: 'phase-video-popup', source: currentSource }});
        sendBridge('reference.set', referencePayload);
      }};
      bridge.onclose = () => setTimeout(connectBridge, 1200);
      bridge.onerror = () => {{}};
      bridge.onmessage = (event) => {{
        try {{
          const packet = JSON.parse(event.data);
          if (packet.op === 'sync.timeline' || packet.op === 'sync.seek') {{
            const payload = packet.payload || {{}};
            const remoteRate = playbackRateFromPayload(payload);
            if (remoteRate !== undefined) setRate(remoteRate, {{ send: false }});
            applyRemoteSeconds(payload.seconds, packet.op === 'sync.seek', packet.op, payload);
          }}
          if (packet.op === 'sync.playback') {{
            const payload = packet.payload || {{}};
            const remoteRate = playbackRateFromPayload(payload);
            if (remoteRate !== undefined) setRate(remoteRate, {{ send: false }});
            phaseDrivenPlayback = payload.playing === true;
            suppressLocalSyncFor(620);
            if (payload.playing === false) requestPlayback(false);
            applyRemoteSeconds(payload.seconds, true, packet.op, payload);
            if (payload.playing === true) requestPlayback(true);
          }}
        }} catch (_) {{}}
      }};
    }} catch (_) {{}}
  }}
  function sendBridge(op, payload) {{
    if (!bridge || bridge.readyState !== WebSocket.OPEN) return;
    bridge.send(JSON.stringify({{
      v: 'phase-video-reference/1',
      id: crypto.randomUUID ? crypto.randomUUID() : `youtube-${{Date.now()}}-${{++seq}}`,
      op,
      reply_to: null,
      token: '',
      sent_at: Date.now() / 1000,
      payload: payload || {{}}
    }}));
  }}
  function playbackRateFromPayload(payload) {{
    const raw = payload?.playback_rate ?? payload?.PlaybackRate ?? payload?.rate;
    const rate = Number(raw);
    if (!Number.isFinite(rate)) return undefined;
    return Math.max(0.05, Math.min(8, rate));
  }}
  function canSendLocalSync() {{
    return Date.now() >= suppressLocalSyncUntil;
  }}
  function suppressLocalSyncFor(ms) {{
    suppressLocalSyncUntil = Math.max(suppressLocalSyncUntil, Date.now() + ms);
  }}
  function applyRemoteSeconds(seconds, force, op, payload) {{
    if (!playerReady || typeof seconds !== 'number' || !Number.isFinite(seconds)) return;
    const reason = payload?.reason ?? '';
    if (
      op === 'sync.timeline' &&
      force !== true &&
      playing &&
      (phaseDrivenPlayback || reason === 'playback_correction')
    ) {{
      return;
    }}
    const target = Math.max(0, seconds);
    const currentSeconds = secondsNow();
    const drift = Math.abs(currentSeconds - target);
    const passivePlaybackHint = op === 'sync.timeline' && playing && force !== true;
    const correctionThreshold = passivePlaybackHint
      ? Math.max(0.85, frameStep * 48)
      : playing
      ? Math.max(0.18, Math.min(0.36, frameStep * 18))
      : Math.max(0.045, Math.min(0.12, frameStep * 1.5));
    const now = Date.now();
    if (passivePlaybackHint && now - lastRemoteCorrectionAt < 1000) return;
    if (force || !playing || drift > correctionThreshold) {{
      lastRemoteCorrectionAt = now;
      suppressLocalSyncFor(260);
      player.seekTo(target, true);
      lastObservedSeconds = target;
      refresh();
    }}
  }}
  function timelinePayload(extra) {{
    const seconds = secondsNow();
    return Object.assign({{
      seq: ++seq,
      seconds,
      fps: sourceFps,
      frame: Math.max(0, Math.round(startFrame + ((seconds - offsetSeconds) * sourceFps))),
      playing,
      playback_rate: currentRate()
    }}, extra || {{}});
  }}
  function seekTo(seconds, send) {{
    if (!playerReady) return;
    phaseDrivenPlayback = false;
    const target = Math.max(0, seconds);
    player.seekTo(target, true);
    refresh();
    if (send !== false && canSendLocalSync()) sendBridge('sync.seek', timelinePayload());
  }}
  function requestPlayback(shouldPlay) {{
    desiredPlaying = shouldPlay === true;
    if (!playerReady) {{
      pendingRemotePlayback = desiredPlaying;
      return;
    }}
    try {{
      if (desiredPlaying) {{
        player?.playVideo?.();
      }} else {{
        player?.pauseVideo?.();
      }}
    }} catch (_) {{}}
    refresh();
    setTimeout(enforceDesiredPlayback, 80);
    setTimeout(enforceDesiredPlayback, 220);
    setTimeout(enforceDesiredPlayback, 520);
  }}
  function enforceDesiredPlayback() {{
    if (!playerReady) return;
    const state = playerState();
    try {{
      if (desiredPlaying && state !== 1) player?.playVideo?.();
      if (!desiredPlaying && (state === 1 || state === 3)) player?.pauseVideo?.();
    }} catch (_) {{}}
    refresh();
  }}
  function currentRate() {{
    if (!playerReady || !player?.getPlaybackRate) return initialPlaybackRate;
    try {{
      const rate = Number(player.getPlaybackRate());
      return Number.isFinite(rate) ? rate : initialPlaybackRate;
    }} catch (_) {{
      return initialPlaybackRate;
    }}
  }}
  function setRate(value, options) {{
    options = options || {{}};
    const nextRate = Math.max(0.05, Math.min(8, Number(value) || 1));
    try {{ player?.setPlaybackRate?.(nextRate); }} catch (_) {{}}
    referencePayload = Object.assign({{}}, referencePayload, {{ playback_rate: nextRate }});
    if (options.send !== false && canSendLocalSync()) {{
      sendBridge('sync.timeline', timelinePayload({{ reason: 'rate_change', playback_rate: nextRate }}));
      sendBridge('reference.set', referencePayload);
    }}
  }}
  function sendTimelineTick() {{
    const seconds = secondsNow();
    const movedWhilePaused = Math.abs(seconds - lastObservedSeconds) > Math.max(0.05, frameStep * 0.75);
    refresh();
    if (!canSendLocalSync()) {{
      lastObservedSeconds = seconds;
      return;
    }}
    const now = Date.now();
    if (playing && !phaseDrivenPlayback && now - lastTimelineSent >= 100) {{
      lastTimelineSent = now;
      sendBridge('sync.timeline', timelinePayload());
    }}
    if (!playing && movedWhilePaused && now - lastTimelineSent >= 100) {{
      lastTimelineSent = now;
      sendBridge('sync.seek', timelinePayload({{ reason: 'youtube_paused_seek' }}));
    }}
    lastObservedSeconds = seconds;
  }}
  function onPlayerReady(event) {{
    player = event.target;
    playerReady = true;
    setRate(initialPlaybackRate, {{ send: false }});
    connectBridge();
    refresh();
    if (pendingRemotePlayback !== null) {{
      const pending = pendingRemotePlayback;
      pendingRemotePlayback = null;
      requestPlayback(pending);
    }}
    setInterval(sendTimelineTick, 100);
  }}
  function onPlayerStateChange(event) {{
    const nextPlaying = event.data === 1;
    playing = nextPlaying;
    refresh();
    if (canSendLocalSync()) desiredPlaying = nextPlaying;
    if ((event.data === 1 || event.data === 2 || event.data === 0) && canSendLocalSync()) {{
      sendBridge('sync.playback', timelinePayload({{ playing: nextPlaying }}));
    }}
    if (!nextPlaying) phaseDrivenPlayback = false;
    if (!canSendLocalSync()) setTimeout(enforceDesiredPlayback, 120);
  }}
  function onPlayerError(event) {{
    document.body.classList.add('load-error');
    const code = event?.data ?? '';
    document.getElementById('phase-error').textContent = `YouTube playback unavailable${{code ? ` (${{code}})` : ''}}.`;
  }}
  window.onYouTubeIframeAPIReady = () => {{
    player = new YT.Player('phase-media', {{
      events: {{
        onReady: onPlayerReady,
        onStateChange: onPlayerStateChange,
        onPlaybackRateChange: () => setRate(currentRate(), {{ send: true }}),
        onError: onPlayerError
      }}
    }});
  }};
  swap?.addEventListener('click', () => {{
    revealChrome();
    document.getElementById('phase-error').textContent = 'Swap is only available for local MP4 references.';
  }});
  window.addEventListener('keydown', (event) => {{
    revealChrome();
    if (event.code === 'Escape') setPopoverPinned(false);
  }});
  mediaKind?.addEventListener('click', (event) => {{
    event.stopPropagation();
    revealChrome();
    positionMediaPopover();
    setPopoverPinned(!document.body.classList.contains('media-popover-pinned'));
  }});
  mediaInfo?.addEventListener('pointerenter', positionMediaPopover);
  mediaInfo?.addEventListener('focusin', positionMediaPopover);
  document.addEventListener('click', (event) => {{
    if (!mediaInfo?.contains(event.target)) setPopoverPinned(false);
  }});
  window.addEventListener('resize', positionMediaPopover);
  window.addEventListener('pointerenter', revealChrome);
  window.addEventListener('pointermove', revealChrome);
  window.addEventListener('pointerleave', hideChrome);
  window.addEventListener('blur', hideChrome);
  document.documentElement.addEventListener('mouseleave', hideChrome);
  document.addEventListener('mouseout', (event) => {{
    if (!event.relatedTarget && !event.toElement) hideChrome();
  }});
  document.addEventListener('focusin', revealChrome);
  document.addEventListener('focusout', scheduleChromeHide);
  document.addEventListener('visibilitychange', () => {{
    if (document.hidden) hideChrome();
  }});
  if (titleLabel) titleLabel.textContent = referencePayload.title || 'YouTube Reference';
  if (pathLabel) pathLabel.textContent = currentSource;
  refresh();
}})();
</script>"#,
        embed = html_escape_attr(&embed),
        title = html_escape_attr(title),
        fps = draft.fps.max(1.0),
        start_frame = draft.start_frame.max(0),
        offset_seconds = draft.offset_seconds,
        playback_rate = draft.playback_rate.clamp(0.05, 8.0),
        bridge_port = DEFAULT_PORT,
        bridge_path = DEFAULT_PATH,
        source_json = serde_json::to_string(&draft.source).unwrap_or_else(|_| "\"\"".to_owned()),
        reference_json =
            serde_json::to_string(&draft.payload()).unwrap_or_else(|_| "{}".to_owned()),
    ))
}

pub fn install_popup_panic_logger() {
    std::panic::set_hook(Box::new(|panic_info| {
        append_popup_log(format!("panic: {panic_info}"));
    }));
}

pub fn append_popup_log(message: impl AsRef<str>) {
    let dir = std::env::temp_dir().join("PhaseAnimatorVideoReference");
    let _ = fs::create_dir_all(&dir);
    let log_path = dir.join("popup.log");
    if let Ok(mut file) = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
    {
        let _ = writeln!(
            file,
            "[{} pid={}] {}",
            popup_log_timestamp(),
            std::process::id(),
            message.as_ref()
        );
    }
}

enum PopupEvent {
    ShowFolder(PathBuf),
    PickSwapVideo,
}

pub fn run_popup_window(html_path: &Path) -> Result<(), String> {
    append_popup_log(format!("popup process start: html={}", html_path.display()));
    if !html_path.exists() {
        append_popup_log("html file does not exist");
        return Err(format!(
            "Video popup HTML does not exist: {}",
            html_path.display()
        ));
    }
    let url = if popup_needs_http_origin(html_path) {
        start_popup_page_server(html_path)?
    } else {
        file_url(html_path)
    };
    append_popup_log(format!("popup url: {url}"));
    let event_loop = tao::event_loop::EventLoopBuilder::<PopupEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();
    let command_url = start_popup_command_server(proxy.clone())?;
    append_popup_log("event loop created");
    append_popup_log(format!("popup command server: {command_url}"));
    let popup_icon = popup_window_icon();
    append_popup_log(format!("popup icon loaded: {}", popup_icon.is_some()));
    let window = tao::window::WindowBuilder::new()
        .with_title("Phase Video Reference")
        .with_inner_size(tao::dpi::LogicalSize::new(1040.0, 640.0))
        .with_min_inner_size(tao::dpi::LogicalSize::new(360.0, 260.0))
        .with_always_on_top(true)
        .with_window_icon(popup_icon)
        .build(&event_loop)
        .map_err(|error| format!("Could not create video popup window: {error}"))?;
    append_popup_log("window created");

    let webview = wry::WebViewBuilder::new()
        .with_url(&url)
        .with_initialization_script(format!(
            "window.__PHASE_POPUP_COMMAND_URL = {};",
            serde_json::to_string(&command_url).unwrap_or_else(|_| "null".to_owned())
        ))
        .with_new_window_req_handler(|url, _features| {
            let _ = open::that(url.as_str());
            wry::NewWindowResponse::Deny
        })
        .build(&window)
        .map_err(|error| format!("Could not render embedded video popup: {error}"))?;
    append_popup_log("webview created");

    event_loop.run(move |event, _, control_flow| {
        let _keep_alive = (&window, &webview);
        *control_flow = tao::event_loop::ControlFlow::Wait;
        if let tao::event::Event::WindowEvent {
            event: tao::event::WindowEvent::CloseRequested,
            ..
        } = event
        {
            append_popup_log("close requested");
            *control_flow = tao::event_loop::ControlFlow::Exit;
        }
        if let tao::event::Event::UserEvent(PopupEvent::PickSwapVideo) = &event {
            let picked = rfd::FileDialog::new()
                .add_filter("Video", &["mp4", "mov", "m4v", "webm"])
                .pick_file();
            let script = popup_swap_script(picked.as_deref());
            if let Err(error) = webview.evaluate_script(&script) {
                append_popup_log(format!("swap video script failed: {error}"));
            }
        }
        if let tao::event::Event::UserEvent(PopupEvent::ShowFolder(folder)) = &event
            && let Err(error) = open_folder(folder)
        {
            append_popup_log(format!("show folder failed: {error}"));
        }
        if matches!(event, tao::event::Event::LoopDestroyed) {
            append_popup_log("event loop destroyed");
        }
    });
}

fn open_folder(path: &Path) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        Command::new("explorer.exe")
            .arg(path)
            .spawn()
            .map(|_| ())
            .map_err(|error| format!("Could not open folder in Explorer: {error}"))
    }
    #[cfg(not(target_os = "windows"))]
    {
        open::that(path).map_err(|error| format!("Could not open folder: {error}"))
    }
}

fn popup_needs_http_origin(html_path: &Path) -> bool {
    fs::read_to_string(html_path)
        .map(|html| html.contains("https://www.youtube.com/embed/"))
        .unwrap_or(false)
}

fn start_popup_page_server(html_path: &Path) -> Result<String, String> {
    let html = fs::read_to_string(html_path)
        .map_err(|error| format!("Could not read video popup page: {error}"))?;
    let listener = TcpListener::bind("127.0.0.1:0")
        .map_err(|error| format!("Could not start video popup page server: {error}"))?;
    let addr = listener
        .local_addr()
        .map_err(|error| format!("Could not read video popup page server address: {error}"))?;
    let origin = format!("http://127.0.0.1:{}", addr.port());
    let page_url = format!("{origin}{POPUP_PAGE_PATH}");
    let html = html.replace(
        YOUTUBE_EMBED_ORIGIN_PLACEHOLDER,
        &percent_encode_query_component(&origin),
    );
    append_popup_log(format!("popup page server: {page_url}"));
    thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(mut stream) => handle_popup_page_stream(&mut stream, &html),
                Err(error) => {
                    append_popup_log(format!("popup page accept failed: {error}"));
                    break;
                }
            }
        }
    });
    Ok(page_url)
}

fn handle_popup_page_stream(stream: &mut TcpStream, html: &str) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let Some((method, target, _)) = read_popup_http_request(stream) else {
        let _ = write_popup_http_response(stream, 400, "Bad Request");
        return;
    };
    let (path, _) = split_request_target(&target);
    if method != "GET" && method != "HEAD" {
        let _ = write_popup_http_response(stream, 405, "Method Not Allowed");
        return;
    }
    if path == "/favicon.ico" {
        let _ = write_popup_http_response(stream, 204, "No Content");
        return;
    }
    if path != POPUP_PAGE_PATH {
        let _ = write_popup_http_response(stream, 404, "Not Found");
        return;
    }
    let _ = write_popup_html_response(stream, html, method == "HEAD");
}

fn start_popup_command_server(
    proxy: tao::event_loop::EventLoopProxy<PopupEvent>,
) -> Result<String, String> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .map_err(|error| format!("Could not start popup command server: {error}"))?;
    let addr = listener
        .local_addr()
        .map_err(|error| format!("Could not read popup command server address: {error}"))?;
    thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(mut stream) => handle_popup_command_stream(&mut stream, &proxy),
                Err(error) => {
                    append_popup_log(format!("popup command accept failed: {error}"));
                    break;
                }
            }
        }
    });
    Ok(format!(
        "http://127.0.0.1:{}{}",
        addr.port(),
        POPUP_COMMAND_PATH
    ))
}

fn handle_popup_command_stream(
    stream: &mut TcpStream,
    proxy: &tao::event_loop::EventLoopProxy<PopupEvent>,
) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let Some((method, path, body)) = read_popup_http_request(stream) else {
        let _ = write_popup_http_response(stream, 400, "Bad Request");
        return;
    };
    let (path, query) = split_request_target(&path);
    if path != POPUP_COMMAND_PATH {
        let _ = write_popup_http_response(stream, 404, "Not Found");
        return;
    }
    if method == "OPTIONS" {
        let _ = write_popup_http_response(stream, 204, "No Content");
        return;
    }
    if method != "POST" && method != "GET" {
        let _ = write_popup_http_response(stream, 405, "Method Not Allowed");
        return;
    }

    let message = if method == "GET" {
        popup_message_from_query(query)
    } else {
        match serde_json::from_str::<Value>(&body) {
            Ok(message) => message,
            Err(_) => {
                append_popup_log(format!("popup command ignored invalid json body={body:?}"));
                let _ = write_popup_http_response(stream, 400, "Bad Request");
                return;
            }
        }
    };
    let op = message
        .get("op")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    let payload = message.get("payload").unwrap_or(&Value::Null);
    match op {
        "show-folder" => {
            if let Some(source) = payload.get("source").and_then(|value| value.as_str()) {
                let folder = Path::new(source)
                    .parent()
                    .unwrap_or_else(|| Path::new(source));
                let _ = proxy.send_event(PopupEvent::ShowFolder(folder.to_path_buf()));
                append_popup_log(format!("show-folder queued: {}", folder.display()));
            }
        }
        "swap-video" => {
            let _ = proxy.send_event(PopupEvent::PickSwapVideo);
            append_popup_log("swap-video queued");
        }
        _ => append_popup_log(format!("popup command ignored op={op}")),
    }
    let _ = write_popup_http_response(stream, 204, "No Content");
}

fn split_request_target(target: &str) -> (&str, &str) {
    target.split_once('?').unwrap_or((target, ""))
}

fn popup_message_from_query(query: &str) -> Value {
    let op = query_value(query, "op").unwrap_or_default();
    let source = query_value(query, "source").unwrap_or_default();
    json!({
        "op": op,
        "payload": {
            "source": source,
        },
    })
}

fn query_value(query: &str, key: &str) -> Option<String> {
    query.split('&').find_map(|part| {
        let (name, value) = part.split_once('=').unwrap_or((part, ""));
        (percent_decode_query(name) == key).then(|| percent_decode_query(value))
    })
}

fn percent_decode_query(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                decoded.push(b' ');
                index += 1;
            }
            b'%' if index + 2 < bytes.len() => {
                let hex = &value[index + 1..index + 3];
                if let Ok(byte) = u8::from_str_radix(hex, 16) {
                    decoded.push(byte);
                    index += 3;
                } else {
                    decoded.push(bytes[index]);
                    index += 1;
                }
            }
            byte => {
                decoded.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&decoded).to_string()
}

fn read_popup_http_request(stream: &mut TcpStream) -> Option<(String, String, String)> {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 1024];
    let mut header_end = None;
    loop {
        let read = stream.read(&mut buffer).ok()?;
        if read == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..read]);
        if header_end.is_none() {
            header_end = find_header_end(&bytes);
        }
        if let Some(end) = header_end {
            let headers = String::from_utf8_lossy(&bytes[..end]);
            let content_length = headers
                .lines()
                .find_map(|line| line.split_once(':'))
                .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                .and_then(|(_, value)| value.trim().parse::<usize>().ok())
                .unwrap_or(0);
            if bytes.len() >= end + 4 + content_length {
                break;
            }
        }
        if bytes.len() > 64 * 1024 {
            return None;
        }
    }

    let header_end = header_end?;
    let headers = String::from_utf8_lossy(&bytes[..header_end]);
    let mut lines = headers.lines();
    let request_line = lines.next()?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next()?.to_owned();
    let path = parts.next()?.to_owned();
    let body = String::from_utf8_lossy(&bytes[header_end + 4..]).to_string();
    Some((method, path, body))
}

fn find_header_end(bytes: &[u8]) -> Option<usize> {
    bytes.windows(4).position(|window| window == b"\r\n\r\n")
}

fn write_popup_http_response(
    stream: &mut TcpStream,
    status: u16,
    message: &str,
) -> std::io::Result<()> {
    let response = format!(
        "HTTP/1.1 {status} {message}\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: POST, OPTIONS\r\nAccess-Control-Allow-Headers: content-type\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(response.as_bytes())
}

fn write_popup_html_response(
    stream: &mut TcpStream,
    html: &str,
    headers_only: bool,
) -> std::io::Result<()> {
    let body = html.as_bytes();
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nCache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(response.as_bytes())?;
    if !headers_only {
        stream.write_all(body)?;
    }
    Ok(())
}

fn popup_swap_script(path: Option<&Path>) -> String {
    let payload = path.map(popup_swap_payload).unwrap_or(Value::Null);
    let encoded = serde_json::to_string(&payload).unwrap_or_else(|_| "null".to_owned());
    format!("window.phaseVideoReferenceSwap && window.phaseVideoReferenceSwap({encoded});")
}

fn popup_swap_payload(path: &Path) -> Value {
    let source = path.to_string_lossy().to_string();
    json!({
        "src": file_url(path),
        "source": source,
        "title": default_title_for(&source),
    })
}

fn popup_window_icon() -> Option<tao::window::Icon> {
    let image = image::load_from_memory(include_bytes!("../assets/PhaseAnimator.png"))
        .ok()?
        .to_rgba8();
    let width = image.width();
    let height = image.height();
    let side = width.min(height).max(1);
    let offset_x = (width - side) / 2;
    let offset_y = (height - side) / 2;
    let mut square = image::RgbaImage::new(side, side);
    for y in 0..side {
        for x in 0..side {
            square.put_pixel(x, y, *image.get_pixel(offset_x + x, offset_y + y));
        }
    }
    let resized = image::imageops::resize(&square, 256, 256, image::imageops::FilterType::Lanczos3);
    tao::window::Icon::from_rgba(resized.into_raw(), 256, 256).ok()
}

fn popup_log_timestamp() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| format!("{:.3}", duration.as_secs_f64()))
        .unwrap_or_else(|_| "0.000".to_owned())
}

fn youtube_embed_url(source: &str) -> Option<String> {
    let trimmed = source.trim();
    let lower = trimmed.to_ascii_lowercase();
    let id = if let Some(index) = lower.find("youtu.be/") {
        trimmed.get(index + "youtu.be/".len()..)
    } else if let Some(index) = lower.find("youtube.com/watch?") {
        trimmed
            .get(index + "youtube.com/watch?".len()..)
            .and_then(query_youtube_id)
    } else if let Some(index) = lower.find("youtube.com/shorts/") {
        trimmed.get(index + "youtube.com/shorts/".len()..)
    } else if let Some(index) = lower.find("youtube.com/embed/") {
        trimmed.get(index + "youtube.com/embed/".len()..)
    } else {
        None
    }?;
    let clean_id = id.split(['?', '&', '/', '#']).next().unwrap_or("").trim();
    (!clean_id.is_empty()).then(|| {
        format!(
            "https://www.youtube.com/embed/{clean_id}?enablejsapi=1&rel=0&origin={}",
            YOUTUBE_EMBED_ORIGIN_PLACEHOLDER
        )
    })
}

fn query_youtube_id(query: &str) -> Option<&str> {
    query.split('&').find_map(|part| part.strip_prefix("v="))
}

fn is_http_url(source: &str) -> bool {
    let lower = source.to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

fn file_url(path: &Path) -> String {
    let mut path = path.to_path_buf();
    if let Ok(canonical) = path.canonicalize() {
        path = canonical;
    }
    let mut path = path.to_string_lossy().replace('\\', "/");
    if let Some(stripped) = path.strip_prefix("//?/") {
        path = stripped.to_owned();
    } else if let Some(stripped) = path.strip_prefix(r"\\?\") {
        path = stripped.replace('\\', "/");
    }
    let prefixed = if path.starts_with('/') {
        path
    } else {
        format!("/{path}")
    };
    format!("file://{}", percent_encode_path(&prefixed))
}

fn percent_encode_path(path: &str) -> String {
    let mut encoded = String::new();
    for byte in path.as_bytes() {
        match *byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b':' | b'.' | b'-' | b'_' | b'~' => {
                encoded.push(*byte as char)
            }
            b' ' => encoded.push_str("%20"),
            other => encoded.push_str(&format!("%{other:02X}")),
        }
    }
    encoded
}

fn percent_encode_query_component(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.as_bytes() {
        match *byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(*byte as char)
            }
            other => encoded.push_str(&format!("%{other:02X}")),
        }
    }
    encoded
}

fn html_escape_text(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn html_escape_attr(text: &str) -> String {
    html_escape_text(text).replace('"', "&quot;")
}

pub fn default_title_for(source: &str) -> String {
    let trimmed = source.trim();
    if trimmed.is_empty() {
        return "Video Reference".to_owned();
    }
    if source_kind_for(trimmed) == ReferenceKind::LocalFile
        && let Some(name) = Path::new(trimmed)
            .file_stem()
            .and_then(|name| name.to_str())
        && !name.trim().is_empty()
    {
        return name.trim().to_owned();
    }
    if source_kind_for(trimmed) == ReferenceKind::Youtube {
        return "YouTube Reference".to_owned();
    }
    "Video Reference".to_owned()
}

pub fn packet_payload(packet: &VideoPacket) -> &Value {
    if packet.payload.is_object() {
        &packet.payload
    } else {
        &Value::Null
    }
}

fn run_server(
    config: BridgeConfig,
    command_rx: Receiver<BridgeCommand>,
    event_tx: Sender<BridgeEvent>,
) {
    let path = clean_path(&config.path);
    let token = config.token.trim().to_owned();
    let bind_addr = format!("127.0.0.1:{}", config.port);
    let listener = match TcpListener::bind(&bind_addr) {
        Ok(listener) => listener,
        Err(error) => {
            let _ = event_tx.send(BridgeEvent::Error(format!(
                "Could not start video bridge on {bind_addr}: {error}"
            )));
            return;
        }
    };
    if let Err(error) = listener.set_nonblocking(true) {
        let _ = event_tx.send(BridgeEvent::Error(format!(
            "Could not configure video bridge listener: {error}"
        )));
        return;
    }

    let url = config.url();
    let _ = event_tx.send(BridgeEvent::Listening { url });
    let mut clients = Vec::<tungstenite::WebSocket<TcpStream>>::new();

    loop {
        let mut stopping = false;
        for command in command_rx.try_iter() {
            match command {
                BridgeCommand::Send {
                    op,
                    payload,
                    reply_to,
                } => {
                    if clients.is_empty() {
                        let _ = event_tx.send(BridgeEvent::SendFailed {
                            op,
                            message: "Studio is not connected.".to_owned(),
                        });
                        continue;
                    }
                    let packet = make_packet(&op, payload, &token, reply_to);
                    let Ok(encoded) = serde_json::to_string(&packet) else {
                        let _ = event_tx.send(BridgeEvent::SendFailed {
                            op,
                            message: "Could not encode video bridge packet.".to_owned(),
                        });
                        continue;
                    };
                    send_to_clients(&mut clients, &event_tx, op, encoded);
                }
                BridgeCommand::Stop => stopping = true,
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
                        let _ = event_tx.send(BridgeEvent::ClientConnected);
                    }
                    Err(error) => {
                        let _ = event_tx.send(BridgeEvent::Error(error));
                    }
                },
                Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                Err(error) => {
                    let _ = event_tx.send(BridgeEvent::Error(format!(
                        "Video bridge accept failed: {error}"
                    )));
                    break;
                }
            }
        }

        poll_clients(&mut clients, &event_tx);
        thread::sleep(Duration::from_millis(16));
    }

    for mut client in clients {
        let _ = client.close(None);
    }
    let _ = event_tx.send(BridgeEvent::Stopped);
}

fn accept_client(
    stream: TcpStream,
    expected_path: &str,
    expected_token: &str,
) -> Result<tungstenite::WebSocket<TcpStream>, String> {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
    let expected_path = expected_path.to_owned();
    let expected_token = expected_token.to_owned();

    accept_hdr(
        stream,
        move |request: &tungstenite::handshake::server::Request, response| {
            if request.uri().path() != expected_path {
                return Err(error_response(404, "Unknown Phase video bridge path."));
            }
            if !expected_token.is_empty() {
                let request_token = request
                    .headers()
                    .get("X-Phase-Video-Reference-Token")
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or("")
                    .trim();
                if request_token != expected_token {
                    return Err(error_response(401, "Invalid Phase video bridge token."));
                }
            }
            Ok(response)
        },
    )
    .map_err(|error| format!("Video bridge handshake failed: {error}"))
}

fn poll_clients(
    clients: &mut Vec<tungstenite::WebSocket<TcpStream>>,
    event_tx: &Sender<BridgeEvent>,
) {
    let mut index = 0;
    while index < clients.len() {
        let mut remove_client = false;
        loop {
            match clients[index].read() {
                Ok(Message::Text(text)) => match serde_json::from_str::<VideoPacket>(&text) {
                    Ok(packet) if packet.v == VERSION => {
                        send_auto_reply(&mut clients[index], &packet);
                        if should_relay_client_packet(&packet.op) {
                            relay_client_packet_to_peers(
                                clients, index, event_tx, &packet.op, &text,
                            );
                        }
                        let _ = event_tx.send(BridgeEvent::PacketReceived(packet));
                    }
                    Ok(packet) => {
                        let _ = event_tx.send(BridgeEvent::Error(format!(
                            "Unsupported video bridge protocol: {}",
                            packet.v
                        )));
                    }
                    Err(error) => {
                        let _ = event_tx.send(BridgeEvent::Error(format!(
                            "Invalid video bridge packet: {error}"
                        )));
                    }
                },
                Ok(Message::Ping(bytes)) => {
                    let _ = clients[index].write(Message::Pong(bytes));
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
                    let _ = event_tx.send(BridgeEvent::Error(format!(
                        "Video bridge client read failed: {error}"
                    )));
                    remove_client = true;
                    break;
                }
            }
        }

        if remove_client {
            clients.remove(index);
            let _ = event_tx.send(BridgeEvent::ClientDisconnected);
        } else {
            index += 1;
        }
    }
}

fn should_relay_client_packet(op: &str) -> bool {
    matches!(
        op,
        "reference.set"
            | "reference.clear"
            | "sync.enabled"
            | "sync.status"
            | "sync.timeline"
            | "sync.seek"
            | "sync.playback"
    )
}

fn relay_client_packet_to_peers(
    clients: &mut [tungstenite::WebSocket<TcpStream>],
    sender_index: usize,
    event_tx: &Sender<BridgeEvent>,
    op: &str,
    encoded: &str,
) {
    for (index, client) in clients.iter_mut().enumerate() {
        if index == sender_index {
            continue;
        }
        match send_text_with_retry(client, encoded) {
            Ok(_) => {
                let _ = event_tx.send(BridgeEvent::PacketSent { op: op.to_owned() });
            }
            Err(error) => {
                let _ = event_tx.send(BridgeEvent::SendFailed {
                    op: op.to_owned(),
                    message: format!("Video bridge relay failed: {error}"),
                });
            }
        }
    }
}

fn send_to_clients(
    clients: &mut Vec<tungstenite::WebSocket<TcpStream>>,
    event_tx: &Sender<BridgeEvent>,
    op: String,
    encoded: String,
) {
    let mut index = 0;
    while index < clients.len() {
        match send_text_with_retry(&mut clients[index], &encoded) {
            Ok(_) => {
                let _ = event_tx.send(BridgeEvent::PacketSent { op: op.clone() });
                index += 1;
            }
            Err(tungstenite::Error::Io(error)) if error.kind() == ErrorKind::WouldBlock => {
                let _ = event_tx.send(BridgeEvent::SendFailed {
                    op: op.clone(),
                    message: "Studio connection is busy; try again.".to_owned(),
                });
                index += 1;
            }
            Err(error) => {
                clients.remove(index);
                let _ = event_tx.send(BridgeEvent::ClientDisconnected);
                let _ = event_tx.send(BridgeEvent::SendFailed {
                    op: op.clone(),
                    message: format!("Studio send failed: {error}"),
                });
            }
        }
    }
}

fn send_text_with_retry(
    client: &mut tungstenite::WebSocket<TcpStream>,
    encoded: &str,
) -> tungstenite::Result<()> {
    for _ in 0..3 {
        match client.send(Message::Text(encoded.to_owned())) {
            Err(tungstenite::Error::Io(error)) if error.kind() == ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(3));
            }
            result => return result,
        }
    }
    client.send(Message::Text(encoded.to_owned()))
}

fn send_auto_reply(client: &mut tungstenite::WebSocket<TcpStream>, packet: &VideoPacket) {
    let reply = match packet.op.as_str() {
        "hello" => Some(make_packet(
            "hello.ok",
            json!({
                "side": "phase-rust-companion",
                "app": "Phase Auto Updater",
                "protocol": VERSION,
            }),
            &packet.token,
            Some(packet.id.clone()),
        )),
        "ping" => Some(make_packet(
            "pong",
            json!({
                "sent_at": packet.sent_at,
                "side": "phase-rust-companion",
            }),
            &packet.token,
            Some(packet.id.clone()),
        )),
        _ => None,
    };
    if let Some(reply) = reply.and_then(|packet| serde_json::to_string(&packet).ok()) {
        let _ = client.send(Message::Text(reply));
    }
}

fn make_packet(op: &str, payload: Value, token: &str, reply_to: Option<String>) -> VideoPacket {
    VideoPacket {
        v: VERSION.to_owned(),
        id: Uuid::new_v4().to_string(),
        op: op.to_owned(),
        reply_to,
        token: token.to_owned(),
        sent_at: unix_now_seconds(),
        payload,
    }
}

fn error_response(status: u16, message: &str) -> ErrorResponse {
    let mut response = ErrorResponse::new(Some(message.to_owned()));
    *response.status_mut() = tungstenite::http::StatusCode::from_u16(status)
        .unwrap_or(tungstenite::http::StatusCode::BAD_REQUEST);
    response
}

fn clean_path(path: &str) -> String {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return DEFAULT_PATH.to_owned();
    }
    if trimmed.starts_with('/') {
        trimmed.to_owned()
    } else {
        format!("/{trimmed}")
    }
}

fn unix_now_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs_f64())
        .unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::thread;
    use std::time::Instant;

    #[test]
    fn detects_reference_kind() {
        assert_eq!(
            source_kind_for("https://www.youtube.com/watch?v=abc"),
            ReferenceKind::Youtube
        );
        assert_eq!(source_kind_for("C:/tmp/ref.mp4"), ReferenceKind::LocalFile);
        assert_eq!(
            source_kind_for("https://example.com/ref"),
            ReferenceKind::External
        );
    }

    #[test]
    fn reference_payload_uses_protocol_field_names() {
        let draft = ReferenceDraft {
            source_kind: ReferenceKind::Youtube,
            source: "https://youtu.be/example".to_owned(),
            title: "Walk".to_owned(),
            duration_seconds: 12.5,
            fps: 60.0,
            start_frame: 10,
            offset_seconds: 0.25,
            playback_rate: 1.0,
        };
        let payload = draft.payload();
        assert_eq!(payload["source_kind"], "YouTube");
        assert_eq!(payload["source"], "https://youtu.be/example");
        assert_eq!(payload["start_frame"], 10);
    }

    #[test]
    fn youtube_urls_render_iframe_player() {
        let draft = ReferenceDraft {
            source_kind: ReferenceKind::Youtube,
            source: "https://www.youtube.com/watch?v=dQw4w9WgXcQ".to_owned(),
            title: "YT".to_owned(),
            duration_seconds: 0.0,
            fps: 60.0,
            start_frame: 0,
            offset_seconds: 0.0,
            playback_rate: 1.0,
        };
        let html = render_player_html(&draft).expect("render youtube player");
        assert!(html.contains("https://www.youtube.com/embed/dQw4w9WgXcQ"));
        assert!(html.contains("referrerpolicy=\"origin\""));
        assert!(html.contains("origin=__PHASE_YOUTUBE_ORIGIN__"));
        assert!(html.contains("<iframe"));
        assert!(html.contains("https://www.youtube.com/iframe_api"));
        assert!(html.contains("new YT.Player('phase-media'"));
        assert!(html.contains("player.getCurrentTime"));
        assert!(html.contains("player.seekTo(target, true)"));
        assert!(html.contains("function requestPlayback(shouldPlay)"));
        assert!(html.contains("function enforceDesiredPlayback()"));
        assert!(html.contains("function suppressLocalSyncFor(ms)"));
        assert!(html.contains("pendingRemotePlayback"));
        assert!(html.contains("payload.playing === false) requestPlayback(false)"));
        assert!(html.contains("payload.playing === true) requestPlayback(true)"));
        assert!(html.contains("phaseDrivenPlayback"));
        assert!(html.contains("reason === 'playback_correction'"));
        assert!(html.contains("passivePlaybackHint"));
        assert!(html.contains("Math.max(0.85, frameStep * 48)"));
        assert!(html.contains("lastRemoteCorrectionAt < 1000"));
        assert!(html.contains("sync.playback"));
        assert!(html.contains("sync.seek"));
        assert!(html.contains("sync.timeline"));
        assert!(html.contains("youtube_paused_seek"));
        assert!(html.contains("setInterval(sendTimelineTick, 100)"));
        assert!(!html.contains("aria-label=\"YouTube controls\""));
        assert!(!html.contains("id=\"phase-play\""));
        assert!(!html.contains("id=\"phase-scrub\""));
        assert!(!html.contains("id=\"phase-volume-slider\""));
    }

    #[test]
    fn query_component_encoding_escapes_local_origin() {
        assert_eq!(
            percent_encode_query_component("http://127.0.0.1:5050"),
            "http%3A%2F%2F127.0.0.1%3A5050"
        );
    }

    #[test]
    fn local_mp4_renders_video_player() {
        let draft = ReferenceDraft {
            source_kind: ReferenceKind::LocalFile,
            source: "C:/tmp/reference clip.mp4".to_owned(),
            title: "Clip".to_owned(),
            duration_seconds: 0.0,
            fps: 24.0,
            start_frame: 0,
            offset_seconds: 0.0,
            playback_rate: 1.0,
        };
        let html = render_player_html(&draft).expect("render mp4 player");
        assert!(html.contains("<video"));
        assert!(html.contains("video-shade"));
        assert!(html.contains("phase-controls"));
        assert!(html.contains("control-button primary"));
        assert!(html.contains("phase-preview-source"));
        assert!(html.contains("scrub-preview"));
        assert!(html.contains("phase-preview-canvas"));
        assert!(html.contains("control-strip"));
        assert!(html.contains("display: flex"));
        assert!(html.contains("justify-content: center"));
        assert!(!html.contains("grid-template-columns: minmax(0, 1fr) auto minmax(0, 1fr)"));
        assert!(html.contains("frame-group"));
        assert!(html.contains("utility-group"));
        assert!(html.contains("play-button"));
        assert!(html.contains("custom-rate"));
        assert!(html.contains("rate-menu"));
        assert!(html.contains("phase-volume-slider"));
        assert!(html.contains("phase-volume-value"));
        assert!(html.contains("volume-control"));
        assert!(html.contains("setVolume(Number(volumeSlider.value) / 100)"));
        assert!(html.contains("overflow: visible"));
        assert!(html.contains("z-index: 70"));
        assert!(html.contains("min-width: 128px"));
        assert!(!html.contains("<select"));
        assert!(!html.contains("<video controls"));
        assert!(!html.contains(" controls preload"));
        assert!(!html.contains("<footer"));
        assert!(!html.contains("body:hover"));
        assert!(html.contains("reference%20clip.mp4"));
        assert!(html.contains("new WebSocket(bridgeUrl)"));
        assert!(html.contains("sync.playback"));
        assert!(html.contains("sync.seek"));
        assert!(html.contains("function applyRemoteSeconds(seconds, force)"));
        assert!(html.contains("function requestPlayback(shouldPlay)"));
        assert!(html.contains("function suppressLocalSyncFor(ms)"));
        assert!(html.contains("payload.playing === false) requestPlayback(false)"));
        assert!(html.contains("payload.playing === true) requestPlayback(true)"));
        assert!(!html.contains("payload.playing === false && !video.paused"));
        assert!(html.contains("function playbackRateFromPayload(payload)"));
        assert!(html.contains("playback_rate: video.playbackRate"));
        assert!(html.contains("reason: 'rate_change'"));
        assert!(html.contains("canSendLocalSync()"));
        assert!(html.contains("Math.max(0.12, Math.min(0.24, frameStep * 12))"));
        assert!(html.contains("function seekTo(seconds)"));
        assert!(html.contains("media-popover"));
        assert!(html.contains("media-popover-pinned"));
        assert!(html.contains("function positionMediaPopover()"));
        assert!(html.contains("--popover-left"));
        assert!(html.contains("position: fixed"));
        assert!(html.contains("window.addEventListener('resize', positionMediaPopover)"));
        assert!(!html.contains("left: -70px"));
        assert!(html.contains("aria-pressed=\"false\""));
        assert!(html.contains("mediaKind?.setAttribute('aria-pressed'"));
        assert!(html.contains(".media-pill[aria-pressed=\"true\"]"));
        assert!(html.contains("Pinned open."));
        assert!(html.contains("pin-state-pinned"));
        assert!(html.contains("<svg viewBox=\"0 0 24 24\""));
        assert!(html.contains("stroke-linecap=\"round\""));
        assert!(!html.contains("<svg viewBox=\"0 0 16 16\""));
        assert!(!html.contains("min-width: 28px"));
        assert!(!html.contains("rotate(-12deg)"));
        assert!(html.contains("Click outside to close."));
        assert!(html.contains("Show folder"));
        assert!(html.contains("Swap video"));
        assert!(html.contains("popupCommand('show-folder'"));
        assert!(html.contains("__PHASE_POPUP_COMMAND_URL"));
        assert!(!html.contains("window.ipc"));
        assert!(html.contains("window.phaseVideoReferenceSwap"));
        assert!(html.contains("scheduleChromeHide"));
        assert!(html.contains("document.addEventListener('mouseout'"));
        assert!(html.contains("new Image()"));
        assert!(html.contains("showScrubPreview"));
        assert!(html.contains("#phase-volume-value"));
        assert!(html.contains("display: none"));
        assert!(html.contains("@media (max-height: 300px)"));
        assert!(!html.contains(".control-group:hover"));
        assert!(html.contains("pointerleave"));
        assert!(html.contains("controls-visible"));
        assert!(html.contains("transition: opacity 180ms ease"));
        assert!(html.contains("color-scheme: dark"));
        // Flat, solid chrome: no glass blur or glow, icon controls.
        assert!(!html.contains("backdrop-filter"));
        assert!(!html.contains("box-shadow"));
        assert!(html.contains("class=\"icon-play\""));
        assert!(html.contains("class=\"icon-pause\""));
        assert!(html.contains("mute.classList.toggle('is-muted'"));
        assert!(html.contains("fit.classList.toggle('is-fill'"));
        assert!(html.contains("function paintSlider(slider)"));
    }

    /// Writes the player page for visual review:
    /// `PHASE_PLAYER_PREVIEW=<video path> cargo test dump_player_preview -- --ignored`
    #[test]
    #[ignore]
    fn dump_player_preview() {
        let source = std::env::var("PHASE_PLAYER_PREVIEW").expect("set PHASE_PLAYER_PREVIEW");
        let draft = ReferenceDraft {
            source_kind: ReferenceKind::LocalFile,
            title: default_title_for(&source),
            source,
            duration_seconds: 0.0,
            fps: 60.0,
            start_frame: 0,
            offset_seconds: 0.0,
            playback_rate: 1.0,
        };
        let html = render_player_html(&draft).expect("render player");
        std::fs::write(std::env::temp_dir().join("phase-player-preview.html"), html).unwrap();
    }

    #[test]
    fn windows_file_urls_do_not_include_verbatim_prefix() {
        let url = file_url(Path::new(
            r"C:\Users\aaron\AppData\Local\Temp\PhaseAnimatorVideoReference\player.html",
        ));
        assert_eq!(
            url,
            "file:///C:/Users/aaron/AppData/Local/Temp/PhaseAnimatorVideoReference/player.html"
        );
    }

    #[test]
    fn popup_window_icon_loads_phase_icon() {
        assert!(popup_window_icon().is_some());
    }

    #[test]
    fn popup_swap_payload_uses_file_url_and_title() {
        let payload = popup_swap_payload(Path::new("C:/tmp/new clip.mp4"));
        assert_eq!(payload["source"], "C:/tmp/new clip.mp4");
        assert_eq!(payload["title"], "new clip");
        assert_eq!(payload["src"], "file:///C:/tmp/new%20clip.mp4");

        let script = popup_swap_script(Some(Path::new("C:/tmp/new clip.mp4")));
        assert!(script.contains("phaseVideoReferenceSwap"));
        assert!(script.contains("new%20clip.mp4"));
    }

    #[test]
    fn popup_query_commands_decode_source() {
        let message = popup_message_from_query(
            "op=show-folder&source=C%3A%5CUsers%5Caaron%5CDownloads%5Csneakpeak.mp4",
        );
        assert_eq!(message["op"], "show-folder");
        assert_eq!(
            message["payload"]["source"],
            r"C:\Users\aaron\Downloads\sneakpeak.mp4"
        );

        let (path, query) = split_request_target(
            "/phase-video-popup-command?op=swap-video&source=C%3A%5Ctmp%5Cclip.mp4",
        );
        assert_eq!(path, POPUP_COMMAND_PATH);
        assert!(query.contains("swap-video"));
    }

    #[test]
    fn server_accepts_client_and_sends_packet() {
        let port = free_local_port();
        let bridge = VideoReferenceBridge::start(BridgeConfig {
            port,
            path: DEFAULT_PATH.to_owned(),
            token: String::new(),
        });
        wait_for_event(&bridge, |event| {
            matches!(event, BridgeEvent::Listening { .. })
        });

        let (mut socket, _) = tungstenite::connect(format!("ws://127.0.0.1:{port}{DEFAULT_PATH}"))
            .expect("connect to video bridge");
        if let tungstenite::stream::MaybeTlsStream::Plain(stream) = socket.get_mut() {
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .expect("set read timeout");
        }
        wait_for_event(&bridge, |event| {
            matches!(event, BridgeEvent::ClientConnected)
        });

        let hello = serde_json::to_string(&make_packet("hello", json!({}), "", None))
            .expect("encode hello");
        socket.send(Message::Text(hello)).expect("send hello");
        let reply = socket
            .read()
            .expect("read hello reply")
            .into_text()
            .expect("text hello reply");
        let reply: VideoPacket = serde_json::from_str(&reply).expect("decode hello reply");
        assert_eq!(reply.op, "hello.ok");

        bridge.send("ping", json!({ "ok": true }), None);
        wait_for_event(
            &bridge,
            |event| matches!(event, BridgeEvent::PacketSent { op } if op == "ping"),
        );
        let packet = socket
            .read()
            .expect("read bridge packet")
            .into_text()
            .expect("text bridge packet");
        let packet: VideoPacket = serde_json::from_str(&packet).expect("decode bridge packet");
        assert_eq!(packet.v, VERSION);
        assert_eq!(packet.op, "ping");
        assert_eq!(packet.payload["ok"], true);

        bridge.stop();
    }

    #[test]
    fn server_relays_popup_sync_to_other_clients() {
        let port = free_local_port();
        let bridge = VideoReferenceBridge::start(BridgeConfig {
            port,
            path: DEFAULT_PATH.to_owned(),
            token: String::new(),
        });
        wait_for_event(&bridge, |event| {
            matches!(event, BridgeEvent::Listening { .. })
        });

        let (mut studio, _) = tungstenite::connect(format!("ws://127.0.0.1:{port}{DEFAULT_PATH}"))
            .expect("connect studio client");
        if let tungstenite::stream::MaybeTlsStream::Plain(stream) = studio.get_mut() {
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .expect("set studio read timeout");
        }
        wait_for_event(&bridge, |event| {
            matches!(event, BridgeEvent::ClientConnected)
        });

        let (mut popup, _) = tungstenite::connect(format!("ws://127.0.0.1:{port}{DEFAULT_PATH}"))
            .expect("connect popup client");
        wait_for_event(&bridge, |event| {
            matches!(event, BridgeEvent::ClientConnected)
        });

        let playback = serde_json::to_string(&make_packet(
            "sync.playback",
            json!({ "seconds": 1.25, "playing": false }),
            "",
            None,
        ))
        .expect("encode playback packet");
        popup
            .send(Message::Text(playback))
            .expect("send popup playback");

        let relayed = studio
            .read()
            .expect("read relayed playback")
            .into_text()
            .expect("text relayed playback");
        let relayed: VideoPacket = serde_json::from_str(&relayed).expect("decode relayed packet");
        assert_eq!(relayed.op, "sync.playback");
        assert_eq!(relayed.payload["seconds"], 1.25);
        assert_eq!(relayed.payload["playing"], false);

        bridge.stop();
    }

    fn wait_for_event(
        bridge: &VideoReferenceBridge,
        mut predicate: impl FnMut(&BridgeEvent) -> bool,
    ) {
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(2) {
            for event in bridge.poll() {
                if predicate(&event) {
                    return;
                }
            }
            thread::sleep(Duration::from_millis(20));
        }
        panic!("timed out waiting for video bridge event");
    }

    fn free_local_port() -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
        listener.local_addr().expect("ephemeral addr").port()
    }
}
