mod config;
mod device;
mod engine;
mod hexutil;
mod rest;
mod tui;

use crate::config::{ClientState, ClientStateParams, FriendEntry, docs_path, state_path};
use crate::device::describe_keys;
use crate::hexutil::decode_hex32;
use crate::rest::{
    DeviceEntry, FriendEntryPayload, PairingClaimResponse, PairingTicket, RestClient,
};
use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use commucat_crypto::{DeviceCertificate, DeviceKeyPair};
use std::fs;
use std::path::Path;
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(
    author = "CommuCat",
    version,
    about = "Interactive CCP-1 console client",
    propagate_version = true
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[allow(clippy::large_enum_variant)]
#[derive(Subcommand)]
enum Command {
    Init(InitArgs),
    Pair(PairArgs),
    #[command(subcommand)]
    Devices(DevicesCommand),
    #[command(subcommand)]
    Friends(FriendsCommand),
    Claim(ClaimArgs),
    Export,
    Docs(DocsArgs),
    Tui,
}

#[derive(Subcommand)]
enum DevicesCommand {
    List(DevicesListArgs),
    Revoke(DevicesRevokeArgs),
    AttachCert(DevicesAttachCertArgs),
}

#[derive(Subcommand)]
enum FriendsCommand {
    List,
    Add(FriendsAddArgs),
    Remove(FriendsRemoveArgs),
    Pull(FriendsSessionArgs),
    Push(FriendsSessionArgs),
}

#[derive(Args)]
struct DevicesListArgs {
    #[arg(long)]
    session: Option<String>,
}

#[derive(Args)]
struct DevicesRevokeArgs {
    device_id: String,
    #[arg(long)]
    session: Option<String>,
}

#[derive(Args)]
struct DevicesAttachCertArgs {
    #[arg(long)]
    certificate: String,
    #[arg(long)]
    issuer: Option<String>,
}

#[derive(Args)]
struct InitArgs {
    #[arg(long)]
    server: String,
    #[arg(long)]
    domain: String,
    #[arg(long)]
    username: Option<String>,
    #[arg(long)]
    user_id: Option<String>,
    #[arg(long)]
    display_name: Option<String>,
    #[arg(long)]
    avatar_url: Option<String>,
    #[arg(long)]
    device_id: Option<String>,
    #[arg(long)]
    device_name: Option<String>,
    #[arg(long, default_value = "XK")]
    pattern: String,
    #[arg(long, default_value = "commucat")]
    prologue: String,
    #[arg(long)]
    tls_ca: Option<String>,
    #[arg(long)]
    server_static: Option<String>,
    #[arg(long, default_value_t = false)]
    insecure: bool,
    #[arg(long, default_value = "online")]
    presence: String,
    #[arg(long, default_value_t = 30)]
    presence_interval: u64,
    #[arg(long)]
    traceparent: Option<String>,
    #[arg(long)]
    session: Option<String>,
    #[arg(long)]
    pair_code: Option<String>,
    #[arg(long, default_value_t = false)]
    force: bool,
}

#[derive(Args)]
struct PairArgs {
    #[arg(long)]
    ttl: Option<i64>,
    #[arg(long)]
    session: Option<String>,
}

#[derive(Args)]
struct ClaimArgs {
    #[arg()]
    pair_code: String,
    #[arg(long)]
    device_name: Option<String>,
    #[arg(long)]
    server: Option<String>,
    #[arg(long)]
    session: Option<String>,
}

#[derive(Args)]
struct FriendsAddArgs {
    #[arg()]
    user_id: String,
    #[arg(long)]
    handle: Option<String>,
    #[arg(long)]
    alias: Option<String>,
    #[arg(long)]
    push: bool,
    #[arg(long)]
    session: Option<String>,
}

#[derive(Args)]
struct FriendsRemoveArgs {
    #[arg()]
    user_id: String,
    #[arg(long)]
    session: Option<String>,
    #[arg(long)]
    push: bool,
}

#[derive(Args)]
struct FriendsSessionArgs {
    #[arg(long)]
    session: Option<String>,
}

#[derive(Args)]
struct DocsArgs {
    #[arg(long, default_value = "ru")]
    lang: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();
    let cli = Cli::parse();
    match cli.command {
        Some(Command::Init(args)) => init_profile(args).await?,
        Some(Command::Pair(args)) => issue_pair(args).await?,
        Some(Command::Devices(cmd)) => handle_devices(cmd).await?,
        Some(Command::Friends(cmd)) => handle_friends(cmd).await?,
        Some(Command::Claim(args)) => claim_device(args).await?,
        Some(Command::Export) => export_profile()?,
        Some(Command::Docs(args)) => print_docs(&args.lang)?,
        Some(Command::Tui) => launch_tui().await?,
        None => launch_tui().await?,
    }
    Ok(())
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .try_init();
}

async fn init_profile(args: InitArgs) -> Result<()> {
    let InitArgs {
        server,
        domain,
        username,
        user_id,
        display_name,
        avatar_url,
        device_id,
        device_name,
        pattern,
        prologue,
        tls_ca,
        server_static,
        insecure,
        presence,
        presence_interval,
        traceparent,
        session,
        pair_code,
        force,
    } = args;
    let mut server_ca_from_info: Option<String> = None;
    let path = state_path()?;
    if path.exists() && !force {
        bail!("профиль уже существует: {}", path.display());
    }
    if pair_code.is_none() && username.is_none() && user_id.is_none() {
        bail!("укажите --username (для нового пользователя) или --user-id (для существующего)");
    }
    if let Some(code) = pair_code {
        let rest = RestClient::new(&server)?;
        let claim = rest.claim_pairing(&code, device_name.as_deref()).await?;
        let server_static_resolved = match server_static.clone() {
            Some(value) => Some(value),
            None => {
                let info = rest.server_info().await.context("fetch server info")?;
                if info.domain != domain {
                    println!("warning: server reports domain {}", info.domain);
                }
                if !info.supported_patterns.is_empty()
                    && !info
                        .supported_patterns
                        .iter()
                        .any(|p| p.eq_ignore_ascii_case(&pattern))
                {
                    println!(
                        "warning: server supports patterns {:?}, requested {}",
                        info.supported_patterns, pattern
                    );
                }
                if !info.supported_versions.is_empty() {
                    println!("server protocol versions: {:?}", info.supported_versions);
                }
                if let Some(pairing) = info.pairing.clone() {
                    println!(
                        "pairing: auto_approve={} max_auto_devices={} ttl={}s",
                        pairing.auto_approve, pairing.max_auto_devices, pairing.pairing_ttl
                    );
                }
                println!("server noise_public={}", info.noise_public);
                server_ca_from_info = info.device_ca_public.clone();
                Some(info.noise_public)
            }
        };
        let private = decode_hex32(&claim.private_key)?;
        let public = decode_hex32(&claim.public_key)?;
        let keys = DeviceKeyPair { public, private };
        let device_ca_public = claim
            .device_ca_public
            .clone()
            .or_else(|| server_ca_from_info.clone());
        let state = ClientState::from_params(ClientStateParams {
            device_id: claim.device_id.clone(),
            server_url: server.clone(),
            domain,
            keys,
            pattern,
            prologue,
            tls_ca_path: tls_ca,
            server_static: server_static_resolved,
            insecure,
            presence_state: presence,
            presence_interval_secs: presence_interval,
            traceparent,
            user_handle: Some(claim.user.handle.clone()),
            user_display_name: claim.user.display_name.clone(),
            user_avatar_url: claim.user.avatar_url.clone(),
            user_id: Some(claim.user.id.clone()),
            session_token: session.clone(),
            device_name: claim.device_name.clone().or(device_name),
            friends: Vec::new(),
            device_certificate: claim.device_certificate.clone(),
            device_ca_public,
        });
        state.save()?;
        println!("state saved to {}", path.display());
        println!(
            "{}",
            describe_keys(&claim.device_id, &state.device_keypair()?)
        );
        print_claim_summary(&claim);
        if let Some(cert) = claim.device_certificate.as_ref() {
            println!(
                "certificate_serial={} expires_at={}",
                cert.data.serial, cert.data.expires_at
            );
        }
        if let Some(ca_hex) = state.device_ca_public.as_ref() {
            println!("device_ca_public={}", ca_hex);
        }
        return Ok(());
    }

    let handle_for_state = username.clone();
    let generated_device = device_id.unwrap_or_else(|| device::generate_device_id("device"));
    let keys = device::generate_keypair()?;
    let server_static_resolved = match server_static.clone() {
        Some(value) => Some(value),
        None => {
            let rest = RestClient::new(&server)?;
            let info = rest.server_info().await.context("fetch server info")?;
            if info.domain != domain {
                println!("warning: server reports domain {}", info.domain);
            }
            if !info.supported_patterns.is_empty()
                && !info
                    .supported_patterns
                    .iter()
                    .any(|p| p.eq_ignore_ascii_case(&pattern))
            {
                println!(
                    "warning: server supports patterns {:?}, requested {}",
                    info.supported_patterns, pattern
                );
            }
            if !info.supported_versions.is_empty() {
                println!("server protocol versions: {:?}", info.supported_versions);
            }
            if let Some(pairing) = info.pairing.clone() {
                println!(
                    "pairing: auto_approve={} max_auto_devices={} ttl={}s",
                    pairing.auto_approve, pairing.max_auto_devices, pairing.pairing_ttl
                );
            }
            println!("server noise_public={}", info.noise_public);
            server_ca_from_info = info.device_ca_public.clone();
            Some(info.noise_public)
        }
    };
    let state = ClientState::from_params(ClientStateParams {
        device_id: generated_device.clone(),
        server_url: server,
        domain,
        keys: keys.clone(),
        pattern,
        prologue,
        tls_ca_path: tls_ca,
        server_static: server_static_resolved,
        insecure,
        presence_state: presence,
        presence_interval_secs: presence_interval,
        traceparent,
        user_handle: handle_for_state,
        user_display_name: display_name.clone(),
        user_avatar_url: avatar_url.clone(),
        user_id: user_id.clone(),
        session_token: session.clone(),
        device_name,
        friends: Vec::new(),
        device_certificate: None,
        device_ca_public: server_ca_from_info.clone(),
    });
    state.save()?;
    println!("state saved to {}", path.display());
    println!("{}", describe_keys(&generated_device, &keys));
    if let Some(name) = username.as_ref() {
        println!(
            "Устройство зарегистрируется автоматически при первом подключении как пользователь '{}'.",
            name
        );
    }
    if let Some(name) = display_name {
        println!("display_name={} (отправляется на сервер)", name);
    }
    if let Some(url) = avatar_url {
        println!("avatar_url={} (будет применён при handshake)", url);
    }
    if let Some(token) = session {
        println!("session={} (будет использована REST API)", token);
    }
    if let Some(ca_hex) = state.device_ca_public.as_ref() {
        println!("device_ca_public={}", ca_hex);
    }
    if let Ok(doc_path) = docs_path("ru") {
        println!("Руководство: {}", doc_path.display());
    }
    Ok(())
}

fn export_profile() -> Result<()> {
    let state = ClientState::load()?;
    let keys = state.device_keypair()?;
    println!("{}", describe_keys(&state.device_id, &keys));
    println!("server_url={} domain={}", state.server_url, state.domain);
    Ok(())
}

fn print_docs(lang: &str) -> Result<()> {
    let path = docs_path(lang)?;
    let text = fs::read_to_string(&path).context("read docs")?;
    println!("{}", text);
    Ok(())
}

async fn launch_tui() -> Result<()> {
    let state = ClientState::load()?;
    tui::run_tui(state).await
}

async fn issue_pair(args: PairArgs) -> Result<()> {
    let PairArgs { ttl, session } = args;
    let mut state = ClientState::load()?;
    let session = resolve_session(session.as_deref(), &state)?;
    let rest = RestClient::new(&state.server_url)?;
    let ticket = rest.create_pairing(&session, ttl).await?;
    state.last_pairing_code = Some(ticket.pair_code.clone());
    state.last_pairing_expires_at = Some(ticket.expires_at.clone());
    state.last_pairing_issuer_device_id = ticket.issuer_device_id.clone();
    state.session_token = Some(session);
    state.save()?;
    print_pairing_summary(&ticket);
    Ok(())
}

async fn handle_devices(command: DevicesCommand) -> Result<()> {
    match command {
        DevicesCommand::List(args) => list_devices(args).await,
        DevicesCommand::Revoke(args) => revoke_device(args).await,
        DevicesCommand::AttachCert(args) => attach_device_certificate(args).await,
    }
}

async fn handle_friends(command: FriendsCommand) -> Result<()> {
    match command {
        FriendsCommand::List => {
            let state = ClientState::load()?;
            if state.friends().is_empty() {
                println!("Список друзей пуст.");
            } else {
                for entry in state.friends() {
                    let handle = entry
                        .alias
                        .as_ref()
                        .or(entry.handle.as_ref())
                        .map(|s| format!(" ({})", s))
                        .unwrap_or_default();
                    println!("{}{}", entry.user_id, handle);
                }
            }
            Ok(())
        }
        FriendsCommand::Add(args) => {
            let mut state = ClientState::load()?;
            let entry = FriendEntry {
                user_id: args.user_id.clone(),
                handle: args.handle.clone(),
                alias: args.alias.clone(),
            };
            state.upsert_friend(entry);
            state.save()?;
            println!("Добавлен друг {}", args.user_id);
            if args.push {
                let session = resolve_session(args.session.as_deref(), &state)?;
                let rest = RestClient::new(&state.server_url)?;
                rest.update_friends(&session, &friends_to_payload(state.friends()))
                    .await?;
                println!("Список друзей синхронизирован.");
            }
            Ok(())
        }
        FriendsCommand::Remove(args) => {
            let mut state = ClientState::load()?;
            if state.remove_friend(&args.user_id) {
                state.save()?;
                println!("Удалён друг {}", args.user_id);
                if args.push {
                    let session = resolve_session(args.session.as_deref(), &state)?;
                    let rest = RestClient::new(&state.server_url)?;
                    rest.update_friends(&session, &friends_to_payload(state.friends()))
                        .await?;
                    println!("Список друзей синхронизирован.");
                }
            } else {
                println!("Друг {} не найден", args.user_id);
            }
            Ok(())
        }
        FriendsCommand::Pull(args) => {
            let mut state = ClientState::load()?;
            let session = resolve_session(args.session.as_deref(), &state)?;
            let rest = RestClient::new(&state.server_url)?;
            let remote = rest.list_friends(&session).await?;
            let entries = remote
                .into_iter()
                .map(friend_from_payload)
                .collect::<Vec<_>>();
            state.set_friends(entries);
            state.save()?;
            println!("Загружено друзей: {}", state.friends().len());
            Ok(())
        }
        FriendsCommand::Push(args) => {
            let state = ClientState::load()?;
            let session = resolve_session(args.session.as_deref(), &state)?;
            let rest = RestClient::new(&state.server_url)?;
            rest.update_friends(&session, &friends_to_payload(state.friends()))
                .await?;
            println!("Список друзей синхронизирован.");
            Ok(())
        }
    }
}

async fn list_devices(args: DevicesListArgs) -> Result<()> {
    let DevicesListArgs { session } = args;
    let state = ClientState::load()?;
    let session = resolve_session(session.as_deref(), &state)?;
    let rest = RestClient::new(&state.server_url)?;
    let devices = rest.list_devices(&session).await?;
    if devices.is_empty() {
        println!("Нет зарегистрированных устройств.");
    } else {
        for device in devices {
            print_device_entry(&device);
        }
    }
    Ok(())
}

async fn revoke_device(args: DevicesRevokeArgs) -> Result<()> {
    let DevicesRevokeArgs { device_id, session } = args;
    let state = ClientState::load()?;
    let session = resolve_session(session.as_deref(), &state)?;
    let rest = RestClient::new(&state.server_url)?;
    rest.revoke_device(&session, &device_id).await?;
    println!("Устройство {} помечено как revoked", device_id);
    Ok(())
}

async fn attach_device_certificate(args: DevicesAttachCertArgs) -> Result<()> {
    let DevicesAttachCertArgs {
        certificate,
        issuer,
    } = args;
    let mut state = ClientState::load()?;
    let raw = if Path::new(&certificate).exists() {
        fs::read_to_string(&certificate).context("read certificate file")?
    } else {
        certificate
    };
    let certificate: DeviceCertificate =
        serde_json::from_str(raw.trim()).context("parse device certificate")?;
    if certificate.data.device_id != state.device_id {
        bail!(
            "сертификат выдан для {}, а профиль настроен для {}",
            certificate.data.device_id,
            state.device_id
        );
    }
    let keys = state.device_keypair()?;
    if certificate.data.public_key != keys.public {
        bail!("сертификат не соответствует текущему публичному ключу устройства");
    }
    match state.user_id.as_ref() {
        Some(expected) if expected != &certificate.data.user_id => {
            bail!(
                "сертификат принадлежит пользователю {}, а профиль связан с {}",
                certificate.data.user_id,
                expected
            );
        }
        _ => {}
    }
    let issuer_bytes = match issuer {
        Some(hex) => {
            let bytes = decode_hex32(&hex)?;
            if bytes != certificate.data.issuer {
                bail!("указанный issuer не совпадает с полем issuer сертификата");
            }
            bytes
        }
        None => certificate.data.issuer,
    };
    certificate
        .verify(&issuer_bytes)
        .context("подпись сертификата невалидна")?;
    state.set_certificate(&certificate)?;
    state.save()?;
    println!(
        "Сертификат устройства serial={} сохранён. Срок действия до {}.",
        certificate.data.serial, certificate.data.expires_at
    );
    Ok(())
}

async fn claim_device(args: ClaimArgs) -> Result<()> {
    let ClaimArgs {
        pair_code,
        device_name,
        server,
        session,
    } = args;
    let mut state_opt = ClientState::load().ok();
    let server = if let Some(server) = server {
        server
    } else if let Some(state) = &state_opt {
        state.server_url.clone()
    } else {
        bail!("укажите --server или инициализируйте профиль через init");
    };
    let rest = RestClient::new(&server)?;
    let claim = rest
        .claim_pairing(&pair_code, device_name.as_deref())
        .await?;
    print_claim_summary(&claim);
    if let Some(session) = session.as_ref() {
        println!("session={} (используйте для REST)", session);
    }
    if let Some(ref mut state) = state_opt {
        let private = decode_hex32(&claim.private_key)?;
        let public = decode_hex32(&claim.public_key)?;
        let keys = DeviceKeyPair { public, private };
        state.device_id = claim.device_id.clone();
        state.update_keys(&keys);
        if let Some(cert) = claim.device_certificate.as_ref() {
            state.set_certificate(cert)?;
        } else if let Some(ca_hex) = claim.device_ca_public.as_ref() {
            match state.device_ca_public.as_ref() {
                Some(existing) if existing == ca_hex => {}
                _ => {
                    state.device_ca_public = Some(ca_hex.clone());
                }
            }
        }
        state.user_handle = Some(claim.user.handle.clone());
        state.user_display_name = claim.user.display_name.clone();
        state.user_avatar_url = claim.user.avatar_url.clone();
        state.user_id = Some(claim.user.id.clone());
        state.device_name = claim.device_name.clone();
        if let Some(session) = session {
            state.session_token = Some(session);
        }
        state.save()?;
        println!("state обновлён в {}", state_path()?.display());
        if let Some(cert) = claim.device_certificate.as_ref() {
            println!(
                "certificate_serial={} expires_at={}",
                cert.data.serial, cert.data.expires_at
            );
        }
        if let Some(ca_hex) = state.device_ca_public.as_ref() {
            println!("device_ca_public={}", ca_hex);
        }
    }
    Ok(())
}

fn friend_from_payload(payload: FriendEntryPayload) -> FriendEntry {
    FriendEntry {
        user_id: payload.user_id,
        handle: payload.handle,
        alias: payload.alias,
    }
}

fn friends_to_payload(entries: &[FriendEntry]) -> Vec<FriendEntryPayload> {
    entries
        .iter()
        .map(|entry| FriendEntryPayload {
            user_id: entry.user_id.clone(),
            handle: entry.handle.clone(),
            alias: entry.alias.clone(),
        })
        .collect()
}

fn resolve_session(explicit: Option<&str>, state: &ClientState) -> Result<String> {
    if let Some(value) = explicit {
        return Ok(value.to_string());
    }
    if let Some(value) = state.session_token.as_ref() {
        return Ok(value.clone());
    }
    bail!("сессионный токен не найден: подключитесь (:connect) или передайте --session");
}

fn print_device_entry(entry: &DeviceEntry) {
    let current = if entry.current {
        " (текущее)"
    } else {
        ""
    };
    println!(
        "{}\t{}\t{}{}",
        entry.device_id, entry.status, entry.created_at, current
    );
}

fn print_pairing_summary(ticket: &PairingTicket) {
    println!("Pair code: {}", ticket.pair_code);
    if let Some(issuer) = ticket.issuer_device_id.as_ref() {
        println!("Выдано устройством: {}", issuer);
    }
    println!("Действителен до: {}", ticket.expires_at);
    println!("Seed: {}", ticket.device_seed);
}

fn print_claim_summary(claim: &PairingClaimResponse) {
    println!("Выдан device_id: {}", claim.device_id);
    println!("Private key: {}", claim.private_key);
    println!("Public key: {}", claim.public_key);
    println!("Seed: {}", claim.seed);
    if let Some(name) = &claim.device_name {
        println!("Имя устройства: {}", name);
    }
    println!("Пользователь: {} ({})", claim.user.handle, claim.user.id);
}
