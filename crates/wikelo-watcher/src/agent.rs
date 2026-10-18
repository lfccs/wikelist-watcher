//! The watcher proper: tail this PC's `Game.log` and send mission events to the shared
//! server, which applies them to the signed-in account (completions, automatic
//! inventory). Optionally it also keeps the server's game data current: once an hour
//! it compares the installed builds (LIVE, PTU…) with what the server already knows
//! and, when the user allowed it, extracts and uploads a signed bundle for a new one.
//!
//! Runs in the foreground (`watch`) or detached in the background (`start`, which
//! spawns `watch --daemon` without a console; `stop` asks it to quit through a
//! stop file; `status` checks the pid file).

use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};
use wikelo_log::{Tailer, TailerConfig, TrackerOutput};

/// How often the watcher tells the server it is alive when nothing happens.
const HEARTBEAT: Duration = Duration::from_secs(20);
/// How often installed game versions are compared with the server's.
const DATA_CHECK_EVERY: Duration = Duration::from_secs(3600);
/// A Game.log written more recently than this means the game is running.
const GAME_ACTIVE_WINDOW: Duration = Duration::from_secs(120);
const POLL: Duration = Duration::from_millis(750);

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentConfig {
    pub server: String,
    pub token: String,
    pub handle: String,
    /// Display name of the account (shown in the window).
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub channel: Option<String>,
    /// Upload new game versions automatically (`None` = not asked yet).
    #[serde(default)]
    pub share_data: Option<bool>,
}

impl AgentConfig {
    pub fn display_name(&self) -> String {
        self.name.clone().filter(|n| !n.trim().is_empty()).unwrap_or_else(|| self.handle.clone())
    }
}

fn data_dir() -> PathBuf {
    wikelo_watcher::data_dir()
}

/// `<data dir>/watcher.json`.
pub fn config_path() -> PathBuf {
    data_dir().join("watcher.json")
}

pub fn pid_path() -> PathBuf {
    data_dir().join("watcher.pid")
}

pub fn stop_path() -> PathBuf {
    data_dir().join("watcher.stop")
}

/// Dropped by the window to ask the running watcher for a game-data check right now.
pub fn check_path() -> PathBuf {
    data_dir().join("watcher.check")
}

pub fn log_path() -> PathBuf {
    data_dir().join("watcher.log")
}

/// Small JSON the background watcher keeps current for the window.
pub fn status_path() -> PathBuf {
    data_dir().join("watcher.status.json")
}

pub fn load_config() -> Option<AgentConfig> {
    std::fs::read_to_string(config_path()).ok().and_then(|s| serde_json::from_str(&s).ok())
}

pub fn save_config(c: &AgentConfig) -> anyhow::Result<()> {
    let p = config_path();
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    std::fs::write(&p, serde_json::to_string_pretty(c)?)?;
    Ok(())
}

fn config_mtime() -> Option<std::time::SystemTime> {
    std::fs::metadata(config_path()).and_then(|m| m.modified()).ok()
}

/// What the background watcher is doing, for `status` and the window.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DaemonStatus {
    pub pid: u32,
    pub started_at: String,
    pub server: String,
    pub channel: String,
    /// Last successful exchange with the server (RFC 3339).
    pub last_ok: Option<String>,
    /// Paused from the site (Dados → Watcher).
    pub paused: bool,
    pub last_error: Option<String>,
    /// Last mission event seen in the Game.log (one line).
    pub last_event: Option<String>,
    /// Last game-data check result (one line).
    pub data_check: Option<String>,
    pub share_data: bool,
}

impl DaemonStatus {
    fn write(&self) {
        if let Ok(s) = serde_json::to_string(self) {
            let _ = std::fs::write(status_path(), s);
        }
    }
}

pub fn read_status() -> Option<DaemonStatus> {
    std::fs::read_to_string(status_path()).ok().and_then(|s| serde_json::from_str(&s).ok())
}

/// Ask the running watcher to compare game versions with the server now.
pub fn request_data_check() -> anyhow::Result<()> {
    std::fs::write(check_path(), "check")?;
    Ok(())
}

fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc().format(&time::format_description::well_known::Rfc3339).unwrap_or_default()
}

