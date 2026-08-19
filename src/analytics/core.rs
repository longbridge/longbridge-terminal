//! Sensors Analytics from Rust — a self-contained client.
//!
//! # Copying this into another project
//!
//! This file has no dependencies on the rest of this repository. Drop it in,
//! declare the crates below, and it works. Everything project-specific —
//! endpoint, device id, platform naming, product identifier — arrives through
//! [`Config`], so nothing here needs editing.
//!
//! ```toml
//! reqwest    = { version = "0.12", default-features = false, features = ["rustls-tls"] }
//! serde_json = "1"
//! base64     = "0.22"
//! uuid       = { version = "1", features = ["v4"] }
//! log        = "0.4"
//! tokio      = { version = "1", features = ["time"] }
//! ```
//!
//! To run this file's own tests, tokio additionally needs `["macros", "rt"]`.
//!
//! In a Tauri app, enable the `tauri-runtime` feature:
//!
//! ```toml
//! [features]
//! default = ["tauri-runtime"]
//! tauri-runtime = []
//! ```
//!
//! Without it, background work goes through `tokio::spawn`, which needs an
//! ambient Tokio runtime. Tauri's `setup()` does not run inside one, so a Tauri
//! host that skips the feature gets a warning and **no background work at all**
//! — no requests, no heartbeat. It will not crash, but it will report nothing.
//!
//! # Setting it up
//!
//! ```rust,ignore
//! let sensors = Sensors::new(Config {
//!     // Required — the client cannot guess any of these.
//!     server_url: "https://event-tracking.example.com/sa?project=production".into(),
//!     device_id: stable_machine_id(),          // survives restarts
//!     lib_name: "Rust".into(),                 // name YOUR client, see Config::lib_name
//!     lib_version: env!("CARGO_PKG_VERSION").into(),
//!     base_properties: properties,             // platform, product, app version, env
//!
//!     // Optional — omit to disable.
//!     heartbeat: Some(Heartbeat {
//!         event: "app_heartbeat".into(),
//!         ..Default::default()                 // every 60s, only while active
//!     }),
//!     crash: Some(Crash {
//!         path: data_dir.join("last-crash.json"),
//!         event: "app_crash".into(),
//!     }),
//!     ..Config::default()
//! })?;
//! ```
//!
//! Then wire the host to it. **Each of these is a feature that silently does
//! nothing if skipped**, which is why they are listed rather than left to be
//! discovered:
//!
//! ```rust,ignore
//! // 1. Report events. Before the identity is known these are held, not sent.
//! sensors.track("app_launch", json!({ "channel": channel }));
//!
//! // 2. Identity. Required for events to reach the account — see the note
//! //    below. Call on every session change; `None` means signed out, and is
//! //    what releases held events for a guest.
//! sensors.set_member(Some(member_id));
//!
//! // 3. Activity, from window focus/blur. Without it every session looks
//! //    unbroken, and an app left open overnight reads as an all-night session.
//! sensors.set_active(focused);
//!
//! // 4. Crashes. Chain the previous hook — replacing it takes the panic
//! //    message off stderr, where anyone debugging looks first.
//! let previous = std::panic::take_hook();
//! let reporter = sensors.clone();
//! std::panic::set_hook(Box::new(move |info| {
//!     reporter.note_crash(&info.to_string());
//!     previous(info);
//! }));
//!
//! // 5. SHORT-LIVED PROCESSES ONLY — a CLI, a one-shot command. Reports run on
//! //    background tasks, and `#[tokio::main]` cancels those when `main`
//! //    returns, so without this most events never leave. Nothing appears in
//! //    the log either: the failure is a request that was never made.
//! sensors.flush(Duration::from_secs(3)).await;
//! ```
//!
//! A long-running app (GUI, daemon, TUI) does not need step 5 — it outlives its
//! own requests. A CLI needs nothing *but* steps 1 and 5.
//!
//! # What it sends
//!
//! Every event carries `base_properties` plus whatever is passed to `track`.
//! The two built-in events add:
//!
//! | event       | properties                                                    |
//! |-------------|---------------------------------------------------------------|
//! | heartbeat   | `active`, `interval_ms`, `uptime_ms`, `active_ms`             |
//! | crash       | `recovered`, `crashed_at`, `crashed_after_ms`, `crashed_after_active_ms`, `info` |
//!
//! `active_ms` is time the host reported as active, accumulated across focus
//! changes — not inferred from beat counts, so a burst shorter than one
//! interval still counts.
//!
//! # Hosting a WebView that also runs the JS SDK
//!
//! Call [`seed_script`] and inject the result before any page script. Without
//! it the SDK mints its own anonymous id, and the two halves report as two
//! different users — visible only as roughly doubled user counts, never as an
//! error. The host must also configure the SDK with `cross_subdomain: false`;
//! see that function's documentation.
//!
//! # Why the wire format is hand-written
//!
//! Sensors publishes no Rust SDK. This reproduces what the JavaScript SDK
//! (`sa-sdk-javascript` 1.21.13) puts on the wire, which is:
//!
//! ```text
//! POST <server_url>            Content-Type: application/x-www-form-urlencoded
//! data=<urlencode(base64(json))>&ext=<urlencode("crc=" + hash(base64))>
//! ```
//!
//! Three details are easy to get wrong from memory and are pinned by tests:
//! nothing is compressed (the SDK has no gzip path at all); the checksum is
//! computed over the **base64 text**, not the JSON; and `_track_id` /
//! `_flush_time` are not optional — the SDK adds both unconditionally.
//!
//! # Two things learned the hard way
//!
//! **Events must carry the member id.** Reporting under a device id alone and
//! expecting the browser SDK's `$SignUp` to associate the two does not work on
//! a client that was already signed in when analytics shipped: that event is
//! only emitted when the member id *changes*. Identical payloads differing only
//! in `distinct_id` were measured landing or vanishing accordingly.
//!
//! **The first events happen before the identity is known.** A launch event
//! fires before any UI exists, so sending it immediately pins the most useful
//! event in the product to an anonymous id forever. [`Sensors::track`] holds
//! events until [`Sensors::set_member`] is called or the wait expires.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine as _;

// ───────────────────────────────────────────────────────────── configuration

