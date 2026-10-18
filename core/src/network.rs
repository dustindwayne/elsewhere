use crate::{
    store::{Peer, Revision, Store, Thought},
    Result,
};
use aes_gcm::{
    aead::{Aead, Payload},
    Aes256Gcm, KeyInit, Nonce,
};
use axum::{
    extract::{ConnectInfo, DefaultBodyLimit, Path, State},
    http::StatusCode,
    routing::post,
    Json, Router,
};
use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
    Engine,
};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    net::{IpAddr, SocketAddr},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use uuid::Uuid;

const PROTOCOL: &[u8] = b"elsewhere-peer-v2";
const MAX_PACKET: usize = 64 * 1024 * 1024;

#[derive(Clone, Serialize, Deserialize)]
pub struct Invite {
    pub version: u32,
    pub node: String,
    pub name: String,
    pub address: String,
    pub channel: String,
    pub key: String,
}
#[derive(Serialize, Deserialize)]
struct Exchange {
    recipient: String,
    node: String,
    name: String,
    port: u16,
    revisions: Vec<Revision>,
    #[serde(default)]
    known: Vec<String>,
    #[serde(default)]
    known_blobs: Vec<String>,
    #[serde(default)]
    blobs: BTreeMap<String, String>,
}
#[derive(Serialize, Deserialize)]
pub struct Packet {
    nonce: String,
    ciphertext: String,
}
struct Pending {
    invite: Invite,
    expires: Instant,
}
struct DeleteArm {
    token: String,
    expires: Instant,
    parents: Vec<String>,
}

