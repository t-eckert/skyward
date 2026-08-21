//! The HTTP/JSON API.
//!
//! Live endpoints serve from an [`ArcSwap`] snapshot published by the decoder
//! thread: no locks, no database, sub-microsecond. History endpoints go to
//! SQLite inside `spawn_blocking`, so a slow disk parks a blocking thread
//! rather than a tokio worker.
//!
//! The old design took an async mutex around a blocking rusqlite connection
//! for *every* request, which serialized them all and blocked the runtime.
//! Live aircraft were never in the database to begin with — they were already
//! in memory.
//!
//! # Contract notes for the SvelteKit client
//!
//! - `now_ms` is in the envelope; compute ages against it, not the browser
//!   clock, or a laptop 40 seconds out of sync shows everything as stale.
//! - All times are epoch milliseconds. Never a formatted string: the old
//!   schema's `"2026-08-06 12:00:00"` is parsed as *local* time by V8 and is
//!   `Invalid Date` in older Safari.
//! - Units are in the field names, and it is `track_deg` — ADS-B reports
//!   ground track, which differs from heading by the wind correction angle.
//! - `/api/v1/receiver` gives you the station position, so the range ring is
//!   not hardcoded in the client and the same client works against any station.
//!   `PUT` moves it and `DELETE` reverts it to configuration — both take
//!   effect inside a second, without restarting the decoder or dropping a
//!   single tracked aircraft.

use crate::run::AppState;
use crate::station::Station;
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{
        IntoResponse, Response,
        sse::{Event, Sse},
    },
    routing::get,
};
use serde::{Deserialize, Serialize};
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route(
            "/api/v1/receiver",
            get(receiver).put(set_receiver).delete(clear_receiver),
        )
        .route("/api/v1/stats", get(stats))
        .route("/api/v1/aircraft", get(aircraft_list))
        .route("/api/v1/aircraft/{icao}", get(aircraft_one))
        .route("/api/v1/aircraft/{icao}/track", get(aircraft_track))
        .route("/api/v1/stream", get(stream))
        .with_state(state)
        // The client, compiled into this binary. A fallback rather than a
        // route, so it only ever answers paths the API did not claim: an
        // unknown `/api/...` path still gets a real 404 instead of a page of
        // HTML, which is the difference between a readable client error and
        // twenty minutes wondering why `fetch` returned `<!doctype html>`.
        .fallback(crate::web::handler)
}

// ---------------------------------------------------------------- DTOs -----

#[derive(Serialize)]
pub struct AircraftDto {
    pub icao: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub callsign: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lat: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lon: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position_age_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position_source: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub altitude_ft: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub altitude_source: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ground_speed_kt: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub track_deg: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vertical_rate_fpm: Option<i32>,
    pub on_ground: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    pub messages: u64,
    pub first_seen_ms: i64,
    pub last_seen_ms: i64,
    pub seen_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rssi_dbfs: Option<f32>,
}

impl From<&adsb_track::AircraftView> for AircraftDto {
    fn from(v: &adsb_track::AircraftView) -> Self {
        AircraftDto {
            icao: v.icao.clone(),
            callsign: v.callsign.clone(),
            lat: v.lat,
            lon: v.lon,
            position_age_ms: v.position_age_ms,
            position_source: v.position_source,
            altitude_ft: v.altitude_ft,
            altitude_source: v.altitude_source,
            ground_speed_kt: v.ground_speed_kt,
            track_deg: v.track_deg,
            vertical_rate_fpm: v.vertical_rate_fpm,
            on_ground: v.on_ground,
            category: v.category.clone(),
            messages: v.messages,
            first_seen_ms: v.first_seen_ms,
            last_seen_ms: v.last_seen_ms,
            seen_ms: v.seen_ms,
            rssi_dbfs: v.rssi_dbfs,
        }
    }
}

#[derive(Serialize)]
pub struct AircraftEnvelope {
    /// The server's clock. Compute ages against this, not the client's.
    pub now_ms: i64,
    pub count: usize,
    pub aircraft: Vec<AircraftDto>,
}

#[derive(Deserialize)]
pub struct ListQuery {
    /// `with_position` restricts the result to aircraft that can be mapped.
    pub filter: Option<String>,
    /// Drop anything not heard within this many milliseconds.
    pub max_age_ms: Option<i64>,
}

