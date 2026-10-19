//! The settings window: what opens when `wikelo-watcher` runs without arguments.
//!
//! One screen with everything a player needs: the account on the server (sign in / out),
//! start / stop of the background watcher, the game channel it follows, "start with
//! Windows", the game-data upload switch, and what the watcher is doing right now. The
//! window never does the watching itself: it drives the same background process the
//! command line uses and reads the small status file that process keeps current.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, RichText};

use crate::agent::{self, AgentConfig, DaemonStatus};

const ACCENT: Color32 = Color32::from_rgb(0x3a, 0xc0, 0xe6);
const OK: Color32 = Color32::from_rgb(0x4c, 0xd1, 0x7a);
const WARN: Color32 = Color32::from_rgb(0xe8, 0xb0, 0x4a);
const BAD: Color32 = Color32::from_rgb(0xe8, 0x6a, 0x6a);
const MUTED: Color32 = Color32::from_rgb(0x8a, 0x97, 0xa6);
/// Lines of the watcher log shown in the window.
const LOG_TAIL_BYTES: u64 = 4000;
/// Window height: just the sign-in box, or every panel once signed in.
const SIZE_SIGNED_OUT: [f32; 2] = [560.0, 330.0];
const SIZE_SIGNED_IN: [f32; 2] = [560.0, 700.0];

/// Result of a background action (network or waiting for the process).
enum Done {
    /// server, handle, token, display name
    Login(Result<(String, String, String, String), String>),
    /// Saved session re-checked at start: display name when still valid.
    Session(Result<Option<String>, String>),
    Started(Result<u32, String>),
    Stopped(Result<bool, String>),
    Autostart(bool, Result<(), String>),
    LoggedOut,
    /// A newer release the server knows of: (version, download page).
    Update(Option<(String, String)>),
}

pub fn run() -> anyhow::Result<()> {
    let signed_in = agent::load_config().is_some_and(|c| !c.token.is_empty() && !c.server.is_empty());
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Wikelist Watcher")
            .with_inner_size(if signed_in { SIZE_SIGNED_IN } else { SIZE_SIGNED_OUT })
            .with_min_inner_size([480.0, 300.0])
            .with_icon(std::sync::Arc::new(icon())),
        centered: true,
        ..Default::default()
    };
    eframe::run_native(
        "Wikelo Watcher",
        options,
        Box::new(|cc| {
            setup_style(&cc.egui_ctx);
            Ok(Box::new(App::new()))
        }),
    )
    .map_err(|e| anyhow::anyhow!("não foi possível abrir a janela: {e}"))
}

/// A simple icon drawn in code: dark disc with a cyan ring.
fn icon() -> egui::IconData {
    const N: usize = 32;
    let mut rgba = vec![0u8; N * N * 4];
    let c = (N as f32 - 1.0) / 2.0;
    for y in 0..N {
        for x in 0..N {
            let d = ((x as f32 - c).powi(2) + (y as f32 - c).powi(2)).sqrt();
            let px = &mut rgba[(y * N + x) * 4..(y * N + x) * 4 + 4];
            if d <= 15.0 {
                let ring = (11.0..=14.0).contains(&d);
                let inner = d <= 5.0;
                let (r, g, b) = if ring || inner { (0x3a, 0xc0, 0xe6) } else { (0x0b, 0x12, 0x1a) };
                px.copy_from_slice(&[r, g, b, 255]);
            }
        }
    }
    egui::IconData { rgba, width: N as u32, height: N as u32 }
}

fn setup_style(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    v.panel_fill = Color32::from_rgb(0x0b, 0x12, 0x1a);
    v.window_fill = v.panel_fill;
    v.extreme_bg_color = Color32::from_rgb(0x07, 0x0c, 0x12);
    v.faint_bg_color = Color32::from_rgb(0x0e, 0x17, 0x21);
    v.widgets.noninteractive.bg_fill = Color32::from_rgb(0x0e, 0x17, 0x21);
    v.widgets.noninteractive.bg_stroke.color = Color32::from_rgb(0x1f, 0x2f, 0x40);
    v.widgets.noninteractive.fg_stroke.color = Color32::from_rgb(0xd6, 0xde, 0xe8);
    v.widgets.inactive.bg_fill = Color32::from_rgb(0x15, 0x22, 0x30);
    v.widgets.inactive.weak_bg_fill = Color32::from_rgb(0x15, 0x22, 0x30);
    v.widgets.inactive.fg_stroke.color = Color32::from_rgb(0xd6, 0xde, 0xe8);
    v.widgets.hovered.bg_fill = Color32::from_rgb(0x1c, 0x2e, 0x40);
    v.widgets.hovered.weak_bg_fill = Color32::from_rgb(0x1c, 0x2e, 0x40);
    v.widgets.hovered.bg_stroke.color = ACCENT;
    v.widgets.active.bg_fill = Color32::from_rgb(0x15, 0x4a, 0x60);
    v.widgets.active.weak_bg_fill = Color32::from_rgb(0x15, 0x4a, 0x60);
    v.selection.bg_fill = Color32::from_rgb(0x12, 0x4a, 0x5e);
    v.selection.stroke.color = ACCENT;
    v.hyperlink_color = ACCENT;
    ctx.set_theme(egui::Theme::Dark);
    ctx.set_visuals_of(egui::Theme::Dark, v);
    ctx.style_mut(|s| {
        s.spacing.item_spacing = egui::vec2(8.0, 6.0);
        s.spacing.button_padding = egui::vec2(12.0, 5.0);
        s.spacing.interact_size.y = 26.0;
    });
}