pub fn prompt(label: &str) -> anyhow::Result<String> {
    eprint!("{label}");
    std::io::stderr().flush()?;
    let mut s = String::new();
    std::io::stdin().read_line(&mut s)?;
    Ok(s.trim().to_string())
}

fn yes_no(label: &str, default_yes: bool) -> anyhow::Result<bool> {
    let a = prompt(&format!("{label} [{}] ", if default_yes { "S/n" } else { "s/N" }))?.to_ascii_lowercase();
    Ok(match a.as_str() {
        "" => default_yes,
        "s" | "sim" | "y" | "yes" => true,
        _ => false,
    })
}

/// Local wall clock for the log and the window.
fn now_local() -> time::OffsetDateTime {
    time::OffsetDateTime::now_local().unwrap_or_else(|_| time::OffsetDateTime::now_utc())
}

fn time_now() -> String {
    let now = now_local();
    format!("{:02}:{:02}:{:02}", now.hour(), now.minute(), now.second())
}

macro_rules! say {
    ($($arg:tt)*) => { eprintln!("[{}] {}", time_now(), format!($($arg)*)) };
}

// ----- HTTP client ------------------------------------------------------------------

#[derive(Serialize)]
struct AgentEvent<'a> {
    replay: bool,
    event: &'a TrackerOutput,
}

#[derive(Serialize)]
struct Batch<'a> {
    channel: &'a str,
    host: &'a str,
    share_data: bool,
    events: Vec<AgentEvent<'a>>,
}

#[derive(Deserialize)]
struct Ack {
    #[serde(default = "default_true")]
    enabled: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Deserialize)]
struct LoginResponse {
    token: String,
    account: AccountInfo,
}

#[derive(Deserialize)]
struct AccountInfo {
    handle: String,
    #[serde(rename = "displayName", default)]
    display_name: String,
}

#[derive(Deserialize)]
struct Me {
    account: Option<AccountInfo>,
}

#[derive(Deserialize)]
struct KnownDataset {
    channel: String,
    version: String,
}

#[derive(Deserialize)]
struct KnownDatasets {
    known: Vec<KnownDataset>,
}

#[derive(Deserialize)]
struct UploadReport {
    status: String,
}

#[derive(Debug)]
pub enum SendError {
    Unauthorized,
    /// The server has no dataset for the channel (409).
    NoData,
    /// The server refused the request and said why (4xx with a message).
    Rejected(String),
    Other(String),
}

impl From<ureq::Error> for SendError {
    fn from(e: ureq::Error) -> Self {
        match e {
            ureq::Error::StatusCode(401) => SendError::Unauthorized,
            ureq::Error::StatusCode(409) => SendError::NoData,
            ureq::Error::StatusCode(code) => SendError::Other(format!("HTTP {code}")),
            other => SendError::Other(other.to_string()),
        }
    }
}

fn http() -> ureq::Agent {
    ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(60))).build().into()
}

#[derive(Clone)]
pub struct Client {
    server: String,
    token: String,
    http: ureq::Agent,
}

