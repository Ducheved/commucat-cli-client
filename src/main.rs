mod config;
mod device;
mod engine;
mod hexutil;
mod tui;

use crate::config::{ClientState, ClientStateParams, docs_path, state_path};
use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
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
    Export,
    Docs(DocsArgs),
    Tui,
}

#[derive(Args)]
struct InitArgs {
    #[arg(long)]
    server: String,
    #[arg(long)]
    domain: String,
    #[arg(long)]
    username: String,
    #[arg(long)]
    display_name: Option<String>,
    #[arg(long)]
    avatar_url: Option<String>,
    #[arg(long)]
    device_id: Option<String>,
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
    #[arg(long, default_value_t = false)]
    force: bool,
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
        Some(Command::Init(args)) => init_profile(args)?,
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

fn init_profile(args: InitArgs) -> Result<()> {
    let InitArgs {
        server,
        domain,
        username,
        display_name,
        avatar_url,
        device_id,
        pattern,
        prologue,
        tls_ca,
        server_static,
        insecure,
        presence,
        presence_interval,
        traceparent,
        force,
    } = args;
    let path = state_path()?;
    if path.exists() && !force {
        bail!("профиль уже существует: {}", path.display());
    }
    let device_id = device_id.unwrap_or_else(|| device::generate_device_id("device"));
    let keys = device::generate_keypair()?;
    let state = ClientState::from_params(ClientStateParams {
        device_id: device_id.clone(),
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
    });
    state.save()?;
    println!("state saved to {}", path.display());
    println!("{}", device::describe_keys(&device_id, &keys));
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
    if let Ok(doc_path) = docs_path("ru") {
        println!("Руководство: {}", doc_path.display());
    }
    Ok(())
}

fn export_profile() -> Result<()> {
    let state = ClientState::load()?;
    let keys = state.device_keypair()?;
    println!("{}", device::describe_keys(&state.device_id, &keys));
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