struct App {
    cfg: AgentConfig,
    server_in: String,
    handle_in: String,
    password_in: String,
    installs: Vec<wikelo_install::InstallInfo>,
    running: Option<u32>,
    autostart: bool,
    status: Option<DaemonStatus>,
    log_tail: String,
    last_poll: Instant,
    /// What a background action is doing right now (disables the buttons).
    busy: Option<String>,
    /// (is error, text) shown under the account box.
    msg: Option<(bool, String)>,
    /// A newer release than this executable, as the daemon or the start-up check reported it.
    update: Option<(String, String)>,
    tx: Sender<Done>,
    rx: Receiver<Done>,
    /// Signed-in state the window is currently sized for.
    sized_for: bool,
}

impl App {
    fn new() -> Self {
        let (tx, rx) = channel();
        let cfg = agent::load_config().unwrap_or_default();
        let sized_for = !cfg.token.is_empty() && !cfg.server.is_empty();
        let mut app = Self {
            server_in: cfg.server.clone(),
            handle_in: cfg.handle.clone(),
            password_in: String::new(),
            cfg,
            installs: wikelo_install::discover_installs(&[]).into_iter().filter(|i| i.game_log_path.exists() || i.has_p4k()).collect(),
            running: None,
            autostart: agent::autostart_enabled(),
            status: None,
            log_tail: String::new(),
            last_poll: Instant::now() - Duration::from_secs(10),
            busy: None,
            msg: None,
            update: None,
            tx,
            rx,
            sized_for,
        };
        app.poll();
        if app.signed_in() {
            // Saved session: make sure the server still accepts it, and ask it for the newest
            // release while there (the daemon does the same daily; this covers a stopped one).
            let cfg = app.cfg.clone();
            let tx = app.tx.clone();
            app.busy = Some("verificando a sessão…".into());
            std::thread::spawn(move || {
                let _ = tx.send(Done::Session(agent::session_name(&cfg).map_err(|e| format!("{e:?}"))));
                let _ = tx.send(Done::Update(agent::update_available(&cfg)));
            });
        }
        app
    }

    fn signed_in(&self) -> bool {
        !self.cfg.token.is_empty() && !self.cfg.server.is_empty()
    }

    /// Re-read what the background process left on disk (cheap: three small files).
    fn poll(&mut self) {
        self.last_poll = Instant::now();
        self.running = agent::running_pid();
        self.status = if self.running.is_some() { agent::read_status() } else { None };
        if let Some(s) = &self.status
            && let (Some(v), Some(u)) = (&s.latest_version, &s.update_url)
        {
            self.update = Some((v.clone(), u.clone()));
        }
        self.log_tail = tail(&agent::log_path(), LOG_TAIL_BYTES);
    }

