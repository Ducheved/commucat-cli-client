use crate::config::{ClientState, FriendEntry};
use crate::engine::{ClientEvent, EngineCommand, EngineHandle, create_engine};
use crate::hexutil::{encode_hex, short_hex};
use crate::rest::{
    DeviceEntry, FriendEntryPayload, P2pAssistRequest, P2pAssistResponse, PairingTicket,
    RestClient, ServerInfo,
};
use anyhow::{Context, Result, anyhow, bail};
use chrono::{DateTime, Utc};
use commucat_proto::{ControlEnvelope, Frame as ProtoFrame, FramePayload, FrameType};
use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use futures::StreamExt;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::{Frame as UiFrame, Terminal};
use std::collections::VecDeque;
use std::io::{Stdout, stdout};
use std::time::Duration;
use tokio::sync::mpsc::Receiver;

const ENGINE_COMMAND_BUFFER: usize = 128;
const ENGINE_EVENT_BUFFER: usize = 256;
const MESSAGE_HISTORY_LIMIT: usize = 200;
const DEFAULT_PAIR_TTL: i64 = 300;

#[derive(Clone, Copy, PartialEq, Eq)]
enum AppView {
    Chat,
    Devices,
    Friends,
    Pairing,
    Info,
    Assist,
}

const MENU_ITEMS: &[(AppView, &str)] = &[
    (AppView::Chat, "Чат"),
    (AppView::Devices, "Устройства"),
    (AppView::Friends, "Друзья"),
    (AppView::Pairing, "Pairing"),
    (AppView::Info, "Сервер"),
    (AppView::Assist, "P2P Assist"),
];

fn default_assist_request() -> P2pAssistRequest {
    P2pAssistRequest {
        prefer_reality: Some(true),
        min_paths: Some(2),
        ..P2pAssistRequest::default()
    }
}

pub async fn run_tui(state: ClientState) -> Result<()> {
    let (engine, events) = create_engine(ENGINE_COMMAND_BUFFER, ENGINE_EVENT_BUFFER);
    let mut app = App::new(state, engine, events);
    let mut terminal = prepare_terminal()?;
    let mut input_stream = EventStream::new();
    let mut ticker = tokio::time::interval(Duration::from_millis(160));
    loop {
        terminal.draw(|frame| app.render(frame))?;
        set_cursor(&mut terminal, app.input_rect, &app.input)?;
        tokio::select! {
            Some(event) = app.events.recv() => {
                app.handle_client_event(event).await?;
            }
            Some(Ok(event)) = input_stream.next() => {
                if let Event::Key(key) = event {
                    app.handle_key(key).await?;
                }
            }
            _ = ticker.tick() => app.on_tick(),
        }
        if app.should_quit {
            break;
        }
    }
    restore_terminal(terminal)?;
    Ok(())
}

struct App {
    state: ClientState,
    engine: EngineHandle,
    events: Receiver<ClientEvent>,
    should_quit: bool,
    connected: bool,
    session_id: Option<String>,
    input: String,
    input_rect: Option<Rect>,
    channels: Vec<ChannelView>,
    active_channel: usize,
    pending_disconnect: bool,
    last_error: Option<String>,
    view: AppView,
    menu_state: ListState,
    devices: Vec<DeviceEntry>,
    devices_state: ListState,
    friends_state: ListState,
    pairing_ticket: Option<PairingTicket>,
    server_info: Option<ServerInfo>,
    assist_report: Option<P2pAssistResponse>,
    assist_request: P2pAssistRequest,
}

impl App {
    fn new(state: ClientState, engine: EngineHandle, events: Receiver<ClientEvent>) -> Self {
        let channels = vec![ChannelView::system()];
        let mut menu_state = ListState::default();
        menu_state.select(Some(0));
        App {
            state,
            engine,
            events,
            should_quit: false,
            connected: false,
            session_id: None,
            input: String::new(),
            input_rect: None,
            channels,
            active_channel: 0,
            pending_disconnect: false,
            last_error: None,
            view: AppView::Chat,
            menu_state,
            devices: Vec::new(),
            devices_state: ListState::default(),
            friends_state: ListState::default(),
            pairing_ticket: None,
            server_info: None,
            assist_report: None,
            assist_request: default_assist_request(),
        }
    }

    async fn handle_key(&mut self, key: KeyEvent) -> Result<()> {
        match key.kind {
            KeyEventKind::Press | KeyEventKind::Repeat => {}
            KeyEventKind::Release => return Ok(()),
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.should_quit = true;
            return Ok(());
        }
        if let KeyCode::F(n) = key.code {
            match n {
                1 => self.set_view(AppView::Chat),
                2 => self.set_view(AppView::Devices),
                3 => self.set_view(AppView::Friends),
                4 => self.set_view(AppView::Pairing),
                5 => self.set_view(AppView::Info),
                6 => self.set_view(AppView::Assist),
                10 => self.should_quit = true,
                _ => {}
            }
            return Ok(());
        }
        match key.code {
            KeyCode::BackTab => {
                self.cycle_view_backward();
            }
            KeyCode::Tab => {
                if self.view == AppView::Chat {
                    self.select_next_channel();
                } else {
                    self.cycle_view_forward();
                }
            }
            KeyCode::Left => {
                self.cycle_view_backward();
            }
            KeyCode::Right => {
                self.cycle_view_forward();
            }
            KeyCode::Up => {
                if self.view == AppView::Chat {
                    self.select_previous_channel();
                } else {
                    self.navigate_view_list(-1);
                }
            }
            KeyCode::Down => {
                if self.view == AppView::Chat {
                    self.select_next_channel();
                } else {
                    self.navigate_view_list(1);
                }
            }
            KeyCode::Char(':') if self.input.is_empty() => {
                self.input.push(':');
            }
            KeyCode::Char(ch) => {
                if key.modifiers.is_empty()
                    && self.input.is_empty()
                    && self.handle_hotkey(ch).await?
                {
                    return Ok(());
                }
                if !matches!(key.modifiers, KeyModifiers::CONTROL | KeyModifiers::ALT) {
                    self.input.push(ch);
                }
            }
            KeyCode::Backspace => {
                self.input.pop();
            }
            KeyCode::Enter => {
                self.process_enter().await?;
            }
            KeyCode::Esc => {
                self.input.clear();
            }
            KeyCode::F(10) => {
                self.should_quit = true;
            }
            _ => {}
        }
        Ok(())
    }