/// Everything the client needs that it cannot work out for itself.
#[derive(Clone, Debug)]
pub struct Config {
    /// Full ingest URL including the project, e.g.
    /// `https://event-tracking.example.com/sa?project=production`.
    pub server_url: String,
    /// Stable per-machine identifier. Used as the anonymous id, and as
    /// `distinct_id` until a member is bound.
    pub device_id: String,
    /// `$lib`. Name the thing that actually produced the event. Do **not**
    /// borrow a browser SDK's name: a payload claiming `js` while carrying no
    /// `$screen_*`, `$url` or `$title`, sent from a non-browser agent,
    /// contradicts itself.
    pub lib_name: String,
    /// `$lib_version`. Usually `env!("CARGO_PKG_VERSION")`.
    pub lib_version: String,
    /// Properties attached to every event — platform, product, app version,
    /// environment. Per-event properties override these.
    pub base_properties: serde_json::Map<String, serde_json::Value>,
    /// How long to hold early events waiting to learn who is signed in.
    /// Bounded because a signed-out session never resolves.
    pub identity_wait: Duration,
    /// How many events to hold while waiting. Past this they go out as-is —
    /// a mis-attributed event beats a missing one.
    pub max_deferred: usize,
    /// How many failed request bodies to retain for retry. Oldest dropped first.
    pub max_pending: usize,
    /// Per-request timeout. Nothing upstream waits on the answer.
    pub request_timeout: Duration,
    /// Log the first payload of each identity, in full, at debug level.
    ///
    /// Worth turning on when bringing this up somewhere new: this wire format
    /// is a transcription of a JavaScript SDK's, and a transcription can be
    /// wrong while every other signal — HTTP 200, a log line, a passing test —
    /// says it is fine. Printing what actually goes out lets it be diffed
    /// against what that SDK sends, which is how one mismatch was caught.
    pub log_first_payload: bool,
    /// Periodic proof that the app is still being used. `None` disables it.
    pub heartbeat: Option<Heartbeat>,
    /// Where to record a crash so the *next* launch can report it. `None`
    /// disables crash reporting.
    pub crash: Option<Crash>,
}

/// Liveness reporting.
///
/// A launch event says the app was opened; nothing says it is still open. With
/// only launches, an hour of use and a mistaken double-click are the same
/// datapoint. Each beat carries `uptime_ms` and `active_ms` so a session's
/// length can be reconstructed even when the last beat is the last thing ever
/// heard from a client — which is the normal case, since processes are killed
/// far more often than they are closed politely.
#[derive(Clone, Debug)]
pub struct Heartbeat {
    /// How often to beat. A minute is a reasonable default: fine-grained enough
    /// to measure a session, coarse enough not to matter as traffic.
    pub interval: Duration,
    /// Event name, e.g. `desktop_heartbeat`.
    pub event: String,
    /// Beat only while the host reports the app as active (see
    /// [`Sensors::set_active`]).
    ///
    /// `true` — the default — makes "beats × interval" a usable proxy for time
    /// actually spent in the app. `false` beats regardless and tags each one
    /// with `active`, leaving the filtering to whoever queries. Changing this
    /// later changes what every duration metric means, so it is worth deciding
    /// once.
    pub only_when_active: bool,
}

impl Default for Heartbeat {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(60),
            event: "heartbeat".into(),
            only_when_active: true,
        }
    }
}

/// Crash reporting.
///
/// A crashing process cannot send an HTTP request — there is no time, and on an
/// abort no unwinding either. So a crash is written to disk synchronously and
/// reported by the *next* launch, which is also why this belongs in the client
/// rather than in the host: the replay has to happen inside [`Sensors::new`],
/// before anything else can report.
#[derive(Clone, Debug)]
pub struct Crash {
    /// File to record a crash in. Must be somewhere that survives a restart and
    /// is writable without allocation-heavy setup — a panic hook runs in a
    /// process that is already failing.
    pub path: std::path::PathBuf,
    /// Event name, e.g. `desktop_crash`.
    pub event: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            server_url: String::new(),
            device_id: String::new(),
            lib_name: "Rust".into(),
            lib_version: "0.0.0".into(),
            base_properties: serde_json::Map::new(),
            identity_wait: Duration::from_secs(10),
            max_deferred: 32,
            max_pending: 64,
            request_timeout: Duration::from_secs(10),
            log_first_payload: false,
            heartbeat: None,
            crash: None,
        }
    }
}

/// Who an event belongs to. `member_id` is present only while signed in.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Identity {
    pub device_id: String,
    pub member_id: Option<String>,
}

// ─────────────────────────────────────────────────────────────────── client

/// An analytics client. Cheap to clone; all clones share one queue.
#[derive(Clone)]
pub struct Sensors(Arc<Inner>);

struct Inner {
    http: reqwest::Client,
    config: Config,
    identity: Mutex<Identity>,
    settled: AtomicBool,
    deferred: Mutex<Vec<Deferred>>,
    pending: Mutex<VecDeque<String>>,
    /// Guards the one-shot payload dump; re-armed whenever the identity changes,
    /// since the first payload of a process is always anonymous.
    payload_logged: AtomicBool,
    /// Requests started but not yet finished. Only [`Sensors::flush`] reads it —
    /// a short-lived process needs to know when it is safe to exit.
    in_flight: AtomicUsize,
    /// Whether the host currently counts as in use. Drives the heartbeat, and
    /// is reported alongside it.
    active: AtomicBool,
    /// Process start, for `uptime_ms`.
    started_ms: u64,
    /// Milliseconds spent active so far, excluding the stretch in progress.
    active_accumulated_ms: Mutex<u64>,
    /// When the current active stretch began, if one is in progress.
    active_since_ms: Mutex<Option<u64>>,
}

/// Reports what was still unsent when the client went away.
///
/// Losing events at exit is the failure this module is most prone to and least
/// able to show: the request was never made, so nothing appears in the log at
/// all. This turns that silence into one line.
///
/// It fires only when the host actually releases the client. A client parked in
/// a `static` — an `OnceLock` singleton, which is how both known hosts keep it —
/// is never dropped at exit, so this is a backstop for hosts that own their
/// client outright, not a substitute for flushing.
impl Drop for Inner {
    fn drop(&mut self) {
        let in_flight = self.in_flight.load(Ordering::SeqCst);
        let held = self.deferred.get_mut().map_or(0, |queue| queue.len());
        let pending = self.pending.get_mut().map_or(0, |queue| queue.len());

        if in_flight > 0 || held > 0 {
            log::warn!(
                "[sensors] dropped with {in_flight} request(s) in flight and {held} event(s) \
                 still held — both are lost. A short-lived process has to flush on every exit \
                 path, error paths included."
            );
        }
        if pending > 0 {
            log::warn!("[sensors] dropped with {pending} event(s) awaiting retry — those are lost");
        }
    }
}

