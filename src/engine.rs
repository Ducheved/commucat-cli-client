use crate::config::ClientState;
use crate::hexutil::{decode_hex, decode_hex32, encode_hex};
use anyhow::{Context, Result, anyhow};
use bytes::{Buf, BufMut, Bytes, BytesMut};
use commucat_crypto::{HandshakePattern, NoiseConfig, build_handshake};
use commucat_proto::{ControlEnvelope, Frame, FramePayload, FrameType, PROTOCOL_VERSION};
use futures::future::poll_fn;
use h2::{RecvStream, SendStream, client};
use http::{Request, Uri, header};
use rustls::client::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::{
    Certificate, ClientConfig, DigitallySignedStruct, OwnedTrustAnchor, RootCertStore, ServerName,
};
use serde_json::json;
use std::convert::TryFrom;
use std::fs::File;
use std::io::BufReader;
use std::sync::Arc;
use std::time::SystemTime;
use tokio::net::{TcpStream, lookup_host};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_rustls::TlsConnector;
use tracing::{error, warn};
use webpki_roots::TLS_SERVER_ROOTS;

const USER_AGENT: &str = "CommuCat-CLI/0.1";

pub struct EngineHandle {
    sender: mpsc::Sender<EngineCommand>,
}

#[derive(Debug)]
pub enum EngineCommand {
    Connect(ClientState),
    Disconnect,
    Join {
        channel_id: u64,
        members: Vec<String>,
        relay: bool,
    },
    SendMessage {
        channel_id: u64,
        body: Vec<u8>,
    },
    Leave {
        channel_id: u64,
    },
    Presence {
        state: String,
    },
}

#[derive(Debug, Clone)]
pub enum ClientEvent {
    Connected { session_id: String },
    Disconnected { reason: String },
    Frame(Frame),
    Error { detail: String },
    Log { line: String },
}

pub fn create_engine(buffer: usize, queue: usize) -> (EngineHandle, mpsc::Receiver<ClientEvent>) {
    let (tx, rx) = mpsc::channel(buffer);
    let (event_tx, event_rx) = mpsc::channel(queue);
    tokio::spawn(async move {
        if let Err(err) = engine_loop(rx, event_tx.clone()).await {
            let _ = event_tx
                .send(ClientEvent::Error {
                    detail: err.to_string(),
                })
                .await;
        }
    });
    (EngineHandle { sender: tx }, event_rx)
}

impl EngineHandle {
    pub async fn send(&self, command: EngineCommand) -> Result<()> {
        self.sender
            .send(command)
            .await
            .map_err(|_| anyhow!("engine offline"))
    }
}

struct ActiveConnection {
    session_id: String,
    send_stream: SendStream<Bytes>,
    sequence: u64,
    reader_task: JoinHandle<()>,
    driver_task: JoinHandle<()>,
}