impl Client {
    pub fn new(server: &str, token: &str) -> Self {
        Self { server: server.trim_end_matches('/').to_string(), token: token.to_string(), http: http() }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.server)
    }

    fn auth(&self) -> String {
        format!("Bearer {}", self.token)
    }

    /// Display name of the signed-in account, `None` when the token is not accepted.
    fn me(&self) -> Result<Option<String>, SendError> {
        if self.token.is_empty() {
            return Ok(None);
        }
        let mut res = self.http.get(self.url("/api/auth/me")).header("Authorization", &self.auth()).call()?;
        let me: Me = res.body_mut().read_json().map_err(|e| SendError::Other(e.to_string()))?;
        Ok(me.account.map(|a| if a.display_name.is_empty() { a.handle } else { a.display_name }))
    }

    fn send(&self, channel: &str, host: &str, share_data: bool, events: &[(bool, TrackerOutput)]) -> Result<Ack, SendError> {
        let body = Batch { channel, host, share_data, events: events.iter().map(|(replay, event)| AgentEvent { replay: *replay, event }).collect() };
        let mut res = self.http.post(self.url("/api/agent/events")).header("Authorization", &self.auth()).send_json(&body)?;
        res.body_mut().read_json().map_err(|e| SendError::Other(e.to_string()))
    }

    fn bye(&self) {
        let _ = self.http.post(self.url("/api/agent/bye")).header("Authorization", &self.auth()).send_empty();
    }

    fn known_datasets(&self) -> Result<Vec<(String, String)>, SendError> {
        let mut res = self.http.get(self.url("/api/agent/datasets")).header("Authorization", &self.auth()).call()?;
        let k: KnownDatasets = res.body_mut().read_json().map_err(|e| SendError::Other(e.to_string()))?;
        Ok(k.known.into_iter().map(|d| (d.channel.to_ascii_uppercase(), d.version)).collect())
    }

    /// Upload a bundle. The server validates it in quarantine; a refusal (4xx) comes back
    /// as `Rejected` with the server's reason.
    fn upload_bundle(&self, bytes: &[u8]) -> Result<UploadReport, SendError> {
        let mut res = self
            .http
            .post(self.url("/api/bundles"))
            .config()
            .http_status_as_error(false)
            .build()
            .header("Authorization", &self.auth())
            .header("Content-Type", "application/zip")
            .send(bytes)?;
        let status = res.status().as_u16();
        if status == 401 {
            return Err(SendError::Unauthorized);
        }
        if status >= 400 {
            let msg = res
                .body_mut()
                .read_json::<serde_json::Value>()
                .ok()
                .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(String::from))
                .unwrap_or_else(|| format!("HTTP {status}"));
            return Err(SendError::Rejected(msg));
        }
        res.body_mut().read_json().map_err(|e| SendError::Other(e.to_string()))
    }
}

/// Exchange handle + password for a long-lived watcher token. Returns `(token, display name)`.
pub fn login(server: &str, handle: &str, password: &str) -> anyhow::Result<(String, String)> {
    let server = normalize_server(server);
    let url = format!("{server}/api/auth/agent-login");
    let mut res = match http().post(&url).send_json(serde_json::json!({ "handle": handle, "password": password })) {
        Ok(r) => r,
        Err(ureq::Error::StatusCode(401)) => bail!("usuário ou senha incorretos"),
        Err(ureq::Error::StatusCode(404)) => bail!("este servidor não tem suporte ao watcher (atualize o servidor)"),
        Err(e) => bail!("não foi possível falar com {server}: {e}"),
    };
    let r: LoginResponse = res.body_mut().read_json().context("resposta inesperada do servidor")?;
    let name = if r.account.display_name.is_empty() { r.account.handle } else { r.account.display_name };
    Ok((r.token, name))
}

/// `wikelo.exemplo.com/` → `https://wikelo.exemplo.com`.
pub fn normalize_server(s: &str) -> String {
    let s = s.trim().trim_end_matches('/').to_string();
    if s.is_empty() || s.starts_with("http://") || s.starts_with("https://") { s } else { format!("https://{s}") }
}

/// Is the saved session still accepted by the server? `Ok(None)` = no / expired.
pub fn session_name(cfg: &AgentConfig) -> Result<Option<String>, SendError> {
    Client::new(&cfg.server, &cfg.token).me()
}

fn interactive_login(server: &str, handle: Option<String>, password: Option<String>) -> anyhow::Result<(String, String, String)> {
    let handle = match handle {
        Some(h) if !h.trim().is_empty() => h.trim().to_string(),
        _ => prompt("Usuário: ")?,
    };
    let password = match password {
        Some(p) => p,
        None => rpassword::prompt_password("Senha: ")?,
    };
    let (token, name) = login(server, &handle, &password)?;
    Ok((token, handle, name))
}

// ----- setup (interactive) ------------------------------------------------------------

pub struct SetupOptions {
    pub server: Option<String>,
    pub channel: Option<String>,
    pub handle: Option<String>,
    pub password: Option<String>,
    /// `Some(x)` sets the automatic data upload without asking.
    pub share_data: Option<bool>,
}