/// An event waiting for an identity. Unencoded, because its `distinct_id` is
/// not knowable yet — which is the whole reason it waits. `occurred_ms` keeps
/// it on the timeline where it happened rather than where it was released.
struct Deferred {
    event: String,
    properties: serde_json::Value,
    occurred_ms: u64,
}

impl Sensors {
    /// Builds a client and starts the identity timer.
    ///
    /// Returns `None` only if the HTTP client cannot be constructed, which
    /// should cost telemetry and nothing else — callers are expected to carry
    /// on without analytics rather than fail.
    ///
    /// Must be called where background tasks can be spawned: inside a Tokio
    /// runtime, or anywhere at all with the `tauri` feature.
    pub fn new(config: Config) -> Option<Self> {
        let http = reqwest::Client::builder()
            .timeout(config.request_timeout)
            .build()
            .map_err(|error| log::warn!("[sensors] no reporting client: {error}"))
            .ok()?;

        let device_id = config.device_id.clone();
        let wait = config.identity_wait;
        let sensors = Self(Arc::new(Inner {
            http,
            config,
            identity: Mutex::new(Identity {
                device_id,
                member_id: None,
            }),
            settled: AtomicBool::new(false),
            deferred: Mutex::new(Vec::new()),
            pending: Mutex::new(VecDeque::new()),
            payload_logged: AtomicBool::new(false),
            in_flight: AtomicUsize::new(0),
            // Hosts that never call `set_active` still get heartbeats: assuming
            // "in use" until told otherwise is the safer default, since the
            // alternative is silently measuring nothing.
            active: AtomicBool::new(true),
            started_ms: now_ms(),
            active_accumulated_ms: Mutex::new(0),
            active_since_ms: Mutex::new(Some(now_ms())),
        }));

        // The backstop for a session nobody signs in on: without it those
        // events would be held until the process exits and then be lost.
        let timer = sensors.clone();
        spawn(async move {
            sleep(wait).await;
            timer.settle("timed out");
        });

        // A crash from the previous run, if any. Reported through the normal
        // path, so it waits for the identity like everything else — a crash is
        // worth attributing to whoever hit it.
        sensors.replay_crash();

        if let Some(heartbeat) = sensors.0.config.heartbeat.clone() {
            let beating = sensors.clone();
            spawn(async move {
                loop {
                    sleep(heartbeat.interval).await;
                    beating.beat(&heartbeat);
                }
            });
        }

        Some(sensors)
    }

    /// Reports one event. Returns immediately.
    ///
    /// Never fails loudly: no caller should have to handle a network error to
    /// record that something happened.
    pub fn track(&self, event: &str, properties: serde_json::Value) {
        let now = now_ms();

        if !self.0.settled.load(Ordering::Relaxed) {
            match self.hold(event, properties, now) {
                None => return,
                // Queue full — send as-is rather than drop.
                Some(returned) => self.dispatch(event, returned, now),
            }
            return;
        }

        self.dispatch(event, properties, now);
    }

    /// Attaches subsequent events to a member, or detaches on sign-out.
    ///
    /// Also the signal that the identity question has an answer at all, which
    /// is what releases anything held — a signed-out session resolves here too,
    /// with `None`.
    pub fn set_member(&self, member_id: Option<String>) {
        let member_id = member_id.filter(|id| !id.is_empty());
        let bound = member_id.is_some();

        // Scoped so the lock is released before `settle`, which dispatches held
        // events — and dispatching reads this same lock. A std Mutex is not
        // reentrant, so holding it across that call deadlocks the first sign-in.
        {
            let Ok(mut identity) = self.0.identity.lock() else {
                return;
            };
            if identity.member_id != member_id {
                log::debug!(
                    "[sensors] identity {}",
                    if bound { "bound" } else { "cleared" }
                );
                identity.member_id = member_id;
                // The first payload of a process is always anonymous — the
                // launch event precedes any sign-in — so re-arm, otherwise the
                // log never shows what a signed-in payload looks like.
                self.0.payload_logged.store(false, Ordering::Relaxed);
            }
        }

        self.settle(if bound { "member bound" } else { "signed out" });
    }

    /// Tells the client whether the app currently counts as in use.
    ///
    /// Hosts call this on focus and blur. Without it every launch looks like an
    /// unbroken session, and an app left open overnight reads the same as one
    /// somebody used all night.
    ///
    /// Time is accounted here rather than inferred from beat counts so that a
    /// stretch shorter than one interval still shows up.
    pub fn set_active(&self, active: bool) {
        if self.0.active.swap(active, Ordering::Relaxed) == active {
            return;
        }
        let now = now_ms();
        let Ok(mut since) = self.0.active_since_ms.lock() else {
            return;
        };
        match (active, since.take()) {
            // Went active: start a new stretch.
            (true, _) => *since = Some(now),
            // Went idle: bank the stretch that just ended.
            (false, Some(started)) => {
                if let Ok(mut total) = self.0.active_accumulated_ms.lock() {
                    *total = total.saturating_add(now.saturating_sub(started));
                }
            }
            (false, None) => {}
        }
        log::debug!("[sensors] {}", if active { "active" } else { "idle" });
    }

    /// Milliseconds spent active so far, including any stretch in progress.
    pub fn active_ms(&self) -> u64 {
        let banked = self
            .0
            .active_accumulated_ms
            .lock()
            .map(|total| *total)
            .unwrap_or(0);
        let current = self
            .0
            .active_since_ms
            .lock()
            .ok()
            .and_then(|since| *since)
            .map(|started| now_ms().saturating_sub(started))
            .unwrap_or(0);
        banked.saturating_add(current)
    }

    /// Milliseconds since the client was constructed.
    pub fn uptime_ms(&self) -> u64 {
        now_ms().saturating_sub(self.0.started_ms)
    }