    fn set_view(&mut self, view: AppView) {
        self.view = view;
        if let Some(index) = MENU_ITEMS.iter().position(|(item, _)| *item == view) {
            self.menu_state.select(Some(index));
        }
        match view {
            AppView::Devices => self.update_devices_state(),
            AppView::Friends => self.sync_friend_state(),
            _ => {}
        }
    }

    fn cycle_view_forward(&mut self) {
        let current = self.menu_state.selected().unwrap_or(0);
        let next = (current + 1) % MENU_ITEMS.len();
        self.set_view(MENU_ITEMS[next].0);
    }

    fn cycle_view_backward(&mut self) {
        let current = self.menu_state.selected().unwrap_or(0);
        let next = if current == 0 {
            MENU_ITEMS.len() - 1
        } else {
            current - 1
        };
        self.set_view(MENU_ITEMS[next].0);
    }

    async fn handle_client_event(&mut self, event: ClientEvent) -> Result<()> {
        match event {
            ClientEvent::Connected {
                session_id,
                pairing_required,
            } => {
                self.connected = true;
                self.session_id = Some(session_id.clone());
                self.pending_disconnect = false;
                self.record_system(format!("сессия {} установлена", session_id));
                self.state.session_token = Some(session_id.clone());
                if let Err(err) = self.state.save() {
                    self.record_system(format!("не удалось сохранить профиль: {}", err));
                }
                if pairing_required {
                    self.record_system(
                        "Лимит auto-approve исчерпан: используйте :pair и init --pair-code на новом устройстве".to_string(),
                    );
                }
            }
            ClientEvent::Disconnected { reason } => {
                self.connected = false;
                self.session_id = None;
                self.record_system(format!("соединение завершено: {}", reason));
                if !self.pending_disconnect {
                    let _ = self.engine.send(EngineCommand::Disconnect).await;
                }
                self.pending_disconnect = false;
            }
            ClientEvent::Error { detail } => {
                self.last_error = Some(detail.clone());
                self.record_system(format!("ошибка: {}", detail));
            }
            ClientEvent::Frame(frame) => self.consume_frame(frame),
            ClientEvent::Log { line } => self.record_system(line),
        }
        Ok(())
    }

    fn consume_frame(&mut self, frame: ProtoFrame) {
        match frame.frame_type {
            FrameType::Msg => {
                if let FramePayload::Opaque(body) = frame.payload {
                    let text = match String::from_utf8(body) {
                        Ok(value) => value,
                        Err(err) => format!("0x{}", encode_hex(&err.into_bytes())),
                    };
                    self.record_message(frame.channel_id, MessageDirection::Inbound, text);
                }
            }
            #[allow(clippy::collapsible_if)]
            FrameType::Ack => {
                if let FramePayload::Control(ControlEnvelope { properties }) = &frame.payload {
                    if let Some(value) = properties.get("ack") {
                        self.record_system(format!(
                            "ACK {} для канала {}",
                            value, frame.channel_id
                        ));
                    }
                }
            }
            FrameType::Presence => {
                if let FramePayload::Control(ControlEnvelope { properties }) = frame.payload {
                    let peer = properties
                        .get("peer")
                        .and_then(|v| v.as_str())
                        .unwrap_or("неизвестно");
                    let state = properties
                        .get("state")
                        .and_then(|v| v.as_str())
                        .unwrap_or("online");
                    self.record_system(format!("presence {} → {}", peer, state));
                }
            }
            FrameType::Error => {
                if let FramePayload::Control(ControlEnvelope { properties }) = frame.payload {
                    let detail = properties
                        .get("detail")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown error");
                    self.record_system(format!("ошибка сервера: {}", detail));
                }
            }
            #[allow(clippy::collapsible_if)]
            FrameType::Join => {
                if let FramePayload::Control(ControlEnvelope { properties }) = &frame.payload {
                    if let Some(array) = properties.get("members").and_then(|v| v.as_array()) {
                        let members = array
                            .iter()
                            .filter_map(|v| v.as_str())
                            .map(ToString::to_string)
                            .collect::<Vec<_>>();
                        self.ensure_channel(frame.channel_id)
                            .set_members(members.clone());
                        self.record_system(format!(
                            "channel {} members: {}",
                            frame.channel_id,
                            members.join(", ")
                        ));
                    }
                }
            }
            FrameType::Leave => {
                self.record_system(format!("канал {} закрыл членство", frame.channel_id));
                if let Some(channel) = self.channels.iter_mut().find(|c| c.id == frame.channel_id) {
                    channel.members.clear();
                }
            }
            FrameType::GroupEvent | FrameType::GroupInvite | FrameType::GroupCreate => {
                self.record_system(format!("group event {}", frame.channel_id));
            }
            other @ (FrameType::CallOffer
            | FrameType::CallAnswer
            | FrameType::CallEnd
            | FrameType::CallStats
            | FrameType::VoiceFrame
            | FrameType::VideoFrame) => {
                self.record_system(format!(
                    "получен {:?} для канала {}",
                    other, frame.channel_id
                ));
            }
            FrameType::Typing => {}
            FrameType::Hello | FrameType::Auth => {}
            FrameType::KeyUpdate => {}
        }
    }