/// Make sure there is a server, a valid session and an install to follow; asks
/// interactively for whatever is missing. Returns the saved config and the account name.
pub fn ensure_setup(opts: SetupOptions, interactive: bool) -> anyhow::Result<(AgentConfig, String)> {
    let mut cfg = load_config().unwrap_or_default();
    if let Some(s) = opts.server {
        let s = s.trim().trim_end_matches('/').to_string();
        if cfg.server != s {
            cfg.token.clear();
        }
        cfg.server = s;
    }
    if cfg.server.is_empty() {
        if !interactive {
            bail!("servidor não configurado; abra o wikelo-watcher ou rode `wikelo-watcher start --server https://...` uma vez");
        }
        cfg.server = prompt("Endereço do servidor (ex.: https://wikelo.exemplo.com): ")?;
    }
    cfg.server = normalize_server(&cfg.server);
    if let Some(c) = &opts.channel {
        cfg.channel = Some(c.to_ascii_uppercase());
    }
    if let Some(s) = opts.share_data {
        cfg.share_data = Some(s);
    }

    let client = Client::new(&cfg.server, &cfg.token);
    let name = match client.me() {
        Ok(Some(n)) => n,
        Ok(None) | Err(SendError::Unauthorized) => {
            if !interactive && opts.password.is_none() {
                bail!("sessão no servidor inválida; abra o wikelo-watcher para entrar de novo");
            }
            let (token, handle, name) = interactive_login(&cfg.server, opts.handle.clone().or_else(|| Some(cfg.handle.clone())), opts.password.clone())?;
            cfg.token = token;
            cfg.handle = handle;
            cfg.name = Some(name.clone());
            name
        }
        Err(e) => bail!("não foi possível falar com {}: {e:?}", cfg.server),
    };
    if cfg.share_data.is_none() {
        cfg.share_data = Some(if interactive {
            eprintln!();
            eprintln!("As missões do Game.log são enviadas sempre que o watcher está rodando; isso não muda.");
            eprintln!("Além disso, ele pode enviar os dados do jogo (contratos e imagens) quando sair uma versão nova:");
            eprintln!("uma vez por hora compara as versões instaladas com as do servidor e, só se aparecer uma versão");
            eprintln!("nova, extrai e envia (uns segundos, com o jogo fechado).");
            yes_no("Enviar também os dados do jogo quando sair uma versão nova?", true)?
        } else {
            false
        });
    }
    save_config(&cfg)?;
    Ok((cfg, name))
}

/// Pick the install to follow (channel from the config, else LIVE, else the first one).
fn pick_install(cfg: &AgentConfig) -> anyhow::Result<wikelo_install::InstallInfo> {
    let installs: Vec<_> = wikelo_install::discover_installs(&[]).into_iter().filter(|i| i.game_log_path.exists() || i.has_p4k()).collect();
    match &cfg.channel {
        Some(c) => installs.iter().find(|i| i.channel.eq_ignore_ascii_case(c)),
        None => installs.iter().find(|i| i.channel == "LIVE").or(installs.first()),
    }
    .cloned()
    .with_context(|| {
        format!(
            "nenhuma instalação do Star Citizen encontrada{}; use --channel ou SC_INSTALL_ROOTS",
            cfg.channel.as_ref().map(|c| format!(" para o canal {c}")).unwrap_or_default()
        )
    })
}

// ----- automatic game data ------------------------------------------------------------

fn game_active(install: &wikelo_install::InstallInfo) -> bool {
    std::fs::metadata(&install.game_log_path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|age| age < GAME_ACTIVE_WINDOW)
}

fn clock() -> String {
    let now = now_local();
    format!("{:02}:{:02}", now.hour(), now.minute())
}