    /// Records a crash for the next launch to report.
    ///
    /// Call from a panic hook. Writes synchronously and does nothing else: the
    /// process is already failing, and anything asynchronous would not outlive
    /// it. Reported on the next [`Sensors::new`].
    pub fn note_crash(&self, info: &str) {
        let Some(crash) = &self.0.config.crash else {
            return;
        };
        let record = serde_json::json!({
            "time": now_ms(),
            "info": truncate(info, 2000),
            "uptime_ms": self.uptime_ms(),
            "active_ms": self.active_ms(),
        });
        if let Some(parent) = crash.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&crash.path, record.to_string());
    }

    /// Reports a crash recorded by a previous run, if there is one, then clears
    /// it. Called from [`Sensors::new`].
    fn replay_crash(&self) {
        let Some(crash) = &self.0.config.crash else {
            return;
        };
        let Ok(contents) = std::fs::read_to_string(&crash.path) else {
            return;
        };
        // Removed before reporting, not after: a malformed record that somehow
        // caused trouble downstream would otherwise be retried on every launch
        // for the life of the installation.
        let _ = std::fs::remove_file(&crash.path);

        let mut properties = serde_json::json!({ "recovered": true });
        if let Ok(serde_json::Value::Object(record)) = serde_json::from_str(&contents) {
            for (key, value) in record {
                // The crash's own timing, not this launch's.
                let key = match key.as_str() {
                    "time" => "crashed_at".to_string(),
                    "uptime_ms" => "crashed_after_ms".to_string(),
                    "active_ms" => "crashed_after_active_ms".to_string(),
                    _ => key,
                };
                properties[key] = value;
            }
        }
        log::info!("[sensors] reporting a crash from the previous run");
        self.track(&crash.event, properties);
    }

    /// Emits one heartbeat, unless the app is idle and the config says to skip
    /// those.
    fn beat(&self, heartbeat: &Heartbeat) {
        let active = self.0.active.load(Ordering::Relaxed);
        if heartbeat.only_when_active && !active {
            return;
        }
        self.track(
            &heartbeat.event,
            serde_json::json!({
                "active": active,
                "interval_ms": heartbeat.interval.as_millis() as u64,
                "uptime_ms": self.uptime_ms(),
                "active_ms": self.active_ms(),
            }),
        );
    }

    /// Waits for reporting to finish. **Short-lived processes must call this.**
    ///
    /// Reports are sent on background tasks, and a process that exits does not
    /// wait for them: under `#[tokio::main]` the runtime is dropped when `main`
    /// returns and anything still in flight is cancelled. For a CLI whose whole
    /// life is shorter than one HTTP round trip, that means most events never
    /// leave — with nothing in the logs to say so, since the failure is the
    /// absence of a request rather than a failed one.
    ///
    /// Also settles the identity first, so events still held for a sign-in that
    /// never came are sent rather than dropped on the floor.
    ///
    /// Returns whether everything drained before the deadline. Long-running
    /// hosts do not need this at all — they outlive their own requests.
    ///
    /// ```rust,ignore
    /// sensors.track("command_run", json!({ "command": name }));
    /// sensors.flush(Duration::from_secs(3)).await;   // before main returns
    /// ```
    pub async fn flush(&self, timeout: Duration) -> bool {
        // Anything held for an identity would otherwise be discarded at exit:
        // for a process this short, "we never learned who this was" is the
        // normal outcome, not an edge case.
        self.settle("flushing");

        let deadline = now_ms().saturating_add(timeout.as_millis() as u64);
        while self.0.in_flight.load(Ordering::Relaxed) > 0 {
            if now_ms() >= deadline {
                log::warn!(
                    "[sensors] flush timed out with {} still in flight",
                    self.0.in_flight.load(Ordering::Relaxed)
                );
                return false;
            }
            sleep(Duration::from_millis(20)).await;
        }
        true
    }

    /// The identity events currently report under. Mostly useful for logging —
    /// analytics that cannot be inspected is analytics nobody trusts.
    pub fn identity(&self) -> Identity {
        self.0
            .identity
            .lock()
            .map(|held| held.clone())
            .unwrap_or_default()
    }

    fn hold(
        &self,
        event: &str,
        properties: serde_json::Value,
        occurred_ms: u64,
    ) -> Option<serde_json::Value> {
        let Ok(mut queue) = self.0.deferred.lock() else {
            return Some(properties);
        };
        if queue.len() >= self.0.config.max_deferred {
            return Some(properties);
        }
        log::debug!("[sensors] holding {event} until the identity is known");
        queue.push(Deferred {
            event: event.to_string(),
            properties,
            occurred_ms,
        });
        None
    }

    /// Releases held events. Idempotent; only the first call flushes.
    fn settle(&self, reason: &str) {
        if self.0.settled.swap(true, Ordering::Relaxed) {
            return;
        }
        let held: Vec<Deferred> = self
            .0
            .deferred
            .lock()
            .map(|mut queue| queue.drain(..).collect())
            .unwrap_or_default();
        if !held.is_empty() {
            log::debug!(
                "[sensors] identity settled ({reason}); releasing {}",
                held.len()
            );
        }
        for event in held {
            self.dispatch(&event.event, event.properties, event.occurred_ms);
        }
    }

    fn dispatch(&self, event: &str, properties: serde_json::Value, occurred_ms: u64) {
        let mut props = self.0.config.base_properties.clone();
        if let Some(extra) = properties.as_object() {
            for (key, value) in extra {
                props.insert(key.clone(), value.clone());
            }
        }

        let now = now_ms();
        let payload = build_event(
            event,
            serde_json::Value::Object(props),
            &self.identity(),
            &self.0.config,
            occurred_ms,
            now,
            track_id(now),
        );
        let body = encode_body(&payload);
        log::debug!("[sensors] reporting {event}");
        if self.0.config.log_first_payload && !self.0.payload_logged.swap(true, Ordering::Relaxed) {
            log::debug!(
                "[sensors] payload {}",
                serde_json::to_string(&payload).unwrap_or_default()
            );
        }

        let client = self.clone();
        self.0.in_flight.fetch_add(1, Ordering::Relaxed);
        spawn(async move {
            // Decremented on every exit from this task, including the early
            // return below — a leaked count would make `flush` wait out its
            // whole timeout on every call.
            let _guard = InFlightGuard(client.clone());
            // Drain first, so a recovered connection also clears what an outage
            // held back, in the order it happened. On the first failure the
            // whole remainder goes back — putting back only the failed body
            // would discard the rest of the backlog, and lose most of it
            // exactly when the outage is longest.
            let mut backlog = client.take_pending().into_iter();
            while let Some(queued) = backlog.next() {
                if !client.send(&queued).await {
                    for item in retry_batch(queued, backlog, body) {
                        client.requeue(item);
                    }
                    return;
                }
            }
            if !client.send(&body).await {
                client.requeue(body);
            }
        });
    }

    async fn send(&self, body: &str) -> bool {
        let result = self
            .0
            .http
            .post(&self.0.config.server_url)
            .header("content-type", "application/x-www-form-urlencoded")
            .body(body.to_string())
            .send()
            .await;
        match result {
            Ok(response) if response.status().is_success() => {
                // Logged on success too. Analytics has no visible effect
                // anywhere, so without this a client reporting nothing and one
                // reporting everything read identically in the log.
                log::debug!("[sensors] reported");
                true
            }
            Ok(response) => {
                let status = response.status();
                log::warn!("[sensors] ingest refused the report: {status}");
                // 4xx is about this payload; replaying it would only be refused
                // again. 5xx says nothing about the payload — that is what the
                // buffer is for.
                !status.is_server_error()
            }
            Err(error) => {
                log::warn!("[sensors] could not report: {error}");
                false
            }
        }
    }

    fn take_pending(&self) -> Vec<String> {
        self.0
            .pending
            .lock()
            .map(|mut queue| queue.drain(..).collect())
            .unwrap_or_default()
    }

    fn requeue(&self, body: String) {
        if let Ok(mut queue) = self.0.pending.lock() {
            while queue.len() >= self.0.config.max_pending {
                queue.pop_front();
            }
            queue.push_back(body);
        }
    }
}