    fn drain(&mut self) {
        while let Ok(done) = self.rx.try_recv() {
            self.busy = None;
            match done {
                Done::Login(Ok((server, handle, token, name))) => {
                    self.cfg.server = server;
                    self.cfg.handle = handle;
                    self.cfg.token = token;
                    self.cfg.name = Some(name.clone());
                    if self.cfg.share_data.is_none() {
                        self.cfg.share_data = Some(true);
                    }
                    self.password_in.clear();
                    self.msg = None;
                    if let Err(e) = agent::save_config(&self.cfg) {
                        self.msg = Some((true, format!("não deu para gravar a configuração: {e}")));
                    } else if self.running.is_none() {
                        self.start();
                    }
                }
                Done::Login(Err(e)) => self.msg = Some((true, e)),
                Done::Session(Ok(Some(name))) => {
                    if self.cfg.name.as_deref() != Some(&name) {
                        self.cfg.name = Some(name);
                        let _ = agent::save_config(&self.cfg);
                    }
                }
                Done::Session(Ok(None)) => {
                    self.cfg.token.clear();
                    self.msg = Some((true, "a sessão expirou; entre de novo".into()));
                }
                Done::Session(Err(e)) => self.msg = Some((true, format!("sem contato com {}: {e}", self.cfg.server))),
                Done::Started(Ok(_)) => self.msg = None,
                Done::Started(Err(e)) => self.msg = Some((true, e)),
                Done::Stopped(Ok(_)) => {}
                Done::Stopped(Err(e)) => self.msg = Some((true, e)),
                Done::Autostart(on, Ok(())) => self.autostart = on,
                Done::Autostart(_, Err(e)) => {
                    self.autostart = agent::autostart_enabled();
                    self.msg = Some((true, e));
                }
                Done::LoggedOut => {
                    self.cfg = AgentConfig { server: self.cfg.server.clone(), handle: self.cfg.handle.clone(), ..Default::default() };
                    self.msg = None;
                }
                Done::Update(u) => {
                    if u.is_some() {
                        self.update = u;
                    }
                    // Not a user action: the spinner belongs to the session check that preceded it.
                    continue;
                }
            }
            self.poll();
        }
    }