    async fn execute_command(&mut self, command: &str) -> Result<()> {
        let mut parts = command.split_whitespace();
        let name = parts.next().unwrap_or("").to_lowercase();
        match name.as_str() {
            "connect" => self.connect().await?,
            "disconnect" => self.disconnect().await?,
            "join" => {
                let channel = parts
                    .next()
                    .unwrap_or("0")
                    .parse::<u64>()
                    .context("channel id")?;
                let members_raw = parts.next().unwrap_or("");
                let members = parse_members(members_raw, &self.state.device_id);
                self.join(channel, members, false).await?;
            }
            "relay" => {
                let channel = parts
                    .next()
                    .unwrap_or("0")
                    .parse::<u64>()
                    .context("channel id")?;
                let members_raw = parts.next().unwrap_or("");
                let members = parse_members(members_raw, &self.state.device_id);
                self.join(channel, members, true).await?;
            }
            "leave" => {
                let channel = parts
                    .next()
                    .unwrap_or("0")
                    .parse::<u64>()
                    .context("channel id")?;
                self.leave(channel).await?;
            }
            "channel" => {
                let channel = parts
                    .next()
                    .unwrap_or("0")
                    .parse::<u64>()
                    .context("channel id")?;
                self.activate_channel(channel);
            }
            "presence" => {
                let state_value = parts.collect::<Vec<_>>().join(" ");
                if !state_value.is_empty() {
                    self.state.presence_state = state_value.clone();
                    self.engine
                        .send(EngineCommand::Presence { state: state_value })
                        .await?;
                }
            }
            "pair" => {
                let ttl = parts
                    .next()
                    .map(|v| v.parse::<i64>().context("ttl должно быть целым"))
                    .transpose()?;
                self.issue_pair_command(ttl).await?;
            }
            "devices" => match parts.next().unwrap_or("list").to_lowercase().as_str() {
                "list" => self.devices_list_command().await?,
                "revoke" => {
                    let target = parts.next().ok_or_else(|| anyhow!("укажите device_id"))?;
                    self.devices_revoke_command(target).await?;
                }
                other => self.record_system(format!("неизвестная подкоманда devices: {}", other)),
            },
            "friends" => {
                let sub = parts.next().unwrap_or("list").to_lowercase();
                match sub.as_str() {
                    "list" => self.friends_list_command(),
                    "add" => {
                        let user_id = parts
                            .next()
                            .ok_or_else(|| anyhow!("укажите user_id"))?
                            .to_string();
                        let alias = parts.next().map(|s| s.to_string());
                        self.friends_add_command(user_id, alias).await?;
                    }
                    "remove" => {
                        let user_id = parts
                            .next()
                            .ok_or_else(|| anyhow!("укажите user_id"))?
                            .to_string();
                        self.friends_remove_command(user_id).await?;
                    }
                    "push" => self.friends_push_command().await?,
                    "pull" => self.friends_pull_command().await?,
                    other => {
                        self.record_system(format!("неизвестная подкоманда friends: {}", other))
                    }
                }
            }
            "revoke" => {
                let target = parts.next().ok_or_else(|| anyhow!("укажите device_id"))?;
                self.devices_revoke_command(target).await?;
            }
            "export" => {
                let summary = crate::device::describe_keys(
                    &self.state.device_id,
                    &self.state.device_keypair()?,
                );
                self.record_system(summary);
            }
            "help" => self.show_help(),
            "clear" => self.clear_messages(),
            "quit" | "exit" => self.should_quit = true,
            other => self.record_system(format!("неизвестная команда: {}", other)),
        }
        Ok(())
    }