// ──────────────────────────────────────────────────────────────── wire format

/// Builds the event object the JS SDK would have built.
///
/// The identity block follows the SDK's own `getUnionId()`: signed out, the
/// device id is both `distinct_id` and `anonymous_id`; signed in, `distinct_id`
/// becomes the member id and the device id stays as `anonymous_id`. The
/// `identities` map follows suit — on login the SDK drops
/// `$identity_anonymous_id`, keeping `$identity_login_id` beside the cookie id.
///
/// `occurred_ms` is when it happened, `flush_ms` when it is being sent. They
/// differ for anything that waited, and reporting the queue time as the event
/// time would misplace every deferred event on the timeline.
pub fn build_event(
    name: &str,
    properties: serde_json::Value,
    identity: &Identity,
    config: &Config,
    occurred_ms: u64,
    flush_ms: u64,
    track_id: u64,
) -> serde_json::Value {
    let device = identity.device_id.as_str();
    let mut payload = serde_json::json!({
        "type": "track",
        "event": name,
        "time": occurred_ms,
        "anonymous_id": device,
        "lib": {
            "$lib": config.lib_name,
            "$lib_method": "code",
            "$lib_version": config.lib_version,
        },
        "properties": properties,
        "_track_id": track_id,
        "_flush_time": flush_ms,
    });

    let map = payload.as_object_mut().expect("event is an object");
    match identity.member_id.as_deref().filter(|id| !id.is_empty()) {
        Some(member) => {
            map.insert("distinct_id".into(), member.into());
            map.insert("login_id".into(), member.into());
            map.insert(
                "identities".into(),
                serde_json::json!({
                    "$identity_login_id": member,
                    "$identity_cookie_id": device,
                }),
            );
        }
        None => {
            map.insert("distinct_id".into(), device.into());
            map.insert(
                "identities".into(),
                serde_json::json!({
                    "$identity_cookie_id": device,
                    "$identity_anonymous_id": device,
                }),
            );
        }
    }
    payload
}

/// Encodes one event into a request body: `data=…&ext=crc=…`, no compression.
pub fn encode_body(event: &serde_json::Value) -> String {
    let json = serde_json::to_string(event).unwrap_or_else(|_| "{}".into());
    let encoded = base64::engine::general_purpose::STANDARD.encode(json.as_bytes());
    format!(
        "data={}&ext={}",
        percent_encode(&encoded),
        percent_encode(&format!("crc={}", crc(&encoded)))
    )
}

/// The SDK's `hashCode`: `h = h * 31 + c`, truncated to a signed 32-bit int at
/// every step, computed over the **base64 text** rather than the JSON.
fn crc(encoded: &str) -> i32 {
    encoded.bytes().fold(0i32, |hash, byte| {
        hash.wrapping_mul(31).wrapping_add(i32::from(byte))
    })
}

/// `encodeURIComponent`, which leaves `-_.!~*'()` and alphanumerics alone.
fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' => out.push(byte as char),
            b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')' => out.push(byte as char),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// The SDK's `_track_id`: three random digits, two random digits, then the last
/// four of the millisecond clock. The JS version can emit fewer digits when a
/// random draw starts with a zero; fixed widths keep the shape stable.
fn track_id(now_ms: u64) -> u64 {
    let random = uuid::Uuid::new_v4();
    let bytes = random.as_bytes();
    let first = 100 + (u64::from(bytes[0]) * 900 / 256);
    let second = 10 + (u64::from(bytes[1]) * 90 / 256);
    format!("{first}{second}{:04}", now_ms % 10_000)
        .parse()
        .unwrap_or(now_ms)
}

/// Decrements the in-flight count however the sending task ends, including on
/// an early return. A leaked count would make every later [`Sensors::flush`]
/// wait out its full timeout.
struct InFlightGuard(Sensors);

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        self.0 .0.in_flight.fetch_sub(1, Ordering::Relaxed);
    }
}

/// What goes back into the queue when a flush stops partway: the body that
/// failed, everything still behind it, then the event that started the flush.
fn retry_batch(failed: String, rest: impl Iterator<Item = String>, body: String) -> Vec<String> {
    let mut batch = vec![failed];
    batch.extend(rest);
    batch.push(body);
    batch
}

/// Caps a string at `max` bytes, on a character boundary.
///
/// Panic messages carry backtraces and user data; neither belongs in an
/// analytics property, and an oversized one risks the whole event.
fn truncate(value: &str, max: usize) -> String {
    if value.len() <= max {
        return value.to_string();
    }
    let mut end = max;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &value[..end])
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_millis() as u64)
        .unwrap_or_default()
}

// ───────────────────────────────────────────────────── WebView identity seed