#[derive(Deserialize)]
pub struct TrackQuery {
    pub since_ms: Option<i64>,
    pub limit: Option<usize>,
}

#[derive(Serialize)]
pub struct TrackPoint {
    pub ts_ms: i64,
    pub lat: f64,
    pub lon: f64,
    pub altitude_ft: Option<i32>,
    pub ground_speed_kt: Option<u16>,
    pub track_deg: Option<f32>,
}

/// An error that renders as JSON rather than an empty 500.
pub struct ApiError(anyhow::Error);

impl<E: Into<anyhow::Error>> From<E> for ApiError {
    fn from(e: E) -> Self {
        ApiError(e.into())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": self.0.to_string() })),
        )
            .into_response()
    }
}

// ------------------------------------------------------------ handlers -----

async fn healthz(State(state): State<Arc<AppState>>) -> Response {
    let health = state.health_json();
    // Freshness, not liveness. The old API answered "ok" whenever SQLite was
    // readable, which stayed true for hours after the decoder died -- exactly
    // the lie you cannot afford on a box you cannot log into.
    let status = match health["status"].as_str() {
        Some("ok") | Some("degraded") => StatusCode::OK,
        // 503 so a bare `curl -f` is a valid probe.
        _ => StatusCode::SERVICE_UNAVAILABLE,
    };
    (status, Json(health)).into_response()
}

async fn readyz(State(state): State<Arc<AppState>>) -> Response {
    if state.snapshot.load().now_ms > 0 {
        (StatusCode::OK, Json(serde_json::json!({"ready": true}))).into_response()
    } else {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"ready": false, "reason": "no snapshot published yet"})),
        )
            .into_response()
    }
}

/// The station position, plus enough context for a client to offer to change
/// it: whether writes are allowed at all, where the current value came from,
/// and what configuration would revert to.
fn receiver_json(state: &AppState) -> serde_json::Value {
    let station = state.station.get();
    let configured = state.station.configured();
    serde_json::json!({
        "lat": station.map(|s| s.lat),
        "lon": station.map(|s| s.lon),
        "altitude_m": station.map(|s| s.altitude_m).unwrap_or(0.0),
        "origin": state.station.origin().as_str(),
        "writable": state.station.is_writable(),
        "configured": configured.map(|s| serde_json::json!({
            "lat": s.lat,
            "lon": s.lon,
            "altitude_m": s.altitude_m,
            "origin": state.station.configured_origin(),
        })),
        "station": state.station_name,
        "version": env!("CARGO_PKG_VERSION"),
        "uptime_s": state.started.elapsed().as_secs(),
        "sample_rate_hz": state.sample_rate_hz,
        "frequency_hz": state.frequency_hz,
        "impl_set": state.impl_set.clone(),
        "source": state.source_description.clone(),
    })
}

async fn receiver(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    Json(receiver_json(&state))
}

/// Move the station.
///
/// Returns the same body `GET` does, so a client can apply the response
/// directly instead of re-fetching and racing its own write.
///
/// A rejected position is a `400` with the reason in `error` — not a `500`,
/// and never a silent clamp. Clamping 91° to 90° would put the station at the
/// pole and produce a receiver that hears nothing for a reason nobody would
/// think to look for.
async fn set_receiver(
    State(state): State<Arc<AppState>>,
    Json(request): Json<Station>,
) -> Response {
    match state.station.set(request) {
        Ok(()) => {
            tracing::info!(
                lat = request.lat,
                lon = request.lon,
                altitude_m = request.altitude_m,
                "station position set through the API"
            );
            Json(receiver_json(&state)).into_response()
        }
        Err(e) => {
            let status = if state.station.is_writable() {
                StatusCode::BAD_REQUEST
            } else {
                // Not the request's fault: this receiver was started with
                // writes off, and no amount of fixing the body will help.
                StatusCode::FORBIDDEN
            };
            (status, Json(serde_json::json!({ "error": e }))).into_response()
        }
    }
}

/// Discard a runtime position and go back to what configuration says.
async fn clear_receiver(State(state): State<Arc<AppState>>) -> Response {
    match state.station.clear() {
        Ok(()) => {
            tracing::info!("station position reverted to the configured value");
            Json(receiver_json(&state)).into_response()
        }
        Err(e) => (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e })),
        )
            .into_response(),
    }
}

