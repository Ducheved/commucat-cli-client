use crate::config::ClientState;
use crate::engine::{ClientEvent, EngineCommand, EngineHandle, create_engine};
use crate::hexutil::encode_hex;
use crate::rest::{DeviceEntry, PairingTicket, RestClient};
use anyhow::{Context, Result, anyhow, bail};
use chrono::{DateTime, Utc};
use commucat_proto::{ControlEnvelope, Frame as ProtoFrame, FramePayload, FrameType};
use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use futures::StreamExt;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::{Frame as UiFrame, Terminal};
use std::collections::VecDeque;
use std::io::{Stdout, stdout};
use std::time::Duration;
use tokio::sync::mpsc::Receiver;

const ENGINE_COMMAND_BUFFER: usize = 128;
const ENGINE_EVENT_BUFFER: usize = 256;
const MESSAGE_HISTORY_LIMIT: usize = 200;

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
}

impl App {
    fn new(state: ClientState, engine: EngineHandle, events: Receiver<ClientEvent>) -> Self {
        let channels = vec![ChannelView::system()];
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
        }
    }

    async fn handle_key(&mut self, key: KeyEvent) -> Result<()> {
        if matches!(key.modifiers, KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.should_quit = true;
            return Ok(());
        }
        match key.code {
            KeyCode::Char(':') if self.input.is_empty() => {
                self.input.push(':');
            }
            KeyCode::Char(ch) => {
                if !matches!(key.modifiers, KeyModifiers::CONTROL | KeyModifiers::ALT) {
                    self.input.push(ch);
                }
            }
            KeyCode::Backspace => {
                self.input.pop();
            }
            KeyCode::Enter => {
                let command = self.input.trim().to_string();
                self.input.clear();
                if let Some(stripped) = command.strip_prefix(':') {
                    self.execute_command(stripped).await?;
                } else if !command.is_empty() {
                    self.send_text(command).await?;
                }
            }
            KeyCode::Esc => {
                self.input.clear();
            }
            KeyCode::Tab => self.select_next_channel(),
            KeyCode::Up => self.select_previous_channel(),
            KeyCode::F(10) => {
                self.should_quit = true;
            }
            _ => {}
        }
        Ok(())
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

    fn rest_client(&self) -> Result<RestClient> {
        RestClient::new(&self.state.server_url)
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
        self.record_system("Команды: :connect, :disconnect, :join <id> <members>, :relay <id> <members>, :leave <id>, :channel <id>, :presence <state>, :pair [ttl], :devices [list|revoke <id>], :export, :clear, :quit".to_string());
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

    fn render_body(&self, frame: &mut UiFrame, area: Rect) {
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

    fn render_input(&self, frame: &mut UiFrame, area: Rect) {
        let block = Block::default()
            .title("Ввод")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Blue));
        let paragraph = Paragraph::new(self.input.clone())
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