    async fn connect(&mut self) -> Result<()> {
        if self.connected {
            self.record_system("уже подключено".to_string());
            return Ok(());
        }
        self.record_system("подключение...".to_string());
        self.engine
            .send(EngineCommand::Connect(Box::new(self.state.clone())))
            .await?;
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<()> {
        if !self.connected {
            self.record_system("соединение и так закрыто".to_string());
        }
        self.pending_disconnect = true;
        self.engine.send(EngineCommand::Disconnect).await?;
        Ok(())
    }

    async fn join(&mut self, channel_id: u64, mut members: Vec<String>, relay: bool) -> Result<()> {
        if !self.connected {
            self.record_system("нет соединения".to_string());
            return Ok(());
        }
        if !members.iter().any(|m| m == &self.state.device_id) {
            members.push(self.state.device_id.clone());
        }
        self.engine
            .send(EngineCommand::Join {
                channel_id,
                members: members.clone(),
                relay,
            })
            .await?;
        let view = self.ensure_channel(channel_id);
        view.set_members(members.clone());
        self.active_channel = self
            .channels
            .iter()
            .position(|c| c.id == channel_id)
            .unwrap_or(0);
        self.record_system(format!("join {} (relay={})", channel_id, relay));
        Ok(())
    }

    async fn leave(&mut self, channel_id: u64) -> Result<()> {
        if !self.connected {
            self.record_system("нет соединения".to_string());
            return Ok(());
        }
        self.engine
            .send(EngineCommand::Leave { channel_id })
            .await?;
        self.record_system(format!("leave {}", channel_id));
        Ok(())
    }

    async fn send_text(&mut self, text: String) -> Result<()> {
        if !self.connected {
            self.record_system("нет соединения".to_string());
            return Ok(());
        }
        if self.active_channel == 0 {
            self.record_system("выберите канал перед отправкой".to_string());
            return Ok(());
        }
        let channel_id = self.channels[self.active_channel].id;
        self.engine
            .send(EngineCommand::SendMessage {
                channel_id,
                body: text.as_bytes().to_vec(),
            })
            .await?;
        self.record_message(channel_id, MessageDirection::Outbound, text);
        Ok(())
    }

    async fn issue_pair_command(&mut self, ttl: Option<i64>) -> Result<()> {
        let client = self.rest_client()?;
        let session = self.resolve_session_token()?;
        let ticket = client.create_pairing(&session, ttl).await?;
        self.pairing_ticket = Some(ticket.clone());
        self.state.last_pairing_code = Some(ticket.pair_code.clone());
        self.state.last_pairing_expires_at = Some(ticket.expires_at.clone());
        self.state.last_pairing_issuer_device_id = ticket.issuer_device_id.clone();
        self.state.session_token = Some(session);
        if let Err(err) = self.state.save() {
            self.record_system(format!("не удалось сохранить профиль: {}", err));
        }
        self.record_pairing_ticket(&ticket);
        Ok(())
    }

    async fn devices_list_command(&mut self) -> Result<()> {
        let client = self.rest_client()?;
        let session = self.resolve_session_token()?;
        let devices = client.list_devices(&session).await?;
        if devices.is_empty() {
            self.record_system("Нет зарегистрированных устройств.".to_string());
        } else {
            for device in devices {
                self.record_device_entry(&device);
            }
        }
        Ok(())
    }

    async fn devices_revoke_command(&mut self, device_id: &str) -> Result<()> {
        let client = self.rest_client()?;
        let session = self.resolve_session_token()?;
        client.revoke_device(&session, device_id).await?;
        self.record_system(format!("Устройство {} помечено как revoked", device_id));
        Ok(())
    }

    fn friends_list_command(&mut self) {
        if self.state.friends().is_empty() {
            self.record_system("Список друзей пуст.".to_string());
        } else {
            let entries = self.state.friends().to_vec();
            for entry in entries {
                let handle = entry
                    .alias
                    .as_ref()
                    .or(entry.handle.as_ref())
                    .map(|s| format!(" ({})", s))
                    .unwrap_or_default();
                self.record_system(format!("{}{}", entry.user_id, handle));
            }
        }
    }

    async fn friends_add_command(&mut self, user_id: String, alias: Option<String>) -> Result<()> {
        let entry = FriendEntry {
            user_id: user_id.clone(),
            handle: None,
            alias,
        };
        self.state.upsert_friend(entry);
        if let Err(err) = self.state.save() {
            self.record_system(format!("не удалось сохранить профиль: {}", err));
        }
        self.sync_friend_state();
        self.record_system(format!("Добавлен друг {}", user_id));
        Ok(())
    }

    async fn friends_remove_command(&mut self, user_id: String) -> Result<()> {
        if self.state.remove_friend(&user_id) {
            if let Err(err) = self.state.save() {
                self.record_system(format!("не удалось сохранить профиль: {}", err));
            }
            self.sync_friend_state();
            self.record_system(format!("Удалён друг {}", user_id));
        } else {
            self.record_system(format!("Друг {} не найден", user_id));
        }
        Ok(())
    }

    async fn friends_push_command(&mut self) -> Result<()> {
        let client = self.rest_client()?;
        let session = self.resolve_session_token()?;
        client
            .update_friends(&session, &tui_friends_to_payload(self.state.friends()))
            .await?;
        self.record_system("Список друзей синхронизирован.".to_string());
        Ok(())
    }

    async fn friends_pull_command(&mut self) -> Result<()> {
        let client = self.rest_client()?;
        let session = self.resolve_session_token()?;
        let remote = client.list_friends(&session).await?;
        let entries = remote.into_iter().map(tui_friend_from_payload).collect();
        self.state.set_friends(entries);
        if let Err(err) = self.state.save() {
            self.record_system(format!("не удалось сохранить профиль: {}", err));
        }
        self.sync_friend_state();
        self.record_system(format!("Загружено друзей: {}", self.state.friends().len()));
        Ok(())
    }

    fn rest_client(&self) -> Result<RestClient> {
        RestClient::new(&self.state.server_url)
    }

    fn navigate_view_list(&mut self, delta: isize) {
        match self.view {
            AppView::Chat | AppView::Pairing | AppView::Info | AppView::Assist => {}
            AppView::Devices => self.move_device_selection(delta),
            AppView::Friends => self.move_friend_selection(delta),
        }
    }

    fn move_device_selection(&mut self, delta: isize) {
        if self.devices.is_empty() {
            self.devices_state.select(None);
            return;
        }
        let len = self.devices.len();
        let current = self.devices_state.selected().unwrap_or(0);
        let mut next = current as isize + delta;
        if next < 0 {
            next = 0;
        }
        if next >= len as isize {
            next = (len - 1) as isize;
        }
        self.devices_state.select(Some(next as usize));
    }

    fn move_friend_selection(&mut self, delta: isize) {
        let friends = self.state.friends();
        if friends.is_empty() {
            self.friends_state.select(None);
            return;
        }
        let len = friends.len();
        let current = self.friends_state.selected().unwrap_or(0);
        let mut next = current as isize + delta;
        if next < 0 {
            next = 0;
        }
        if next >= len as isize {
            next = (len - 1) as isize;
        }
        self.friends_state.select(Some(next as usize));
    }

    fn update_devices_state(&mut self) {
        if self.devices.is_empty() {
            self.devices_state.select(None);
        } else {
            let len = self.devices.len();
            let index = self.devices_state.selected().unwrap_or(0).min(len - 1);
            self.devices_state.select(Some(index));
        }
    }

    fn sync_friend_state(&mut self) {
        let friends = self.state.friends();
        if friends.is_empty() {
            self.friends_state.select(None);
        } else {
            let index = self
                .friends_state
                .selected()
                .unwrap_or(0)
                .min(friends.len() - 1);
            self.friends_state.select(Some(index));
        }
    }

    async fn handle_hotkey(&mut self, ch: char) -> Result<bool> {
        match (self.view, ch) {
            (AppView::Devices, 'r') => {
                self.refresh_devices().await?;
                Ok(true)
            }
            (AppView::Devices, 'v') => {
                self.devices_revoke_selected().await?;
                Ok(true)
            }
            (AppView::Devices, 'i') => {
                self.inspect_selected_device();
                Ok(true)
            }
            (AppView::Friends, 'r') => {
                self.friends_pull_command().await?;
                Ok(true)
            }
            (AppView::Friends, 'p') => {
                self.friends_push_command().await?;
                Ok(true)
            }
            (AppView::Friends, 'd') => {
                self.remove_selected_friend().await?;
                Ok(true)
            }
            (AppView::Pairing, 'g') => {
                self.issue_pair_command(Some(DEFAULT_PAIR_TTL)).await?;
                Ok(true)
            }
            (AppView::Info, 'r') => {
                self.refresh_server_info().await?;
                Ok(true)
            }
            (AppView::Assist, 'r') => {
                self.refresh_assist().await?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    async fn process_enter(&mut self) -> Result<()> {
        let command = self.input.trim().to_string();
        self.input.clear();
        if let Some(stripped) = command.strip_prefix(':') {
            self.execute_command(stripped).await?;
            return Ok(());
        }
        if command.is_empty() {
            self.handle_enter_on_view().await?;
            return Ok(());
        }
        if self.view == AppView::Chat {
            self.send_text(command).await?;
        } else {
            self.record_system(
                "Текстовый ввод доступен только в режиме чата (начните команду с ':')".to_string(),
            );
        }
        Ok(())
    }

    async fn handle_enter_on_view(&mut self) -> Result<()> {
        match self.view {
            AppView::Devices => {
                self.inspect_selected_device();
            }
            AppView::Friends => {
                self.inspect_selected_friend();
            }
            AppView::Pairing => {
                self.show_pairing_snapshot();
            }
            AppView::Info => {
                if self.server_info.is_none() {
                    self.refresh_server_info().await?;
                }
            }
            AppView::Assist => {
                if self.assist_report.is_none() {
                    self.refresh_assist().await?;
                }
            }
            AppView::Chat => {}
        }
        Ok(())
    }

    async fn refresh_devices(&mut self) -> Result<()> {
        let client = self.rest_client()?;
        let session = self.resolve_session_token()?;
        let devices = client.list_devices(&session).await?;
        let count = devices.len();
        self.devices = devices;
        self.update_devices_state();
        self.record_system(format!("Получено устройств: {}", count));
        Ok(())
    }

    async fn devices_revoke_selected(&mut self) -> Result<()> {
        let Some(device) = self.selected_device().cloned() else {
            self.record_system("Выберите устройство для отзыва (стрелки вверх/вниз)".to_string());
            return Ok(());
        };
        let client = self.rest_client()?;
        let session = self.resolve_session_token()?;
        client.revoke_device(&session, &device.device_id).await?;
        self.record_system(format!(
            "Устройство {} отмечено как revoked",
            device.device_id
        ));
        self.refresh_devices().await
    }

    async fn remove_selected_friend(&mut self) -> Result<()> {
        let Some(friend) = self.selected_friend().cloned() else {
            self.record_system("Список друзей пуст".to_string());
            return Ok(());
        };
        self.friends_remove_command(friend.user_id).await?;
        self.sync_friend_state();
        Ok(())
    }

    async fn refresh_server_info(&mut self) -> Result<()> {
        let client = self.rest_client()?;
        let info = client.server_info().await?;
        self.server_info = Some(info.clone());
        self.record_system(format!("Информация сервера обновлена для {}", info.domain));
        Ok(())
    }

    async fn refresh_assist(&mut self) -> Result<()> {
        let client = self.rest_client()?;
        let session = self.resolve_session_token()?;
        let report = client.p2p_assist(&session, &self.assist_request).await?;
        self.assist_report = Some(report);
        self.record_system("Получены рекомендации P2P assist".to_string());
        Ok(())
    }

    fn inspect_selected_device(&mut self) {
        if let Some(device) = self.selected_device().cloned() {
            self.record_system(format!(
                "{}: status={}, created={}, current={}",
                device.device_id, device.status, device.created_at, device.current
            ));
            self.record_system(format!("pubkey: {}", device.public_key));
        } else {
            self.record_system("Нет устройств для отображения".to_string());
        }
    }

    fn inspect_selected_friend(&mut self) {
        if let Some(friend) = self.selected_friend().cloned() {
            let alias = friend
                .alias
                .as_deref()
                .or(friend.handle.as_deref())
                .unwrap_or("-");
            self.record_system(format!("{} (alias: {})", friend.user_id, alias));
        } else {
            self.record_system("Нет друзей для отображения".to_string());
        }
    }

    fn show_pairing_snapshot(&mut self) {
        if let Some(ticket) = self.pairing_ticket.clone() {
            self.record_pairing_ticket(&ticket);
            return;
        }
        match (
            &self.state.last_pairing_code,
            &self.state.last_pairing_expires_at,
        ) {
            (Some(code), Some(expiry)) => {
                self.record_system(format!("Последний код: {} (истекает {})", code, expiry));
            }
            _ => self.record_system("Pairing кодов ещё не создавалось".to_string()),
        }
    }

    fn selected_device(&self) -> Option<&DeviceEntry> {
        self.devices_state
            .selected()
            .and_then(|index| self.devices.get(index))
    }

    fn selected_friend(&self) -> Option<&FriendEntry> {
        self.friends_state
            .selected()
            .and_then(|index| self.state.friends().get(index))
    }

    fn resolve_session_token(&self) -> Result<String> {
        if let Some(active) = self.session_id.as_ref() {
            return Ok(active.clone());
        }
        if let Some(token) = self.state.session_token.as_ref() {
            return Ok(token.clone());
        }
        bail!("нет активной сессии: выполните :connect или сохраните session через init");
    }

    fn record_pairing_ticket(&mut self, ticket: &PairingTicket) {
        self.record_system(format!("Pair code: {}", ticket.pair_code));
        if let Some(issuer) = ticket.issuer_device_id.as_ref() {
            self.record_system(format!("Выдано устройством: {}", issuer));
        }
        self.record_system(format!("Действителен до: {}", ticket.expires_at));
        self.record_system(format!("Seed: {}", ticket.device_seed));
    }

    fn record_device_entry(&mut self, entry: &DeviceEntry) {
        let current = if entry.current {
            " (текущее)"
        } else {
            ""
        };
        self.record_system(format!(
            "{}\t{}\t{}{}",
            entry.device_id, entry.status, entry.created_at, current
        ));
    }

    fn record_system(&mut self, text: String) {
        self.record_message(0, MessageDirection::System, text);
    }

    fn record_message(&mut self, channel_id: u64, direction: MessageDirection, content: String) {
        let channel = self.ensure_channel(channel_id);
        channel.push(MessageEntry {
            timestamp: Utc::now(),
            direction,
            content,
        });
        if channel.messages.len() > MESSAGE_HISTORY_LIMIT {
            channel.messages.pop_front();
        }
    }

    fn ensure_channel(&mut self, channel_id: u64) -> &mut ChannelView {
        if let Some(index) = self.channels.iter().position(|c| c.id == channel_id) {
            return &mut self.channels[index];
        }
        self.channels.push(ChannelView::new(channel_id));
        let index = self.channels.len() - 1;
        &mut self.channels[index]
    }

    fn select_next_channel(&mut self) {
        if self.channels.is_empty() {
            return;
        }
        self.active_channel = (self.active_channel + 1) % self.channels.len();
    }

    fn select_previous_channel(&mut self) {
        if self.channels.is_empty() {
            return;
        }
        if self.active_channel == 0 {
            self.active_channel = self.channels.len() - 1;
        } else {
            self.active_channel -= 1;
        }
    }

    fn activate_channel(&mut self, channel_id: u64) {
        if let Some(index) = self.channels.iter().position(|c| c.id == channel_id) {
            self.active_channel = index;
        } else {
            self.record_system(format!("канал {} не найден", channel_id));
        }
    }

    fn show_help(&mut self) {
        self.record_system("Команды: :connect, :disconnect, :join <id> <members>, :relay <id> <members>, :leave <id>, :channel <id>, :presence <state>, :pair [ttl], :devices [list|revoke <id>], :friends [list|add <id> [alias]|remove <id>|pull|push], :export, :clear, :quit".to_string());
    }

    fn clear_messages(&mut self) {
        for channel in self.channels.iter_mut() {
            channel.messages.clear();
        }
    }

    fn on_tick(&mut self) {}

    fn render(&mut self, frame: &mut UiFrame) {
        let area = frame.size();
        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(5),
                Constraint::Length(3),
            ])
            .split(area);
        self.render_status(frame, layout[0]);
        self.render_body(frame, layout[1]);
        self.render_input(frame, layout[2]);
        self.input_rect = Some(layout[2]);
    }

    fn render_status(&self, frame: &mut UiFrame, area: Rect) {
        let status = build_status_line(
            &self.state,
            self.connected,
            self.session_id.as_deref(),
            self.last_error.as_deref(),
        );
        let paragraph = Paragraph::new(status)
            .style(Style::default().fg(Color::Cyan))
            .wrap(Wrap { trim: true });
        frame.render_widget(paragraph, area);
    }

    fn render_body(&mut self, frame: &mut UiFrame, area: Rect) {
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(20), Constraint::Min(10)])
            .split(area);
        self.render_menu(frame, chunks[0]);
        self.render_view(frame, chunks[1]);
    }