    fn spawn(&mut self, label: &str, work: impl FnOnce() -> Done + Send + 'static) {
        self.busy = Some(label.to_string());
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(work());
        });
    }

    fn login(&mut self) {
        let server = agent::normalize_server(&self.server_in);
        let handle = self.handle_in.trim().to_string();
        let password = self.password_in.clone();
        if server.is_empty() || handle.is_empty() || password.is_empty() {
            self.msg = Some((true, "preencha servidor, usuário e senha".into()));
            return;
        }
        self.msg = None;
        self.spawn("entrando…", move || Done::Login(agent::login(&server, &handle, &password).map(|(token, name)| (server, handle, token, name)).map_err(|e| format!("{e:#}"))));
    }

    fn start(&mut self) {
        self.spawn("iniciando…", || Done::Started(agent::start_background().map_err(|e| format!("{e:#}"))));
    }

    fn stop(&mut self) {
        self.spawn("parando…", || Done::Stopped(agent::stop_background().map_err(|e| format!("{e:#}"))));
    }

    fn restart(&mut self) {
        self.spawn("reiniciando…", || {
            if let Err(e) = agent::stop_background() {
                return Done::Started(Err(format!("{e:#}")));
            }
            Done::Started(agent::start_background().map_err(|e| format!("{e:#}")))
        });
    }

    fn logout(&mut self) {
        self.spawn("saindo…", || {
            let _ = agent::stop_background();
            let _ = std::fs::remove_file(agent::config_path());
            Done::LoggedOut
        });
    }

    fn set_autostart(&mut self, on: bool) {
        self.autostart = on;
        self.spawn("gravando…", move || Done::Autostart(on, agent::set_autostart(on).map_err(|e| format!("{e:#}"))));
    }

    fn save(&mut self) {
        if let Err(e) = agent::save_config(&self.cfg) {
            self.msg = Some((true, format!("não deu para gravar a configuração: {e}")));
        }
    }

    // ----- panels -------------------------------------------------------------------

    fn header(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("WIKELO WATCHER").color(ACCENT).strong().size(18.0));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if let Some(b) = &self.busy {
                    ui.spinner();
                    ui.label(RichText::new(b.clone()).color(MUTED));
                } else if self.running.is_some() {
                    if ui.button("Parar").clicked() {
                        self.stop();
                    }
                    ui.label(RichText::new("● rodando em segundo plano").color(OK));
                } else {
                    if ui.add_enabled(self.signed_in(), egui::Button::new("Iniciar")).clicked() {
                        self.start();
                    }
                    ui.label(RichText::new("○ parado").color(MUTED));
                }
            });
        });
        if let Some((version, url)) = self.update.clone() {
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("Versão {version} do wikelo-watcher disponível (esta é a {})", wikelo_core::PKG_VERSION)).color(WARN));
                if ui.button("Abrir download").on_hover_text(url.clone()).clicked() {
                    let _ = open::that_detached(&url);
                }
            });
        }
    }

    fn account(&mut self, ui: &mut egui::Ui) {
        let signed_in = self.signed_in();
        group(ui, "Conta", |ui| {
            if signed_in {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(self.cfg.display_name()).strong());
                    ui.label(RichText::new(format!("em {}", self.cfg.server)).color(MUTED));
                });
                ui.horizontal(|ui| {
                    if ui.button("Abrir o site").clicked() {
                        let _ = open::that_detached(&self.cfg.server);
                    }
                    if ui.add_enabled(self.busy.is_none(), egui::Button::new("Sair da conta")).on_hover_text("Para o watcher e esquece a sessão neste PC").clicked() {
                        self.logout();
                    }
                });
            } else {
                let mut submit = false;
                egui::Grid::new("login").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                    ui.label("Servidor");
                    let r = ui.add(egui::TextEdit::singleline(&mut self.server_in).hint_text("https://wikelo.exemplo.com").desired_width(f32::INFINITY));
                    submit |= r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    ui.end_row();
                    ui.label("Usuário");
                    let r = ui.add(egui::TextEdit::singleline(&mut self.handle_in).hint_text("o mesmo do site").desired_width(f32::INFINITY));
                    submit |= r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    ui.end_row();
                    ui.label("Senha");
                    let r = ui.add(egui::TextEdit::singleline(&mut self.password_in).password(true).desired_width(f32::INFINITY));
                    submit |= r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    ui.end_row();
                });
                ui.horizontal(|ui| {
                    let can = self.busy.is_none() && !self.server_in.trim().is_empty() && !self.handle_in.trim().is_empty() && !self.password_in.is_empty();
                    if ui.add_enabled(can, egui::Button::new("Entrar")).clicked() || (submit && can) {
                        self.login();
                    }
                    ui.label(RichText::new("A conta é a mesma do site. O watcher começa assim que você entrar.").color(MUTED).small());
                });
            }
            if let Some((is_err, text)) = &self.msg {
                ui.label(RichText::new(text).color(if *is_err { BAD } else { OK }));
            }
        });
    }

    fn game(&mut self, ui: &mut egui::Ui) {
        group(ui, "Jogo", |ui| {
            if self.installs.is_empty() {
                ui.label(RichText::new("Nenhuma instalação do Star Citizen foi encontrada neste PC.").color(WARN));
                ui.label(RichText::new("O watcher precisa rodar no PC onde o jogo está instalado.").color(MUTED).small());
                return;
            }
            let current = self
                .cfg
                .channel
                .clone()
                .or_else(|| self.installs.iter().find(|i| i.channel == "LIVE").or(self.installs.first()).map(|i| i.channel.clone()))
                .unwrap_or_default();
            let mut selected = current.clone();
            ui.horizontal(|ui| {
                ui.label("Canal");
                egui::ComboBox::from_id_salt("canal").selected_text(&selected).show_ui(ui, |ui| {
                    for i in &self.installs {
                        ui.selectable_value(&mut selected, i.channel.clone(), format!("{} · {}", i.channel, wikelo_core::version_label(&i.manifest.branch, &i.manifest.version)));
                    }
                });
                if let Some(i) = self.installs.iter().find(|i| i.channel == selected) {
                    ui.label(RichText::new(format!("versão {} ({})", wikelo_core::version_label(&i.manifest.branch, &i.manifest.version), i.manifest.version)).color(MUTED));
                }
            });
            if let Some(i) = self.installs.iter().find(|i| i.channel == selected) {
                ui.label(RichText::new(i.root.display().to_string()).color(MUTED).small());
            }
            if selected != current {
                self.cfg.channel = Some(selected);
                self.save();
                if self.running.is_some() {
                    self.restart();
                }
            }
        });
    }

    fn options(&mut self, ui: &mut egui::Ui) {
        group(ui, "Opções", |ui| {
            let mut auto = self.autostart;
            if ui.add_enabled(self.busy.is_none(), egui::Checkbox::new(&mut auto, "Iniciar junto com o Windows")).changed() {
                self.set_autostart(auto);
            }
            let mut share = self.cfg.share_data.unwrap_or(true);
            if ui.checkbox(&mut share, "Enviar também os dados do jogo quando sair uma versão nova").changed() {
                self.cfg.share_data = Some(share);
                self.save();
            }
            ui.label(
                RichText::new(
                    "As missões do Game.log são enviadas sempre que o watcher está rodando; isso não muda. \
                     Esta opção é só para os contratos e imagens do jogo: uma vez por versão nova, com o jogo fechado, uns segundos.",
                )
                .color(MUTED)
                .small(),
            );
            ui.horizontal(|ui| {
                if ui.add_enabled(self.running.is_some() && self.busy.is_none(), egui::Button::new("Verificar versões agora")).clicked() {
                    let _ = agent::request_data_check();
                }
                if let Some(s) = self.status.as_ref().and_then(|s| s.data_check.clone()) {
                    ui.label(RichText::new(s).color(MUTED).small());
                }
            });
        });
    }

    fn activity(&mut self, ui: &mut egui::Ui) {
        group(ui, "Atividade", |ui| {
            match (&self.running, &self.status) {
                (None, _) => {
                    ui.label(RichText::new("O watcher está parado. Nada é lido nem enviado.").color(MUTED));
                }
                (Some(_), None) => {
                    ui.label(RichText::new("iniciando…").color(MUTED));
                }
                (Some(_), Some(s)) => {
                    ui.horizontal_wrapped(|ui| {
                        if s.paused {
                            ui.label(RichText::new("pausado pelo site").color(WARN));
                        } else if let Some(e) = &s.last_error {
                            ui.label(RichText::new(e).color(BAD));
                        } else if s.last_ok.is_some() {
                            ui.label(RichText::new("conectado ao servidor").color(OK));
                        } else {
                            ui.label(RichText::new("conectando…").color(MUTED));
                        }
                        ui.label(RichText::new(format!("· canal {}", s.channel)).color(MUTED));
                        if let Some(t) = &s.last_ok {
                            ui.label(RichText::new(format!("· último contato {}", hhmmss(t))).color(MUTED));
                        }
                    });
                    if let Some(e) = &s.last_event {
                        ui.label(RichText::new(e).small());
                    }
                }
            }
            egui::Frame::new().fill(ui.visuals().extreme_bg_color).corner_radius(4.0).inner_margin(6.0).show(ui, |ui| {
                ui.set_width(ui.available_width());
                egui::ScrollArea::vertical().max_height(170.0).stick_to_bottom(true).auto_shrink([false, false]).show(ui, |ui| {
                    let text = if self.log_tail.trim().is_empty() { "(sem registros ainda)".to_string() } else { self.log_tail.clone() };
                    ui.add(egui::Label::new(RichText::new(text).monospace().size(11.0).color(Color32::from_rgb(0xb8, 0xc2, 0xcf))).wrap());
                });
            });
            ui.horizontal(|ui| {
                if ui.button("Abrir o registro completo").clicked() {
                    let _ = open::that_detached(agent::log_path());
                }
                ui.label(RichText::new(agent::log_path().display().to_string()).color(MUTED).small());
            });
        });
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain();
        if self.last_poll.elapsed() > Duration::from_secs(1) {
            self.poll();
        }
        if self.signed_in() != self.sized_for {
            self.sized_for = self.signed_in();
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::Vec2::from(if self.sized_for { SIZE_SIGNED_IN } else { SIZE_SIGNED_OUT })));
        }
        ctx.request_repaint_after(Duration::from_secs(1));
        egui::CentralPanel::default().frame(egui::Frame::new().fill(ctx.style().visuals.panel_fill).inner_margin(14.0)).show(ctx, |ui| {
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                self.header(ui);
                ui.add_space(6.0);
                self.account(ui);
                if self.signed_in() {
                    self.game(ui);
                    self.options(ui);
                    self.activity(ui);
                }
            });
        });
    }
}