impl ActiveConnection {
    async fn connect(state: ClientState, events: mpsc::Sender<ClientEvent>) -> Result<Self> {
        let uri: Uri = state.server_url.parse().context("invalid server url")?;
        let scheme = uri.scheme_str().unwrap_or("https");
        if scheme != "https" {
            return Err(anyhow!("only https is supported"));
        }
        let host = uri
            .host()
            .ok_or_else(|| anyhow!("host missing"))?
            .to_string();
        let authority = uri
            .authority()
            .map(|a| a.to_string())
            .unwrap_or_else(|| host.clone());
        let port = uri.port_u16().unwrap_or(443);
        let path = match uri.path_and_query() {
            Some(pq) if pq.as_str() != "/" => pq.as_str().to_string(),
            _ => "/connect".to_string(),
        };
        let addr = format!("{}:{}", host, port);
        let addrs = lookup_host(addr.clone())
            .await
            .context("dns lookup failed")?
            .collect::<Vec<_>>();
        if addrs.is_empty() {
            return Err(anyhow!("no address for server"));
        }
        let mut last_err = None;
        let mut tcp_opt = None;
        for candidate in addrs.iter() {
            match TcpStream::connect(candidate).await {
                Ok(stream) => {
                    tcp_opt = Some(stream);
                    let _ = events
                        .send(ClientEvent::Log {
                            line: format!("connected to {}", candidate),
                        })
                        .await;
                    break;
                }
                Err(err) => {
                    let err_msg = err.to_string();
                    last_err = Some(err);
                    let _ = events
                        .send(ClientEvent::Log {
                            line: format!("connect attempt {} failed: {}", candidate, err_msg),
                        })
                        .await;
                }
            }
        }
        let tcp = tcp_opt.ok_or_else(|| {
            let err = last_err
                .map(|e| e.to_string())
                .unwrap_or_else(|| "all sockets failed".to_string());
            anyhow!("tcp connect failed: {}", err)
        })?;
        tcp.set_nodelay(true).ok();
        let connector = build_tls_connector(&state)?;
        let server_name =
            ServerName::try_from(host.as_str()).map_err(|_| anyhow!("invalid server name"))?;
        let tls = connector
            .connect(server_name, tcp)
            .await
            .context("tls connect failed")?;
        let (mut sender, connection) = client::handshake(tls)
            .await
            .context("h2 handshake failed")?;
        let driver_task = tokio::spawn(async move {
            if let Err(err) = connection.await {
                warn!("h2 connection ended: {}", err);
            }
        });
        let mut request_builder = Request::builder()
            .method("POST")
            .uri(
                Uri::builder()
                    .scheme("https")
                    .authority(authority.as_str())
                    .path_and_query(path.as_str())
                    .build()
                    .context("build request uri")?,
            )
            .header(header::CONTENT_TYPE, "application/octet-stream")
            .header(header::USER_AGENT, USER_AGENT)
            .header(header::TE, "trailers");
        if let Some(tp) = state.traceparent.as_ref() {
            request_builder = request_builder.header("traceparent", tp.as_str());
        }
        let request = request_builder.body(())?;
        let (response, mut send_stream) = sender
            .send_request(request, false)
            .context("send request")?;
        let device_id = state.device_id.clone();
        let pattern_label = state.noise_pattern.to_uppercase();
        let device_keys = state.into_device_keypair()?;
        let pattern = parse_pattern(&state.noise_pattern)?;
        let remote_static = if let HandshakePattern::Ik = pattern {
            let raw = state
                .server_static
                .as_ref()
                .ok_or_else(|| anyhow!("server_static required for IK"))?;
            Some(decode_hex32(raw)?)
        } else {
            None
        };
        let noise = NoiseConfig {
            pattern,
            prologue: state.prologue.as_bytes().to_vec(),
            local_private: device_keys.private,
            local_static_public: Some(device_keys.public),
            remote_static_public: remote_static,
        };
        let mut handshake = build_handshake(&noise, true).context("noise init")?;
        let hello_bytes = handshake.write_message(&[]).context("noise message one")?;
        let hello_frame = Frame {
            channel_id: 0,
            sequence: 1,
            frame_type: FrameType::Hello,
            payload: FramePayload::Control(ControlEnvelope {
                properties: json!({
                    "protocol_version": PROTOCOL_VERSION,
                    "pattern": pattern_label.clone(),
                    "device_id": device_id.clone(),
                    "client_static": encode_hex(&device_keys.public),
                    "handshake": encode_hex(&hello_bytes),
                    "capabilities": ["noise", "zstd"],
                }),
            }),
        };
        let _ = events
            .send(ClientEvent::Log {
                line: format!("handshake start for {}", device_id),
            })
            .await;
        send_frame_raw(
            &mut send_stream,
            hello_frame.encode().context("encode hello")?,
        )
        .await?;
        let response = response.await.context("handshake response")?;
        let mut recv_stream = response.into_body();
        let mut buffer = BytesMut::new();
        let mut session_id = String::new();
        let mut next_sequence = 2u64;
        let connection = 'handshake: loop {
            match recv_stream.data().await {
                Some(Ok(bytes)) => buffer.put_slice(&bytes),
                Some(Err(err)) => return Err(anyhow!(format!("handshake read failed: {}", err))),
                None => return Err(anyhow!("server closed during handshake")),
            }
            loop {
                match Frame::decode(&buffer) {
                    Ok((frame, consumed)) => {
                        buffer.advance(consumed);
                        match frame.frame_type {
                            FrameType::Auth => {
                                let envelope = control_payload(frame.payload)?;
                                let handshake_hex = envelope
                                    .get("handshake")
                                    .and_then(|v| v.as_str())
                                    .ok_or_else(|| anyhow!("missing handshake"))?;
                                let handshake_bytes = decode_hex(handshake_hex)?;
                                let payload = handshake
                                    .read_message(&handshake_bytes)
                                    .context("noise message two")?;
                                if !payload.is_empty() {
                                    let value: serde_json::Value = serde_json::from_slice(&payload)
                                        .context("handshake payload decode")?;
                                    session_id = value
                                        .get("session")
                                        .and_then(|v| v.as_str())
                                        .ok_or_else(|| anyhow!("session missing"))?
                                        .to_string();
                                }
                                let final_bytes = handshake
                                    .write_message(&[])
                                    .context("noise message three")?;
                                let response_frame = Frame {
                                    channel_id: frame.channel_id,
                                    sequence: 2,
                                    frame_type: FrameType::Auth,
                                    payload: FramePayload::Control(ControlEnvelope {
                                        properties: json!({
                                            "handshake": encode_hex(&final_bytes),
                                        }),
                                    }),
                                };
                                send_frame_raw(
                                    &mut send_stream,
                                    response_frame.encode().context("encode auth")?,
                                )
                                .await?;
                                next_sequence = 3;
                            }
                            FrameType::Ack => {
                                if is_handshake_ack(&frame) {
                                    if session_id.is_empty() {
                                        session_id = "unknown".to_string();
                                    }
                                    let reader_task =
                                        spawn_reader(recv_stream, buffer, events.clone());
                                    let connection = ActiveConnection {
                                        session_id: session_id.clone(),
                                        send_stream,
                                        sequence: next_sequence,
                                        reader_task,
                                        driver_task,
                                    };
                                    let _ = events
                                        .send(ClientEvent::Log {
                                            line: format!("handshake ok: session {}", session_id),
                                        })
                                        .await;
                                    break 'handshake connection;
                                }
                                let _ = events.send(ClientEvent::Frame(frame)).await;
                            }
                            FrameType::Error => {
                                let _ = events.send(ClientEvent::Frame(frame.clone())).await;
                                return Err(anyhow!("handshake rejected"));
                            }
                            other => {
                                let _ = events.send(ClientEvent::Frame(frame.clone())).await;
                                warn!("unexpected frame during handshake: {:?}", other);
                            }
                        }
                    }
                    Err(commucat_proto::CodecError::UnexpectedEof) => break,
                    Err(err) => return Err(anyhow!(format!("handshake decode failed: {:?}", err))),
                }
            }
        };
        Ok(connection)
    }

    async fn send_join(
        &mut self,
        channel_id: u64,
        members: Vec<String>,
        relay: bool,
    ) -> Result<()> {
        let frame = Frame {
            channel_id,
            sequence: self.next_sequence(),
            frame_type: FrameType::Join,
            payload: FramePayload::Control(ControlEnvelope {
                properties: json!({
                    "members": members,
                    "relay": relay,
                }),
            }),
        };
        self.send(frame).await
    }

    async fn send_leave(&mut self, channel_id: u64) -> Result<()> {
        let frame = Frame {
            channel_id,
            sequence: self.next_sequence(),
            frame_type: FrameType::Leave,
            payload: FramePayload::Control(ControlEnvelope {
                properties: json!({}),
            }),
        };
        self.send(frame).await
    }

    async fn send_message(&mut self, channel_id: u64, body: Vec<u8>) -> Result<()> {
        let frame = Frame {
            channel_id,
            sequence: self.next_sequence(),
            frame_type: FrameType::Msg,
            payload: FramePayload::Opaque(body),
        };
        self.send(frame).await
    }

    async fn send_presence(&mut self, state: String) -> Result<()> {
        let frame = Frame {
            channel_id: 0,
            sequence: self.next_sequence(),
            frame_type: FrameType::Presence,
            payload: FramePayload::Control(ControlEnvelope {
                properties: json!({
                    "state": state,
                }),
            }),
        };
        self.send(frame).await
    }

    async fn send(&mut self, frame: Frame) -> Result<()> {
        let payload = frame.encode().context("encode frame")?;
        send_frame_raw(&mut self.send_stream, payload).await
    }

    fn next_sequence(&mut self) -> u64 {
        let current = self.sequence;
        self.sequence += 1;
        current
    }

    async fn shutdown(&mut self) {
        let _ = self.send_stream.send_data(Bytes::new(), true);
    }
}