    fn render_menu(&mut self, frame: &mut UiFrame, area: Rect) {
        let items = MENU_ITEMS
            .iter()
            .map(|(_, label)| ListItem::new(*label))
            .collect::<Vec<_>>();
        let block = Block::default()
            .title("Разделы")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Blue));
        let list = List::new(items)
            .block(block)
            .highlight_style(
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("› ");
        frame.render_stateful_widget(list, area, &mut self.menu_state);
    }

    fn render_view(&mut self, frame: &mut UiFrame, area: Rect) {
        match self.view {
            AppView::Chat => self.render_chat(frame, area),
            AppView::Devices => self.render_devices(frame, area),
            AppView::Friends => self.render_friends(frame, area),
            AppView::Pairing => self.render_pairing(frame, area),
            AppView::Info => self.render_info(frame, area),
            AppView::Assist => self.render_assist(frame, area),
        }
    }

    fn render_chat(&mut self, frame: &mut UiFrame, area: Rect) {
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(28), Constraint::Min(10)])
            .split(area);
        self.render_channels(frame, chunks[0]);
        self.render_messages(frame, chunks[1]);
    }

    fn render_channels(&self, frame: &mut UiFrame, area: Rect) {
        let items = self
            .channels
            .iter()
            .map(|channel| {
                let title = if channel.id == 0 {
                    "system".to_string()
                } else {
                    format!("ch#{}", channel.id)
                };
                ListItem::new(title)
            })
            .collect::<Vec<_>>();
        let block = Block::default()
            .title("Каналы")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Blue));
        let list = List::new(items)
            .block(block)
            .highlight_style(
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("› ");
        let mut state = ListState::default();
        state.select(Some(self.active_channel));
        frame.render_stateful_widget(list, area, &mut state);
    }

    fn render_messages(&self, frame: &mut UiFrame, area: Rect) {
        let messages = &self.channels[self.active_channel].messages;
        let rows = messages
            .iter()
            .rev()
            .take((area.height as usize).saturating_sub(2))
            .collect::<Vec<_>>();
        let mut lines = Vec::new();
        for entry in rows.into_iter().rev() {
            let prefix = match entry.direction {
                MessageDirection::Inbound => Span::styled("<", Style::default().fg(Color::Green)),
                MessageDirection::Outbound => {
                    Span::styled(">", Style::default().fg(Color::Magenta))
                }
                MessageDirection::System => Span::styled("*", Style::default().fg(Color::Gray)),
            };
            let time = entry.timestamp.format("%H:%M:%S").to_string();
            let header = Span::styled(time, Style::default().fg(Color::DarkGray));
            let content = Span::styled(entry.content.clone(), Style::default().fg(Color::White));
            lines.push(Line::from(vec![
                prefix,
                Span::raw(" "),
                header,
                Span::raw(" "),
                content,
            ]));
        }
        let block = Block::default()
            .title("Сообщения")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Blue));
        let paragraph = Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false });
        frame.render_widget(paragraph, area);
    }

    fn render_devices(&mut self, frame: &mut UiFrame, area: Rect) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(6), Constraint::Length(6)])
            .split(area);
        let items = if self.devices.is_empty() {
            vec![ListItem::new(
                "Нажмите r, чтобы загрузить устройства".to_string(),
            )]
        } else {
            self.devices
                .iter()
                .map(|device| {
                    let label = format!(
                        "{} [{}]{}",
                        device.device_id,
                        device.status,
                        if device.current { " *" } else { "" }
                    );
                    ListItem::new(label)
                })
                .collect::<Vec<_>>()
        };
        let block = Block::default()
            .title("Устройства (r – обновить, v – отозвать, Enter/i – детали)")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Blue));
        let list = List::new(items)
            .block(block)
            .highlight_style(
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("› ");
        frame.render_stateful_widget(list, chunks[0], &mut self.devices_state);

        let mut lines = Vec::new();
        if let Some(device) = self.selected_device() {
            lines.push(Line::from(format!("ID: {}", device.device_id)));
            lines.push(Line::from(format!("Статус: {}", device.status)));
            lines.push(Line::from(format!("Создано: {}", device.created_at)));
            lines.push(Line::from(if device.current {
                "Текущее устройство: да"
            } else {
                "Текущее устройство: нет"
            }));
            lines.push(Line::from(format!(
                "Публичный ключ: {}",
                short_hex(&device.public_key)
            )));
        } else {
            lines.push(Line::from("Нет устройств. Нажмите r для синхронизации."));
        }
        let detail = Paragraph::new(lines)
            .block(
                Block::default()
                    .title("Детали")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Blue)),
            )
            .wrap(Wrap { trim: true });
        frame.render_widget(detail, chunks[1]);
    }

    fn render_friends(&mut self, frame: &mut UiFrame, area: Rect) {
        let friends = self.state.friends();
        let items = if friends.is_empty() {
            vec![ListItem::new(
                "Используйте :friends add <id> или нажмите r для загрузки".to_string(),
            )]
        } else {
            friends
                .iter()
                .map(|friend| {
                    let alias = friend
                        .alias
                        .as_deref()
                        .or(friend.handle.as_deref())
                        .unwrap_or("-");
                    ListItem::new(format!("{} ({})", friend.user_id, alias))
                })
                .collect::<Vec<_>>()
        };
        let block = Block::default()
            .title("Друзья (r – загрузить, p – отправить, d – удалить, Enter – детали)")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Blue));
        let list = List::new(items)
            .block(block)
            .highlight_style(
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("› ");
        frame.render_stateful_widget(list, area, &mut self.friends_state);
    }

    fn render_pairing(&mut self, frame: &mut UiFrame, area: Rect) {
        let mut lines = Vec::new();
        if let Some(ticket) = self.pairing_ticket.as_ref() {
            lines.push(Line::from(format!("Код: {}", ticket.pair_code)));
            lines.push(Line::from(format!(
                "Действителен до: {}",
                ticket.expires_at
            )));
            if let Some(issuer) = ticket.issuer_device_id.as_deref() {
                lines.push(Line::from(format!("Выдан: {}", issuer)));
            }
            lines.push(Line::from(format!("Seed: {}", ticket.device_seed)));
        } else {
            match (
                &self.state.last_pairing_code,
                &self.state.last_pairing_expires_at,
            ) {
                (Some(code), Some(expiry)) => {
                    lines.push(Line::from(format!("Последний код: {}", code)));
                    lines.push(Line::from(format!("Истекает: {}", expiry)));
                }
                _ => lines.push(Line::from("Код ещё не выпускался.")),
            }
        }
        lines.push(Line::from(
            "g – создать код (TTL 300s) · Enter – показать текущий",
        ));
        let paragraph = Paragraph::new(lines)
            .block(
                Block::default()
                    .title("Pairing")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Blue)),
            )
            .wrap(Wrap { trim: true });
        frame.render_widget(paragraph, area);
    }

    fn render_info(&mut self, frame: &mut UiFrame, area: Rect) {
        let mut lines = Vec::new();
        if let Some(info) = self.server_info.as_ref() {
            lines.push(Line::from(format!("Домен: {}", info.domain)));
            lines.push(Line::from(format!(
                "Noise static: {}",
                short_hex(&info.noise_public)
            )));
            if let Some(ca) = info.device_ca_public.as_deref() {
                lines.push(Line::from(format!("Device CA: {}", short_hex(ca))));
            }
            if !info.supported_patterns.is_empty() {
                lines.push(Line::from(format!(
                    "Noise patterns: {}",
                    info.supported_patterns.join(", ")
                )));
            }
            if !info.supported_versions.is_empty() {
                lines.push(Line::from(format!(
                    "Protocol versions: {:?}",
                    info.supported_versions
                )));
            }
            if let Some(pairing) = info.pairing.as_ref() {
                lines.push(Line::from(format!(
                    "Auto-approve: {}, max auto devices: {}",
                    if pairing.auto_approve {
                        "да"
                    } else {
                        "нет"
                    },
                    pairing.max_auto_devices
                )));
                lines.push(Line::from(format!("Pairing TTL: {}s", pairing.pairing_ttl)));
            }
            lines.push(Line::from("r – обновить сведения"));
        } else {
            lines.push(Line::from("Нажмите r, чтобы запросить /api/server-info"));
        }
        let paragraph = Paragraph::new(lines)
            .block(
                Block::default()
                    .title("Сервер")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Blue)),
            )
            .wrap(Wrap { trim: true });
        frame.render_widget(paragraph, area);
    }

    fn render_assist(&mut self, frame: &mut UiFrame, area: Rect) {
        let mut lines = Vec::new();
        if let Some(report) = self.assist_report.as_ref() {
            lines.push(Line::from(format!(
                "Noise pattern: {} static {}",
                report.noise.pattern,
                short_hex(&report.noise.static_public_hex)
            )));
            lines.push(Line::from(format!(
                "Noise prologue: {} seed {}",
                short_hex(&report.noise.prologue_hex),
                short_hex(&report.noise.device_seed_hex)
            )));
            lines.push(Line::from(format!(
                "PQ identity: {}",
                short_hex(&report.pq.identity_public_hex)
            )));
            lines.push(Line::from(format!(
                "PQ keys: signed {} kem {} sig {}",
                short_hex(&report.pq.signed_prekey_public_hex),
                short_hex(&report.pq.kem_public_hex),
                short_hex(&report.pq.signature_public_hex)
            )));
            if !report.transports.is_empty() {
                lines.push(Line::from("Транспорты:"));
                for advice in report.transports.iter().take(4) {
                    let label = format!(
                        "• {} → {} (устойчивость {}, латентность {}, пропуск {} )",
                        advice.path_id,
                        advice.transport,
                        advice.resistance,
                        advice.latency,
                        advice.throughput
                    );
                    lines.push(Line::from(label));
                }
            }
            lines.push(Line::from(format!(
                "FEC mtu {} overhead {:.2}",
                report.multipath.fec_mtu, report.multipath.fec_overhead
            )));
            if let Some(primary) = report.multipath.primary_path.as_deref() {
                lines.push(Line::from(format!("Основной путь: {}", primary)));
            }
            if !report.multipath.sample_segments.is_empty() {
                lines.push(Line::from("Сегменты выборки:"));
                for (path, breakdown) in report.multipath.sample_segments.iter().take(3) {
                    lines.push(Line::from(format!(
                        "• {} total={} repair={}",
                        path, breakdown.total, breakdown.repair
                    )));
                }
                if report.multipath.sample_segments.len() > 3 {
                    lines.push(Line::from(format!(
                        "… ещё {} сегментов",
                        report.multipath.sample_segments.len() - 3
                    )));
                }
            }
            if report.obfuscation.domain_fronting
                || report.obfuscation.protocol_mimicry
                || report.obfuscation.tor_bridge
            {
                let mut flags = Vec::new();
                if report.obfuscation.domain_fronting {
                    flags.push("domain-fronting");
                }
                if report.obfuscation.protocol_mimicry {
                    flags.push("protocol-mimicry");
                }
                if report.obfuscation.tor_bridge {
                    flags.push("tor-bridge");
                }
                lines.push(Line::from(format!("Obfuscation: {}", flags.join(", "))));
            }
            if let Some(fingerprint) = report.obfuscation.reality_fingerprint_hex.as_deref() {
                lines.push(Line::from(format!(
                    "Reality fingerprint: {}",
                    short_hex(fingerprint)
                )));
            }
            lines.push(Line::from(format!(
                "Security: noise={}, pq={}, fec={}, multipath={} (avg {:.1}), deflections={}",
                report.security.noise_handshakes,
                report.security.pq_handshakes,
                report.security.fec_packets,
                report.security.multipath_sessions,
                report.security.average_paths,
                report.security.censorship_deflections
            )));
            lines.push(Line::from("r – обновить P2P assist"));
        } else {
            lines.push(Line::from(
                "Нажмите r, чтобы запросить рекомендации /api/p2p/assist",
            ));
        }
        let paragraph = Paragraph::new(lines)
            .block(
                Block::default()
                    .title("P2P Assist")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Blue)),
            )
            .wrap(Wrap { trim: true });
        frame.render_widget(paragraph, area);
    }

    fn view_help(&self) -> String {
        match self.view {
            AppView::Devices => {
                "Устройства: r — обновить · v — revoke · i/Enter — детали".to_string()
            }
            AppView::Chat => "Чат: Enter — отправить · ':' — команда · Tab — канал".to_string(),
            AppView::Friends => "Друзья: r — pull · p — push · d/Enter — детали".to_string(),
            AppView::Pairing => "Pairing: g — новый код · Enter — показать последний".to_string(),
            AppView::Info => "Сервер: r/Enter — обновить информацию".to_string(),
            AppView::Assist => "P2P Assist: r/Enter — запросить рекомендации".to_string(),
        }
    }

    fn input_title(&self) -> String {
        let view_label = match self.view {
            AppView::Chat => "Чат",
            AppView::Devices => "Устройства",
            AppView::Friends => "Друзья",
            AppView::Pairing => "Pairing",
            AppView::Info => "Сервер",
            AppView::Assist => "P2P Assist",
        };
        let state = if self.connected { "online" } else { "offline" };
        format!("Ввод · {} · {}", view_label, state)
    }

    fn render_input(&self, frame: &mut UiFrame, area: Rect) {
        let help_line = Line::from(vec![Span::raw(self.view_help())]);
        let input_line = Line::from(format!("> {}", self.input));
        let text = Text::from(vec![help_line, input_line]);
        let block = Block::default()
            .title(self.input_title())
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Blue));
        let paragraph = Paragraph::new(text)
            .block(block)
            .style(Style::default().fg(Color::White));
        frame.render_widget(paragraph, area);
    }
}