/// Compare installed builds with the server's and upload a bundle for each new one. The
/// bundle is not signed: the server validates it in quarantine and recomputes its hash.
/// Skips while the game runs (it is using the files and the CPU). Returns a one-line summary.
fn data_check(client: &Client, name: &str, handle: &str) -> String {
    let known = match client.known_datasets() {
        Ok(k) => k,
        Err(e) => {
            say!("não deu para consultar as versões do servidor ({e:?}); tento de novo em 1 h");
            return format!("{}: servidor não respondeu", clock());
        }
    };
    let installs: Vec<_> = wikelo_install::discover_installs(&[]).into_iter().filter(|i| i.has_p4k()).collect();
    let mut lines: Vec<String> = Vec::new();
    for install in installs {
        let channel = install.channel.to_ascii_uppercase();
        let version = install.manifest.version.clone();
        if known.iter().any(|(c, v)| *c == channel && *v == version) {
            continue;
        }
        if game_active(&install) {
            say!("{channel} {version} é nova para o servidor, mas o jogo está aberto; envio depois");
            lines.push(format!("{channel} {version}: envio adiado, jogo aberto"));
            continue;
        }
        say!("{channel} {version} não existe no servidor: extraindo os dados do jogo…");
        let quiet = |_p: wikelo_watcher::ExtractProgress| {};
        match crate::bundling::make_bundle(&install, name, handle, &quiet) {
            Ok(built) => match client.upload_bundle(&built.bytes) {
                Ok(r) => {
                    let outcome = match r.status.as_str() {
                        "activated" => "já em uso",
                        "pending" => "aguardando o administrador ativar",
                        "invalid" => "recusado na validação",
                        other => other,
                    };
                    say!(
                        "{channel} {}: {} contratos, {:.1} MB enviados ({outcome}) · hash {}",
                        built.label,
                        built.contracts,
                        built.bytes.len() as f64 / 1048576.0,
                        built.content_hash
                    );
                    lines.push(format!("{channel} {} enviado: {outcome}", built.label));
                }
                Err(SendError::Rejected(m)) => {
                    say!("o servidor recusou os dados de {channel}: {m}");
                    lines.push(format!("{channel}: recusado ({m})"));
                }
                Err(e) => {
                    say!("falha ao enviar os dados de {channel}: {e:?}");
                    lines.push(format!("{channel}: falha ao enviar"));
                }
            },
            Err(e) => {
                say!("falha ao extrair {channel}: {e:#}");
                lines.push(format!("{channel}: falha ao extrair"));
            }
        }
    }
    if lines.is_empty() { format!("{}: versões em dia no servidor", clock()) } else { format!("{}: {}", clock(), lines.join(" · ")) }
}

// ----- main loop --------------------------------------------------------------------

fn describe(out: &TrackerOutput) -> Option<String> {
    Some(match out {
        TrackerOutput::SessionStart { .. } => "nova sessão do jogo".into(),
        TrackerOutput::Identity { nickname, .. } => format!("jogador: {nickname}"),
        TrackerOutput::MissionLinked { mission } => {
            format!("→ missão aceita: {}", mission.title.clone().or(mission.debug_name.clone()).unwrap_or_default())
        }
        TrackerOutput::ObjectiveUpdated { mission, state, item_id, .. } if state.ends_with("COMPLETED") => {
            format!("  ✓ objetivo concluído em {} ({})", mission.title.clone().or(mission.debug_name.clone()).unwrap_or_default(), item_id.clone().unwrap_or_default())
        }
        TrackerOutput::MissionEnded { mission } => {
            let name = mission.title.clone().or(mission.debug_name.clone()).unwrap_or_default();
            let how = mission.completion.as_ref().map(|c| format!("{c:?}")).unwrap_or_default();
            let whose = if mission.shared_by.is_some() { " (de outro jogador)" } else { "" };
            if mission.is_wikelo() { format!("✔ Wikelo {how}: {name}{whose}") } else { format!("✔ missão {how}: {name}{whose}") }
        }
        TrackerOutput::BlueprintReceived { name, .. } => format!("blueprint recebido: {name}"),
        TrackerOutput::AtEmporium { location_class, .. } => format!("no Wikelo: {location_class}"),
        _ => return None,
    })
}

#[cfg(windows)]
fn lower_priority() {
    use windows_sys::Win32::System::Threading::{BELOW_NORMAL_PRIORITY_CLASS, GetCurrentProcess, SetPriorityClass};
    // SAFETY: plain Win32 calls on our own process handle.
    unsafe {
        SetPriorityClass(GetCurrentProcess(), BELOW_NORMAL_PRIORITY_CLASS);
    }
}

#[cfg(not(windows))]
fn lower_priority() {}