impl Drop for ActiveConnection {
    fn drop(&mut self) {
        self.reader_task.abort();
        self.driver_task.abort();
    }
}

async fn engine_loop(
    mut commands: mpsc::Receiver<EngineCommand>,
    events: mpsc::Sender<ClientEvent>,
) -> Result<()> {
    let mut connection: Option<ActiveConnection> = None;
    while let Some(command) = commands.recv().await {
        match command {
            EngineCommand::Connect(state) => {
                if connection.is_some() {
                    let _ = events
                        .send(ClientEvent::Error {
                            detail: "already connected".to_string(),
                        })
                        .await;
                    continue;
                }
                match ActiveConnection::connect(state.clone(), events.clone()).await {
                    Ok(conn) => {
                        let session = conn.session_id.clone();
                        let _ = events
                            .send(ClientEvent::Connected {
                                session_id: session,
                            })
                            .await;
                        connection = Some(conn);
                    }
                    Err(err) => {
                        error!("connect failed: {}", err);
                        let _ = events
                            .send(ClientEvent::Error {
                                detail: err.to_string(),
                            })
                            .await;
                    }
                }
            }
            EngineCommand::Disconnect => {
                if let Some(mut conn) = connection.take() {
                    conn.shutdown().await;
                    let _ = events
                        .send(ClientEvent::Disconnected {
                            reason: "disconnected".to_string(),
                        })
                        .await;
                }
            }
            EngineCommand::Join {
                channel_id,
                members,
                relay,
            } => {
                if let Some(conn) = connection.as_mut() {
                    if let Err(err) = conn.send_join(channel_id, members, relay).await {
                        let _ = events
                            .send(ClientEvent::Error {
                                detail: err.to_string(),
                            })
                            .await;
                    }
                } else {
                    let _ = events
                        .send(ClientEvent::Error {
                            detail: "no active connection".to_string(),
                        })
                        .await;
                }
            }
            EngineCommand::SendMessage { channel_id, body } => {
                if let Some(conn) = connection.as_mut() {
                    if let Err(err) = conn.send_message(channel_id, body).await {
                        let _ = events
                            .send(ClientEvent::Error {
                                detail: err.to_string(),
                            })
                            .await;
                    }
                } else {
                    let _ = events
                        .send(ClientEvent::Error {
                            detail: "no active connection".to_string(),
                        })
                        .await;
                }
            }
            EngineCommand::Leave { channel_id } => {
                if let Some(conn) = connection.as_mut() {
                    if let Err(err) = conn.send_leave(channel_id).await {
                        let _ = events
                            .send(ClientEvent::Error {
                                detail: err.to_string(),
                            })
                            .await;
                    }
                } else {
                    let _ = events
                        .send(ClientEvent::Error {
                            detail: "no active connection".to_string(),
                        })
                        .await;
                }
            }
            EngineCommand::Presence { state } => {
                if let Some(conn) = connection.as_mut() {
                    if let Err(err) = conn.send_presence(state).await {
                        let _ = events
                            .send(ClientEvent::Error {
                                detail: err.to_string(),
                            })
                            .await;
                    }
                } else {
                    let _ = events
                        .send(ClientEvent::Error {
                            detail: "no active connection".to_string(),
                        })
                        .await;
                }
            }
        }
    }
    Ok(())
}

