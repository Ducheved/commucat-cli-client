mod config;
mod device;
mod engine;
mod hexutil;
mod rest;
mod tui;

use crate::config::{ClientState, ClientStateParams, docs_path, state_path};
use crate::device::describe_keys;
use crate::hexutil::decode_hex32;
use crate::rest::{DeviceEntry, PairingClaimResponse, PairingTicket, RestClient};
use anyhow::{Context, Result, anyhow, bail};
use clap::{Args, Parser, Subcommand};
use commucat_crypto::DeviceKeyPair;
use std::fs;
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
    Claim(ClaimArgs),
    Export,
    Docs(DocsArgs),
    Tui,
}

#[derive(Subcommand)]
enum DevicesCommand {
    List(DevicesListArgs),
    Revoke(DevicesRevokeArgs),
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
struct InitArgs {
    #[arg(long)]
    server: String,
    #[arg(long)]
    domain: String,
    #[arg(long)]
    username: Option<String>,
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
    let path = state_path()?;
    if path.exists() && !force {
        bail!("профиль уже существует: {}", path.display());
    }
    if let Some(code) = pair_code {
        let rest = RestClient::new(&server)?;
        let claim = rest.claim_pairing(&code, device_name.as_deref()).await?;
        let private = decode_hex32(&claim.private_key)?;
        let public = decode_hex32(&claim.public_key)?;
        let keys = DeviceKeyPair { public, private };
        let state = ClientState::from_params(ClientStateParams {
            device_id: claim.device_id.clone(),
            server_url: server.clone(),
            domain,
            keys,
            pattern,
            prologue,
            tls_ca_path: tls_ca,
            server_static,
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
        });
        state.save()?;
        println!("state saved to {}", path.display());
        println!(
            "{}",
            describe_keys(&claim.device_id, &state.device_keypair()?)
        );
        print_claim_summary(&claim);
        return Ok(());
    }

    let username = username.ok_or_else(|| anyhow!("--username обязателен без --pair-code"))?;
    let generated_device = device_id.unwrap_or_else(|| device::generate_device_id("device"));
    let keys = device::generate_keypair()?;
    let state = ClientState::from_params(ClientStateParams {
        device_id: generated_device.clone(),
        server_url: server,
        domain,
        keys: keys.clone(),
        pattern,
        prologue,
        tls_ca_path: tls_ca,
        server_static,
        insecure,
        presence_state: presence,
        presence_interval_secs: presence_interval,
        traceparent,
        user_handle: Some(username.clone()),
        user_display_name: display_name.clone(),
        user_avatar_url: avatar_url.clone(),
        user_id: None,
        session_token: session.clone(),
        device_name,
    });
    state.save()?;
    println!("state saved to {}", path.display());
    println!("{}", describe_keys(&generated_device, &keys));
    println!(
        "Устройство зарегистрируется автоматически при первом подключении как пользователь '{}'.",
        username
    );
    if let Some(name) = display_name {
        println!("display_name={} (отправляется на сервер)", name);
    }
    if let Some(url) = avatar_url {
        println!("avatar_url={} (будет применён при handshake)", url);
    }
    if let Some(token) = session {
        println!("session={} (будет использована REST API)", token);
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
    }
    Ok(())
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
