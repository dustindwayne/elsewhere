use crate::{network::Runtime, Result};
use axum::{
    extract::{ConnectInfo, DefaultBodyLimit, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Value};
use std::{
    net::{IpAddr, SocketAddr},
    sync::Arc,
};
use uuid::Uuid;

pub struct WebServer {
    _previous: Option<Box<WebServer>>,
    pub port: u16,
    pub lan: bool,
    key: String,
    hosts: Vec<String>,
    task: tokio::task::JoinHandle<()>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    #[tokio::test]
    async fn sessions_gate_records_and_phone_access_preserves_the_local_backend() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = Runtime::new(Store::open(dir.path()).unwrap());
        let local = start(&runtime, false).await.unwrap();
        let url = local["url"].as_str().unwrap();
        let client = reqwest::Client::new();
        let request = json!({"name":"snapshot","args":{}});
        assert_eq!(
            client
                .post(format!("{url}/command"))
                .json(&request)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        let response = client.get(url).send().await.unwrap();
        let cookie = response.headers()["set-cookie"]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_string();
        assert_eq!(
            client
                .post(format!("{url}/command"))
                .header("cookie", &cookie)
                .json(&request)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        assert_eq!(
            client
                .post(format!("{url}/command"))
                .header("cookie", &cookie)
                .header("origin", "https://example.invalid")
                .json(&request)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
        let phone = start(&runtime, true).await.unwrap();
        let phone_url = phone["url"].as_str().unwrap();
        assert_eq!(
            client
                .post(format!("{phone_url}/session"))
                .json(&json!({"key":"invalid"}))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        let key = runtime.web.lock().unwrap().as_ref().unwrap().key.clone();
        let response = client
            .post(format!("{phone_url}/session"))
            .json(&json!({"key":key}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let phone_cookie = response.headers()["set-cookie"]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap();
        assert_eq!(
            client
                .post(format!("{phone_url}/command"))
                .header("cookie", phone_cookie)
                .json(&request)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        assert_eq!(
            client
                .post(format!("{url}/command"))
                .header("cookie", cookie)
                .json(&request)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        assert_eq!(
            client
                .get(format!("{url}/space.js"))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
    }
}
impl Drop for WebServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}
#[derive(Clone)]
struct WebState {
    runtime: Arc<Runtime>,
    key: String,
    hosts: Vec<String>,
}

fn valid_origin(headers: &HeaderMap, hosts: &[String]) -> bool {
    let host = headers
        .get("host")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    hosts.iter().any(|v| v == host)
        && headers
            .get("origin")
            .and_then(|v| v.to_str().ok())
            .is_none_or(|v| v == format!("http://{host}"))
}
fn authenticated(headers: &HeaderMap, key: &str) -> bool {
    let value = headers
        .get("cookie")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .split(';')
        .filter_map(|v| v.trim().strip_prefix("elsewhere_session="))
        .next()
        .unwrap_or("");
    value.len() == key.len()
        && value
            .bytes()
            .zip(key.bytes())
            .fold(0u8, |n, (a, b)| n | (a ^ b))
            == 0
}
fn cookie(key: &str) -> HeaderValue {
    HeaderValue::from_str(&format!(
        "elsewhere_session={key}; HttpOnly; SameSite=Strict; Path=/"
    ))
    .unwrap()
}
async fn index(
    State(s): State<WebState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    if !valid_origin(&headers, &s.hosts) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let mut response = Html(include_str!("../../web/index.html")).into_response();
    if peer.ip().is_loopback() {
        response
            .headers_mut()
            .insert(axum::http::header::SET_COOKIE, cookie(&s.key));
    }
    response
}
async fn session_status(State(s): State<WebState>, headers: HeaderMap) -> Response {
    if !valid_origin(&headers, &s.hosts) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if !authenticated(&headers, &s.key) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    Json(json!({"connected":true})).into_response()
}
async fn session(
    State(s): State<WebState>,
    headers: HeaderMap,
    Json(value): Json<Value>,
) -> Response {
    if !valid_origin(&headers, &s.hosts) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let key = value["key"].as_str().unwrap_or("");
    if key.len() != s.key.len()
        || key
            .bytes()
            .zip(s.key.bytes())
            .fold(0u8, |n, (a, b)| n | (a ^ b))
            != 0
    {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let mut response = Json(json!({"connected":true})).into_response();
    response
        .headers_mut()
        .insert(axum::http::header::SET_COOKIE, cookie(&s.key));
    response
}
async fn command(
    State(s): State<WebState>,
    headers: HeaderMap,
    Json(value): Json<Value>,
) -> Response {
    if !valid_origin(&headers, &s.hosts) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if !authenticated(&headers, &s.key) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error":"Connect this device first"})),
        )
            .into_response();
    }
    match s
        .runtime
        .command(value["name"].as_str().unwrap_or(""), value["args"].clone())
        .await
    {
        Ok(value) => Json(value).into_response(),
        Err(error) => (StatusCode::BAD_REQUEST, Json(json!({"error":error}))).into_response(),
    }
}
fn info(server: &WebServer) -> Value {
    let urls = server
        .hosts
        .iter()
        .filter(|h| !h.starts_with("127.") && !h.starts_with("localhost:"))
        .map(|h| format!("http://{h}/#connect={}", server.key))
        .collect::<Vec<_>>();
    let qr = urls
        .first()
        .and_then(|url| qrcode::QrCode::new(url.as_bytes()).ok())
        .map(|qr| {
            qr.render::<qrcode::render::svg::Color>()
                .min_dimensions(240, 240)
                .build()
        });
    json!({"url":format!("http://127.0.0.1:{}",server.port),"phone_urls":urls,"qr":qr,"lan":server.lan})
}
pub fn start(
    runtime: &Arc<Runtime>,
    lan: bool,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Value>> + Send + '_>> {
    Box::pin(async move {
        {
            let existing = runtime.web.lock().unwrap();
            if let Some(server) = existing.as_ref() {
                if server.lan || !lan {
                    return Ok(info(server));
                }
            }
        }
        let address = if lan {
            "0.0.0.0:47831"
        } else {
            "127.0.0.1:47831"
        };
        let listener = match tokio::net::TcpListener::bind(address).await {
            Ok(listener) => listener,
            Err(_) => tokio::net::TcpListener::bind(if lan { "0.0.0.0:0" } else { "127.0.0.1:0" })
                .await
                .map_err(|e| e.to_string())?,
        };
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        let mut hosts = vec![format!("127.0.0.1:{port}"), format!("localhost:{port}")];
        if lan {
            for (_, ip) in local_ip_address::list_afinet_netifas().map_err(|e| e.to_string())? {
                if let IpAddr::V4(ip) = ip {
                    if ip.is_private() && !ip.is_loopback() {
                        let host = format!("{ip}:{port}");
                        if !hosts.contains(&host) {
                            hosts.push(host);
                        }
                    }
                }
            }
        }
        let key = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
        let state = WebState {
            runtime: runtime.clone(),
            key: key.clone(),
            hosts: hosts.clone(),
        };
        let app = Router::new()
            .route("/", get(index))
            .route("/session", get(session_status).post(session))
            .route("/command", post(command))
            .route(
                "/app.js",
                get(|| async {
                    (
                        [("content-type", "text/javascript")],
                        include_str!("../../web/app.js"),
                    )
                }),
            )
            .route(
                "/workspace.js",
                get(|| async {
                    (
                        [("content-type", "text/javascript")],
                        include_str!("../../web/workspace.js"),
                    )
                }),
            )
            .route(
                "/space.js",
                get(|| async {
                    (
                        [("content-type", "text/javascript")],
                        include_str!("../../web/space.js"),
                    )
                }),
            )
            .route(
                "/transitions.js",
                get(|| async {
                    (
                        [("content-type", "text/javascript")],
                        include_str!("../../web/transitions.js"),
                    )
                }),
            )
            .route(
                "/style.css",
                get(|| async {
                    (
                        [("content-type", "text/css")],
                        include_str!("../../web/style.css"),
                    )
                }),
            )
            .layer(DefaultBodyLimit::max(64 * 1024 * 1024))
            .with_state(state);
        let task = tokio::spawn(async move {
            if let Err(error) = axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            {
                eprintln!("Web backend stopped: {error}");
            }
        });
        let previous = runtime.web.lock().unwrap().take().map(Box::new);
        let server = WebServer {
            _previous: previous,
            port,
            lan,
            key,
            hosts,
            task,
        };
        let result = info(&server);
        *runtime.web.lock().unwrap() = Some(server);
        Ok(result)
    })
}