fn parse_pattern(pattern: &str) -> Result<HandshakePattern> {
    match pattern.to_uppercase().as_str() {
        "XK" => Ok(HandshakePattern::Xk),
        "IK" => Ok(HandshakePattern::Ik),
        other => Err(anyhow!(format!("unsupported pattern: {}", other))),
    }
}

fn control_payload(payload: FramePayload) -> Result<serde_json::Value> {
    match payload {
        FramePayload::Control(ControlEnvelope { properties }) => Ok(properties),
        _ => Err(anyhow!("expected control payload")),
    }
}

fn is_handshake_ack(frame: &Frame) -> bool {
    if let FramePayload::Control(ControlEnvelope { properties }) = &frame.payload {
        if let Some(value) = properties.get("handshake") {
            return value == "ok";
        }
    }
    false
}

fn spawn_reader(
    mut stream: RecvStream,
    mut buffer: BytesMut,
    events: mpsc::Sender<ClientEvent>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            loop {
                match Frame::decode(&buffer) {
                    Ok((frame, consumed)) => {
                        buffer.advance(consumed);
                        if events.send(ClientEvent::Frame(frame)).await.is_err() {
                            return;
                        }
                    }
                    Err(commucat_proto::CodecError::UnexpectedEof) => break,
                    Err(err) => {
                        let detail = format!("decode error: {:?}", err);
                        let _ = events
                            .send(ClientEvent::Error {
                                detail: detail.clone(),
                            })
                            .await;
                        buffer.clear();
                        break;
                    }
                }
            }
            match stream.data().await {
                Some(Ok(bytes)) => buffer.put_slice(&bytes),
                Some(Err(err)) => {
                    let detail = format!("receive failed: {}", err);
                    let _ = events
                        .send(ClientEvent::Error {
                            detail: detail.clone(),
                        })
                        .await;
                    let _ = events
                        .send(ClientEvent::Disconnected { reason: detail })
                        .await;
                    return;
                }
                None => {
                    let _ = events
                        .send(ClientEvent::Disconnected {
                            reason: "remote closed".to_string(),
                        })
                        .await;
                    return;
                }
            }
        }
    })
}