pub struct WatchOptions {
    pub setup: SetupOptions,
    pub no_replay: bool,
    /// Detached background mode: pid file, stop file, lower priority, no prompts.
    pub daemon: bool,
}

/// Run the watcher until Ctrl+C or the stop file appears.
pub fn run(opts: WatchOptions) -> anyhow::Result<()> {
    let (mut cfg, name) = ensure_setup(opts.setup, !opts.daemon)?;
    let install = pick_install(&cfg)?;
    cfg.channel = Some(install.channel.clone());
    save_config(&cfg)?;

    if opts.daemon {
        lower_priority();
        std::fs::write(pid_path(), std::process::id().to_string())?;
        let _ = std::fs::remove_file(stop_path());
    }
    let _ = std::fs::remove_file(check_path());
    say!("conectado a {} como {name}", cfg.server);
    say!("acompanhando {} em {}", install.channel, install.game_log_path.display());
    say!(
        "envio dos dados do jogo quando sair uma versão nova: {}",
        if cfg.share_data == Some(true) { "ativado (checagem a cada hora)" } else { "desativado" }
    );
    if !opts.daemon {
        eprintln!("Deixe esta janela aberta enquanto joga. Ctrl+C para sair.\n");
    }

    let stop = Arc::new(AtomicBool::new(false));
    {
        let s = stop.clone();
        let _ = ctrlc::set_handler(move || s.store(true, Ordering::Relaxed));
    }
    let host = std::env::var("COMPUTERNAME").or_else(|_| std::env::var("HOSTNAME")).unwrap_or_else(|_| "pc".into());
    let channel = install.channel.clone();
    let mut share = cfg.share_data == Some(true);
    let author_name = cfg.display_name();
    let author_handle = cfg.handle.clone();
    let mut client = Client::new(&cfg.server, &cfg.token);
    let mut tailer = Tailer::new(TailerConfig { path: install.game_log_path.clone(), from_start: !opts.no_replay, poll_interval: POLL });

    let mut status = DaemonStatus {
        pid: std::process::id(),
        started_at: now_rfc3339(),
        server: cfg.server.clone(),
        channel: channel.clone(),
        share_data: share,
        ..Default::default()
    };
    status.write();

    let mut queue: Vec<(bool, TrackerOutput)> = Vec::new();
    let mut last_try = Instant::now() - Duration::from_secs(3600);
    let mut backoff = Duration::from_secs(0);
    let mut last_ok = Instant::now() - HEARTBEAT;
    let mut replayed = 0usize;
    let mut announced_replay = false;
    let mut paused = false;
    let mut last_data_check: Option<Instant> = None;
    let checking: Arc<Mutex<bool>> = Arc::new(Mutex::new(false));
    let check_result: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let mut cfg_mtime = config_mtime();
    let stop_file = stop_path();
    let check_file = check_path();
    let stop_loop = stop.clone();

    tailer.run_batched(stop_loop.clone(), &mut |batch| {
        let mut dirty = false;
        if opts.daemon && stop_file.exists() {
            stop_loop.store(true, Ordering::Relaxed);
        }
        // The window rewrote watcher.json: pick up the data-upload switch without a restart.
        let mt = config_mtime();
        if mt != cfg_mtime {
            cfg_mtime = mt;
            if let Some(c) = load_config() {
                let s = c.share_data == Some(true);
                if s != share {
                    share = s;
                    status.share_data = s;
                    dirty = true;
                    say!("envio dos dados do jogo {}", if share { "ativado" } else { "desativado" });
                    if share {
                        last_data_check = None;
                    }
                }
            }
        }
        for (replay, out) in &batch {
            if *replay {
                replayed += 1;
            } else if let Some(line) = describe(out) {
                say!("{line}");
                status.last_event = Some(format!("{} {line}", time_now()));
                dirty = true;
            }
        }
        if !announced_replay && replayed > 0 && batch.iter().all(|(r, _)| !*r) {
            say!("histórico da sessão atual lido ({replayed} eventos)");
            announced_replay = true;
        }
        if !paused {
            queue.extend(batch);
        }

        // Hourly (first check right after start), or now when the window asks for it.
        let forced = check_file.exists();
        if forced {
            let _ = std::fs::remove_file(&check_file);
        }
        if forced || (share && last_data_check.is_none_or(|t| t.elapsed() > DATA_CHECK_EVERY)) {
            last_data_check = Some(Instant::now());
            if let Ok(mut busy) = checking.try_lock()
                && !*busy
            {
                *busy = true;
                drop(busy);
                let c = client.clone();
                let (n, h) = (author_name.clone(), author_handle.clone());
                let flag = checking.clone();
                let result = check_result.clone();
                std::thread::spawn(move || {
                    let summary = data_check(&c, &n, &h);
                    *result.lock().unwrap() = Some(summary);
                    *flag.lock().unwrap() = false;
                });
            }
        }
        if let Some(summary) = check_result.lock().unwrap().take() {
            status.data_check = Some(summary);
            dirty = true;
        }

        let heartbeat_due = last_ok.elapsed() > HEARTBEAT;
        if (queue.is_empty() && !heartbeat_due) || last_try.elapsed() < backoff {
            if dirty {
                status.write();
            }
            return;
        }
        last_try = Instant::now();
        match client.send(&channel, &host, share, &queue) {
            Ok(ack) => {
                queue.clear();
                backoff = Duration::from_secs(0);
                last_ok = Instant::now();
                status.last_ok = Some(now_rfc3339());
                status.last_error = None;
                if ack.enabled == paused {
                    paused = !ack.enabled;
                    status.paused = paused;
                    if paused {
                        say!("pausado pelo site (Dados → Watcher); os eventos são ignorados até reativar");
                    } else {
                        say!("reativado pelo site");
                    }
                }
            }
            Err(SendError::Unauthorized) => {
                if opts.daemon {
                    say!("a sessão no servidor expirou; abra o wikelo-watcher para entrar de novo");
                    status.last_error = Some("sessão expirada: entre de novo".into());
                    stop_loop.store(true, Ordering::Relaxed);
                } else {
                    say!("a sessão no servidor expirou; entre de novo.");
                    match interactive_login(&cfg.server, Some(cfg.handle.clone()), None) {
                        Ok((token, handle, name)) => {
                            cfg.token = token;
                            cfg.handle = handle;
                            cfg.name = Some(name);
                            let _ = save_config(&cfg);
                            cfg_mtime = config_mtime();
                            client = Client::new(&cfg.server, &cfg.token);
                        }
                        Err(e) => {
                            say!("login falhou: {e}");
                            backoff = Duration::from_secs(30);
                        }
                    }
                }
            }
            Err(SendError::NoData) => {
                say!("o servidor ainda não tem os dados do canal {channel}; tento de novo em 20 s");
                status.last_error = Some(format!("o servidor ainda não tem os dados do canal {channel}"));
                backoff = Duration::from_secs(20);
            }
            Err(SendError::Rejected(m)) | Err(SendError::Other(m)) => {
                backoff = (if backoff.is_zero() { Duration::from_secs(2) } else { backoff * 2 }).min(Duration::from_secs(60));
                say!("falha ao enviar ({m}); nova tentativa em {} s", backoff.as_secs());
                status.last_error = Some(format!("sem contato com o servidor ({m})"));
            }
        }
        status.write();
        if queue.len() > 5000 {
            let drop_n = queue.len() - 5000;
            queue.drain(..drop_n);
        }
    });

    client.bye();
    let _ = std::fs::remove_file(status_path());
    if opts.daemon {
        let _ = std::fs::remove_file(pid_path());
        let _ = std::fs::remove_file(stop_path());
    }
    say!("watcher encerrado");
    Ok(())
}

