//! Home Assistant bridge over MQTT discovery.
//!
//! The bridge is a client of the daemon's own loopback HTTP API, not of its
//! internals: every command goes through the same validation as the dashboard,
//! and the daemon keeps working unchanged when MQTT is disabled or the broker
//! is down. Home Assistant (and HomeKit Bridge on top of it) then sees the
//! daemon as one device with switches, selects and numbers.
//!
//! Topics, with `<base>` defaulting to `lg-hue-sync`:
//!
//! - `<base>/availability`  `online`/`offline` (retained, last-will)
//! - `<base>/state`         one retained JSON document with every entity value
//! - `<base>/<entity>/set`  command topic for each controllable entity
//!
//! Changes made over MQTT are live, like dashboard changes before **Save**;
//! nothing here writes the configuration file.

use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use rumqttc::{AsyncClient, Event, LastWill, MqttOptions, Packet, QoS};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use tokio::sync::mpsc;
use tokio::time::{interval, sleep, MissedTickBehavior};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

const DEVICE_NAME: &str = "LG TV Ambient Sync";

#[derive(Clone, Serialize, Deserialize)]
pub struct MqttConfig {
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    #[serde(default = "default_base_topic")]
    pub base_topic: String,
    #[serde(default = "default_discovery_prefix")]
    pub discovery_prefix: String,
}

fn default_enabled() -> bool {
    true
}
fn default_port() -> u16 {
    1883
}
fn default_base_topic() -> String {
    "lg-hue-sync".into()
}
fn default_discovery_prefix() -> String {
    "homeassistant".into()
}

// `Config` derives Debug and is logged in places; never expose the broker password.
impl std::fmt::Debug for MqttConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MqttConfig")
            .field("enabled", &self.enabled)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("username", &self.username)
            .field("password", &self.password.as_ref().map(|_| "<redacted>"))
            .field("base_topic", &self.base_topic)
            .field("discovery_prefix", &self.discovery_prefix)
            .finish()
    }
}

impl MqttConfig {
    pub fn validate(&self) -> Result<()> {
        if self.host.trim().is_empty() {
            bail!("mqtt.host must not be empty");
        }
        if self.port == 0 {
            bail!("mqtt.port must not be 0");
        }
        for (name, topic) in [
            ("base_topic", &self.base_topic),
            ("discovery_prefix", &self.discovery_prefix),
        ] {
            let bad = topic.is_empty()
                || topic.starts_with('/')
                || topic.ends_with('/')
                || topic.contains(['#', '+', '\0'])
                || topic.contains("//");
            if bad {
                bail!("mqtt.{name} must be a plain topic without wildcards or leading/trailing slashes");
            }
        }
        Ok(())
    }

    /// Stable identifier derived from the base topic, safe in topics and unique ids.
    fn node_id(&self) -> String {
        self.base_topic
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect()
    }

    fn availability_topic(&self) -> String {
        format!("{}/availability", self.base_topic)
    }
    fn state_topic(&self) -> String {
        format!("{}/state", self.base_topic)
    }
    fn command_topic(&self, entity: &str) -> String {
        format!("{}/{entity}/set", self.base_topic)
    }
}

// ---------------------------------------------------------------------------
// Entity model
// ---------------------------------------------------------------------------

/// UI labels exposed to Home Assistant, matched to the dashboard's own tables.
const COLOUR: [(&str, &str); 3] = [
    ("Soft", "softColor"),
    ("Default", "neutral"),
    ("Vivid", "vividColor"),
];
/// (label, smoothing, rise, fall, max colour step)
const RESPONSE: [(&str, [f64; 3], u64); 4] = [
    ("Slow", [0.2, 0.18, 0.22], 6),
    ("Default", [0.35, 0.35, 0.35], 12),
    ("Fast", [0.65, 0.65, 0.7], 64),
    ("Instant", [1.0, 1.0, 1.0], 255),
];
/// Response mode driven by on-screen energy (see `color::energy`).
const DYNAMIC: &str = "Dynamic";
const BLEND: [(&str, f64); 3] = [("Local", 0.0), ("Balanced", 0.15), ("Soft", 0.3)];

/// Everything Instant sets besides the response curve. Keep in sync with
/// `instantBundle` in frontend/src/lib/settings.ts.
#[allow(clippy::type_complexity)]
const INSTANT_BUNDLE: [(&str, fn() -> Value); 5] = [
    ("gentle_room_lights", || false.into()),
    ("black_frame_hold_ms", || 0.into()),
    ("black_frame_fade_ms", || 0.into()),
    ("instant_scene_cuts", || true.into()),
    ("strobe_boost", || true.into()),
];

struct NumberSpec {
    id: &'static str,
    name: &'static str,
    setting: &'static str,
    min: f64,
    max: f64,
    step: f64,
    icon: &'static str,
}

