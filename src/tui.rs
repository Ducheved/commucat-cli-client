use crate::animations::{
    Animation, create_loading_animation, create_neko_walk, create_pulse_animation,
    create_wave_animation,
};
use crate::ascii_art;
use crate::calls::{CallAnswer, CallEnd, CallManager, CallOffer, CallStats};
use crate::config::ClientState;
use crate::engine::{ClientEvent, EngineCommand, EngineHandle, create_engine};
use crate::groups::{Group, GroupAction, GroupRole};
use crate::hexutil::short_hex;
use crate::media::{AudioMetrics, MediaManager, VideoMetrics};
use crate::rest::{
    AssistFecHint, AssistPathHint, DeviceEntry, P2pAssistRequest, P2pAssistResponse, RestClient,
};
use crate::voice::{VoiceMessage, visualize_audio_wave};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use commucat_proto::{ControlEnvelope, Frame as ProtoFrame, FramePayload, FrameType};
use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use futures::StreamExt;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Borders, Clear, Gauge, List, ListItem, ListState, Paragraph, Sparkline,
    Tabs, Wrap,
};
use ratatui::{Frame as UiFrame, Terminal};
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::io::{Stdout, stdout};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::Receiver;
use uuid::Uuid;

const ENGINE_COMMAND_BUFFER: usize = 256;
const ENGINE_EVENT_BUFFER: usize = 512;
const MESSAGE_HISTORY_LIMIT: usize = 500;
const ANIMATION_FPS: u64 = 60;

// Enhanced kawaii emoticons and stickers
const KAWAII_REACTIONS: &[(&str, &str, &str)] = &[
    ("happy", "(◕‿◕)", "✨"),
    ("love", "(♡ω♡)", "💕"),
    ("sad", "(╥﹏╥)", "💧"),
    ("angry", "(╬ಠ益ಠ)", "💢"),
    ("surprised", "(°o°)", "‼️"),
    ("sleepy", "(－ω－)", "💤"),
    ("confused", "(・_・?)", "❓"),
    ("excited", "＼(≧▽≦)／", "🎉"),
    ("singing", "♪(´▽｀)", "🎵"),
    ("eating", "(っ˘ڡ˘ς)", "🍴"),
    ("working", "(・ω・)b", "💻"),
    ("thinking", "(｡･ω･｡)", "💭"),
];

// Enhanced view states
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum AppView {
    Splash,
    Chat,
    Groups,
    Calls,
    Voice,
    Devices,
    Friends,
    Settings,
}

pub struct EnhancedApp {
    // Core state
    state: ClientState,
    engine: EngineHandle,
    events: Receiver<ClientEvent>,
    should_quit: bool,

    // Connection state
    connected: bool,
    session_id: Option<String>,

    // UI state
    view: AppView,
    input: String,
    input_rect: Option<Rect>,
    last_error: Option<String>,
    notifications: VecDeque<Notification>,

    // Animations
    loading_animation: Animation,
    pulse_animation: Animation,
    neko_animation: Animation,
    wave_animation: Animation,
    frame_counter: u64,
    last_frame: Instant,
    transition_progress: f32,

    // Chat state
    channels: Vec<ChannelView>,
    active_channel: usize,
    message_scroll: usize,

    // Groups state
    groups: HashMap<String, Group>,
    groups_state: ListState,

    // Calls state
    call_manager: CallManager,
    active_call: Option<String>,
    call_quality_history: VecDeque<f32>,
    call_audio_metrics: Option<AudioMetrics>,
    call_video_metrics: Option<VideoMetrics>,

    // Voice state
    voice_recording: bool,
    voice_amplitude: f32,
    voice_buffer: Vec<u8>,

    // Menu state
    menu_items: Vec<MenuItem>,

    // Settings
    theme: Theme,
    animations_enabled: bool,
    sound_enabled: bool,
    emoji_mode: bool,

    // Presence and directory
    presence: HashMap<String, PresenceInfo>,
    devices: Vec<DeviceEntry>,

    // Media pipeline
    media: MediaManager,
    call_channels: HashMap<u64, String>,

    // REST integration
    rest_client: Option<RestClient>,
}

#[derive(Clone)]
struct MenuItem {
    view: AppView,
    label: String,
    icon: String,
    hotkey: Option<char>,
}

#[derive(Clone)]
struct Notification {
    message: String,
    level: NotificationLevel,
    timestamp: DateTime<Utc>,
    expires_at: DateTime<Utc>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum NotificationLevel {
    Info,
    Success,
    Warning,
    Error,
}

#[derive(Clone, Copy, Debug)]
enum Theme {
    Dark,
    Light,
    Cyberpunk,
    Kawaii,
}

#[derive(Clone)]
struct ChannelView {
    id: u64,
    name: String,
    members: Vec<String>,
    messages: VecDeque<MessageEntry>,
    typing: HashMap<String, TypingIndicator>,
    unread_count: usize,
    is_group: bool,
    group_id: Option<String>,
}

#[derive(Clone)]
struct MessageEntry {
    timestamp: DateTime<Utc>,
    sender: String,
    content: MessageContent,
    reactions: HashMap<String, Vec<String>>,
}

#[derive(Clone)]
enum MessageContent {
    Text(String),
    Voice(VoiceMessage),
    System(String),
    Call(CallInfo),
    GroupEvent(String),
}

#[derive(Clone)]
struct CallInfo {
    call_id: String,
    action: String,
    duration: Option<Duration>,
}

#[derive(Clone)]
struct TypingIndicator {
    label: String,
    expires_at: DateTime<Utc>,
    animation_frame: usize,
}

#[derive(Clone)]
struct PresenceInfo {
    state: String,
    expires_at: Option<DateTime<Utc>>,
    handle: Option<String>,
    display_name: Option<String>,
    avatar_url: Option<String>,
    user_id: Option<String>,
    updated_at: DateTime<Utc>,
}

impl PresenceInfo {
    fn is_active(&self) -> bool {
        self.expires_at.map(|ts| ts > Utc::now()).unwrap_or(true)
    }
}

impl EnhancedApp {
    pub fn new(state: ClientState, engine: EngineHandle, events: Receiver<ClientEvent>) -> Self {
        let menu_items = vec![
            MenuItem {
                view: AppView::Chat,
                label: "Чат".to_string(),
                icon: "💬".to_string(),
                hotkey: Some('1'),
            },
            MenuItem {
                view: AppView::Groups,
                label: "Группы".to_string(),
                icon: "👥".to_string(),
                hotkey: Some('2'),
            },
            MenuItem {
                view: AppView::Calls,
                label: "Звонки".to_string(),
                icon: "📞".to_string(),
                hotkey: Some('3'),
            },
            MenuItem {
                view: AppView::Voice,
                label: "Голос".to_string(),
                icon: "🎤".to_string(),
                hotkey: Some('4'),
            },
            MenuItem {
                view: AppView::Friends,
                label: "Друзья".to_string(),
                icon: "👫".to_string(),
                hotkey: Some('5'),
            },
            MenuItem {
                view: AppView::Devices,
                label: "Устройства".to_string(),
                icon: "📱".to_string(),
                hotkey: Some('6'),
            },
            MenuItem {
                view: AppView::Settings,
                label: "Настройки".to_string(),
                icon: "⚙️".to_string(),
                hotkey: Some('9'),
            },
        ];

        let channels = vec![ChannelView::system()];
        let rest_client = match RestClient::new(&state.server_url) {
            Ok(client) => Some(client),
            Err(err) => {
                eprintln!("REST client init failed: {err}");
                None
            }
        };

        EnhancedApp {
            state,
            engine,
            events,
            should_quit: false,
            connected: false,
            session_id: None,
            view: AppView::Splash,
            input: String::new(),
            input_rect: None,
            last_error: None,
            notifications: VecDeque::new(),
            loading_animation: create_loading_animation(),
            pulse_animation: create_pulse_animation(),
            neko_animation: create_neko_walk(),
            wave_animation: create_wave_animation(),
            frame_counter: 0,
            last_frame: Instant::now(),
            transition_progress: 0.0,
            channels,
            active_channel: 0,
            message_scroll: 0,
            groups: HashMap::new(),
            groups_state: ListState::default(),
            call_manager: CallManager::new(),
            active_call: None,
            call_quality_history: VecDeque::new(),
            call_audio_metrics: None,
            call_video_metrics: None,
            voice_recording: false,
            voice_amplitude: 0.0,
            voice_buffer: Vec::new(),
            menu_items,
            theme: Theme::Cyberpunk,
            animations_enabled: true,
            sound_enabled: true,
            emoji_mode: true,
            presence: HashMap::new(),
            devices: Vec::new(),
            media: MediaManager::new(),
            call_channels: HashMap::new(),
            rest_client,
        }
    }

    pub async fn run(&mut self) -> Result<()> {
        let mut terminal = prepare_terminal()?;
        let mut input_stream = EventStream::new();
        let mut ticker = tokio::time::interval(Duration::from_millis(1000 / ANIMATION_FPS));

        // Show splash screen
        self.show_splash_animation(&mut terminal).await?;
        self.view = AppView::Chat;

        // Auto-connect
        self.connect().await?;

        loop {
            // Update animations
            let now = Instant::now();
            let delta = now.duration_since(self.last_frame);
            self.last_frame = now;
            self.update_animations(delta);

            // Render frame
            terminal.draw(|frame| self.render(frame))?;
            set_cursor(&mut terminal, self.input_rect, &self.input)?;

            // Handle events
            tokio::select! {
                Some(event) = self.events.recv() => {
                    self.handle_client_event(event).await?;
                }
                Some(Ok(event)) = input_stream.next() => {
                    if let Event::Key(key) = event {
                        self.handle_key(key).await?;
                    }
                }
                _ = ticker.tick() => {
                    self.frame_counter += 1;
                    self.cleanup_expired_notifications();
                }
            }

            if self.should_quit {
                break;
            }
        }

        restore_terminal(terminal)?;
        Ok(())
    }

    async fn show_splash_animation(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    ) -> Result<()> {
        let splash_duration = Duration::from_secs(2);
        let start = Instant::now();

        while start.elapsed() < splash_duration && !self.should_quit {
            terminal.draw(|frame| self.render_splash(frame))?;
            tokio::time::sleep(Duration::from_millis(50)).await;
            self.transition_progress =
                start.elapsed().as_millis() as f32 / splash_duration.as_millis() as f32;
        }

        Ok(())
    }

    fn render_splash(&mut self, frame: &mut UiFrame) {
        let area = frame.size();

        // Clear background
        frame.render_widget(Clear, area);

        // Calculate center area
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Percentage(30),
                Constraint::Length(15),
                Constraint::Percentage(30),
            ])
            .split(area);

        let cat = Paragraph::new(ascii_art::CAT_HAPPY)
            .style(Style::default().fg(Color::LightMagenta))
            .alignment(Alignment::Center);
        frame.render_widget(cat, chunks[0]);

        // Render ASCII art logo with fade-in effect
        let alpha = (self.transition_progress * 255.0) as u8;
        let color = Color::Rgb(0, alpha, alpha);

        let logo = Paragraph::new(ascii_art::LOGO)
            .style(Style::default().fg(color))
            .alignment(Alignment::Center);
        frame.render_widget(logo, chunks[1]);