// ----- background process control -----------------------------------------------------

#[cfg(windows)]
fn pid_alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
    use windows_sys::Win32::System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    // SAFETY: plain Win32 calls; the handle is closed before returning.
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h == 0 {
            return false;
        }
        let mut code: u32 = 0;
        let ok = GetExitCodeProcess(h, &mut code) != 0;
        CloseHandle(h);
        ok && code == STILL_ACTIVE as u32
    }
}

#[cfg(not(windows))]
fn pid_alive(_pid: u32) -> bool {
    false
}

/// Pid of the running background watcher, if any.
pub fn running_pid() -> Option<u32> {
    let pid: u32 = std::fs::read_to_string(pid_path()).ok()?.trim().parse().ok()?;
    if pid_alive(pid) {
        Some(pid)
    } else {
        let _ = std::fs::remove_file(pid_path());
        None
    }
}

/// Spawn `watch --daemon` detached from this console.
pub fn start_background() -> anyhow::Result<u32> {
    if let Some(pid) = running_pid() {
        bail!("o watcher já está rodando em segundo plano (pid {pid})");
    }
    let exe = std::env::current_exe()?;
    let log = log_path();
    if let Some(d) = log.parent() {
        std::fs::create_dir_all(d)?;
    }
    if std::fs::metadata(&log).map(|m| m.len() > 2 * 1024 * 1024).unwrap_or(false) {
        let _ = std::fs::remove_file(&log);
    }
    let out = std::fs::OpenOptions::new().create(true).append(true).open(&log)?;
    let err = out.try_clone()?;
    let mut cmd = std::process::Command::new(exe);
    cmd.args(["watch", "--daemon"]).stdin(std::process::Stdio::null()).stdout(out).stderr(err);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        cmd.creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS);
    }
    // Our own std handles must not leak into the daemon: a script that captured this
    // process's output would otherwise wait for the daemon to exit.
    #[cfg(windows)]
    unset_std_inherit();
    let child = cmd.spawn()?;
    // The pid file is written here too, so `status` is right before the daemon finishes its setup.
    std::fs::write(pid_path(), child.id().to_string())?;
    Ok(child.id())
}