async fn send_frame_raw(stream: &mut SendStream<Bytes>, payload: Vec<u8>) -> Result<()> {
    let len = payload.len();
    stream.reserve_capacity(len);
    while stream.capacity() < len {
        match poll_fn(|cx| stream.poll_capacity(cx)).await {
            Some(Ok(_)) => {}
            Some(Err(err)) => return Err(anyhow!(format!("capacity error: {}", err))),
            None => return Err(anyhow!("stream closed")),
        }
    }
    stream
        .send_data(Bytes::from(payload), false)
        .map_err(|err| anyhow!(format!("send failed: {}", err)))
}

fn build_tls_connector(state: &ClientState) -> Result<TlsConnector> {
    let mut roots = RootCertStore::empty();
    if let Some(path) = state.tls_ca_path.as_ref() {
        let file = File::open(path).context("open tls ca")?;
        let mut reader = BufReader::new(file);
        let certs = rustls_pemfile::certs(&mut reader).context("parse tls ca")?;
        let (added, _) = roots.add_parsable_certificates(&certs);
        if added == 0 {
            return Err(anyhow!("no certificates loaded"));
        }
    } else {
        roots.add_trust_anchors(TLS_SERVER_ROOTS.iter().map(|ta| {
            OwnedTrustAnchor::from_subject_spki_name_constraints(
                ta.subject,
                ta.spki,
                ta.name_constraints,
            )
        }));
    }
    let mut config = ClientConfig::builder()
        .with_safe_defaults()
        .with_root_certificates(roots)
        .with_no_client_auth();
    config.alpn_protocols.push(b"h2".to_vec());
    config.alpn_protocols.push(b"http/1.1".to_vec());
    config.enable_early_data = false;
    if state.insecure {
        config
            .dangerous()
            .set_certificate_verifier(Arc::new(NoVerifier));
    }
    Ok(TlsConnector::from(Arc::new(config)))
}

struct NoVerifier;

impl ServerCertVerifier for NoVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &Certificate,
        _intermediates: &[Certificate],
        _server_name: &ServerName,
        _scts: &mut dyn Iterator<Item = &[u8]>,
        _ocsp: &[u8],
        _now: SystemTime,
    ) -> std::result::Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &Certificate,
        _dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &Certificate,
        _dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }
}