async fn stats(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    Json(state.stats_json())
}

async fn aircraft_list(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ListQuery>,
) -> Json<AircraftEnvelope> {
    let snapshot = state.snapshot.load();
    let only_positioned = query.filter.as_deref() == Some("with_position");

    let aircraft: Vec<AircraftDto> = snapshot
        .aircraft
        .iter()
        .filter(|a| !only_positioned || a.lat.is_some())
        .filter(|a| query.max_age_ms.is_none_or(|max| a.seen_ms <= max))
        .map(AircraftDto::from)
        .collect();

    Json(AircraftEnvelope {
        now_ms: snapshot.now_ms,
        count: aircraft.len(),
        aircraft,
    })
}

async fn aircraft_one(State(state): State<Arc<AppState>>, Path(icao): Path<String>) -> Response {
    let snapshot = state.snapshot.load();
    match snapshot.find(&icao) {
        Some(view) => Json(serde_json::json!({
            "now_ms": snapshot.now_ms,
            "aircraft": AircraftDto::from(view),
        }))
        .into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": format!("{icao} is not currently tracked") })),
        )
            .into_response(),
    }
}

async fn aircraft_track(
    State(state): State<Arc<AppState>>,
    Path(icao): Path<String>,
    Query(query): Query<TrackQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let icao = icao.to_ascii_uppercase();
    let db_path = state.db_path.clone();
    let since = query.since_ms.unwrap_or(0);
    let limit = query.limit.unwrap_or(500).min(10_000);

    // rusqlite is blocking; keeping it off the async workers is the whole
    // point of spawn_blocking.
    let points = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<TrackPoint>> {
        let conn = adsb_store::open_readonly(&db_path)?;
        let mut stmt = conn.prepare_cached(
            "SELECT ts_ms, lat, lon, altitude_ft, ground_speed_kt, track_deg
             FROM positions
             WHERE icao = ?1 AND ts_ms >= ?2
             ORDER BY ts_ms DESC
             LIMIT ?3",
        )?;
        let rows = stmt.query_map(
            adsb_store::rusqlite::params![icao, since, limit as i64],
            |row| {
                Ok(TrackPoint {
                    ts_ms: row.get(0)?,
                    lat: row.get(1)?,
                    lon: row.get(2)?,
                    altitude_ft: row.get(3)?,
                    ground_speed_kt: row.get::<_, Option<i64>>(4)?.map(|v| v as u16),
                    track_deg: row.get(5)?,
                })
            },
        )?;
        let mut points: Vec<TrackPoint> = rows.collect::<Result<_, _>>()?;
        // Query newest-first so LIMIT keeps the recent tail, then hand back
        // oldest-first so the client can draw a line without reversing.
        points.reverse();
        Ok(points)
    })
    .await
    .map_err(anyhow::Error::from)??;

    Ok(Json(serde_json::json!({
        "now_ms": crate::run::now_ms(),
        "count": points.len(),
        "track": points,
    })))
}

/// Server-sent events: a full snapshot once a second.
///
/// SSE rather than WebSocket because the traffic is one-way, `EventSource`
/// reconnects by itself, it survives proxies, and it is about five lines in
/// SvelteKit. There are no client-to-server messages, so an upgrade handshake
/// would buy nothing.
async fn stream(
    State(state): State<Arc<AppState>>,
) -> Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>> {
    use tokio_stream::{StreamExt, wrappers::IntervalStream};

    let ticker = IntervalStream::new(tokio::time::interval(Duration::from_secs(1)));
    let stream = ticker.map(move |_| {
        let snapshot = state.snapshot.load();
        let envelope = AircraftEnvelope {
            now_ms: snapshot.now_ms,
            count: snapshot.aircraft.len(),
            aircraft: snapshot.aircraft.iter().map(AircraftDto::from).collect(),
        };
        let json = serde_json::to_string(&envelope).unwrap_or_else(|_| "{}".to_string());
        Ok(Event::default().event("snapshot").data(json))
    });

    Sse::new(stream).keep_alive(axum::response::sse::KeepAlive::default())
}