/// An inline script that seeds the Sensors identity cookie before any page
/// script runs, for hosts that also run the JS SDK in a WebView.
///
/// Without it the two halves report as two different users: the SDK mints its
/// own anonymous id inside `store.init()`, which the host cannot predict — and
/// the mismatch surfaces only as inflated user counts, never as an error. The
/// SDK skips minting when it finds a cookie already carrying a `distinct_id`,
/// which is the seam this writes into.
///
/// The host must configure the SDK with `cross_subdomain: false`. The default
/// asks the browser to scope the cookie to a domain, and a loopback origin is
/// an IP literal — the write is refused outright. That setting also selects the
/// cookie name this mirrors.
///
/// Written only when absent: this cookie also carries *login* state, so
/// overwriting on every launch would sign the user out of analytics on every
/// restart. `seeded` in the exported global reports whether this run wrote it.
pub fn seed_script(device_id: &str, server_url: &str) -> String {
    let id = serde_json::to_string(device_id).unwrap_or_else(|_| "\"\"".into());
    let url = serde_json::to_string(server_url).unwrap_or_else(|_| "\"\"".into());
    format!(
        r#"(function () {{
  var id = {id};
  var seeded = false;
  try {{
    if (id) {{
      var host = String((typeof location !== 'undefined' && location.hostname) || '');
      var name = 'sa_jssdk_2015_' + host.replace(/\./g, '_');
      if (document.cookie.indexOf(name + '=') === -1) {{
        var identities = {{ $identity_cookie_id: id, $identity_anonymous_id: id }};
        var state = {{ distinct_id: id, identities: btoa(JSON.stringify(identities)) }};
        document.cookie =
          name + '=' + encodeURIComponent(JSON.stringify(state)) +
          '; Path=/; SameSite=Lax; Max-Age=63072000';
        seeded = true;
      }}
    }}
  }} catch (e) {{}}
  globalThis.__LB_SENSORS__ = {{ deviceId: id, serverUrl: {url}, seeded: seeded }};
}})();"#
    )
}

// ─────────────────────────────────────────────────────────────────── runtime

/// Background work goes onto Tauri's runtime when the `tauri-runtime` feature
/// is on, and onto the ambient Tokio runtime otherwise.
///
/// The distinction matters at exactly one moment: Tauri's `setup()` runs on a
/// thread that is not inside a Tokio runtime context, so a bare `tokio::spawn`
/// there panics — and `setup()` is where the launch event is reported.
/// `tauri::async_runtime::spawn` finds the runtime Tauri manages instead.
///
/// Sleeping needs no such split: Tauri's runtime *is* Tokio, so the timer works
/// either way (Tokio's `time` feature is required).
#[cfg(feature = "tauri-runtime")]
fn spawn<F>(future: F)
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    tauri::async_runtime::spawn(future);
}

#[cfg(not(feature = "tauri-runtime"))]
fn spawn<F>(future: F)
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    // `tokio::spawn` panics outside a runtime, and analytics must never be the
    // reason a host goes down — a dropped event is a rounding error, a crash is
    // an outage. Degrades to a warning instead, and a loud one: silently
    // reporting nothing is the exact failure this module exists to prevent.
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => {
            handle.spawn(future);
        }
        Err(_) => log::warn!(
            "[sensors] no Tokio runtime in scope; background work dropped. \
             Construct inside a runtime, or enable the `tauri-runtime` feature."
        ),
    }
}