        // Animated loading bar
        let progress = (self.transition_progress * 100.0) as u16;
        let gauge = Gauge::default()
            .block(Block::default().borders(Borders::NONE))
            .gauge_style(Style::default().fg(Color::Cyan).bg(Color::Black))
            .percent(progress)
            .label(format!("Loading CommuCat... {}%", progress));

        let gauge_area = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1)])
            .margin(2)
            .split(chunks[2])[0];

        frame.render_widget(gauge, gauge_area);

        // Animated neko
        let neko = self.neko_animation.tick(Duration::from_millis(50));
        let neko_text = Paragraph::new(neko)
            .style(Style::default().fg(Color::Magenta))
            .alignment(Alignment::Center);

        let neko_area = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(3)])
            .split(chunks[2])[0];

        frame.render_widget(neko_text, neko_area);
    }

    fn update_animations(&mut self, delta: Duration) {
        if !self.animations_enabled {
            return;
        }

        self.loading_animation.tick(delta);
        self.pulse_animation.tick(delta);
        self.neko_animation.tick(delta);
        self.wave_animation.tick(delta);

        // Update transition animation
        if self.transition_progress < 1.0 {
            self.transition_progress += delta.as_millis() as f32 / 300.0; // 300ms transition
            if self.transition_progress > 1.0 {
                self.transition_progress = 1.0;
            }
        }

        // Update typing indicators
        let now = Utc::now();
        for channel in &mut self.channels {
            for indicator in channel.typing.values_mut() {
                indicator.animation_frame = (indicator.animation_frame + 1) % 10;
            }
            channel
                .typing
                .retain(|_, indicator| indicator.expires_at > now);
        }

        // Simulate voice amplitude changes when recording
        if self.voice_recording {
            self.voice_amplitude = ((self.frame_counter as f32 * 0.1).sin() + 1.0) * 0.5;
        }
    }

    fn render(&mut self, frame: &mut UiFrame) {
        match self.view {
            AppView::Splash => self.render_splash(frame),
            _ => self.render_main(frame),
        }
    }

    fn render_main(&mut self, frame: &mut UiFrame) {
        let area = frame.size();

        // Main layout with animated borders
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3), // Header
                Constraint::Min(10),   // Content
                Constraint::Length(4), // Input
                Constraint::Length(1), // Status bar
            ])
            .split(area);

        self.render_header(frame, chunks[0]);
        self.render_content(frame, chunks[1]);
        self.render_input(frame, chunks[2]);
        self.render_status_bar(frame, chunks[3]);
        self.render_notifications(frame, area);

        self.input_rect = Some(chunks[2]);
    }

    fn render_header(&mut self, frame: &mut UiFrame, area: Rect) {
        let header_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(20),
                Constraint::Min(10),
                Constraint::Length(30),
            ])
            .split(area);

        // Logo and title
        let title = format!(" {} CommuCat ", self.get_view_icon());
        let title_block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(self.get_theme_border_style());
        frame.render_widget(title_block, header_chunks[0]);

        // Navigation tabs
        let titles = self
            .menu_items
            .iter()
            .map(|item| format!("{} {}", item.icon, item.label))
            .collect::<Vec<_>>();

        let selected = self
            .menu_items
            .iter()
            .position(|item| item.view == self.view)
            .unwrap_or(0);

        let tabs = Tabs::new(titles)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded),
            )
            .select(selected)
            .style(Style::default().fg(Color::Gray))
            .highlight_style(
                Style::default()
                    .fg(self.get_theme_primary_color())
                    .add_modifier(Modifier::BOLD),
            );
        frame.render_widget(tabs, header_chunks[1]);

        // Connection status with animation
        let status_text = if self.connected {
            format!("{} Online", self.pulse_animation.tick(Duration::ZERO))
        } else {
            format!("{} Offline", self.loading_animation.tick(Duration::ZERO))
        };

        let status = Paragraph::new(status_text)
            .style(Style::default().fg(if self.connected {
                Color::Green
            } else {
                Color::Red
            }))
            .alignment(Alignment::Center)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded),
            );
        frame.render_widget(status, header_chunks[2]);
    }

    fn render_content(&mut self, frame: &mut UiFrame, area: Rect) {
        match self.view {
            AppView::Chat => self.render_chat(frame, area),
            AppView::Groups => self.render_groups(frame, area),
            AppView::Calls => self.render_calls(frame, area),
            AppView::Voice => self.render_voice(frame, area),
            AppView::Friends => self.render_friends(frame, area),
            AppView::Devices => self.render_devices(frame, area),
            AppView::Settings => self.render_settings(frame, area),
            _ => {}
        }
    }

    fn render_chat(&mut self, frame: &mut UiFrame, area: Rect) {
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(30),
                Constraint::Min(40),
                Constraint::Length(25),
            ])
            .split(area);

        // Channel list
        self.render_channel_list(frame, chunks[0]);

        // Messages
        self.render_messages(frame, chunks[1]);

        // Channel info / members
        self.render_channel_info(frame, chunks[2]);
    }

    fn render_channel_list(&mut self, frame: &mut UiFrame, area: Rect) {
        let items: Vec<ListItem> = self
            .channels
            .iter()
            .enumerate()
            .map(|(i, channel)| {
                let icon = if channel.is_group { "👥" } else { "💬" };
                let unread = if channel.unread_count > 0 {
                    format!(" ({})", channel.unread_count)
                } else {
                    String::new()
                };

                let style = if i == self.active_channel {
                    Style::default()
                        .fg(self.get_theme_primary_color())
                        .add_modifier(Modifier::BOLD)
                } else if channel.unread_count > 0 {
                    Style::default().fg(Color::Yellow)
                } else {
                    Style::default()
                };

                ListItem::new(format!("{} {}{}", icon, channel.name, unread)).style(style)
            })
            .collect();

        let list = List::new(items)
            .block(
                Block::default()
                    .title(" Каналы ")
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded),
            )
            .highlight_style(
                Style::default()
                    .bg(self.get_theme_secondary_color())
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("▶ ");

        let mut state = ListState::default();
        state.select(Some(self.active_channel));
        frame.render_stateful_widget(list, area, &mut state);
    }

    fn render_messages(&mut self, frame: &mut UiFrame, area: Rect) {
        let channel = &self.channels[self.active_channel];

        // Split for messages and typing indicator
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(5), Constraint::Length(2)])
            .split(area);

        // Messages
        let mut lines = Vec::new();
        for entry in channel.messages.iter().rev().take(50) {
            let timestamp = entry.timestamp.format("%H:%M").to_string();

            let (prefix, content) = match &entry.content {
                MessageContent::Text(text) => {
                    let sender = self.get_friend_display_name(&entry.sender);
                    (format!("[{}] {}", timestamp, sender), text.clone())
                }
                MessageContent::Voice(voice) => {
                    let sender = self.get_friend_display_name(&entry.sender);
                    let duration = format!("{}s", voice.duration_ms / 1000);
                    (
                        format!("[{}] {} 🎤", timestamp, sender),
                        format!("Voice message ({})", duration),
                    )
                }
                MessageContent::System(text) => (format!("[{}] System", timestamp), text.clone()),
                MessageContent::Call(info) => {
                    let id = self.short_id(&info.call_id);
                    let details = if let Some(duration) = info.duration {
                        format!("Call {} {} ({}s)", id, info.action, duration.as_secs())
                    } else {
                        format!("Call {} {}", id, info.action)
                    };
                    (format!("[{}] 📞", timestamp), details)
                }
                MessageContent::GroupEvent(event) => (format!("[{}] 👥", timestamp), event.clone()),
            };

            // Add message with styling
            let mut spans = vec![
                Span::styled(prefix, Style::default().fg(Color::DarkGray)),
                Span::raw(": "),
                Span::raw(content),
            ];

            // Add reactions
            if !entry.reactions.is_empty() {
                let reactions_text = entry
                    .reactions
                    .iter()
                    .map(|(emoji, users)| format!("{}{}", emoji, users.len()))
                    .collect::<Vec<_>>()
                    .join(" ");
                spans.push(Span::raw(" "));
                spans.push(Span::styled(
                    reactions_text,
                    Style::default().fg(Color::Yellow),
                ));
            }

            lines.push(Line::from(spans));
        }

        let messages = Paragraph::new(lines)
            .block(
                Block::default()
                    .title(format!(" {} ", channel.name))
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded),
            )
            .wrap(Wrap { trim: true })
            .scroll((self.message_scroll as u16, 0));

        frame.render_widget(messages, chunks[0]);

        // Typing indicator
        if !channel.typing.is_empty() {
            let typing_text = channel
                .typing
                .values()
                .map(|ind| {
                    let anim = ["⣾", "⣽", "⣻", "⢿", "⡿", "⣟", "⣯", "⣷"];
                    format!(
                        "{} {} is typing{}",
                        ind.label,
                        anim[ind.animation_frame % anim.len()],
                        ".".repeat((ind.animation_frame % 3) + 1)
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");

            let typing = Paragraph::new(typing_text)
                .style(Style::default().fg(Color::Gray).italic())
                .block(Block::default().borders(Borders::TOP));

            frame.render_widget(typing, chunks[1]);
        }
    }

    fn render_channel_info(&self, frame: &mut UiFrame, area: Rect) {
        let channel = &self.channels[self.active_channel];

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(10), // Channel details
                Constraint::Min(5),     // Members list
                Constraint::Length(8),  // Quick actions
            ])
            .split(area);

        // Channel details
        let mut details = vec![
            Line::from(format!("📍 {}", channel.name)),
            Line::from(format!("👥 {} members", channel.members.len())),
        ];

        if let Some(group) = channel.group_id.as_ref().and_then(|id| self.groups.get(id)) {
            details.push(Line::from(format!("👑 Owner: {}", group.owner)));
            details.push(Line::from(format!(
                "📅 Created: {}",
                DateTime::<Utc>::from_timestamp(group.created_at, 0)
                    .map(|ts| ts.format("%Y-%m-%d").to_string())
                    .unwrap_or_else(|| "unknown".to_string())
            )));
        }

        let info = Paragraph::new(details).block(
            Block::default()
                .title(" Info ")
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded),
        );
        frame.render_widget(info, chunks[0]);

        // Members
        let members: Vec<ListItem> = channel
            .members
            .iter()
            .map(|member| {
                let display = self.get_friend_display_name(member);
                let online = self.is_online(member);
                let status_icon = if online { "🟢" } else { "⚫" };
                ListItem::new(format!("{} {}", status_icon, display))
            })
            .collect();

        let members_list = List::new(members).block(
            Block::default()
                .title(" Members ")
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded),
        );
        frame.render_widget(members_list, chunks[1]);

        // Quick actions
        let actions = vec![
            Line::from("📞 F3 - Voice call"),
            Line::from("🎥 F4 - Video call"),
            Line::from("📎 F5 - Send file"),
            Line::from("🎤 F6 - Voice message"),
            Line::from("➕ F7 - Add member"),
            Line::from("⚙️ F8 - Settings"),
        ];

        let actions_widget = Paragraph::new(actions).block(
            Block::default()
                .title(" Actions ")
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded),
        );
        frame.render_widget(actions_widget, chunks[2]);
    }

    fn render_groups(&mut self, frame: &mut UiFrame, area: Rect) {
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(35), Constraint::Min(40)])
            .split(area);

        // Groups list
        let groups: Vec<ListItem> = self
            .groups
            .values()
            .map(|group| {
                let member_count = group.members.len();
                let role_icon = group
                    .members
                    .get(&self.state.device_id)
                    .map(|role| match role {
                        GroupRole::Owner => "👑",
                        GroupRole::Admin => "⭐",
                        GroupRole::Member => "👤",
                    })
                    .unwrap_or("❓");

                ListItem::new(format!(
                    "{} {} ({} members)",
                    role_icon, group.name, member_count
                ))
            })
            .collect();

        let list = List::new(groups)
            .block(
                Block::default()
                    .title(" Группы ")
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded),
            )
            .highlight_style(
                Style::default()
                    .bg(self.get_theme_secondary_color())
                    .add_modifier(Modifier::BOLD),
            );

        frame.render_stateful_widget(list, chunks[0], &mut self.groups_state);

        // Group details
        if let Some(selected) = self.groups_state.selected() {
            if let Some(group) = self.groups.values().nth(selected) {
                self.render_group_details(frame, chunks[1], group);
            }
        } else {
            let help = Paragraph::new(vec![
                Line::from(""),
                Line::from("  Выберите группу для просмотра деталей"),
                Line::from(""),
                Line::from("  Горячие клавиши:"),
                Line::from("    n - Создать группу"),
                Line::from("    i - Пригласить в группу"),
                Line::from("    l - Покинуть группу"),
                Line::from("    Enter - Открыть чат группы"),
            ])
            .block(
                Block::default()
                    .title(" Детали группы ")
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded),
            );
            frame.render_widget(help, chunks[1]);
        }
    }

    fn render_group_details(&self, frame: &mut UiFrame, area: Rect, group: &Group) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(8), // Info
                Constraint::Min(10),   // Members
                Constraint::Length(6), // Actions
            ])
            .split(area);

        // Group info
        let info = vec![
            Line::from(format!("📌 Name: {}", group.name)),
            Line::from(format!("🆔 ID: {}", group.id)),
            Line::from(format!(
                "👑 Owner: {}",
                self.get_friend_display_name(&group.owner)
            )),
            Line::from(format!("👥 Members: {}", group.members.len())),
            Line::from(format!(
                "📅 Created: {}",
                DateTime::<Utc>::from_timestamp(group.created_at, 0)
                    .unwrap()
                    .format("%Y-%m-%d %H:%M")
            )),
            Line::from(format!(
                "📡 Relay: {}",
                if group.relay { "Enabled" } else { "Disabled" }
            )),
        ];

        let info_widget = Paragraph::new(info).block(
            Block::default()
                .title(" Group Info ")
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded),
        );
        frame.render_widget(info_widget, chunks[0]);

        // Members list with roles
        let members: Vec<Line> = group
            .members
            .iter()
            .map(|(device_id, role)| {
                let role_icon = match role {
                    GroupRole::Owner => "👑",
                    GroupRole::Admin => "⭐",
                    GroupRole::Member => "👤",
                };
                let display = self.get_friend_display_name(device_id);
                let online = self.is_online(device_id);
                let status = if online { "🟢" } else { "⚫" };

                Line::from(format!(
                    "{} {} {} {}",
                    status,
                    role_icon,
                    display,
                    if device_id == &self.state.device_id {
                        "(You)"
                    } else {
                        ""
                    }
                ))
            })
            .collect();

        let members_widget = Paragraph::new(members)
            .block(
                Block::default()
                    .title(" Members ")
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded),
            )
            .scroll((0, 0));
        frame.render_widget(members_widget, chunks[1]);

        // Available actions based on role
        let my_role = group.members.get(&self.state.device_id);
        let mut actions = vec![Line::from("Enter - Open group chat")];

        if matches!(my_role, Some(GroupRole::Owner | GroupRole::Admin)) {
            actions.push(Line::from("i - Invite member"));
            actions.push(Line::from("k - Kick member"));
            actions.push(Line::from("r - Change role"));
        }

        if matches!(my_role, Some(GroupRole::Owner)) {
            actions.push(Line::from("t - Transfer ownership"));
            actions.push(Line::from("d - Delete group"));
        }

        let actions_widget = Paragraph::new(actions).block(
            Block::default()
                .title(" Actions ")
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded),
        );
        frame.render_widget(actions_widget, chunks[2]);
    }

    fn render_calls(&mut self, frame: &mut UiFrame, area: Rect) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(15), // Active call
                Constraint::Min(10),    // Call history
                Constraint::Length(8),  // Stats
            ])
            .split(area);

        // Active call display
        if let Some(call_id) = &self.active_call {
            self.render_active_call(frame, chunks[0], call_id);
        } else {
            let no_call = Paragraph::new(vec![
                Line::from(""),
                Line::from("  📞 No active call"),
                Line::from(""),
                Line::from("  Press 'c' to start a new call"),
                Line::from("  Press 'v' for video call"),
            ])
            .alignment(Alignment::Center)
            .block(
                Block::default()
                    .title(" Active Call ")
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded),
            );
            frame.render_widget(no_call, chunks[0]);
        }

        // Call history
        let history = self.call_manager.get_active_calls();
        let history_items: Vec<ListItem> = history
            .iter()
            .map(|call_id| ListItem::new(format!("📞 Call: {}", &call_id[..8])))
            .collect();

        let history_list = List::new(history_items).block(
            Block::default()
                .title(" Call History ")
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded),
        );
        frame.render_widget(history_list, chunks[1]);

        // Call quality stats
        self.render_call_stats(frame, chunks[2]);
    }

    fn render_active_call(&self, frame: &mut UiFrame, area: Rect, _call_id: &str) {
        // Animated call display
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Length(5),
                Constraint::Length(3),
                Constraint::Length(2),
            ])
            .split(area);

        // Call status
        let status = Paragraph::new("🔴 Connected")
            .style(Style::default().fg(Color::Red).add_modifier(Modifier::BOLD))
            .alignment(Alignment::Center);
        frame.render_widget(status, chunks[0]);

        // Participants
        let participants = Paragraph::new(vec![
            Line::from(""),
            Line::from("  You ←→ Peer"),
            Line::from(""),
        ])
        .alignment(Alignment::Center);
        frame.render_widget(participants, chunks[1]);

        // Duration
        let duration = Paragraph::new("Duration: 00:42").alignment(Alignment::Center);
        frame.render_widget(duration, chunks[2]);

        // Controls
        let controls = Paragraph::new("🔇 Mute (m) | 📹 Video (v) | 📴 End (e)")
            .alignment(Alignment::Center)
            .style(Style::default().fg(Color::Gray));
        frame.render_widget(controls, chunks[3]);
    }

    fn render_call_stats(&mut self, frame: &mut UiFrame, area: Rect) {
        let sections = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(6), Constraint::Min(3)])
            .split(area);

        let quality_data: Vec<u64> = self
            .call_quality_history
            .iter()
            .map(|&v| (v * 100.0) as u64)
            .collect();

        if quality_data.is_empty() {
            let no_stats = Paragraph::new("No call statistics available")
                .alignment(Alignment::Center)
                .block(
                    Block::default()
                        .title(" Call Quality ")
                        .borders(Borders::ALL)
                        .border_type(BorderType::Rounded),
                );
            frame.render_widget(no_stats, sections[0]);
        } else {
            let sparkline = Sparkline::default()
                .block(
                    Block::default()
                        .title(" Call Quality (%) ")
                        .borders(Borders::ALL)
                        .border_type(BorderType::Rounded),
                )
                .data(&quality_data)
                .max(100)
                .style(Style::default().fg(Color::Green));
            frame.render_widget(sparkline, sections[0]);
        }

        let mut info_lines = Vec::new();
        if let Some(audio) = &self.call_audio_metrics {
            info_lines.push(Line::from(format!(
                "Audio · {} Hz · {} ch · {} samples",
                audio.sample_rate, audio.channels, audio.samples
            )));
            info_lines.push(Line::from(format!(
                "Audio updated {}",
                audio.timestamp.format("%H:%M:%S")
            )));
        }
        if let Some(video) = &self.call_video_metrics {
            info_lines.push(Line::from(format!(
                "Video · {}×{} · {} frames",
                video.width, video.height, video.frames_decoded
            )));
            info_lines.push(Line::from(format!(
                "Video updated {}",
                video.timestamp.format("%H:%M:%S")
            )));
        }
        if info_lines.is_empty() {
            info_lines.push(Line::from("No media metrics available yet."));
        }

        let metrics = Paragraph::new(info_lines)
            .alignment(Alignment::Left)
            .block(
                Block::default()
                    .title(" Media Metrics ")
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded),
            )
            .wrap(Wrap { trim: true });
        frame.render_widget(metrics, sections[1]);
    }

    fn render_voice(&mut self, frame: &mut UiFrame, area: Rect) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(10), // Recording controls
                Constraint::Length(5),  // Waveform
                Constraint::Min(10),    // Voice messages
            ])
            .split(area);

        let wave_frame = if self.animations_enabled {
            self.wave_animation.tick(Duration::from_millis(50))
        } else {
            self.wave_animation.tick(Duration::ZERO)
        };
        let cat_art = if self.voice_recording {
            ascii_art::CAT_TYPING
        } else {
            ascii_art::CAT_SLEEPING
        };

        // Recording controls
        let recording_status = if self.voice_recording {
            vec![
                Line::from(""),
                Line::from("🔴 RECORDING")
                    .style(Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
                Line::from(""),
                Line::from(format!(
                    "{} {}",
                    wave_frame,
                    visualize_audio_wave(self.voice_amplitude.clamp(0.0, 1.0), 30)
                )),
                Line::from(""),
                Line::from("Press SPACE to stop"),
            ]
        } else {
            vec![
                Line::from(""),
                Line::from("🎤 Voice Messages"),
                Line::from(""),
                Line::from("Press SPACE to start recording"),
                Line::from("Press P to play last message"),
            ]
        };

        let controls = Paragraph::new(recording_status)
            .alignment(Alignment::Center)
            .block(
                Block::default()
                    .title(" Voice Recorder ")
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded),
            );
        frame.render_widget(controls, chunks[0]);

        // Audio waveform visualization
        let mut waveform_data: Vec<u64> = if self.voice_buffer.is_empty() {
            vec![0; 64]
        } else {
            self.voice_buffer
                .iter()
                .rev()
                .take(64)
                .cloned()
                .map(|value| value as u64)
                .collect()
        };
        waveform_data.reverse();
        if waveform_data.is_empty() {
            waveform_data.push((self.voice_amplitude.clamp(0.0, 1.0) * 255.0) as u64);
        }

        let waveform = Sparkline::default()
            .block(
                Block::default()
                    .title(" Waveform ")
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded),
            )
            .data(&waveform_data)
            .max(255)
            .style(Style::default().fg(Color::Cyan));
        let waveform_layout = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
            .split(chunks[1]);
        frame.render_widget(waveform, waveform_layout[0]);

        let cat_widget = Paragraph::new(cat_art).alignment(Alignment::Center).block(
            Block::default()
                .title(" Mood ")
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded),
        );
        frame.render_widget(cat_widget, waveform_layout[1]);

        // Voice messages list
        let mut voice_messages = Vec::new();
        for channel in &self.channels {
            for entry in channel.messages.iter().rev() {
                if let MessageContent::Voice(voice) = &entry.content {
                    voice_messages.push(Line::from(format!(
                        "🎵 {} ({} frames, {} ms)",
                        self.get_friend_display_name(&entry.sender),
                        voice.frames.len(),
                        voice.duration_ms
                    )));
                    if voice_messages.len() >= 8 {
                        break;
                    }
                }
            }
            if voice_messages.len() >= 8 {
                break;
            }
        }
        if voice_messages.is_empty() {
            voice_messages.push(Line::from("No voice messages yet"));
        }

        let messages = Paragraph::new(voice_messages).block(
            Block::default()
                .title(" Recent Voice Messages ")
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded),
        );
        frame.render_widget(messages, chunks[2]);
    }

    fn render_friends(&mut self, frame: &mut UiFrame, area: Rect) {
        // Similar to original but with enhanced styling
        let friends = self.state.friends();
        let items: Vec<ListItem> = if friends.is_empty() {
            vec![ListItem::new("No friends yet. Press 'a' to add.")]
        } else {
            friends
                .iter()
                .map(|friend| {
                    let fallback = friend
                        .alias
                        .as_ref()
                        .or(friend.handle.as_ref())
                        .unwrap_or(&friend.user_id);
                    let presence = self.presence.get(&friend.user_id);
                    let online = presence
                        .map(|info| info.state == "online" && info.is_active())
                        .unwrap_or(false);
                    let status = if online { "🟢" } else { "⚫" };
                    let mut label = format!("{} {}", status, fallback);
                    if let Some(info) = presence {
                        if let Some(name) = info.display_name.as_ref() {
                            label.push_str(&format!(" · {}", name));
                        }
                        if let Some(id) = info.user_id.as_ref() {
                            label.push_str(&format!(" · {}", self.short_id(id)));
                        }
                        if info.avatar_url.is_some() {
                            label.push_str(" · 📸");
                        }
                        label.push_str(&format!(
                            " · updated {}",
                            info.updated_at.format("%H:%M:%S")
                        ));
                    }
                    ListItem::new(label)
                })
                .collect()
        };

        let list = List::new(items)
            .block(
                Block::default()
                    .title(" Friends ")
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded),
            )
            .highlight_style(
                Style::default()
                    .bg(self.get_theme_secondary_color())
                    .add_modifier(Modifier::BOLD),
            );

        let mut state = ListState::default();
        frame.render_stateful_widget(list, area, &mut state);
    }

    fn render_devices(&mut self, frame: &mut UiFrame, area: Rect) {
        let mut lines = vec![
            Line::from(format!(
                "📱 Current device: {}",
                short_hex(&self.state.device_id)
            )),
            Line::from(""),
        ];
        if self.devices.is_empty() {
            lines.push(Line::from("No devices loaded. Press 'r' to refresh."));
        } else {
            for entry in &self.devices {
                lines.push(Line::from(format!(
                    "{} {} [{}] created {}",
                    if entry.current { "⭐" } else { "•" },
                    short_hex(&entry.device_id),
                    entry.status,
                    entry.created_at
                )));
            }
        }
        lines.push(Line::from(""));
        lines.push(Line::from("Press 'r' to refresh devices"));

        let devices = Paragraph::new(lines).block(
            Block::default()
                .title(" Devices ")
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded),
        );
        frame.render_widget(devices, area);
    }

    fn render_settings(&mut self, frame: &mut UiFrame, area: Rect) {
        let settings = vec![
            Line::from(format!("🎨 Theme: {:?}", self.theme)),
            Line::from(format!(
                "✨ Animations: {}",
                if self.animations_enabled { "ON" } else { "OFF" }
            )),
            Line::from(format!(
                "🔊 Sound: {}",
                if self.sound_enabled { "ON" } else { "OFF" }
            )),
            Line::from(format!(
                "😊 Emoji mode: {}",
                if self.emoji_mode { "ON" } else { "OFF" }
            )),
            Line::from(""),
            Line::from("Press 't' to change theme"),
            Line::from("Press 'a' to toggle animations"),
            Line::from("Press 's' to toggle sound"),
            Line::from("Press 'e' to toggle emoji mode"),
        ];

        let settings_widget = Paragraph::new(settings).block(
            Block::default()
                .title(" Settings ")
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded),
        );
        frame.render_widget(settings_widget, area);
    }

    fn render_input(&self, frame: &mut UiFrame, area: Rect) {
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Min(10), Constraint::Length(20)])
            .split(area);

        // Input field
        let input = Paragraph::new(format!("> {}", self.input))
            .style(Style::default().fg(Color::White))
            .block(
                Block::default()
                    .title(format!(" Input - {} ", self.get_view_name()))
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .border_style(Style::default().fg(self.get_theme_primary_color())),
            );
        frame.render_widget(input, chunks[0]);

        // Emoji picker hint
        if self.emoji_mode {
            let emoji_hint = Paragraph::new(vec![
                Line::from("😊 Alt+1-9 for emoji"),
                Line::from("✨ :emoji: for more"),
            ])
            .alignment(Alignment::Center)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded),
            );
            frame.render_widget(emoji_hint, chunks[1]);
        }
    }

    fn render_status_bar(&self, frame: &mut UiFrame, area: Rect) {
        let status = format!(
            " {} | Device: {} | Server: {} | Session: {} | F1: Help | F10: Quit ",
            if self.connected {
                "🟢 Online"
            } else {
                "🔴 Offline"
            },
            &self.state.device_id[..8],
            self.state.server_url,
            self.session_id.as_ref().map(|s| &s[..8]).unwrap_or("none")
        );

        let status_bar = Paragraph::new(status).style(
            Style::default()
                .bg(self.get_theme_secondary_color())
                .fg(Color::White),
        );
        frame.render_widget(status_bar, area);
    }

    fn render_notifications(&mut self, frame: &mut UiFrame, area: Rect) {
        let notifications = &self.notifications;
        if notifications.is_empty() {
            return;
        }

        let notification_area = Rect {
            x: area.width.saturating_sub(40),
            y: 4,
            width: 38.min(area.width),
            height: (notifications.len() as u16 * 3).min(12),
        };

        frame.render_widget(Clear, notification_area);

        let mut y_offset = 0;
        for notification in notifications.iter().take(4) {
            let color = match notification.level {
                NotificationLevel::Info => Color::Blue,
                NotificationLevel::Success => Color::Green,
                NotificationLevel::Warning => Color::Yellow,
                NotificationLevel::Error => Color::Red,
            };

            let timestamp = notification.timestamp.format("%H:%M:%S").to_string();
            let notification_widget =
                Paragraph::new(format!("[{}] {}", timestamp, notification.message))
                    .style(Style::default().fg(color))
                    .block(
                        Block::default()
                            .borders(Borders::ALL)
                            .border_type(BorderType::Rounded)
                            .border_style(Style::default().fg(color)),
                    );

            let notification_rect = Rect {
                x: notification_area.x,
                y: notification_area.y + y_offset,
                width: notification_area.width,
                height: 3,
            };

            frame.render_widget(notification_widget, notification_rect);
            y_offset += 3;
        }
    }

    // Helper methods
    fn get_view_name(&self) -> &str {
        match self.view {
            AppView::Chat => "Chat",
            AppView::Groups => "Groups",
            AppView::Calls => "Calls",
            AppView::Voice => "Voice",
            AppView::Friends => "Friends",
            AppView::Devices => "Devices",
            AppView::Settings => "Settings",
            _ => "CommuCat",
        }
    }

    fn get_view_icon(&self) -> &str {
        match self.view {
            AppView::Chat => "💬",
            AppView::Groups => "👥",
            AppView::Calls => "📞",
            AppView::Voice => "🎤",
            AppView::Friends => "👫",
            AppView::Devices => "📱",
            AppView::Settings => "⚙️",
            _ => "🐱",
        }
    }

    fn get_theme_primary_color(&self) -> Color {
        match self.theme {
            Theme::Dark => Color::Cyan,
            Theme::Light => Color::Blue,
            Theme::Cyberpunk => Color::Magenta,
            Theme::Kawaii => Color::LightMagenta,
        }
    }

    fn get_theme_secondary_color(&self) -> Color {
        match self.theme {
            Theme::Dark => Color::DarkGray,
            Theme::Light => Color::Gray,
            Theme::Cyberpunk => Color::Rgb(64, 0, 128),
            Theme::Kawaii => Color::Rgb(255, 192, 203),
        }
    }

    fn get_theme_border_style(&self) -> Style {
        Style::default().fg(self.get_theme_primary_color())
    }

    fn get_friend_display_name(&self, device_id: &str) -> String {
        if device_id == self.state.device_id {
            return "You".to_string();
        }

        if let Some(info) = self.presence.get(device_id) {
            if let Some(name) = info.display_name.as_ref() {
                return name.clone();
            }
            if let Some(handle) = info.handle.as_ref() {
                return handle.clone();
            }
        }

        self.state
            .friends()
            .iter()
            .find(|f| f.user_id == device_id)
            .and_then(|f| f.alias.as_ref().or(f.handle.as_ref()))
            .map(|value| value.to_string())
            .unwrap_or_else(|| device_id.to_string())
    }

    fn is_online(&self, device_id: &str) -> bool {
        self.presence
            .get(device_id)
            .map(|info| info.state == "online" && info.is_active())
            .unwrap_or(false)
    }

    fn add_notification(&mut self, message: String, level: NotificationLevel) {
        let mut text = message;
        if level == NotificationLevel::Success && self.emoji_mode {
            text = format!("{} {}", text, ascii_art::random_kawaii());
        }
        let notification = Notification {
            message: text,
            level,
            timestamp: Utc::now(),
            expires_at: Utc::now() + ChronoDuration::seconds(5),
        };
        self.notifications.push_back(notification);

        // Keep only last 10 notifications
        while self.notifications.len() > 10 {
            self.notifications.pop_front();
        }
    }

    fn cleanup_expired_notifications(&mut self) {
        let now = Utc::now();
        self.notifications.retain(|n| n.expires_at > now);
    }

    // Event handlers (stubs for now)
    async fn handle_key(&mut self, key: KeyEvent) -> Result<()> {
        // Handle key input
        match key.code {
            KeyCode::F(10) | KeyCode::Esc if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.should_quit = true;
            }
            KeyCode::Tab => {
                // Cycle through views
                let current_idx = self
                    .menu_items
                    .iter()
                    .position(|item| item.view == self.view)
                    .unwrap_or(0);
                let next_idx = (current_idx + 1) % self.menu_items.len();
                self.view = self.menu_items[next_idx].view;
                self.transition_progress = 0.0;
            }
            KeyCode::Up => {
                if self.active_channel > 0 {
                    self.active_channel -= 1;
                }
            }
            KeyCode::Down => {
                if self.active_channel + 1 < self.channels.len() {
                    self.active_channel += 1;
                }
            }
            KeyCode::Char('r') if self.view == AppView::Devices => {
                self.refresh_devices().await?;
            }
            KeyCode::Char(' ') if self.view == AppView::Voice => {
                self.voice_recording = !self.voice_recording;
                if self.voice_recording {
                    self.wave_animation.reset();
                    self.voice_buffer.clear();
                    self.voice_amplitude = 0.0;
                } else {
                    self.voice_amplitude = 0.0;
                    self.finalize_voice_recording()?;
                }
            }
            KeyCode::Char(c) if key.modifiers.contains(KeyModifiers::ALT) => {
                // Emoji shortcuts
                if let Some(emoji) = c
                    .to_digit(10)
                    .filter(|digit| (1..=9).contains(digit))
                    .and_then(|digit| KAWAII_REACTIONS.get((digit - 1) as usize))
                {
                    self.input.push_str(emoji.1);
                    self.input.push(' ');
                }
            }
            KeyCode::Char(c) => {
                if let Some(view) = self
                    .menu_items
                    .iter()
                    .find_map(|item| (item.hotkey == Some(c)).then_some(item.view))
                {
                    self.view = view;
                    self.transition_progress = 0.0;
                } else {
                    self.input.push(c);
                }
            }
            KeyCode::Backspace => {
                self.input.pop();
            }
            KeyCode::Enter => {
                let input = self.input.clone();
                self.input.clear();
                self.process_input(input).await?;
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
                self.session_id = Some(session_id);
                self.add_notification(
                    "✅ Connected successfully".to_string(),
                    NotificationLevel::Success,
                );
                if pairing_required {
                    self.add_notification(
                        "🔐 Pairing required to access secure features".to_string(),
                        NotificationLevel::Warning,
                    );
                }
                let _ = self.refresh_devices().await;
            }
            ClientEvent::Disconnected { reason } => {
                self.connected = false;
                self.session_id = None;
                self.add_notification(
                    format!("❌ Disconnected: {}", reason),
                    NotificationLevel::Error,
                );
            }
            ClientEvent::Error { detail } => {
                self.last_error = Some(detail.clone());
                self.add_notification(format!("⚠️ {}", detail), NotificationLevel::Error);
            }
            ClientEvent::Frame(frame) => {
                self.handle_protocol_frame(frame).await?;
            }
            ClientEvent::Log { line } => {
                // Add to system channel
                self.add_system_message(line);
            }
        }
        Ok(())
    }

    async fn handle_protocol_frame(&mut self, frame: ProtoFrame) -> Result<()> {
        match frame {
            ProtoFrame {
                frame_type: FrameType::Msg,
                channel_id,
                sequence,
                payload,
                ..
            } => match payload {
                FramePayload::Opaque(data) => self.process_msg_frame(channel_id, sequence, data)?,
                other => bail!("unexpected payload {:?} for MSG frame", other),
            },
            ProtoFrame {
                frame_type: FrameType::Ack,
                channel_id,
                payload,
                ..
            } => match payload {
                FramePayload::Control(envelope) => self.process_ack_frame(channel_id, envelope)?,
                other => bail!("unexpected payload {:?} for ACK frame", other),
            },
            ProtoFrame {
                frame_type: FrameType::Typing,
                channel_id,
                payload,
                ..
            } => match payload {
                FramePayload::Control(envelope) => {
                    self.process_typing_frame(channel_id, envelope)?
                }
                other => bail!("unexpected payload {:?} for TYPING frame", other),
            },
            ProtoFrame {
                frame_type: FrameType::Presence,
                payload,
                ..
            } => match payload {
                FramePayload::Control(envelope) => self.process_presence_frame(envelope)?,
                other => bail!("unexpected payload {:?} for PRESENCE frame", other),
            },
            ProtoFrame {
                frame_type: FrameType::Join,
                channel_id,
                payload,
                ..
            } => match payload {
                FramePayload::Control(envelope) => self.process_join_frame(channel_id, envelope)?,
                other => bail!("unexpected payload {:?} for JOIN frame", other),
            },
            ProtoFrame {
                frame_type: FrameType::Leave,
                channel_id,
                payload,
                ..
            } => match payload {
                FramePayload::Control(envelope) => {
                    self.process_leave_frame(channel_id, envelope)?
                }
                other => bail!("unexpected payload {:?} for LEAVE frame", other),
            },
            ProtoFrame {
                frame_type: FrameType::GroupCreate,
                channel_id,
                payload,
                ..
            } => match payload {
                FramePayload::Control(envelope) => {
                    self.process_group_create(channel_id, envelope)?
                }
                other => bail!("unexpected payload {:?} for GROUP_CREATE frame", other),
            },
            ProtoFrame {
                frame_type: FrameType::GroupInvite,
                channel_id,
                payload,
                ..
            } => match payload {
                FramePayload::Control(envelope) => {
                    self.process_group_invite(channel_id, envelope)?
                }
                other => bail!("unexpected payload {:?} for GROUP_INVITE frame", other),
            },
            ProtoFrame {
                frame_type: FrameType::GroupEvent,
                channel_id,
                payload,
                ..
            } => match payload {
                FramePayload::Control(envelope) => {
                    self.process_group_event(channel_id, envelope)?
                }
                other => bail!("unexpected payload {:?} for GROUP_EVENT frame", other),
            },
            ProtoFrame {
                frame_type: FrameType::CallOffer,
                channel_id,
                payload,
                ..
            } => match payload {
                FramePayload::Control(envelope) => self.process_call_offer(channel_id, envelope)?,
                other => bail!("unexpected payload {:?} for CALL_OFFER frame", other),
            },
            ProtoFrame {
                frame_type: FrameType::CallAnswer,
                channel_id,
                payload,
                ..
            } => match payload {
                FramePayload::Control(envelope) => {
                    self.process_call_answer(channel_id, envelope)?
                }
                other => bail!("unexpected payload {:?} for CALL_ANSWER frame", other),
            },
            ProtoFrame {
                frame_type: FrameType::CallEnd,
                channel_id,
                payload,
                ..
            } => match payload {
                FramePayload::Control(envelope) => self.process_call_end(channel_id, envelope)?,
                other => bail!("unexpected payload {:?} for CALL_END frame", other),
            },
            ProtoFrame {
                frame_type: FrameType::CallStats,
                payload,
                ..
            } => match payload {
                FramePayload::Control(envelope) => self.process_call_stats(envelope)?,
                other => bail!("unexpected payload {:?} for CALL_STATS frame", other),
            },
            ProtoFrame {
                frame_type: FrameType::VoiceFrame,
                channel_id,
                payload,
                ..
            } => match payload {
                FramePayload::Opaque(data) => self.process_voice_frame(channel_id, data)?,
                other => bail!("unexpected payload {:?} for VOICE_FRAME", other),
            },
            ProtoFrame {
                frame_type: FrameType::VideoFrame,
                channel_id,
                payload,
                ..
            } => match payload {
                FramePayload::Opaque(data) => self.process_video_frame(channel_id, data)?,
                other => bail!("unexpected payload {:?} for VIDEO_FRAME", other),
            },
            ProtoFrame {
                frame_type: FrameType::Error,
                payload,
                ..
            } => match payload {
                FramePayload::Control(envelope) => self.process_error_frame(envelope),
                other => bail!("unexpected payload {:?} for ERROR frame", other),
            },
            ProtoFrame {
                frame_type: FrameType::KeyUpdate,
                channel_id,
                payload,
                ..
            } => match payload {
                FramePayload::Opaque(data) => {
                    self.add_system_message(format!(
                        "🔐 Key update on channel {} ({} bytes)",
                        channel_id,
                        data.len()
                    ));
                }
                other => bail!("unexpected payload {:?} for KEY_UPDATE frame", other),
            },
            ProtoFrame {
                frame_type: FrameType::Hello,
                ..
            }
            | ProtoFrame {
                frame_type: FrameType::Auth,
                ..
            } => {}
        }
        Ok(())
    }

    fn process_msg_frame(&mut self, channel_id: u64, _sequence: u64, data: Vec<u8>) -> Result<()> {
        let idx = self.ensure_channel(channel_id);
        let now = Utc::now();

        let mut sender = String::new();
        let mut body: Option<String> = None;
        let mut reactions: HashMap<String, Vec<String>> = HashMap::new();

        if let Ok(value) = serde_json::from_slice::<Value>(&data) {
            if let Some(s) = value.get("sender").and_then(|v| v.as_str()) {
                sender = s.to_string();
            } else if let Some(from) = value.get("from").and_then(|v| v.as_str()) {
                sender = from.to_string();
            }
            if let Some(text) = value.get("text").and_then(|v| v.as_str()) {
                body = Some(text.to_string());
            } else if let Some(text) = value.get("body").and_then(|v| v.as_str()) {
                body = Some(text.to_string());
            }
            if let Some(map) = value.get("reactions").and_then(|v| v.as_object()) {
                for (emoji, users) in map {
                    if let Some(array) = users.as_array() {
                        let mut list = Vec::new();
                        for entry in array {
                            if let Some(user) = entry.as_str() {
                                list.push(user.to_string());
                            }
                        }
                        if !list.is_empty() {
                            reactions.insert(emoji.clone(), list);
                        }
                    }
                }
            }
        }

        if sender.is_empty() {
            sender = "unknown".to_string();
        }

        let text = body.unwrap_or_else(|| String::from_utf8_lossy(&data).to_string());
        if sender != "unknown" && !self.channels[idx].members.contains(&sender) {
            self.channels[idx].members.push(sender.clone());
        }

        let entry = MessageEntry {
            timestamp: now,
            sender: sender.clone(),
            content: MessageContent::Text(text.clone()),
            reactions,
        };
        self.push_channel_message(idx, entry);

        if sender != self.state.device_id {
            if idx != self.active_channel {
                self.channels[idx].unread_count = self.channels[idx].unread_count.saturating_add(1);
            }
            let preview = self.preview_text(&text);
            self.add_notification(
                format!("💌 {}: {}", self.get_friend_display_name(&sender), preview),
                NotificationLevel::Info,
            );
        }

        Ok(())
    }

    fn process_ack_frame(&mut self, channel_id: u64, envelope: ControlEnvelope) -> Result<()> {
        if let Some(obj) = envelope.properties.as_object() {
            if let Some(seq) = obj.get("ack").and_then(|v| v.as_u64()) {
                self.add_system_message(format!("✅ ACK {} on channel {}", seq, channel_id));
            }
            if let Some(call_id) = obj.get("call_id").and_then(|v| v.as_str()) {
                self.add_notification(
                    format!("📶 Call {} acknowledged", self.short_id(call_id)),
                    NotificationLevel::Success,
                );
            }
        }
        Ok(())
    }

    fn process_typing_frame(&mut self, channel_id: u64, envelope: ControlEnvelope) -> Result<()> {
        let idx = self.ensure_channel(channel_id);
        let payload = envelope
            .properties
            .as_object()
            .context("typing payload must be an object")?;
        let device = payload
            .get("device")
            .or_else(|| payload.get("device_id"))
            .or_else(|| payload.get("sender"))
            .and_then(|v| v.as_str())
            .context("typing frame missing device id")?;
        let active = payload
            .get("typing")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let ttl_ms = payload
            .get("ttl_ms")
            .or_else(|| payload.get("expires_in"))
            .and_then(|v| v.as_u64())
            .unwrap_or(3_000);
        if active {
            let label = payload
                .get("label")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| self.get_friend_display_name(device));
            self.channels[idx].typing.insert(
                device.to_string(),
                TypingIndicator {
                    label,
                    expires_at: Utc::now() + ChronoDuration::milliseconds(ttl_ms as i64),
                    animation_frame: 0,
                },
            );
        } else {
            self.channels[idx].typing.remove(device);
        }
        Ok(())
    }

    fn process_presence_frame(&mut self, envelope: ControlEnvelope) -> Result<()> {
        let obj = envelope
            .properties
            .as_object()
            .context("presence payload must be an object")?;
        let entity = obj
            .get("entity")
            .and_then(|v| v.as_str())
            .context("presence payload missing entity")?;
        let state = obj
            .get("state")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        let expires_at = obj
            .get("expires_at")
            .and_then(|v| v.as_str())
            .and_then(|v| DateTime::parse_from_rfc3339(v).ok())
            .map(|dt| dt.with_timezone(&Utc));
        let user_obj = obj.get("user").and_then(|v| v.as_object());
        let handle = user_obj
            .and_then(|map| map.get("handle"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let display_name = user_obj
            .and_then(|map| map.get("display_name"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let avatar_url = user_obj
            .and_then(|map| map.get("avatar_url"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let user_id = user_obj
            .and_then(|map| map.get("id"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let notify = self
            .presence
            .get(entity)
            .map(|info| info.state != state)
            .unwrap_or(true);

        self.presence.insert(
            entity.to_string(),
            PresenceInfo {
                state: state.clone(),
                expires_at,
                handle,
                display_name,
                avatar_url,
                user_id,
                updated_at: Utc::now(),
            },
        );

        if notify {
            let icon = if state == "online" { "🟢" } else { "⚫" };
            self.add_notification(
                format!(
                    "{} {} {}",
                    icon,
                    self.get_friend_display_name(entity),
                    state
                ),
                NotificationLevel::Info,
            );
        }

        Ok(())
    }

    fn process_join_frame(&mut self, channel_id: u64, envelope: ControlEnvelope) -> Result<()> {
        let idx = self.ensure_channel(channel_id);
        let obj = envelope
            .properties
            .as_object()
            .context("join payload must be an object")?;
        if let Some(members) = obj.get("members").and_then(|v| v.as_array()) {
            self.channels[idx].members = members
                .iter()
                .filter_map(|item| item.as_str().map(|s| s.to_string()))
                .collect();
        }
        if let Some(name) = obj.get("name").and_then(|v| v.as_str()) {
            self.channels[idx].name = name.to_string();
        }
        if let Some(group_id) = obj.get("group_id").and_then(|v| v.as_str()) {
            self.channels[idx].is_group = true;
            self.channels[idx].group_id = Some(group_id.to_string());
            if let Some(group) = self.groups.get(group_id) {
                self.channels[idx].name = group.name.clone();
            } else {
                self.channels[idx].name = format!("Group {}", short_hex(group_id));
            }
        }
        Ok(())
    }

    fn process_leave_frame(&mut self, channel_id: u64, envelope: ControlEnvelope) -> Result<()> {
        let idx = self.ensure_channel(channel_id);
        if let Some(device) = envelope
            .properties
            .as_object()
            .and_then(|obj| obj.get("device").or_else(|| obj.get("device_id")))
            .and_then(|v| v.as_str())
        {
            self.channels[idx].members.retain(|member| member != device);
            self.add_system_message(format!(
                "👋 {} left channel {}",
                self.get_friend_display_name(device),
                channel_id
            ));
        }
        Ok(())
    }

    fn process_group_create(&mut self, channel_id: u64, envelope: ControlEnvelope) -> Result<()> {
        let obj = envelope
            .properties
            .as_object()
            .context("group create payload must be an object")?;
        let group_id = obj
            .get("group_id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        let name = obj
            .get("name")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("Group {}", short_hex(&group_id)));
        let owner = obj
            .get("owner")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| self.state.device_id.clone());
        let relay = obj.get("relay").and_then(|v| v.as_bool()).unwrap_or(true);

        let mut group = Group::new(group_id.clone(), name.clone(), owner.clone());
        group.relay = relay;

        if let Some(members) = obj.get("members").and_then(|v| v.as_array()) {
            let roles = obj.get("roles").and_then(|v| v.as_object());
            for member in members.iter().filter_map(|v| v.as_str()) {
                if member == owner {
                    continue;
                }
                let role = roles
                    .and_then(|map| map.get(member))
                    .and_then(|value| value.as_str())
                    .map(Self::parse_group_role)
                    .unwrap_or(GroupRole::Member);
                group.add_member(member.to_string(), role);
            }
        }

        self.groups.insert(group_id.clone(), group);

        let idx = self.ensure_channel(channel_id);
        self.channels[idx].is_group = true;
        self.channels[idx].group_id = Some(group_id.clone());
        self.channels[idx].name = name.clone();

        self.add_notification(
            format!("👥 Group {} created", short_hex(&group_id)),
            NotificationLevel::Success,
        );
        Ok(())
    }

    fn process_group_invite(&mut self, channel_id: u64, envelope: ControlEnvelope) -> Result<()> {
        let obj = envelope
            .properties
            .as_object()
            .context("group invite payload must be an object")?;
        let group_id = obj
            .get("group_id")
            .and_then(|v| v.as_str())
            .context("group invite missing group_id")?
            .to_string();
        let device = obj
            .get("device")
            .or_else(|| obj.get("member"))
            .and_then(|v| v.as_str())
            .context("group invite missing device")?
            .to_string();
        let role = obj
            .get("role")
            .and_then(|v| v.as_str())
            .map(Self::parse_group_role)
            .unwrap_or(GroupRole::Member);

        let group = self.groups.entry(group_id.clone()).or_insert_with(|| {
            Group::new(
                group_id.clone(),
                format!("Group {}", short_hex(&group_id)),
                self.state.device_id.clone(),
            )
        });
        group.add_member(device.clone(), role);

        let idx = self.ensure_channel(channel_id);
        if !self.channels[idx].members.contains(&device) {
            self.channels[idx].members.push(device.clone());
        }

        self.add_notification(
            format!(
                "➕ {} joined {}",
                self.get_friend_display_name(&device),
                short_hex(&group_id)
            ),
            NotificationLevel::Success,
        );
        Ok(())
    }

    fn process_group_event(&mut self, channel_id: u64, envelope: ControlEnvelope) -> Result<()> {
        let idx = self.ensure_channel(channel_id);
        let description = envelope
            .properties
            .get("description")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .or_else(|| {
                envelope
                    .properties
                    .get("event")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            })
            .unwrap_or_else(|| {
                serde_json::to_string(&envelope.properties)
                    .unwrap_or_else(|_| "group event".to_string())
            });
        let entry = MessageEntry {
            timestamp: Utc::now(),
            sender: "System".to_string(),
            content: MessageContent::GroupEvent(description.clone()),
            reactions: HashMap::new(),
        };
        self.push_channel_message(idx, entry);
        Ok(())
    }

    fn process_call_offer(&mut self, channel_id: u64, envelope: ControlEnvelope) -> Result<()> {
        let offer = CallOffer::try_from(&envelope).context("decode CALL_OFFER payload")?;
        self.call_manager.upsert_offer(offer.clone());
        self.call_channels.insert(channel_id, offer.call_id.clone());
        self.media
            .initialise_from_media(&offer.call_id, &offer.media)
            .with_context(|| format!("initialise media pipeline for call {}", offer.call_id))?;
        self.active_call = Some(offer.call_id.clone());

        let idx = self.ensure_channel(channel_id);
        let entry = MessageEntry {
            timestamp: Utc::now(),
            sender: offer.from.clone(),
            content: MessageContent::Call(CallInfo {
                call_id: offer.call_id.clone(),
                action: "offer".to_string(),
                duration: None,
            }),
            reactions: HashMap::new(),
        };
        self.push_channel_message(idx, entry);
        let is_target = offer
            .to
            .iter()
            .any(|target| target == &self.state.device_id);
        let label = if is_target { "Incoming" } else { "Relay" };
        self.add_notification(
            format!(
                "📞 {} call from {}",
                label,
                self.get_friend_display_name(&offer.from)
            ),
            NotificationLevel::Info,
        );
        Ok(())
    }

    fn process_call_answer(&mut self, channel_id: u64, envelope: ControlEnvelope) -> Result<()> {
        let answer = CallAnswer::try_from(&envelope).context("decode CALL_ANSWER payload")?;
        let accepted = answer.accept;
        if !self.call_manager.accept_answer(answer.clone()) {
            self.add_system_message(format!(
                "ℹ️ Received answer for unknown call {}",
                self.short_id(&answer.call_id)
            ));
        }
        if accepted {
            self.active_call = Some(answer.call_id.clone());
        } else if self.active_call.as_deref() == Some(&answer.call_id) {
            self.active_call = None;
        }
        let idx = self.ensure_channel(channel_id);
        let reason = answer.reason;
        let action = if accepted {
            "answer".to_string()
        } else if let Some(reason) = reason {
            format!("rejected ({reason:?})")
        } else {
            "rejected".to_string()
        };
        let entry = MessageEntry {
            timestamp: Utc::now(),
            sender: "Call".to_string(),
            content: MessageContent::Call(CallInfo {
                call_id: answer.call_id.clone(),
                action,
                duration: None,
            }),
            reactions: HashMap::new(),
        };
        self.push_channel_message(idx, entry);
        Ok(())
    }

    fn process_call_end(&mut self, channel_id: u64, envelope: ControlEnvelope) -> Result<()> {
        let end = CallEnd::try_from(&envelope).context("decode CALL_END payload")?;
        self.call_manager.end_call(&end.call_id);
        self.media.remove_call(&end.call_id);
        self.call_channels.retain(|_, id| id != &end.call_id);
        if self.active_call.as_deref() == Some(&end.call_id) {
            self.active_call = None;
        }
        self.call_audio_metrics = None;
        self.call_video_metrics = None;
        let duration = self
            .call_manager
            .get_call(&end.call_id)
            .and_then(|call| call.started_at.zip(call.ended_at))
            .and_then(|(start, end_ts)| {
                let diff = end_ts - start;
                if diff > 0 {
                    Some(Duration::from_secs(diff as u64))
                } else {
                    None
                }
            });
        let idx = self.ensure_channel(channel_id);
        let entry = MessageEntry {
            timestamp: Utc::now(),
            sender: "Call".to_string(),
            content: MessageContent::Call(CallInfo {
                call_id: end.call_id.clone(),
                action: format!("ended ({:?})", end.reason),
                duration,
            }),
            reactions: HashMap::new(),
        };
        self.push_channel_message(idx, entry);
        self.add_notification(
            format!("📴 Call {} ended", self.short_id(&end.call_id)),
            NotificationLevel::Info,
        );
        Ok(())
    }

    fn process_call_stats(&mut self, envelope: ControlEnvelope) -> Result<()> {
        let stats = CallStats::try_from(&envelope).context("decode CALL_STATS payload")?;
        self.call_manager.push_stats(stats.clone());
        let audio_quality = stats
            .audio
            .as_ref()
            .map(|audio| (1.0_f32 - audio.packet_loss.clamp(0.0, 1.0)).clamp(0.0, 1.0))
            .unwrap_or(1.0_f32);
        let video_quality = stats
            .video
            .as_ref()
            .map(|video| (1.0_f32 - video.packet_loss.clamp(0.0, 1.0)).clamp(0.0, 1.0))
            .unwrap_or(1.0_f32);
        let combined = ((audio_quality + video_quality) / 2.0_f32).clamp(0.0, 1.0);
        self.push_quality_sample(combined);
        Ok(())
    }

    fn process_voice_frame(&mut self, channel_id: u64, data: Vec<u8>) -> Result<()> {
        if let Some(call_id) = self.call_channels.get(&channel_id).cloned() {
            if let Some(metrics) = self.media.decode_audio(&call_id, &data)? {
                let level = metrics.level.clamp(0.0, 1.0);
                self.voice_amplitude = level;
                self.call_audio_metrics = Some(metrics.clone());
                let bucket = (level * 255.0) as u8;
                self.voice_buffer.push(bucket);
                if self.voice_buffer.len() > 1024 {
                    let drop = self.voice_buffer.len() - 1024;
                    self.voice_buffer.drain(0..drop);
                }
            }
        } else {
            self.add_system_message(format!(
                "🎤 Voice frame on channel {} ({} bytes)",
                channel_id,
                data.len()
            ));
        }
        Ok(())
    }

    fn process_video_frame(&mut self, channel_id: u64, data: Vec<u8>) -> Result<()> {
        if let Some(call_id) = self.call_channels.get(&channel_id).cloned() {
            if let Some(metrics) = self.media.decode_video(&call_id, &data)? {
                self.call_video_metrics = Some(metrics.clone());
                let quality = ((metrics.frames_decoded % 60) as f32 / 60.0).clamp(0.0, 1.0);
                self.push_quality_sample((0.7 + quality).min(1.0));
            }
        } else {
            self.add_system_message(format!(
                "📹 Video frame on channel {} ({} bytes)",
                channel_id,
                data.len()
            ));
        }
        Ok(())
    }

    fn process_error_frame(&mut self, envelope: ControlEnvelope) {
        let mut title = "Protocol error".to_string();
        let mut detail = String::new();
        if let Some(obj) = envelope.properties.as_object() {
            if let Some(t) = obj.get("title").and_then(|v| v.as_str()) {
                title = t.to_string();
            }
            if let Some(d) = obj.get("detail").and_then(|v| v.as_str()) {
                detail = d.to_string();
            }
        }
        self.add_notification(format!("❌ {} {}", title, detail), NotificationLevel::Error);
        self.add_system_message(ascii_art::CAT_ERROR.trim().to_string());
    }

    fn ensure_channel(&mut self, channel_id: u64) -> usize {
        if let Some(idx) = self.channels.iter().position(|c| c.id == channel_id) {
            idx
        } else {
            let channel = ChannelView {
                id: channel_id,
                name: format!("Channel {}", channel_id),
                members: Vec::new(),
                messages: VecDeque::new(),
                typing: HashMap::new(),
                unread_count: 0,
                is_group: false,
                group_id: None,
            };
            self.channels.push(channel);
            self.channels.len() - 1
        }
    }

    fn push_channel_message(&mut self, idx: usize, entry: MessageEntry) {
        let channel = &mut self.channels[idx];
        channel.messages.push_back(entry);
        while channel.messages.len() > MESSAGE_HISTORY_LIMIT {
            channel.messages.pop_front();
        }
    }

    fn preview_text(&self, text: &str) -> String {
        if text.len() <= 64 {
            text.to_string()
        } else {
            format!("{}…", &text[..64])
        }
    }

    fn short_id(&self, id: &str) -> String {
        let trimmed = id.trim();
        if trimmed.is_empty() {
            return "-".to_string();
        }
        if let Ok(uuid) = Uuid::parse_str(trimmed) {
            return short_hex(&uuid.simple().to_string());
        }
        short_hex(trimmed)
    }

    fn push_quality_sample(&mut self, value: f32) {
        const MAX_SAMPLES: usize = 128;
        let clamped = value.clamp(0.0, 1.0);
        self.call_quality_history.push_back(clamped);
        while self.call_quality_history.len() > MAX_SAMPLES {
            self.call_quality_history.pop_front();
        }
    }
    fn parse_group_role(value: &str) -> GroupRole {
        match value.to_lowercase().as_str() {
            "owner" => GroupRole::Owner,
            "admin" => GroupRole::Admin,
            _ => GroupRole::Member,
        }
    }

    async fn process_input(&mut self, input: String) -> Result<()> {
        if let Some(command) = input.strip_prefix('/') {
            // Process command
            self.process_command(command).await?;
        } else if !input.is_empty() {
            // Send message
            self.send_message(input).await?;
        }
        Ok(())
    }

    async fn process_command(&mut self, command: &str) -> Result<()> {
        let parts: Vec<&str> = command.split_whitespace().collect();
        if parts.is_empty() {
            return Ok(());
        }

        match parts[0] {
            "connect" => self.connect().await?,
            "disconnect" => self.disconnect().await?,
            "join" => {
                if parts.len() < 2 {
                    self.add_notification(
                        "Usage: /join <channel_id> [relay]".to_string(),
                        NotificationLevel::Warning,
                    );
                } else {
                    match parts[1].parse::<u64>() {
                        Ok(channel_id) => {
                            let relay = parts
                                .get(2)
                                .map(|value| value.eq_ignore_ascii_case("relay"))
                                .unwrap_or(true);
                            self.join_channel(channel_id, relay).await?;
                        }
                        Err(_) => {
                            self.add_notification(
                                format!("Invalid channel id: {}", parts[1]),
                                NotificationLevel::Error,
                            );
                        }
                    }
                }
            }
            "leave" => {
                if parts.len() < 2 {
                    self.add_notification(
                        "Usage: /leave <channel_id>".to_string(),
                        NotificationLevel::Warning,
                    );
                } else {
                    match parts[1].parse::<u64>() {
                        Ok(channel_id) => self.leave_channel(channel_id).await?,
                        Err(_) => {
                            self.add_notification(
                                format!("Invalid channel id: {}", parts[1]),
                                NotificationLevel::Error,
                            );
                        }
                    }
                }
            }
            "presence" => {
                if parts.len() < 2 {
                    self.add_notification(
                        "Usage: /presence <state>".to_string(),
                        NotificationLevel::Warning,
                    );
                } else {
                    let state = parts[1..].join(" ");
                    self.update_presence(state).await?;
                }
            }
            "theme" => {
                self.theme = match self.theme {
                    Theme::Dark => Theme::Light,
                    Theme::Light => Theme::Cyberpunk,
                    Theme::Cyberpunk => Theme::Kawaii,
                    Theme::Kawaii => Theme::Dark,
                };
                self.add_notification(
                    format!("Theme changed to {:?}", self.theme),
                    NotificationLevel::Info,
                );
            }
            "group" => self.handle_group_command(&parts[1..]).await?,
            "assist" => {
                if parts.len() < 2 {
                    self.add_notification(
                        "Usage: /assist <peer_hint>".to_string(),
                        NotificationLevel::Warning,
                    );
                } else {
                    self.request_p2p_assist(parts[1]).await?;
                }
            }
            "quit" | "exit" => self.should_quit = true,
            _ => {
                self.add_notification(
                    format!("Unknown command: {}", command),
                    NotificationLevel::Warning,
                );
            }
        }

        Ok(())
    }

    async fn handle_group_command(&mut self, args: &[&str]) -> Result<()> {
        if args.is_empty() {
            self.add_notification(
                "Usage: /group <invite|remove|grant> ...".to_string(),
                NotificationLevel::Warning,
            );
            return Ok(());
        }

        match args[0] {
            "invite" => {
                if args.len() < 3 {
                    self.add_notification(
                        "Usage: /group invite <group_id> <device_id> [role]".to_string(),
                        NotificationLevel::Warning,
                    );
                    return Ok(());
                }
                let group_id = args[1];
                let device = args[2];
                let role = args
                    .get(3)
                    .map(|value| Self::parse_group_role(value))
                    .unwrap_or(GroupRole::Member);
                if let Some(group) = self.groups.get_mut(group_id) {
                    if !group.has_permission(&self.state.device_id, GroupAction::Invite) {
                        self.add_notification(
                            format!(
                                "You lack invite permission in group {}",
                                short_hex(group_id)
                            ),
                            NotificationLevel::Warning,
                        );
                        return Ok(());
                    }
                    if group.add_member(device.to_string(), role.clone()) {
                        for channel in self
                            .channels
                            .iter_mut()
                            .filter(|ch| ch.group_id.as_deref() == Some(group_id))
                        {
                            if !channel.members.contains(&device.to_string()) {
                                channel.members.push(device.to_string());
                            }
                        }
                        self.add_notification(
                            format!(
                                "Invited {} to {} as {:?}",
                                self.get_friend_display_name(device),
                                short_hex(group_id),
                                role
                            ),
                            NotificationLevel::Success,
                        );
                    } else {
                        self.add_notification(
                            format!(
                                "{} is already a member of {}",
                                self.get_friend_display_name(device),
                                short_hex(group_id)
                            ),
                            NotificationLevel::Info,
                        );
                    }
                } else {
                    self.add_notification(
                        format!("Unknown group {}", short_hex(group_id)),
                        NotificationLevel::Warning,
                    );
                }
            }
            "remove" => {
                if args.len() < 3 {
                    self.add_notification(
                        "Usage: /group remove <group_id> <device_id>".to_string(),
                        NotificationLevel::Warning,
                    );
                    return Ok(());
                }
                let group_id = args[1];
                let device = args[2];
                if let Some(group) = self.groups.get_mut(group_id) {
                    if !group.has_permission(&self.state.device_id, GroupAction::Kick) {
                        self.add_notification(
                            format!("You lack kick permission in group {}", short_hex(group_id)),
                            NotificationLevel::Warning,
                        );
                        return Ok(());
                    }
                    if group.remove_member(device) {
                        for channel in self
                            .channels
                            .iter_mut()
                            .filter(|ch| ch.group_id.as_deref() == Some(group_id))
                        {
                            channel.members.retain(|member| member != device);
                        }
                        self.add_notification(
                            format!(
                                "Removed {} from {}",
                                self.get_friend_display_name(device),
                                short_hex(group_id)
                            ),
                            NotificationLevel::Success,
                        );
                    } else {
                        self.add_notification(
                            format!(
                                "{} is not in {}",
                                self.get_friend_display_name(device),
                                short_hex(group_id)
                            ),
                            NotificationLevel::Info,
                        );
                    }
                } else {
                    self.add_notification(
                        format!("Unknown group {}", short_hex(group_id)),
                        NotificationLevel::Warning,
                    );
                }
            }
            "grant" => {
                if args.len() < 4 {
                    self.add_notification(
                        "Usage: /group grant <group_id> <device_id> <role>".to_string(),
                        NotificationLevel::Warning,
                    );
                    return Ok(());
                }
                let group_id = args[1];
                let device = args[2];
                let role = Self::parse_group_role(args[3]);
                if let Some(group) = self.groups.get_mut(group_id) {
                    if !group.has_permission(&self.state.device_id, GroupAction::ChangeRole) {
                        self.add_notification(
                            format!("You lack role permissions in {}", short_hex(group_id)),
                            NotificationLevel::Warning,
                        );
                        return Ok(());
                    }
                    if group.change_role(device, role.clone()) {
                        self.add_notification(
                            format!(
                                "{} is now {:?} in {}",
                                self.get_friend_display_name(device),
                                role,
                                short_hex(group_id)
                            ),
                            NotificationLevel::Success,
                        );
                    } else {
                        self.add_notification(
                            format!(
                                "Unable to change role for {}",
                                self.get_friend_display_name(device)
                            ),
                            NotificationLevel::Warning,
                        );
                    }
                } else {
                    self.add_notification(
                        format!("Unknown group {}", short_hex(group_id)),
                        NotificationLevel::Warning,
                    );
                }
            }
            _ => {
                self.add_notification(
                    "Usage: /group <invite|remove|grant>".to_string(),
                    NotificationLevel::Warning,
                );
            }
        }

        Ok(())
    }

    async fn request_p2p_assist(&mut self, peer_hint: &str) -> Result<()> {
        let Some(client) = self.rest_client.clone() else {
            self.add_notification(
                "REST client unavailable".to_string(),
                NotificationLevel::Warning,
            );
            return Ok(());
        };
        let Some(session) = self.session_id.clone() else {
            self.add_notification(
                "No active session for assist".to_string(),
                NotificationLevel::Warning,
            );
            return Ok(());
        };

        let request = P2pAssistRequest {
            peer_hint: Some(peer_hint.to_string()),
            paths: vec![AssistPathHint {
                address: Some("127.0.0.1".to_string()),
                id: Some(format!("hint-{}", short_hex(peer_hint))),
                port: Some(3478),
                server_name: Some(self.state.server_url.clone()),
                priority: Some(1),
                ..Default::default()
            }],
            prefer_reality: Some(true),
            fec: Some(AssistFecHint {
                mtu: Some(1200),
                repair_overhead: Some(0.18),
            }),
            min_paths: Some(1),
        };

        match client.p2p_assist(&session, &request).await {
            Ok(response) => {
                self.handle_assist_response(peer_hint, response);
            }
            Err(err) => {
                self.add_notification(
                    format!("Assist request failed: {}", err),
                    NotificationLevel::Error,
                );
            }
        }

        Ok(())
    }

    fn handle_assist_response(&mut self, peer_hint: &str, response: P2pAssistResponse) {
        let transport_summary = if response.transports.is_empty() {
            "No transports suggested".to_string()
        } else {
            response
                .transports
                .iter()
                .map(|t| {
                    format!(
                        "{} via {} ({} · {} · {})",
                        t.path_id, t.transport, t.resistance, t.latency, t.throughput
                    )
                })
                .collect::<Vec<_>>()
                .join(" | ")
        };

        let (total_samples, repair_samples) = response
            .multipath
            .sample_segments
            .values()
            .fold((0usize, 0usize), |(total, repair), seg| {
                (total + seg.total, repair + seg.repair)
            });

        let fingerprint = response
            .obfuscation
            .reality_fingerprint_hex
            .as_deref()
            .map(short_hex)
            .unwrap_or_else(|| "-".to_string());

        let notification = format!(
            "Assist {} · {} transports · primary {} · MTU {} ({:.0}% FEC)",
            short_hex(peer_hint),
            response.transports.len(),
            response
                .multipath
                .primary_path
                .clone()
                .unwrap_or_else(|| "none".to_string()),
            response.multipath.fec_mtu,
            response.multipath.fec_overhead * 100.0,
        );
        self.add_notification(notification, NotificationLevel::Success);

        let noise_summary = format!(
            "Noise {} prologue {} static {} seed {}",
            response.noise.pattern,
            short_hex(&response.noise.prologue_hex),
            short_hex(&response.noise.static_public_hex),
            short_hex(&response.noise.device_seed_hex)
        );

        let pq_summary = format!(
            "PQ id {} signed {} kem {} sig {}",
            short_hex(&response.pq.identity_public_hex),
            short_hex(&response.pq.signed_prekey_public_hex),
            short_hex(&response.pq.kem_public_hex),
            short_hex(&response.pq.signature_public_hex)
        );

        let obfuscation_summary = format!(
            "Obfuscation fingerprint {} fronting {} mimicry {} tor {}",
            fingerprint,
            response.obfuscation.domain_fronting,
            response.obfuscation.protocol_mimicry,
            response.obfuscation.tor_bridge
        );

        let sample_summary = format!(
            "Samples total={} repair={} across {} paths",
            total_samples,
            repair_samples,
            response.multipath.sample_segments.len()
        );

        let security = &response.security;
        let security_summary = format!(
            "Security: noise={} pq={} fec={} sessions={} paths={:.1} deflections={}",
            security.noise_handshakes,
            security.pq_handshakes,
            security.fec_packets,
            security.multipath_sessions,
            security.average_paths,
            security.censorship_deflections
        );

        self.add_system_message(format!(
            "Assist guidance:\n{}\n{}\n{}\n{}\n{}\n{}",
            noise_summary,
            pq_summary,
            obfuscation_summary,
            transport_summary,
            sample_summary,
            security_summary
        ));
    }

    async fn connect(&mut self) -> Result<()> {
        self.add_notification("Connecting...".to_string(), NotificationLevel::Info);
        self.engine
            .send(EngineCommand::Connect(Box::new(self.state.clone())))
            .await?;
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<()> {
        self.add_notification("Disconnecting...".to_string(), NotificationLevel::Info);
        self.engine.send(EngineCommand::Disconnect).await?;
        Ok(())
    }

    async fn join_channel(&mut self, channel_id: u64, relay: bool) -> Result<()> {
        if !self.connected {
            self.add_notification("Not connected".to_string(), NotificationLevel::Warning);
            return Ok(());
        }

        self.engine
            .send(EngineCommand::Join {
                channel_id,
                members: vec![self.state.device_id.clone()],
                relay,
            })
            .await?;
        self.add_notification(
            format!("Joined channel {}", channel_id),
            NotificationLevel::Success,
        );
        Ok(())
    }

    async fn leave_channel(&mut self, channel_id: u64) -> Result<()> {
        if !self.connected {
            self.add_notification("Not connected".to_string(), NotificationLevel::Warning);
            return Ok(());
        }

        self.engine
            .send(EngineCommand::Leave { channel_id })
            .await?;
        self.add_notification(
            format!("Left channel {}", channel_id),
            NotificationLevel::Info,
        );
        Ok(())
    }

    async fn update_presence(&mut self, state: String) -> Result<()> {
        if !self.connected {
            self.add_notification("Not connected".to_string(), NotificationLevel::Warning);
            return Ok(());
        }

        self.engine
            .send(EngineCommand::Presence {
                state: state.clone(),
            })
            .await?;
        self.add_notification(
            format!("Presence updated to {}", state),
            NotificationLevel::Success,
        );
        Ok(())
    }

    async fn refresh_devices(&mut self) -> Result<()> {
        if let (Some(client), Some(session)) = (self.rest_client.clone(), self.session_id.clone()) {
            match client.list_devices(&session).await {
                Ok(devices) => {
                    self.devices = devices;
                    self.add_notification(
                        format!("🔁 Devices synced ({} entries)", self.devices.len()),
                        NotificationLevel::Success,
                    );
                }
                Err(err) => {
                    self.add_notification(
                        format!("Device sync failed: {}", err),
                        NotificationLevel::Error,
                    );
                }
            }
        }
        Ok(())
    }

    async fn send_message(&mut self, text: String) -> Result<()> {
        if !self.connected {
            self.add_notification("Not connected".to_string(), NotificationLevel::Warning);
            return Ok(());
        }

        let channel_group_id = self
            .channels
            .get(self.active_channel)
            .and_then(|channel| channel.group_id.clone());

        if let Some(group_id) = channel_group_id
            && self.groups.get(&group_id).is_some_and(|group| {
                !group.has_permission(&self.state.device_id, GroupAction::SendMessage)
            })
        {
            self.add_notification(
                format!("You lack send permission in group {}", short_hex(&group_id)),
                NotificationLevel::Warning,
            );
            return Ok(());
        }

        let channel = &mut self.channels[self.active_channel];
        let channel_id = channel.id;

        // Add message to local history
        let entry = MessageEntry {
            timestamp: Utc::now(),
            sender: self.state.device_id.clone(),
            content: MessageContent::Text(text.clone()),
            reactions: HashMap::new(),
        };
        channel.messages.push_back(entry);

        // Send via engine
        self.engine
            .send(EngineCommand::SendMessage {
                channel_id,
                body: text.into_bytes(),
            })
            .await?;

        Ok(())
    }

    fn finalize_voice_recording(&mut self) -> Result<()> {
        if self.voice_buffer.is_empty() {
            self.add_notification(
                "Voice recording discarded (no audio captured)".to_string(),
                NotificationLevel::Warning,
            );
            return Ok(());
        }

        let frame_count = (self.voice_buffer.len() as u32).div_ceil(160);
        let duration_ms = (frame_count.max(1)) * 20;
        let mut voice = VoiceMessage::new(duration_ms);
        for chunk in self.voice_buffer.chunks(160) {
            voice.add_frame(chunk);
        }

        let bytes = voice.to_bytes()?;
        let restored = VoiceMessage::from_bytes(bytes.as_ref())?;

        let entry = MessageEntry {
            timestamp: Utc::now(),
            sender: self.state.device_id.clone(),
            content: MessageContent::Voice(restored.clone()),
            reactions: HashMap::new(),
        };
        self.channels[self.active_channel].messages.push_back(entry);

        self.add_notification(
            format!("🎙️ Voice memo saved ({} frames)", restored.frames.len()),
            NotificationLevel::Success,
        );

        self.voice_buffer.clear();
        Ok(())
    }

    fn add_system_message(&mut self, message: String) {
        let system_channel = &mut self.channels[0];
        let entry = MessageEntry {
            timestamp: Utc::now(),
            sender: "System".to_string(),
            content: MessageContent::System(message),
            reactions: HashMap::new(),
        };
        system_channel.messages.push_back(entry);

        // Limit history
        while system_channel.messages.len() > MESSAGE_HISTORY_LIMIT {
            system_channel.messages.pop_front();
        }
    }
}

impl ChannelView {
    fn system() -> Self {
        ChannelView {
            id: 0,
            name: "System".to_string(),
            members: vec![],
            messages: VecDeque::new(),
            typing: HashMap::new(),
            unread_count: 0,
            is_group: false,
            group_id: None,
        }
    }
}

// Terminal helpers
fn prepare_terminal() -> Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let terminal = Terminal::new(backend)?;
    Ok(terminal)
}

fn restore_terminal(mut terminal: Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
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
        terminal.set_cursor(x, y)?;
    }
    Ok(())
}

pub async fn run_tui(state: ClientState) -> Result<()> {
    run_enhanced_tui(state).await
}

pub async fn run_enhanced_tui(state: ClientState) -> Result<()> {
    let (engine, events) = create_engine(ENGINE_COMMAND_BUFFER, ENGINE_EVENT_BUFFER);
    let mut app = EnhancedApp::new(state, engine, events);
    app.run().await
}