#[cfg(windows)]
fn unset_std_inherit() {
    use windows_sys::Win32::Foundation::{HANDLE_FLAG_INHERIT, SetHandleInformation};
    use windows_sys::Win32::System::Console::{GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE};
    // SAFETY: plain Win32 calls on the process's own standard handles.
    unsafe {
        for which in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
            let h = GetStdHandle(which);
            if h != 0 && h != -1 {
                SetHandleInformation(h, HANDLE_FLAG_INHERIT, 0);
            }
        }
    }
}

/// Ask the background watcher to quit and wait for it.
pub fn stop_background() -> anyhow::Result<bool> {
    let Some(pid) = running_pid() else { return Ok(false) };
    std::fs::write(stop_path(), "stop")?;
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(15) {
        if !pid_alive(pid) {
            let _ = std::fs::remove_file(pid_path());
            let _ = std::fs::remove_file(stop_path());
            return Ok(true);
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    bail!("o watcher (pid {pid}) não respondeu ao pedido de parada")
}

#[cfg(windows)]
const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";

/// `reg.exe` without a console window popping up (we have no console of our own).
#[cfg(windows)]
fn reg(args: &[&str]) -> std::io::Result<std::process::Output> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    std::process::Command::new("reg").args(args).creation_flags(CREATE_NO_WINDOW).stdin(std::process::Stdio::null()).output()
}

/// Register / remove the watcher in the user's Windows autostart (HKCU Run key).
#[cfg(windows)]
pub fn set_autostart(on: bool) -> anyhow::Result<()> {
    let status = if on {
        let exe = std::env::current_exe()?;
        let value = format!("\"{}\" start --quiet", exe.display());
        reg(&["add", RUN_KEY, "/v", "WikeloWatcher", "/t", "REG_SZ", "/d", &value, "/f"])?
    } else {
        reg(&["delete", RUN_KEY, "/v", "WikeloWatcher", "/f"])?
    };
    if !status.status.success() && on {
        bail!("não foi possível gravar a entrada de inicialização: {}", String::from_utf8_lossy(&status.stderr).trim());
    }
    Ok(())
}

#[cfg(windows)]
pub fn autostart_enabled() -> bool {
    reg(&["query", RUN_KEY, "/v", "WikeloWatcher"]).map(|o| o.status.success()).unwrap_or(false)
}

#[cfg(not(windows))]
pub fn set_autostart(_on: bool) -> anyhow::Result<()> {
    bail!("inicialização automática só no Windows")
}

#[cfg(not(windows))]
pub fn autostart_enabled() -> bool {
    false
}