const NUMBERS: [NumberSpec; 5] = [
    NumberSpec {
        id: "brightness",
        name: "Brightness",
        setting: "brightness_multiplier",
        min: 0.5,
        max: 2.5,
        step: 0.05,
        icon: "mdi:brightness-6",
    },
    NumberSpec {
        id: "hue_brightness",
        name: "Hue brightness",
        setting: "hue_output_brightness",
        min: 0.0,
        max: 2.5,
        step: 0.05,
        icon: "mdi:lightbulb-on",
    },
    NumberSpec {
        id: "nanoleaf_brightness",
        name: "Nanoleaf brightness",
        setting: "nanoleaf_output_brightness",
        min: 0.0,
        max: 2.5,
        step: 0.05,
        icon: "mdi:hexagon-multiple",
    },
    NumberSpec {
        id: "saturation",
        name: "Saturation",
        setting: "saturation_boost",
        min: 0.5,
        max: 2.5,
        step: 0.05,
        icon: "mdi:palette",
    },
    NumberSpec {
        id: "gamma",
        name: "Gamma",
        setting: "gamma",
        min: 0.5,
        max: 2.0,
        step: 0.05,
        icon: "mdi:contrast-circle",
    },
];

fn labels<T>(table: &[(&'static str, T)]) -> Vec<&'static str> {
    table.iter().map(|(label, _)| *label).collect()
}

/// Every discovery message as `(topic, retained JSON payload)`.
pub fn discovery_messages(cfg: &MqttConfig) -> Vec<(String, String)> {
    let node = cfg.node_id();
    let device = json!({
        "identifiers": [node],
        "name": DEVICE_NAME,
        "manufacturer": "lg-hue-sync",
        "model": "webOS ambient sync daemon",
    });
    let mut out = Vec::new();
    let mut add = |component: &str, id: &str, name: &str, extra: Value| {
        let mut body = json!({
            "name": name,
            "unique_id": format!("{node}_{id}"),
            "has_entity_name": true,
            "device": device,
            "availability_topic": cfg.availability_topic(),
            "state_topic": cfg.state_topic(),
            "value_template": format!("{{{{ value_json.{id} }}}}"),
        });
        body.as_object_mut()
            .unwrap()
            .extend(extra.as_object().cloned().unwrap_or_default());
        out.push((
            format!("{}/{component}/{node}/{id}/config", cfg.discovery_prefix),
            body.to_string(),
        ));
    };

    let switch = |id: &str, icon: &str| {
        json!({
            "command_topic": cfg.command_topic(id),
            "payload_on": "ON", "payload_off": "OFF",
            "state_on": "ON", "state_off": "OFF",
            "icon": icon,
        })
    };
    add(
        "switch",
        "screen_sync",
        "Screen sync",
        switch("screen_sync", "mdi:television-ambient-light"),
    );
    add(
        "switch",
        "hue_sync",
        "Hue sync",
        switch("hue_sync", "mdi:lightbulb-group"),
    );
    add(
        "switch",
        "nanoleaf_sync",
        "Nanoleaf sync",
        switch("nanoleaf_sync", "mdi:hexagon-multiple"),
    );
    add(
        "switch",
        "display",
        "Display",
        switch("display", "mdi:television"),
    );
    add(
        "switch",
        "strobe_boost",
        "Strobe boost",
        switch("strobe_boost", "mdi:flash"),
    );

    let select = |id: &str, options: Vec<&str>, icon: &str| json!({"command_topic": cfg.command_topic(id), "options": options, "icon": icon});
    add(
        "select",
        "colour",
        "Colour style",
        select("colour", labels(&COLOUR), "mdi:palette-swatch"),
    );
    let mut response: Vec<&str> = RESPONSE.iter().map(|(l, _, _)| *l).collect();
    response.push(DYNAMIC);
    add(
        "select",
        "response",
        "Response",
        select("response", response, "mdi:speedometer"),
    );
    add(
        "select",
        "blend",
        "Spatial blending",
        select("blend", labels(&BLEND), "mdi:blur"),
    );

    for n in &NUMBERS {
        add(
            "number",
            n.id,
            n.name,
            json!({
                "command_topic": cfg.command_topic(n.id),
                "min": n.min, "max": n.max, "step": n.step,
                "mode": "slider", "icon": n.icon,
            }),
        );
    }

    for (id, name) in [
        ("hue_connected", "Hue connected"),
        ("nanoleaf_connected", "Nanoleaf connected"),
    ] {
        add(
            "binary_sensor",
            id,
            name,
            json!({
                "payload_on": "ON", "payload_off": "OFF",
                "device_class": "connectivity",
                "entity_category": "diagnostic",
            }),
        );
    }
    add(
        "sensor",
        "pipeline_state",
        "Pipeline state",
        json!({"entity_category": "diagnostic", "icon": "mdi:state-machine"}),
    );
    add(
        "sensor",
        "tv_power",
        "TV power state",
        json!({"entity_category": "diagnostic", "icon": "mdi:power"}),
    );
    add(
        "sensor",
        "energy",
        "Screen energy",
        json!({
            "unit_of_measurement": "%",
            "state_class": "measurement",
            "icon": "mdi:lightning-bolt",
        }),
    );
    add(
        "sensor",
        "light_fps",
        "Light update rate",
        json!({
            "entity_category": "diagnostic",
            "unit_of_measurement": "fps",
            "state_class": "measurement",
            "icon": "mdi:sine-wave",
        }),
    );
    out
}

fn on_off(value: bool) -> Value {
    Value::from(if value { "ON" } else { "OFF" })
}

fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-4
}

/// Mirrors the dashboard: sync counts as on while running or requested and not parked.
fn sync_requested(status: &Value) -> bool {
    let state = status["pipeline_state"].as_str().unwrap_or("");
    state == "running"
        || (!matches!(state, "paused" | "faulted") && status["requested_running"] == true)
}

/// Build the retained state document from an `/api/status` response and the
/// separately polled panel state. Values that cannot be determined (an
/// unrecognised preset, a standby panel) are omitted so Home Assistant shows
/// them as unknown instead of a wrong value.
pub fn state_from(status: &Value, display: Option<bool>) -> Value {
    let mut state = Map::new();
    let settings = &status["settings"];
    let flag = |key: &str| settings[key].as_bool();

    state.insert("screen_sync".into(), on_off(sync_requested(status)));
    if let Some(v) = flag("hue_sync_enabled") {
        state.insert("hue_sync".into(), on_off(v));
    }
    if let Some(v) = flag("nanoleaf_sync_enabled") {
        state.insert("nanoleaf_sync".into(), on_off(v));
    }
    if let Some(v) = flag("strobe_boost") {
        state.insert("strobe_boost".into(), on_off(v));
    }
    if let Some(v) = display {
        state.insert("display".into(), on_off(v));
    }
    state.insert(
        "hue_connected".into(),
        on_off(status["hue_connected"] == true),
    );
    state.insert(
        "nanoleaf_connected".into(),
        on_off(status["nanoleaf_connected"] == true),
    );
    if let Some(v) = status["pipeline_state"].as_str() {
        state.insert("pipeline_state".into(), v.into());
    }
    if let Some(v) = status["tv_power_state"].as_str() {
        state.insert("tv_power".into(), v.into());
    }
    if let Some(v) = status["energy"].as_f64() {
        state.insert("energy".into(), (v * 100.0).round().into());
    }
    if let Some(v) = status["light_update_fps"].as_f64() {
        state.insert("light_fps".into(), round2(v).into());
    }

    if let Some(preset) = status["active_preset"].as_str() {
        if let Some((label, _)) = COLOUR.iter().find(|(_, name)| *name == preset) {
            state.insert("colour".into(), (*label).into());
        }
    }
    let f = |key: &str| settings[key].as_f64();
    if let (Some(s), Some(r), Some(fall), Some(step)) = (
        f("smoothing_factor"),
        f("rise_smoothing_factor"),
        f("fall_smoothing_factor"),
        settings["max_color_step"].as_u64(),
    ) {
        if settings["dynamic_response"] == true {
            state.insert("response".into(), DYNAMIC.into());
        } else if let Some((label, _, _)) = RESPONSE.iter().find(|(_, v, max)| {
            close(v[0], s) && close(v[1], r) && close(v[2], fall) && *max == step
        }) {
            state.insert("response".into(), (*label).into());
        }
    }
    if let Some(blend) = f("spatial_blend") {
        if let Some((label, _)) = BLEND.iter().find(|(_, v)| close(*v, blend)) {
            state.insert("blend".into(), (*label).into());
        }
    }
    for n in &NUMBERS {
        if let Some(v) = f(n.setting) {
            state.insert(n.id.into(), round2(v).into());
        }
    }
    Value::Object(state)
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// A request to the daemon's own HTTP API.
#[derive(Debug, PartialEq)]
pub struct ApiRequest {
    pub path: String,
    pub body: Option<Value>,
}

fn post(path: &str) -> ApiRequest {
    ApiRequest {
        path: path.into(),
        body: None,
    }
}

/// `/api/settings` replaces the whole settings object, so patch the freshest status.
fn settings_patch(status: &Value, patch: &[(&str, Value)]) -> Result<ApiRequest> {
    let mut settings = status["settings"]
        .as_object()
        .cloned()
        .ok_or_else(|| anyhow!("daemon status has no settings"))?;
    for (key, value) in patch {
        settings.insert((*key).into(), value.clone());
    }
    Ok(ApiRequest {
        path: "/api/settings".into(),
        body: Some(Value::Object(settings)),
    })
}

fn parse_on_off(payload: &str) -> Result<bool> {
    match payload.trim().to_ascii_uppercase().as_str() {
        "ON" | "TRUE" | "1" => Ok(true),
        "OFF" | "FALSE" | "0" => Ok(false),
        other => bail!("expected ON or OFF, got {other:?}"),
    }
}

/// Translate a command (entity id and payload) into a daemon API request.
pub fn parse_command(entity: &str, payload: &str, status: &Value) -> Result<ApiRequest> {
    let payload = payload.trim();
    match entity {
        "screen_sync" => Ok(post(if parse_on_off(payload)? {
            "/api/start"
        } else {
            "/api/stop"
        })),
        "display" => Ok(post(if parse_on_off(payload)? {
            "/api/display/on"
        } else {
            "/api/display/off"
        })),
        "hue_sync" => settings_patch(
            status,
            &[("hue_sync_enabled", parse_on_off(payload)?.into())],
        ),
        "strobe_boost" => {
            settings_patch(status, &[("strobe_boost", parse_on_off(payload)?.into())])
        }
        "nanoleaf_sync" => settings_patch(
            status,
            &[("nanoleaf_sync_enabled", parse_on_off(payload)?.into())],
        ),
        "colour" => {
            let (_, name) = COLOUR
                .iter()
                .find(|(label, _)| label.eq_ignore_ascii_case(payload))
                .ok_or_else(|| anyhow!("unknown colour style {payload:?}"))?;
            Ok(post(&format!("/api/presets/{name}")))
        }
        "response" if payload.eq_ignore_ascii_case(DYNAMIC) => {
            // Matches the dashboard: Default as the calm base, energy does the rest.
            let mut patch = vec![
                ("dynamic_response", true.into()),
                ("strobe_boost", true.into()),
                ("smoothing_factor", 0.35.into()),
                ("rise_smoothing_factor", 0.35.into()),
                ("fall_smoothing_factor", 0.35.into()),
                ("max_color_step", 12.into()),
            ];
            let settings = &status["settings"];
            if settings["black_frame_hold_ms"] == 0 && settings["black_frame_fade_ms"] == 0 {
                patch.push(("black_frame_hold_ms", 750.into()));
                patch.push(("black_frame_fade_ms", 2000.into()));
            }
            settings_patch(status, &patch)
        }
        "response" => {
            let (_, [s, r, f], max) = RESPONSE
                .iter()
                .find(|(label, _, _)| label.eq_ignore_ascii_case(payload))
                .ok_or_else(|| anyhow!("unknown response {payload:?}"))?;
            let mut patch = vec![
                ("smoothing_factor", (*s).into()),
                ("rise_smoothing_factor", (*r).into()),
                ("fall_smoothing_factor", (*f).into()),
                ("max_color_step", (*max).into()),
                ("dynamic_response", false.into()),
            ];
            // Matches the dashboard: Instant applies the whole low-latency bundle.
            if *s >= 1.0 {
                patch.extend(INSTANT_BUNDLE.iter().map(|(k, v)| (*k, v())));
            }
            settings_patch(status, &patch)
        }
        "blend" => {
            let (_, v) = BLEND
                .iter()
                .find(|(label, _)| label.eq_ignore_ascii_case(payload))
                .ok_or_else(|| anyhow!("unknown blending {payload:?}"))?;
            settings_patch(status, &[("spatial_blend", (*v).into())])
        }
        other => {
            let spec = NUMBERS
                .iter()
                .find(|n| n.id == other)
                .ok_or_else(|| anyhow!("unknown entity {other:?}"))?;
            let value: f64 = payload
                .parse()
                .map_err(|_| anyhow!("{} expects a number, got {payload:?}", spec.name))?;
            if !value.is_finite() || value < spec.min || value > spec.max {
                bail!(
                    "{} must be between {} and {}",
                    spec.name,
                    spec.min,
                    spec.max
                );
            }
            settings_patch(status, &[(spec.setting, round2(value).into())])
        }
    }
}

/// Entity id from `<base>/<entity>/set`.
fn entity_from_topic<'a>(cfg: &MqttConfig, topic: &'a str) -> Option<&'a str> {
    let rest = topic
        .strip_prefix(cfg.base_topic.as_str())?
        .strip_prefix('/')?;
    let entity = rest.strip_suffix("/set")?;
    (!entity.is_empty() && !entity.contains('/')).then_some(entity)
}

// ---------------------------------------------------------------------------
// Transport
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct Api {
    base: String,
    agent: ureq::Agent,
}

impl Api {
    fn new(port: u16) -> Self {
        Self {
            base: format!("http://127.0.0.1:{port}"),
            agent: crate::local_http::agent(Some(Duration::from_secs(8))),
        }
    }

    async fn get(&self, path: &'static str) -> Result<Value> {
        let api = self.clone();
        tokio::task::spawn_blocking(move || {
            api.agent
                .get(format!("{}{path}", api.base))
                .call()
                .map_err(|e| crate::local_http::request_error("daemon API read", e))?
                .body_mut()
                .read_json::<Value>()
                .context("daemon API returned invalid JSON")
        })
        .await?
    }

    async fn send(&self, request: ApiRequest) -> Result<()> {
        let api = self.clone();
        tokio::task::spawn_blocking(move || {
            let url = format!("{}{}", api.base, request.path);
            let result = match &request.body {
                Some(body) => api.agent.post(url).send_json(body),
                None => api.agent.post(url).send_empty(),
            };
            result
                .map(|_| ())
                .map_err(|e| crate::local_http::request_error("daemon API command", e))
        })
        .await?
    }

    /// Panel state needs a Luna call, so it is polled less often than status.
    async fn display(&self) -> Option<bool> {
        self.get("/api/display").await.ok()?["enabled"].as_bool()
    }
}

type BrokerEvent = std::result::Result<Event, String>;

/// Run the bridge until `shutdown` fires. Broker outages are retried forever;
/// they never affect the daemon.
pub async fn run(cfg: MqttConfig, api_port: u16, shutdown: CancellationToken) {
    let api = Api::new(api_port);
    let mut options = MqttOptions::new(
        format!("lg-hue-sync-{}", cfg.node_id()),
        cfg.host.trim(),
        cfg.port,
    );
    options.set_keep_alive(Duration::from_secs(30));
    if let Some(user) = &cfg.username {
        options.set_credentials(user, cfg.password.clone().unwrap_or_default());
    }
    options.set_last_will(LastWill::new(
        cfg.availability_topic(),
        "offline",
        QoS::AtLeastOnce,
        true,
    ));
    let (client, mut eventloop) = AsyncClient::new(options, 64);

    // The event loop runs in its own task so a slow HTTP call can never stall keep-alives.
    let (tx, mut rx) = mpsc::channel::<BrokerEvent>(64);
    let loop_stop = CancellationToken::new();
    let loop_task = {
        let stop = loop_stop.clone();
        tokio::spawn(async move {
            loop {
                let event = tokio::select! {
                    _ = stop.cancelled() => break,
                    event = eventloop.poll() => event,
                };
                let failed = event.is_err();
                // Forward only what the main loop acts on; acks and pings would just fill the channel.
                let forward = match event {
                    Ok(Event::Incoming(Packet::ConnAck(_) | Packet::Publish(_))) | Err(_) => {
                        Some(event.map_err(|e| e.to_string()))
                    }
                    Ok(_) => None,
                };
                if let Some(event) = forward {
                    if tx.send(event).await.is_err() {
                        break;
                    }
                }
                if failed {
                    tokio::select! {
                        _ = stop.cancelled() => break,
                        _ = sleep(Duration::from_secs(5)) => {}
                    }
                }
            }
        })
    };

    info!(
        "MQTT bridge started for {}:{} (base topic {})",
        cfg.host, cfg.port, cfg.base_topic
    );
    let mut connected = false;
    let mut warned = false;
    let mut last_state = String::new();
    let mut display: Option<bool> = None;
    let mut tick = interval(Duration::from_secs(1));
    tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut ticks = 0u64;
    let mut refresh_now = false;

    loop {
        tokio::select! {
            _ = shutdown.cancelled() => break,
            event = rx.recv() => match event {
                None => break,
                Some(Ok(Event::Incoming(Packet::ConnAck(_)))) => {
                    info!("MQTT connected");
                    connected = true;
                    warned = false;
                    last_state.clear();
                    display = api.display().await;
                    if let Err(error) = announce(&client, &cfg).await {
                        warn!("MQTT discovery publish failed: {error}");
                    }
                    refresh_now = true;
                }
                Some(Ok(Event::Incoming(Packet::Publish(message)))) => {
                    let Some(entity) = entity_from_topic(&cfg, &message.topic) else { continue };
                    let payload = String::from_utf8_lossy(&message.payload).into_owned();
                    match handle_command(&api, entity, &payload).await {
                        Ok(()) => debug!("MQTT command {entity}={payload}"),
                        Err(error) => warn!("MQTT command {entity}={payload:?} rejected: {error}"),
                    }
                    if entity == "display" {
                        display = api.display().await;
                    }
                    refresh_now = true;
                }
                Some(Ok(_)) => {}
                Some(Err(error)) => {
                    connected = false;
                    if !warned {
                        warn!("MQTT broker unavailable ({error}); retrying every 5 s");
                        warned = true;
                    }
                }
            },
            _ = tick.tick() => {
                ticks += 1;
                if connected {
                    if ticks.is_multiple_of(5) {
                        display = api.display().await;
                    }
                    refresh_now = true;
                }
            }
        }
        if connected && refresh_now {
            refresh_now = false;
            refresh(&api, &client, &cfg, display, &mut last_state).await;
        }
    }

    // Say goodbye cleanly so Home Assistant marks entities unavailable immediately.
    let _ = client
        .publish(cfg.availability_topic(), QoS::AtLeastOnce, true, "offline")
        .await;
    let _ = client.disconnect().await;
    let _ = tokio::time::timeout(Duration::from_secs(1), async {
        while rx.recv().await.is_some() {}
    })
    .await;
    loop_stop.cancel();
    let _ = loop_task.await;
    info!("MQTT bridge stopped");
}

/// Publish the state document when it differs from what the broker last received.
async fn refresh(
    api: &Api,
    client: &AsyncClient,
    cfg: &MqttConfig,
    display: Option<bool>,
    last_state: &mut String,
) {
    match api.get("/api/status").await {
        Ok(status) => {
            let state = state_from(&status, display).to_string();
            if state != *last_state {
                match client
                    .publish(cfg.state_topic(), QoS::AtLeastOnce, true, state.clone())
                    .await
                {
                    Ok(()) => *last_state = state,
                    Err(error) => warn!("MQTT state publish failed: {error}"),
                }
            }
        }
        Err(error) => debug!("MQTT status poll failed: {error}"),
    }
}

async fn announce(client: &AsyncClient, cfg: &MqttConfig) -> Result<()> {
    for (topic, payload) in discovery_messages(cfg) {
        client
            .publish(topic, QoS::AtLeastOnce, true, payload)
            .await?;
    }
    client
        .publish(cfg.availability_topic(), QoS::AtLeastOnce, true, "online")
        .await?;
    client
        .subscribe(format!("{}/+/set", cfg.base_topic), QoS::AtLeastOnce)
        .await?;
    Ok(())
}

async fn handle_command(api: &Api, entity: &str, payload: &str) -> Result<()> {
    // Settings commands patch the freshest status so concurrent dashboard changes survive.
    let status = api.get("/api/status").await?;
    let request = parse_command(entity, payload, &status)?;
    api.send(request).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> MqttConfig {
        serde_json::from_value(json!({"host": "broker.local"})).unwrap()
    }

    fn status() -> Value {
        json!({
            "pipeline_state": "running",
            "requested_running": true,
            "active_preset": "neutral",
            "hue_connected": true,
            "nanoleaf_connected": false,
            "tv_power_state": "active",
            "light_update_fps": 29.987,
            "settings": {
                "brightness_multiplier": 1.0,
                "hue_output_brightness": 0.8,
                "nanoleaf_output_brightness": 1.0,
                "saturation_boost": 1.25,
                "gamma": 1.0,
                "spatial_blend": 0.15,
                "smoothing_factor": 0.35,
                "rise_smoothing_factor": 0.35,
                "fall_smoothing_factor": 0.35,
                "max_color_step": 12,
                "hue_sync_enabled": true,
                "nanoleaf_sync_enabled": false,
                "strict_blackout": false
            }
        })
    }

    #[test]
    fn defaults_and_validation() {
        let c = cfg();
        assert_eq!(c.port, 1883);
        assert_eq!(c.base_topic, "lg-hue-sync");
        assert_eq!(c.discovery_prefix, "homeassistant");
        assert!(c.enabled);
        c.validate().unwrap();

        for bad in ["", "/x", "x/", "a/#", "a/+/b", "a//b"] {
            let mut c = cfg();
            c.base_topic = bad.into();
            assert!(c.validate().is_err(), "{bad:?}");
        }
        let mut c = cfg();
        c.host = "  ".into();
        assert!(c.validate().is_err());
    }

    #[test]
    fn debug_output_never_contains_the_password() {
        let mut c = cfg();
        c.password = Some("hunter2".into());
        assert!(!format!("{c:?}").contains("hunter2"));
    }

    #[test]
    fn discovery_covers_every_entity_with_unique_ids() {
        let c = cfg();
        let messages = discovery_messages(&c);
        let mut ids = std::collections::HashSet::new();
        for (topic, payload) in &messages {
            assert!(topic.starts_with("homeassistant/") && topic.ends_with("/config"));
            let body: Value = serde_json::from_str(payload).unwrap();
            assert!(ids.insert(body["unique_id"].as_str().unwrap().to_owned()));
            assert_eq!(body["availability_topic"], "lg-hue-sync/availability");
            assert_eq!(body["state_topic"], "lg-hue-sync/state");
            assert_eq!(body["device"]["identifiers"][0], "lg_hue_sync");
            if let Some(command) = body["command_topic"].as_str() {
                assert!(command.starts_with("lg-hue-sync/") && command.ends_with("/set"));
            }
        }
        // 5 switches, 3 selects, 5 numbers, 2 binary sensors, 4 sensors.
        assert_eq!(messages.len(), 19);
    }

    #[test]
    fn every_advertised_option_is_a_valid_command() {
        let c = cfg();
        let s = status();
        for (_, payload) in discovery_messages(&c) {
            let body: Value = serde_json::from_str(&payload).unwrap();
            let Some(command) = body["command_topic"].as_str() else {
                continue;
            };
            let entity = entity_from_topic(&c, command).unwrap();
            let samples: Vec<String> = if let Some(options) = body["options"].as_array() {
                options
                    .iter()
                    .map(|o| o.as_str().unwrap().to_owned())
                    .collect()
            } else if body["min"].is_number() {
                vec![body["min"].to_string(), body["max"].to_string()]
            } else {
                vec!["ON".into(), "OFF".into()]
            };
            for sample in samples {
                parse_command(entity, &sample, &s)
                    .unwrap_or_else(|e| panic!("{entity}={sample}: {e}"));
            }
        }
    }

    #[test]
    fn state_reflects_status() {
        let state = state_from(&status(), Some(true));
        assert_eq!(state["screen_sync"], "ON");
        assert_eq!(state["hue_sync"], "ON");
        assert_eq!(state["nanoleaf_sync"], "OFF");
        assert_eq!(state["display"], "ON");
        assert_eq!(state["hue_connected"], "ON");
        assert_eq!(state["nanoleaf_connected"], "OFF");
        assert_eq!(state["colour"], "Default");
        assert_eq!(state["response"], "Default");
        assert_eq!(state["blend"], "Balanced");
        assert_eq!(state["hue_brightness"], 0.8);
        assert_eq!(state["saturation"], 1.25);
        assert_eq!(state["light_fps"], 29.99);
        assert_eq!(state["pipeline_state"], "running");
    }

    #[test]
    fn unknown_values_are_omitted_not_guessed() {
        let mut s = status();
        s["active_preset"] = json!("neonContrast");
        s["settings"]["smoothing_factor"] = json!(0.5);
        s["settings"]["spatial_blend"] = json!(0.2);
        let state = state_from(&s, None);
        assert!(state.get("colour").is_none());
        assert!(state.get("response").is_none());
        assert!(state.get("blend").is_none());
        assert!(state.get("display").is_none());
    }

    #[test]
    fn screen_sync_state_matches_the_dashboard_rules() {
        let on = |state: &str, requested: bool| {
            let mut s = status();
            s["pipeline_state"] = json!(state);
            s["requested_running"] = json!(requested);
            state_from(&s, None)["screen_sync"] == "ON"
        };
        assert!(on("running", false));
        assert!(on("starting", true));
        assert!(on("recovering", true));
        assert!(!on("paused", true));
        assert!(!on("faulted", true));
        assert!(!on("stopping", false));
    }

    #[test]
    fn commands_map_to_daemon_endpoints() {
        let s = status();
        assert_eq!(
            parse_command("screen_sync", "ON", &s).unwrap(),
            post("/api/start")
        );
        assert_eq!(
            parse_command("screen_sync", "off", &s).unwrap(),
            post("/api/stop")
        );
        assert_eq!(
            parse_command("display", "ON", &s).unwrap(),
            post("/api/display/on")
        );
        assert_eq!(
            parse_command("colour", "Vivid", &s).unwrap(),
            post("/api/presets/vividColor")
        );
        assert_eq!(
            parse_command("colour", "soft", &s).unwrap(),
            post("/api/presets/softColor")
        );
    }

    #[test]
    fn settings_commands_patch_the_whole_object() {
        let s = status();
        let request = parse_command("nanoleaf_sync", "ON", &s).unwrap();
        assert_eq!(request.path, "/api/settings");
        let body = request.body.unwrap();
        assert_eq!(body["nanoleaf_sync_enabled"], true);
        // Untouched fields travel unchanged, including ones this bridge does not know.
        assert_eq!(body["hue_output_brightness"], 0.8);
        assert_eq!(body["strict_blackout"], false);

        let body = parse_command("response", "Fast", &s).unwrap().body.unwrap();
        assert_eq!(body["smoothing_factor"], 0.65);
        assert_eq!(body["rise_smoothing_factor"], 0.65);
        assert_eq!(body["fall_smoothing_factor"], 0.7);
        assert_eq!(body["max_color_step"], 64);

        let body = parse_command("response", "Instant", &s)
            .unwrap()
            .body
            .unwrap();
        assert_eq!(body["smoothing_factor"], 1.0);
        assert_eq!(body["gentle_room_lights"], false);
        assert_eq!(body["max_color_step"], 255);
        assert_eq!(body["black_frame_hold_ms"], 0);
        assert_eq!(body["instant_scene_cuts"], true);
        assert_eq!(body["strobe_boost"], true);

        let body = parse_command("response", "Fast", &s).unwrap().body.unwrap();
        assert_eq!(body["dynamic_response"], false);

        let body = parse_command("response", "dynamic", &s)
            .unwrap()
            .body
            .unwrap();
        assert_eq!(body["dynamic_response"], true);
        assert_eq!(body["strobe_boost"], true);
        assert_eq!(body["smoothing_factor"], 0.35);

        let mut dynamic = s.clone();
        dynamic["settings"]["dynamic_response"] = json!(true);
        dynamic["energy"] = json!(0.426);
        let state = state_from(&dynamic, None);
        assert_eq!(state["response"], "Dynamic");
        assert_eq!(state["energy"], 43.0);

        let body = parse_command("blend", "Soft", &s).unwrap().body.unwrap();
        assert_eq!(body["spatial_blend"], 0.3);

        let body = parse_command("brightness", "1.234", &s)
            .unwrap()
            .body
            .unwrap();
        assert_eq!(body["brightness_multiplier"], 1.23);
    }

    #[test]
    fn invalid_commands_are_rejected_before_reaching_the_daemon() {
        let s = status();
        for (entity, payload) in [
            ("screen_sync", "maybe"),
            ("colour", "Neon"),
            ("response", "Warp"),
            ("blend", "Heavy"),
            ("brightness", "9"),
            ("brightness", "abc"),
            ("brightness", "NaN"),
            ("hue_brightness", "-1"),
            ("gamma", "2.5"),
            ("no_such_entity", "ON"),
        ] {
            assert!(
                parse_command(entity, payload, &s).is_err(),
                "{entity}={payload}"
            );
        }
        assert!(parse_command("hue_sync", "ON", &json!({})).is_err());
    }

    #[test]
    fn command_topics_resolve_to_entities() {
        let c = cfg();
        assert_eq!(
            entity_from_topic(&c, "lg-hue-sync/screen_sync/set"),
            Some("screen_sync")
        );
        assert_eq!(entity_from_topic(&c, "lg-hue-sync/state"), None);
        assert_eq!(entity_from_topic(&c, "lg-hue-sync//set"), None);
        assert_eq!(entity_from_topic(&c, "lg-hue-sync/a/b/set"), None);
        assert_eq!(entity_from_topic(&c, "other/screen_sync/set"), None);
        assert_eq!(entity_from_topic(&c, "lg-hue-syncx/screen_sync/set"), None);
    }

    /// Real rumqttc client and codec on both ends; only the broker and daemon API are stubs.
    #[tokio::test(flavor = "multi_thread")]
    async fn bridge_round_trip_against_fake_broker_and_api() {
        use bytes::BytesMut;
        use rumqttc::{ConnAck, ConnectReturnCode, PubAck, SubAck, SubscribeReasonCode};
        use std::sync::{Arc, Mutex};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        type Log = Arc<Mutex<Vec<(String, String)>>>;
        let posts: Log = Default::default();
        let published: Log = Default::default();

        // Stub of the daemon's loopback API.
        let api = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let api_port = api.local_addr().unwrap().port();
        let api_posts = posts.clone();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = api.accept().await {
                let posts = api_posts.clone();
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    let mut tmp = [0u8; 4096];
                    let (head, body) = loop {
                        let n = sock.read(&mut tmp).await.unwrap_or(0);
                        if n == 0 {
                            return;
                        }
                        buf.extend_from_slice(&tmp[..n]);
                        let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") else {
                            continue;
                        };
                        let head = String::from_utf8_lossy(&buf[..end]).to_string();
                        let length = head
                            .lines()
                            .find_map(|l| {
                                l.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|v| v.trim().parse::<usize>().unwrap())
                            })
                            .unwrap_or(0);
                        if buf.len() >= end + 4 + length {
                            break (
                                head,
                                String::from_utf8_lossy(&buf[end + 4..end + 4 + length])
                                    .to_string(),
                            );
                        }
                    };
                    let mut line = head.lines().next().unwrap().split(' ');
                    let (method, path) = (line.next().unwrap(), line.next().unwrap());
                    let reply = match (method, path) {
                        ("GET", "/api/status") => status().to_string(),
                        ("GET", "/api/display") => json!({"enabled": true}).to_string(),
                        _ => {
                            posts.lock().unwrap().push((path.to_owned(), body));
                            json!({"status": "ok"}).to_string()
                        }
                    };
                    let response = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{reply}",
                        reply.len()
                    );
                    let _ = sock.write_all(response.as_bytes()).await;
                });
            }
        });

        // Stub broker: acknowledges, records client publishes, and "sends" one HA command on subscribe.
        let broker = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mqtt_port = broker.local_addr().unwrap().port();
        let seen = published.clone();
        tokio::spawn(async move {
            let (mut sock, _) = broker.accept().await.unwrap();
            let mut inbuf = BytesMut::new();
            let mut tmp = [0u8; 8192];
            loop {
                let n = sock.read(&mut tmp).await.unwrap_or(0);
                if n == 0 {
                    return;
                }
                inbuf.extend_from_slice(&tmp[..n]);
                let mut out = BytesMut::new();
                while let Ok(packet) = Packet::read(&mut inbuf, 1 << 20) {
                    match packet {
                        Packet::Connect(_) => {
                            Packet::ConnAck(ConnAck::new(ConnectReturnCode::Success, false))
                                .write(&mut out, 1 << 20)
                                .unwrap();
                        }
                        Packet::Subscribe(sub) => {
                            let codes = vec![
                                SubscribeReasonCode::Success(QoS::AtLeastOnce);
                                sub.filters.len()
                            ];
                            Packet::SubAck(SubAck::new(sub.pkid, codes))
                                .write(&mut out, 1 << 20)
                                .unwrap();
                            let command = rumqttc::Publish::new(
                                "lg-hue-sync/colour/set",
                                QoS::AtMostOnce,
                                "Vivid",
                            );
                            Packet::Publish(command).write(&mut out, 1 << 20).unwrap();
                        }
                        Packet::Publish(p) => {
                            seen.lock().unwrap().push((
                                p.topic.clone(),
                                String::from_utf8_lossy(&p.payload).into_owned(),
                            ));
                            if p.qos == QoS::AtLeastOnce {
                                Packet::PubAck(PubAck::new(p.pkid))
                                    .write(&mut out, 1 << 20)
                                    .unwrap();
                            }
                        }
                        Packet::PingReq => {
                            Packet::PingResp.write(&mut out, 1 << 20).unwrap();
                        }
                        Packet::Disconnect => return,
                        _ => {}
                    }
                }
                if !out.is_empty() {
                    sock.write_all(&out).await.unwrap();
                }
            }
        });

        let mut config = cfg();
        config.host = "127.0.0.1".into();
        config.port = mqtt_port;
        let shutdown = CancellationToken::new();
        let bridge = tokio::spawn(run(config, api_port, shutdown.clone()));

        let wait = async {
            loop {
                let ready = {
                    let seen = published.lock().unwrap();
                    let has = |t: &str| seen.iter().any(|(topic, _)| topic == t);
                    seen.iter().filter(|(t, _)| t.ends_with("/config")).count() == 19
                        && has("lg-hue-sync/state")
                        && seen
                            .iter()
                            .any(|(t, p)| t == "lg-hue-sync/availability" && p == "online")
                        && !posts.lock().unwrap().is_empty()
                };
                if ready {
                    break;
                }
                sleep(Duration::from_millis(50)).await;
            }
        };
        tokio::time::timeout(Duration::from_secs(10), wait)
            .await
            .expect("bridge did not announce, publish state and apply the command in time");

        let state: Value = {
            let seen = published.lock().unwrap();
            let (_, payload) = seen
                .iter()
                .rev()
                .find(|(t, _)| t == "lg-hue-sync/state")
                .unwrap();
            serde_json::from_str(payload).unwrap()
        };
        assert_eq!(state["screen_sync"], "ON");
        assert_eq!(state["colour"], "Default");
        assert_eq!(state["display"], "ON");
        assert_eq!(posts.lock().unwrap()[0].0, "/api/presets/vividColor");

        shutdown.cancel();
        tokio::time::timeout(Duration::from_secs(5), bridge)
            .await
            .expect("bridge shut down")
            .unwrap();
        assert!(published
            .lock()
            .unwrap()
            .iter()
            .any(|(t, p)| t == "lg-hue-sync/availability" && p == "offline"));
    }
}