#[derive(Clone)]
struct ChannelView {
    id: u64,
    members: Vec<String>,
    messages: VecDeque<MessageEntry>,
}

impl ChannelView {
    fn system() -> Self {
        ChannelView {
            id: 0,
            members: Vec::new(),
            messages: VecDeque::new(),
        }
    }

    fn new(id: u64) -> Self {
        ChannelView {
            id,
            members: Vec::new(),
            messages: VecDeque::new(),
        }
    }

    fn push(&mut self, entry: MessageEntry) {
        self.messages.push_back(entry);
    }

    fn set_members(&mut self, members: Vec<String>) {
        self.members = members;
    }
}

#[derive(Clone)]
struct MessageEntry {
    timestamp: DateTime<Utc>,
    direction: MessageDirection,
    content: String,
}

#[derive(Clone, Copy)]
enum MessageDirection {
    Inbound,
    Outbound,
    System,
}

fn parse_members(raw: &str, self_id: &str) -> Vec<String> {
    let mut members = raw
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    if members.is_empty() {
        members.push(self_id.to_string());
    }
    members
}

fn build_status_line(
    state: &ClientState,
    connected: bool,
    session_id: Option<&str>,
    error: Option<&str>,
) -> Line<'static> {
    let mut spans = Vec::new();
    spans.push(Span::styled(
        format!(" host:{} ", state.server_url),
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    ));
    spans.push(Span::raw(" "));
    spans.push(Span::styled(
        format!(" device:{} ", state.device_id),
        Style::default().fg(Color::Green),
    ));
    spans.push(Span::raw(" "));
    let status = if connected { "connected" } else { "offline" };
    let status_color = if connected { Color::Green } else { Color::Red };
    spans.push(Span::styled(
        format!(" status:{} ", status),
        Style::default().fg(status_color),
    ));
    if let Some(session) = session_id {
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            format!(" session:{} ", session),
            Style::default().fg(Color::Yellow),
        ));
    }
    spans.push(Span::raw(" "));
    spans.push(Span::styled(
        format!(" presence:{} ", state.presence_state),
        Style::default().fg(Color::Magenta),
    ));
    if let Some(err) = error {
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            format!(" error:{} ", err),
            Style::default().fg(Color::Red),
        ));
    }
    Line::from(spans)
}