/// Framed box with an uppercase eyebrow title, like the panels on the site.
fn group(ui: &mut egui::Ui, title: &str, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::group(ui.style()).inner_margin(10.0).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(RichText::new(title.to_uppercase()).color(MUTED).small().strong());
        add(ui);
    });
}

/// `12:34:56` from an RFC 3339 timestamp (local time).
fn hhmmss(rfc3339: &str) -> String {
    let Ok(t) = time::OffsetDateTime::parse(rfc3339, &time::format_description::well_known::Rfc3339) else { return rfc3339.to_string() };
    let t = time::UtcOffset::current_local_offset().map(|o| t.to_offset(o)).unwrap_or(t);
    format!("{:02}:{:02}:{:02}", t.hour(), t.minute(), t.second())
}

/// Last `max` bytes of a text file, starting at a line boundary.
fn tail(path: &std::path::Path, max: u64) -> String {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(path) else { return String::new() };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let start = len.saturating_sub(max);
    if f.seek(SeekFrom::Start(start)).is_err() {
        return String::new();
    }
    let mut buf = Vec::new();
    let _ = f.read_to_end(&mut buf);
    let mut s = String::from_utf8_lossy(&buf).to_string();
    if start > 0
        && let Some(nl) = s.find('\n')
    {
        s = s[nl + 1..].to_string();
    }
    s.replace("\r\n", "\n").trim_end().to_string()
}