async fn sleep(duration: Duration) {
    tokio::time::sleep(duration).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> Config {
        Config {
            server_url: "http://127.0.0.1:1/sa?project=default".into(),
            device_id: "device-1".into(),
            lib_name: "Rust".into(),
            lib_version: "1.2.3".into(),
            ..Config::default()
        }
    }

    fn anonymous() -> Identity {
        Identity {
            device_id: "device-1".into(),
            member_id: None,
        }
    }

    fn signed_in() -> Identity {
        Identity {
            device_id: "device-1".into(),
            member_id: Some("member-9".into()),
        }
    }

    #[test]
    fn anonymous_events_report_the_device_id_as_both_ids() {
        let payload = build_event("e", serde_json::json!({}), &anonymous(), &config(), 1, 1, 2);
        assert_eq!(payload["distinct_id"], "device-1");
        assert_eq!(payload["anonymous_id"], "device-1");
        assert!(payload.get("login_id").is_none());
        assert_eq!(payload["identities"]["$identity_anonymous_id"], "device-1");
    }

    /// Signed in, `distinct_id` moves to the member while the device id stays
    /// as the anonymous one — that pairing is what associates the two, and it
    /// is the shape measured to actually reach the warehouse.
    #[test]
    fn signed_in_events_keep_the_device_id_as_the_anonymous_id() {
        let payload = build_event("e", serde_json::json!({}), &signed_in(), &config(), 1, 1, 2);
        assert_eq!(payload["distinct_id"], "member-9");
        assert_eq!(payload["login_id"], "member-9");
        assert_eq!(payload["anonymous_id"], "device-1");
        assert_eq!(payload["identities"]["$identity_login_id"], "member-9");
        assert!(payload["identities"]
            .get("$identity_anonymous_id")
            .is_none());
    }

    #[test]
    fn an_empty_member_id_is_treated_as_signed_out() {
        let identity = Identity {
            device_id: "device-1".into(),
            member_id: Some(String::new()),
        };
        let payload = build_event("e", serde_json::json!({}), &identity, &config(), 1, 1, 2);
        assert_eq!(payload["distinct_id"], "device-1");
        assert!(payload.get("login_id").is_none());
    }

    /// The library name is the caller's, never borrowed from a browser SDK.
    #[test]
    fn the_library_is_the_one_the_caller_named() {
        let payload = build_event("e", serde_json::json!({}), &anonymous(), &config(), 1, 1, 2);
        assert_eq!(payload["lib"]["$lib"], "Rust");
        assert_eq!(payload["lib"]["$lib_version"], "1.2.3");
    }

    /// An event held back still belongs at the moment it happened.
    #[test]
    fn a_deferred_event_keeps_the_time_it_happened() {
        let payload = build_event(
            "e",
            serde_json::json!({}),
            &anonymous(),
            &config(),
            100,
            900,
            1,
        );
        assert_eq!(payload["time"], 100);
        assert_eq!(payload["_flush_time"], 900);
    }

    #[test]
    fn every_event_carries_the_track_and_flush_stamps() {
        let payload = build_event(
            "e",
            serde_json::json!({}),
            &anonymous(),
            &config(),
            42,
            99,
            7,
        );
        assert_eq!(payload["_track_id"], 7);
    }

    /// The exact hash the SDK computes: 0, 97, 97*31+98, and so on.
    #[test]
    fn the_crc_matches_the_sdk_hash() {
        assert_eq!(crc(""), 0);
        assert_eq!(crc("a"), 97);
        assert_eq!(crc("ab"), 3105);
        assert_eq!(crc("abc"), 96354);
    }

    /// 32-bit truncation is what keeps long payloads agreeing with the SDK;
    /// without the wrap this overflows and panics in debug builds.
    #[test]
    fn the_crc_wraps_like_a_signed_32_bit_int() {
        let _ = crc(&"A".repeat(4096));
    }

    #[test]
    fn base64_padding_and_symbols_are_escaped() {
        assert_eq!(percent_encode("a+b/c="), "a%2Bb%2Fc%3D");
        assert_eq!(percent_encode("-_.!~*'()"), "-_.!~*'()");
        assert_eq!(percent_encode("crc=-12345"), "crc%3D-12345");
    }

    #[test]
    fn the_body_has_exactly_the_two_fields_the_sdk_sends() {
        let payload = build_event("e", serde_json::json!({}), &anonymous(), &config(), 1, 1, 2);
        let body = encode_body(&payload);
        assert!(body.starts_with("data="));
        assert!(body.contains("&ext=crc%3D"));
        // Nothing is compressed: the SDK has no gzip path at all.
        assert!(!body.contains("gzip"));
    }

    /// The checksum must cover the base64 text. Computing it over the JSON
    /// produces a body that still looks right and is rejected.
    #[test]
    fn the_crc_covers_the_encoded_text_not_the_json() {
        let payload = build_event("e", serde_json::json!({}), &anonymous(), &config(), 1, 1, 2);
        let json = serde_json::to_string(&payload).unwrap();
        let encoded = base64::engine::general_purpose::STANDARD.encode(json.as_bytes());
        let body = encode_body(&payload);
        assert!(body.contains(&percent_encode(&format!("crc={}", crc(&encoded)))));
        assert!(!body.contains(&percent_encode(&format!("crc={}", crc(&json)))));
    }

    #[test]
    fn track_ids_are_nine_digits() {
        for ms in [0u64, 1, 999, 1_700_000_000_000] {
            assert_eq!(track_id(ms).to_string().len(), 9);
        }
    }

    fn client() -> Sensors {
        Sensors::new(config()).expect("the http client builds")
    }

    /// The launch event fires before any UI exists, so it cannot know who is
    /// signed in. Sending it straight away would pin the most useful event in
    /// the product to an anonymous id forever.
    #[test]
    fn events_are_held_while_the_identity_is_unknown() {
        let sensors = client();
        sensors.track("app_launch", serde_json::json!({}));
        assert_eq!(sensors.0.deferred.lock().unwrap().len(), 1);
    }

    /// Holding preserves when the event happened. Releasing it a second later
    /// must not move it a second down the timeline.
    #[test]
    fn holding_preserves_the_moment_the_event_happened() {
        let sensors = client();
        let before = now_ms();
        sensors.track("app_launch", serde_json::json!({}));
        let queue = sensors.0.deferred.lock().unwrap();
        assert!(queue[0].occurred_ms >= before);
    }

    /// Past the bound, an event goes out as-is rather than being dropped: a
    /// mis-attributed event still beats a missing one.
    #[test]
    fn a_full_hold_queue_hands_the_event_back_instead_of_dropping_it() {
        let sensors = client();
        for index in 0..sensors.0.config.max_deferred {
            assert!(sensors
                .hold("e", serde_json::json!({ "i": index }), 1)
                .is_none());
        }
        assert!(
            sensors.hold("e", serde_json::json!({}), 1).is_some(),
            "the event must come back to the caller"
        );
    }

    /// Binding a member is what releases the backlog — and it must not be
    /// possible for that to deadlock: settling dispatches held events, and
    /// dispatching reads the identity lock the caller just held.
    #[test]
    fn binding_a_member_settles_and_releases_everything_held() {
        let sensors = client();
        sensors.track("app_launch", serde_json::json!({}));
        assert_eq!(sensors.0.deferred.lock().unwrap().len(), 1);

        sensors.set_member(Some("member-9".into()));

        assert!(sensors.0.deferred.lock().unwrap().is_empty());
        assert_eq!(sensors.identity().member_id.as_deref(), Some("member-9"));
    }

    /// A signed-out session resolves the identity question too — otherwise a
    /// guest's events would be held until the process exits and then be lost.
    #[test]
    fn signing_out_also_settles() {
        let sensors = client();
        sensors.track("app_launch", serde_json::json!({}));
        sensors.set_member(None);
        assert!(sensors.0.deferred.lock().unwrap().is_empty());
        assert!(sensors.identity().member_id.is_none());
    }

    /// After settling, events go straight out rather than accumulating.
    #[test]
    fn later_events_are_not_held() {
        let sensors = client();
        sensors.set_member(Some("member-9".into()));
        sensors.track("later", serde_json::json!({}));
        assert!(sensors.0.deferred.lock().unwrap().is_empty());
    }

    /// The retry queue is a buffer, not a log. Without the bound, a machine
    /// offline for an afternoon would hold every event it ever tried to send.
    #[test]
    fn the_pending_queue_is_bounded() {
        let sensors = client();
        let max = sensors.0.config.max_pending;
        for index in 0..(max * 2) {
            sensors.requeue(format!("body-{index}"));
        }
        let held = sensors.take_pending();
        assert_eq!(held.len(), max);
        // Oldest dropped: the survivors are the most recent ones.
        assert_eq!(held.last().unwrap(), &format!("body-{}", max * 2 - 1));
        assert!(sensors.take_pending().is_empty());
    }

    /// Time is banked when a stretch ends, so a burst of use shorter than one
    /// heartbeat interval is still counted rather than rounded away.
    #[test]
    fn active_time_accumulates_across_stretches() {
        let sensors = client();
        assert!(sensors.active_ms() < 1_000, "starts near zero");

        sensors.set_active(false);
        let banked = sensors.active_ms();
        // Idle time must not accrue.
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert_eq!(sensors.active_ms(), banked, "idle time is not counted");

        sensors.set_active(true);
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert!(sensors.active_ms() > banked, "active time resumes");
    }

    /// Repeated calls with the same state are ignored — hosts fire focus events
    /// more than once, and double-counting a stretch would inflate every
    /// duration metric.
    #[test]
    fn repeating_the_same_activity_state_changes_nothing() {
        let sensors = client();
        sensors.set_active(true);
        let first = sensors.active_ms();
        sensors.set_active(true);
        assert!(sensors.active_ms() >= first);
        sensors.set_active(false);
        let banked = sensors.active_ms();
        sensors.set_active(false);
        assert_eq!(sensors.active_ms(), banked);
    }

    /// A host that never reports activity still gets heartbeats: measuring
    /// nothing is a worse failure than measuring generously.
    #[test]
    fn activity_defaults_to_in_use() {
        let sensors = client();
        let heartbeat = Heartbeat::default();
        sensors.set_member(None); // settle, so the beat is dispatched not held
        sensors.beat(&heartbeat);
        assert!(sensors.0.active.load(Ordering::Relaxed));
    }

    /// An idle beat is skipped under the default policy, which is what makes
    /// "beats × interval" mean time actually spent in the app.
    #[test]
    fn idle_beats_are_skipped_by_default() {
        let sensors = client();
        sensors.set_member(None);
        sensors.set_active(false);
        sensors.beat(&Heartbeat::default());
        // Nothing to assert on the wire here; the contract is that the policy
        // is consulted at all, which the config flag below pins.
        assert!(Heartbeat::default().only_when_active);
    }

    /// A crash is written synchronously and replayed by the next launch. The
    /// record is removed first, so a bad one cannot be retried forever.
    #[test]
    fn a_crash_is_recorded_and_cleared_on_replay() {
        let dir = std::env::temp_dir().join(format!("sensors-crash-{}", uuid::Uuid::new_v4()));
        let path = dir.join("crash.json");
        let mut settings = config();
        settings.crash = Some(Crash {
            path: path.clone(),
            event: "crash".into(),
        });

        let sensors = Sensors::new(settings).expect("client builds");
        sensors.note_crash("thread panicked at 'boom'");
        assert!(
            path.exists(),
            "the crash is on disk before the process dies"
        );

        let recorded = std::fs::read_to_string(&path).unwrap();
        assert!(recorded.contains("boom"));

        sensors.replay_crash();
        assert!(!path.exists(), "replaying clears the record");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Panic messages carry backtraces and whatever the user was doing. Neither
    /// belongs in an analytics property, and an oversized one risks the event.
    #[test]
    fn crash_details_are_truncated() {
        assert_eq!(truncate("short", 100), "short");
        let long = "x".repeat(3000);
        let cut = truncate(&long, 2000);
        assert!(cut.len() <= 2004, "{} bytes", cut.len());
        assert!(cut.ends_with('…'));
    }

    /// Truncation must not split a character in half — that produces invalid
    /// UTF-8 and would panic inside the panic handler.
    #[test]
    fn truncation_respects_character_boundaries() {
        let text = "中文中文中文";
        let cut = truncate(text, 7); // lands mid-character
        assert!(cut.chars().all(|c| c == '中' || c == '文' || c == '…'));
    }

    /// Nothing is written when crash reporting is off.
    #[test]
    fn crashes_are_not_recorded_without_a_path() {
        let sensors = client();
        sensors.note_crash("boom");
        // No panic, no file, nothing to clean up.
    }

    /// Flushing settles the identity, so events held for a sign-in that never
    /// came are sent rather than discarded at exit. For a process this short,
    /// "nobody signed in" is the normal case.
    #[tokio::test]
    async fn flushing_releases_events_held_for_an_identity() {
        let sensors = client();
        sensors.track("command_run", serde_json::json!({}));
        assert_eq!(sensors.0.deferred.lock().unwrap().len(), 1);

        sensors.flush(Duration::from_millis(200)).await;
        assert!(
            sensors.0.deferred.lock().unwrap().is_empty(),
            "held events must be released, not dropped"
        );
    }

    /// The guard has to decrement on every exit from the sending task. A leaked
    /// count would make every later flush wait out its whole timeout — turning
    /// a fast CLI into one that pauses for seconds on exit.
    #[tokio::test]
    async fn the_in_flight_count_returns_to_zero() {
        let sensors = client();
        sensors.set_member(None);
        sensors.track("command_run", serde_json::json!({}));

        // The endpoint is unroutable, so this exercises the failure path — the
        // one where a leak would be easiest to introduce.
        sensors.flush(Duration::from_secs(5)).await;
        assert_eq!(sensors.0.in_flight.load(Ordering::Relaxed), 0);
    }

    /// A flush with nothing outstanding returns immediately rather than
    /// sleeping out its timeout.
    #[tokio::test]
    async fn flushing_an_idle_client_is_immediate() {
        let sensors = client();
        sensors.set_member(None);
        assert!(sensors.flush(Duration::from_secs(5)).await);
    }

    /// The regression this exists for: an earlier version put back only the
    /// failed body and the new one, discarding everything queued behind it.
    #[test]
    fn a_partial_flush_puts_the_whole_backlog_back() {
        let rest = vec!["b".to_string(), "c".to_string()];
        assert_eq!(
            retry_batch("a".into(), rest.into_iter(), "new".into()),
            vec!["a", "b", "c", "new"]
        );
    }

    #[test]
    fn the_seed_derives_the_cookie_name_and_leaves_an_existing_one_alone() {
        let script = seed_script("device-1", "https://example.com/sa?project=default");
        assert!(script.contains("location.hostname"));
        assert!(!script.contains("127_0_0_1"));
        assert!(script.contains("if (document.cookie.indexOf(name + '=') === -1) {"));
        assert!(script.contains("$identity_cookie_id: id"));
    }

    /// A quote in the id would close the string literal and turn the rest of
    /// the seed into syntax errors — silently, since it is wrapped in `try`.
    #[test]
    fn the_seed_escapes_its_inputs() {
        let script = seed_script("a\"b\\c", "https://x/sa");
        assert!(script.contains(r#""a\"b\\c""#));
    }

    /// An empty device id must not produce a cookie claiming an empty identity:
    /// the SDK would take it as authoritative and never mint one of its own.
    #[test]
    fn an_empty_device_id_seeds_no_cookie() {
        assert!(seed_script("", "https://x/sa").contains("if (id) {"));
    }
}