fn prepare_terminal() -> Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode().context("enable raw mode")?;
    let mut stdout = stdout();
    execute!(stdout, EnterAlternateScreen).context("enter screen")?;
    let backend = CrosstermBackend::new(stdout);
    let terminal = Terminal::new(backend).context("create terminal")?;
    Ok(terminal)
}

fn restore_terminal(mut terminal: Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    disable_raw_mode().context("disable raw mode")?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen).context("leave screen")?;
    terminal.show_cursor().ok();
    Ok(())
}

fn set_cursor(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    area: Option<Rect>,
    input: &str,
) -> Result<()> {
    if let Some(rect) = area {
        let x = rect.x + 2 + input.len() as u16;
        let y = rect.y + 1;
        terminal.set_cursor(x, y).context("set cursor")?;
    }
    Ok(())
}

fn tui_friend_from_payload(payload: FriendEntryPayload) -> FriendEntry {
    FriendEntry {
        user_id: payload.user_id,
        handle: payload.handle,
        alias: payload.alias,
    }
}

fn tui_friends_to_payload(entries: &[FriendEntry]) -> Vec<FriendEntryPayload> {
    entries
        .iter()
        .map(|entry| FriendEntryPayload {
            user_id: entry.user_id.clone(),
            handle: entry.handle.clone(),
            alias: entry.alias.clone(),
        })
        .collect()
}