pub struct Runtime {
    pub(crate) web: Mutex<Option<crate::web::WebServer>>,
    pub store: Mutex<Store>,
    pub(crate) agents: Mutex<crate::agents::AgentServer>,
    discovery: Mutex<Option<mdns_sd::ServiceDaemon>>,
    nearby: Mutex<HashMap<String, String>>,
    pending: Mutex<HashMap<String, Pending>>,
    arms: Mutex<HashMap<String, DeleteArm>>,
    port: Mutex<Option<u16>>,
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    syncing: tokio::sync::Mutex<()>,
}
impl Runtime {
    pub fn new(store: Store) -> Arc<Self> {
        Arc::new(Self {
            web: Mutex::new(None),
            store: Mutex::new(store),
            agents: Mutex::new(crate::agents::AgentServer::default()),
            discovery: Mutex::new(None),
            nearby: Mutex::new(HashMap::new()),
            pending: Mutex::new(HashMap::new()),
            arms: Mutex::new(HashMap::new()),
            port: Mutex::new(None),
            task: Mutex::new(None),
            syncing: tokio::sync::Mutex::new(()),
        })
    }
    pub async fn start(self: &Arc<Self>) -> Result<u16> {
        let mut task = self.task.lock().map_err(|_| "Network unavailable")?;
        if let Some(port) = *self.port.lock().unwrap() {
            return Ok(port);
        }
        // Binding does not grant note access; every exchange is authenticated and encrypted.
        let preferred = self.store.lock().unwrap().listen_port();
        let listener = std::net::TcpListener::bind(("0.0.0.0", preferred))
            .or_else(|_| std::net::TcpListener::bind(("0.0.0.0", 0)))
            .map_err(|e| format!("Nearby connections unavailable: {e}"))?;
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        let listener = tokio::net::TcpListener::from_std(listener).map_err(|e| e.to_string())?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        self.store.lock().unwrap().set_listen_port(port)?;
        *self.port.lock().unwrap() = Some(port);
        let app = Router::new()
            .route("/exchange/{channel}", post(exchange))
            .layer(DefaultBodyLimit::max(MAX_PACKET))
            .with_state(self.clone());
        *task = Some(tokio::spawn(async move {
            let _ = axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await;
        }));
        drop(task);
        self.start_discovery(port);
        Ok(port)
    }
    fn start_discovery(self: &Arc<Self>, port: u16) {
        let Ok(daemon) = mdns_sd::ServiceDaemon::new() else {
            return;
        };
        let node = self.store.lock().unwrap().node.clone();
        let props = [("node", node.as_str()), ("version", "2")];
        let Ok(info) = mdns_sd::ServiceInfo::new(
            "_elsewhere._tcp.local.",
            &node,
            &format!("{node}.local."),
            "",
            port,
            &props[..],
        ) else {
            return;
        };
        if daemon.register(info.enable_addr_auto()).is_err() {
            return;
        }
        let Ok(receiver) = daemon.browse("_elsewhere._tcp.local.") else {
            return;
        };
        *self.discovery.lock().unwrap() = Some(daemon);
        let weak = Arc::downgrade(self);
        std::thread::spawn(move || loop {
            let Some(rt) = weak.upgrade() else {
                break;
            };
            if rt.port.lock().unwrap().is_none() {
                break;
            }
            drop(rt);
            let Ok(event) = receiver.recv_timeout(Duration::from_secs(3)) else {
                continue;
            };
            if let mdns_sd::ServiceEvent::ServiceResolved(info) = event {
                if let Some(node) = info.get_property_val_str("node") {
                    if Uuid::parse_str(node).is_err() {
                        continue;
                    }
                    if let Some(ip) = info
                        .addresses
                        .iter()
                        .map(|ip| ip.to_ip_addr())
                        .find(|ip| ip.is_ipv4() && is_local(*ip))
                    {
                        if let Some(rt) = weak.upgrade() {
                            let mut nearby = rt.nearby.lock().unwrap();
                            if nearby.len() < 1000 || nearby.contains_key(node) {
                                nearby.insert(
                                    node.into(),
                                    SocketAddr::new(ip, info.port).to_string(),
                                );
                            }
                        }
                    }
                }
            }
        });
    }
    pub fn stop(&self) {
        if let Some(daemon) = self.discovery.lock().unwrap().take() {
            let _ = daemon.shutdown();
        }
        self.nearby.lock().unwrap().clear();
        if let Some(t) = self.task.lock().unwrap().take() {
            t.abort();
        }
        *self.port.lock().unwrap() = None;
        self.pending.lock().unwrap().clear();
    }
    pub async fn invite(self: &Arc<Self>) -> Result<Value> {
        let port = self.start().await?;
        let ip = local_ip_address::local_ip().map_err(|_| "Connect to Wi-Fi before pairing")?;
        validate_address(&format!("{ip}:{port}"))?;
        let mut key = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut key);
        let store = self.store.lock().unwrap();
        let invite = Invite {
            version: 2,
            node: store.node.clone(),
            name: store.name.clone(),
            address: format!("{ip}:{port}"),
            channel: Uuid::new_v4().to_string(),
            key: URL_SAFE_NO_PAD.encode(key),
        };
        drop(store);
        let code = format!(
            "elsewhere: {}",
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&invite).unwrap())
        );
        let qr = qrcode::QrCode::new(code.as_bytes())
            .map_err(|e| e.to_string())?
            .render::<qrcode::render::svg::Color>()
            .min_dimensions(240, 240)
            .build();
        let mut pending = self.pending.lock().unwrap();
        pending.clear();
        pending.insert(
            invite.channel.clone(),
            Pending {
                invite,
                expires: Instant::now() + Duration::from_secs(300),
            },
        );
        Ok(json!({"code":code,"qr":qr,"expires_seconds":300}))
    }
    pub async fn pair(self: &Arc<Self>, code: &str) -> Result<Value> {
        let raw = code
            .trim()
            .strip_prefix("elsewhere:")
            .ok_or("Paste the Elsewhere pairing code")?
            .trim();
        if raw.len() > 4096 {
            return Err("Invalid pairing code".into());
        }
        let invite: Invite = serde_json::from_slice(
            &URL_SAFE_NO_PAD
                .decode(raw)
                .map_err(|_| "Invalid pairing code")?,
        )
        .map_err(|_| "Invalid pairing code")?;
        if invite.version != 2 {
            return Err("Update Elsewhere on both devices".into());
        }
        Uuid::parse_str(&invite.channel).map_err(|_| "Invalid channel")?;
        Uuid::parse_str(&invite.node).map_err(|_| "Invalid node")?;
        validate_address(&invite.address)?;
        decode_key(&invite.key)?;
        if self.store.lock().unwrap().node == invite.node {
            return Err("Use this code on your other device".into());
        }
        self.start().await?;
        let peer = Peer {
            id: invite.node,
            name: invite.name,
            address: invite.address,
            key: invite.key,
            channel: invite.channel,
        };
        self.exchange_peer(&peer).await?;
        self.store.lock().unwrap().put_peer(&peer)?;
        Ok(json!({"paired":true,"name":peer.name}))
    }
    async fn exchange_peer(&self, peer: &Peer) -> Result<()> {
        validate_address(&peer.address)?;
        let port = self
            .port
            .lock()
            .unwrap()
            .unwrap_or_else(|| self.store.lock().unwrap().listen_port());
        let mut remote_known = HashSet::<String>::new();
        let mut remote_blobs = Vec::<String>::new();
        for phase in 0..2 {
            let request = {
                let s = self.store.lock().unwrap();
                Exchange {
                    recipient: peer.id.clone(),
                    node: s.node.clone(),
                    name: s.name.clone(),
                    port,
                    revisions: if phase == 0 {
                        vec![]
                    } else {
                        s.revisions()?
                            .into_iter()
                            .filter(|r| !remote_known.contains(&r.id))
                            .collect()
                    },
                    known: s.revision_ids()?,
                    known_blobs: s.blob_ids()?,
                    blobs: if phase == 0 {
                        BTreeMap::new()
                    } else {
                        s.blobs_missing(&remote_blobs)?
                    },
                }
            };
            let packet = seal(&peer.key, &request)?;
            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(20))
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(|e| e.to_string())?;
            let response = client
                .post(format!("http://{}/exchange/{}", peer.address, peer.channel))
                .json(&packet)
                .send()
                .await
                .map_err(|_| {
                    format!(
                        "{} is unreachable. Open Elsewhere on that device and use the same Wi-Fi.",
                        peer.name
                    )
                })?;
            if !response.status().is_success() {
                return Err(format!(
                    "{} rejected the exchange. Try a new pairing code.",
                    peer.name
                ));
            }
            if response
                .content_length()
                .is_some_and(|n| n > MAX_PACKET as u64)
            {
                return Err("Space is too large to exchange".into());
            }
            let mut response = response;
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(|e| e.to_string())? {
                if bytes.len() + chunk.len() > MAX_PACKET {
                    return Err("Space is too large to exchange".into());
                }
                bytes.extend_from_slice(&chunk);
            }
            let packet: Packet = serde_json::from_slice(&bytes).map_err(|_| "Invalid reply")?;
            let reply: Exchange = open(&peer.key, &packet)?;
            let mut s = self.store.lock().unwrap();
            if reply.node != peer.id || reply.recipient != s.node {
                return Err("Wrong device identity".into());
            }
            s.receive_blobs(&reply.blobs)?;
            s.merge(&reply.revisions)?;
            remote_known = reply.known.into_iter().collect();
            remote_blobs = reply.known_blobs;
        }
        Ok(())
    }
    pub async fn sync(&self) -> Result<Value> {
        let _guard = self.syncing.lock().await;
        let peers = self.store.lock().unwrap().peers()?;
        let mut results = Vec::new();
        for mut peer in peers {
            let discovered = self.nearby.lock().unwrap().get(&peer.id).cloned();
            let original = peer.address.clone();
            if let Some(address) = discovered {
                peer.address = address;
            }
            let mut result = self.exchange_peer(&peer).await;
            if result.is_err() && peer.address != original {
                peer.address = original;
                result = self.exchange_peer(&peer).await;
            }
            if result.is_ok() {
                self.store.lock().unwrap().put_peer(&peer)?;
            }
            results.push(
                json!({"id":peer.id,"name":peer.name,"ok":result.is_ok(),"error":result.err()}),
            );
        }
        Ok(json!({"devices":results}))
    }
    pub fn arm(&self, id: &str) -> Result<Value> {
        let heads = self
            .store
            .lock()
            .unwrap()
            .heads()?
            .remove(id)
            .ok_or("Thought not found")?;
        let token = Uuid::new_v4().to_string();
        let mut arms = self.arms.lock().unwrap();
        arms.retain(|_, a| a.expires > Instant::now());
        arms.insert(
            id.into(),
            DeleteArm {
                token: token.clone(),
                expires: Instant::now() + Duration::from_secs(8),
                parents: heads.iter().map(|r| r.id.clone()).collect(),
            },
        );
        Ok(json!({"token":token,"expires_seconds":8}))
    }
    pub fn confirm(&self, id: &str, token: &str) -> Result<()> {
        let arm = self
            .arms
            .lock()
            .unwrap()
            .remove(id)
            .ok_or("Deletion is not armed")?;
        if arm.token != token || arm.expires <= Instant::now() {
            return Err("Delete confirmation expired".into());
        }
        self.store.lock().unwrap().delete(id, &arm.parents)
    }
    pub async fn command(self: &Arc<Self>, name: &str, args: Value) -> Result<Value> {
        let str_arg = |key: &str| {
            args.get(key)
                .and_then(Value::as_str)
                .ok_or_else(|| format!("Missing {key}"))
        };
        match name {
            "get_file" => {
                let s = self.store.lock().unwrap();
                let r = s.record(str_arg("id")?)?;
                let f = r
                    .files
                    .iter()
                    .find(|f| Some(f.id.as_str()) == args["file_id"].as_str())
                    .ok_or("Attachment not found")?;
                Ok(
                    json!({"name":f.name,"mime":f.mime,"data":if f.blob.is_empty(){f.data.clone()}else{s.blob(&f.blob)?}}),
                )
            }
            "agent_status" => Ok(crate::agents::status(self)),
            "web_phone_invite" => crate::web::start(self, true).await,
            "agent_grant" => {
                crate::agents::grant(
                    self,
                    str_arg("name")?,
                    args["write"].as_bool().unwrap_or(false),
                )
                .await
            }
            "agent_revoke" => {
                crate::agents::revoke(self, str_arg("id")?)?;
                Ok(json!({}))
            }
            "new_record" => {
                let mut s = self.store.lock().unwrap();
                let mut r = s.new_record(
                    str_arg("kind")?,
                    args["template_id"].as_str(),
                    args["title"].as_str().unwrap_or(""),
                    args["body"].as_str().unwrap_or(""),
                )?;
                if let (Some(x), Some(y)) = (args["x"].as_f64(), args["y"].as_f64()) {
                    let parent = r.note.revision_id.clone();
                    r.x = x;
                    r.y = y;
                    r = s.commit(r, &[parent], false)?;
                }
                serde_json::to_value(r).map_err(|e| e.to_string())
            }
            "search" => Ok(json!(self.store.lock().unwrap().search(
                args["query"].as_str().unwrap_or(""),
                args["kind"].as_str(),
                args["limit"].as_u64().unwrap_or(50) as usize
            )?)),
            "record_history" => {
                let id = str_arg("id")?;
                Ok(json!(self
                    .store
                    .lock()
                    .unwrap()
                    .revisions()?
                    .into_iter()
                    .filter(|r| r.entity == id)
                    .collect::<Vec<_>>()))
            }
            "snapshot" => serde_json::to_value(self.store.lock().unwrap().snapshot()?)
                .map_err(|e| e.to_string()),
            "create" => serde_json::to_value(self.store.lock().unwrap().create(
                args["x"].as_f64().unwrap_or(0.0),
                args["y"].as_f64().unwrap_or(0.0),
            )?)
            .map_err(|e| e.to_string()),
            "save" => {
                let t: Thought =
                    serde_json::from_value(args["thought"].clone()).map_err(|e| e.to_string())?;
                let parents: Vec<String> =
                    serde_json::from_value(args["parents"].clone()).map_err(|e| e.to_string())?;
                self.arms.lock().unwrap().remove(&t.note.id);
                serde_json::to_value(self.store.lock().unwrap().commit(
                    t,
                    &parents,
                    args["resolve"].as_bool().unwrap_or(false),
                )?)
                .map_err(|e| e.to_string())
            }
            "rename" => {
                self.store.lock().unwrap().rename(str_arg("name")?)?;
                Ok(json!({}))
            }
            "arm_delete" => self.arm(str_arg("id")?),
            "confirm_delete" => {
                self.confirm(str_arg("id")?, str_arg("token")?)?;
                Ok(json!({}))
            }
            "cancel_delete" => {
                self.arms.lock().unwrap().remove(str_arg("id")?);
                Ok(json!({}))
            }
            "invite" => self.invite().await,
            "pair" => self.pair(str_arg("code")?).await,
            "sync" => self.sync().await,
            "network" => Ok(json!({"enabled":self.port.lock().unwrap().is_some()})),
            "network_start" => {
                self.start().await?;
                Ok(json!({}))
            }
            "network_stop" => {
                self.stop();
                Ok(json!({}))
            }
            "forget_peer" => {
                self.store.lock().unwrap().remove_peer(str_arg("id")?)?;
                Ok(json!({}))
            }
            "export" => {
                let s = self.store.lock().unwrap();
                Ok(
                    json!({"format":"elsewhere-v2","thoughts":s.snapshot()?.thoughts,"revisions":s.revisions()?,"blobs":s.blobs_missing(&[])?}),
                )
            }
            "import" => {
                let mut s = self.store.lock().unwrap();
                let data = args.get("data").ok_or("Missing export")?;
                if data["format"] == "elsewhere-v1" || data["format"] == "elsewhere-v2" {
                    if data["format"] == "elsewhere-v2" {
                        s.receive_blobs(
                            &serde_json::from_value(data["blobs"].clone())
                                .map_err(|e| e.to_string())?,
                        )?;
                    }
                    let rs: Vec<Revision> = serde_json::from_value(data["revisions"].clone())
                        .map_err(|e| e.to_string())?;
                    let count = rs.len();
                    s.merge(&rs)?;
                    Ok(json!({"imported":count}))
                } else {
                    Ok(json!({"imported":s.import(data.clone())?}))
                }
            }
            "templates" => Ok(json!(self.store.lock().unwrap().template_catalog()?)),
            "install_template" => {
                let p =
                    serde_json::from_value(args["package"].clone()).map_err(|e| e.to_string())?;
                self.store.lock().unwrap().install_template(p)?;
                Ok(json!({}))
            }
            _ => Err("Unknown command".into()),
        }
    }
}

async fn exchange(
    State(rt): State<Arc<Runtime>>,
    Path(channel): Path<String>,
    ConnectInfo(remote): ConnectInfo<SocketAddr>,
    Json(packet): Json<Packet>,
) -> std::result::Result<Json<Packet>, (StatusCode, String)> {
    let deny = |s: &str| (StatusCode::UNAUTHORIZED, s.to_string());
    if !is_local(remote.ip()) {
        return Err(deny("Local network only"));
    }
    let (key, expected) = {
        let peers = rt
            .store
            .lock()
            .unwrap()
            .peers()
            .map_err(|_| deny("Store unavailable"))?;
        if let Some(p) = peers.iter().find(|p| p.channel == channel) {
            (p.key.clone(), Some(p.id.clone()))
        } else {
            let pending = rt.pending.lock().unwrap();
            let p = pending
                .get(&channel)
                .filter(|p| p.expires > Instant::now())
                .ok_or_else(|| deny("Pairing expired"))?;
            (p.invite.key.clone(), None)
        }
    };
    let request: Exchange = open(&key, &packet).map_err(|_| deny("Invalid exchange"))?;
    Uuid::parse_str(&request.node).map_err(|_| deny("Invalid node"))?;
    if request.port == 0 || request.name.len() > 100 || request.name.trim().is_empty() {
        return Err(deny("Invalid device"));
    }
    let mut s = rt.store.lock().unwrap();
    if request.recipient != s.node
        || request.node == s.node
        || expected.is_some_and(|id| id != request.node)
    {
        return Err(deny("Wrong identity"));
    }
    s.receive_blobs(&request.blobs)
        .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    s.merge(&request.revisions)
        .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    s.put_peer(&Peer {
        id: request.node.clone(),
        name: request.name,
        address: SocketAddr::new(remote.ip(), request.port).to_string(),
        key: key.clone(),
        channel: channel.clone(),
    })
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    let reply = Exchange {
        recipient: request.node,
        node: s.node.clone(),
        name: s.name.clone(),
        port: rt.port.lock().unwrap().unwrap_or(47832),
        revisions: {
            let known: HashSet<_> = request.known.iter().collect();
            s.revisions()
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?
                .into_iter()
                .filter(|r| !known.contains(&r.id))
                .collect()
        },
        known: s
            .revision_ids()
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?,
        known_blobs: s
            .blob_ids()
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?,
        blobs: s
            .blobs_missing(&request.known_blobs)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?,
    };
    drop(s);
    rt.pending.lock().unwrap().remove(&channel);
    Ok(Json(
        seal(&key, &reply).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?,
    ))
}
fn is_local(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => v.is_private() || v.is_loopback() || v.is_link_local(),
        IpAddr::V6(v) => v.is_loopback() || v.is_unique_local() || v.is_unicast_link_local(),
    }
}
fn validate_address(address: &str) -> Result<SocketAddr> {
    let addr: SocketAddr = address.parse().map_err(|_| "Invalid network address")?;
    if !is_local(addr.ip()) || addr.port() == 0 {
        return Err("Only local network addresses are allowed".into());
    }
    Ok(addr)
}
fn decode_key(key: &str) -> Result<[u8; 32]> {
    URL_SAFE_NO_PAD
        .decode(key)
        .map_err(|_| "Invalid pairing key")?
        .try_into()
        .map_err(|_| "Invalid pairing key".into())
}
fn seal<T: Serialize>(key: &str, value: &T) -> Result<Packet> {
    let cipher = Aes256Gcm::new_from_slice(&decode_key(key)?).unwrap();
    let mut nonce = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut nonce);
    let bytes = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    if bytes.len() > 45 * 1024 * 1024 {
        return Err("Space is too large for this sync version; export a backup".into());
    }
    let encrypted = cipher
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: &bytes,
                aad: PROTOCOL,
            },
        )
        .map_err(|_| "Encryption failed")?;
    Ok(Packet {
        nonce: STANDARD.encode(nonce),
        ciphertext: STANDARD.encode(encrypted),
    })
}
fn open<T: for<'de> Deserialize<'de>>(key: &str, packet: &Packet) -> Result<T> {
    let cipher = Aes256Gcm::new_from_slice(&decode_key(key)?).unwrap();
    let nonce = STANDARD
        .decode(&packet.nonce)
        .map_err(|_| "Invalid nonce")?;
    if nonce.len() != 12 || packet.ciphertext.len() > MAX_PACKET {
        return Err("Invalid encrypted packet".into());
    }
    let bytes = STANDARD
        .decode(&packet.ciphertext)
        .map_err(|_| "Invalid ciphertext")?;
    let plain = cipher
        .decrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: &bytes,
                aad: PROTOCOL,
            },
        )
        .map_err(|_| "Authentication failed")?;
    serde_json::from_slice(&plain).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn encrypted_packets_reject_wrong_key_and_tampering() {
        let key = URL_SAFE_NO_PAD.encode([9u8; 32]);
        let mut p = seal(&key, &json!({"body":"private thought"})).unwrap();
        assert!(!p.ciphertext.contains("private"));
        assert_eq!(open::<Value>(&key, &p).unwrap()["body"], "private thought");
        assert!(open::<Value>(&URL_SAFE_NO_PAD.encode([8u8; 32]), &p).is_err());
        p.nonce = STANDARD.encode([1u8; 12]);
        assert!(open::<Value>(&key, &p).is_err());
    }
    #[test]
    fn pairing_is_lan_only() {
        assert!(validate_address("192.168.1.2:47832").is_ok());
        assert!(validate_address("8.8.8.8:47832").is_err());
        assert!(validate_address("example.com:80").is_err());
    }
    #[tokio::test]
    async fn two_devices_pair_and_exchange_then_reject_revoked_peer() {
        let da = tempfile::tempdir().unwrap();
        let db = tempfile::tempdir().unwrap();
        let a = Runtime::new(Store::open(da.path()).unwrap());
        let b = Runtime::new(Store::open(db.path()).unwrap());
        a.store.lock().unwrap().create(0.0, 0.0).unwrap();
        b.store.lock().unwrap().create(310.0, 0.0).unwrap();
        let invitation = a.invite().await.unwrap();
        let raw = invitation["code"]
            .as_str()
            .unwrap()
            .strip_prefix("elsewhere: ")
            .unwrap();
        let mut invite: Invite =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(raw).unwrap()).unwrap();
        invite.address = format!("127.0.0.1:{}", a.port.lock().unwrap().unwrap());
        let code = format!(
            "elsewhere: {}",
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&invite).unwrap())
        );
        b.pair(&code).await.unwrap();
        assert_eq!(
            a.store.lock().unwrap().snapshot().unwrap().thoughts.len(),
            2
        );
        assert_eq!(
            b.store.lock().unwrap().snapshot().unwrap().thoughts.len(),
            2
        );
        let t = b.store.lock().unwrap().snapshot().unwrap().thoughts[0].clone();
        let mut edited = t.clone();
        edited.note.body = "from the other device".into();
        b.store
            .lock()
            .unwrap()
            .commit(edited, &[t.note.revision_id], false)
            .unwrap();
        let result = b.sync().await.unwrap();
        assert_eq!(result["devices"][0]["ok"], true);
        assert!(a
            .store
            .lock()
            .unwrap()
            .snapshot()
            .unwrap()
            .thoughts
            .iter()
            .any(|t| t.note.body == "from the other device"));
        let peer_id = b.store.lock().unwrap().node.clone();
        a.store.lock().unwrap().remove_peer(&peer_id).unwrap();
        assert_eq!(b.sync().await.unwrap()["devices"][0]["ok"], false);
        a.stop();
        b.stop();
    }
    #[tokio::test]
    async fn deleting_needs_a_live_server_token() {
        let d = tempfile::tempdir().unwrap();
        let rt = Runtime::new(Store::open(d.path()).unwrap());
        let t = rt.store.lock().unwrap().create(0.0, 0.0).unwrap();
        assert!(rt.confirm(&t.note.id, "made-up").is_err());
        let arm = rt.arm(&t.note.id).unwrap();
        assert!(rt.confirm(&t.note.id, "wrong").is_err());
        assert_eq!(
            rt.store.lock().unwrap().snapshot().unwrap().thoughts.len(),
            1
        );
        let arm2 = rt.arm(&t.note.id).unwrap();
        rt.confirm(&t.note.id, arm2["token"].as_str().unwrap())
            .unwrap();
        assert!(rt
            .confirm(&t.note.id, arm["token"].as_str().unwrap())
            .is_err());
    }
}
