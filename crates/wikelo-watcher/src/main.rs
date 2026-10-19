//! `wikelo-watcher`: the program players run on the PC with the game.
//!
//! Double-click (no arguments) opens the settings window (`gui`): account, start/stop,
//! channel, start with Windows, game-data upload, activity. The subcommands do the same
//! from a terminal or a script: `start`, `stop`, `status`, `autostart`, `watch`
//! (foreground), `logout`, `bundle` (signed data package by hand). The remaining
//! subcommands are developer tools.
#![cfg_attr(windows, windows_subsystem = "windows")]

use std::path::PathBuf;
use std::time::Instant;

mod agent;
mod bundling;
mod console;
mod gui;

use anyhow::{Context, bail};
use clap::{Parser, Subcommand};
use starbreaker_common::Progress;
use starbreaker_datacore::Database;
use starbreaker_p4k::MappedP4k;

#[derive(Parser)]
#[command(name = "wikelo-watcher", about = "Acompanha o Game.log e mantém o servidor Wikelist atualizado")]
struct Cli {
    /// Without a subcommand: the settings window.
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Start the watcher in the background (sets it up on the first run).
    Start {
        /// Server address, e.g. https://wikelo.exemplo.com (remembered after the first run).
        #[arg(long)]
        server: Option<String>,
        /// Channel to follow (default: LIVE, or the only one installed).
        #[arg(long)]
        channel: Option<String>,
        /// Account handle (asked interactively when missing).
        #[arg(long)]
        handle: Option<String>,
        /// Password (asked interactively, hidden, when missing).
        #[arg(long)]
        password: Option<String>,
        /// Upload new game versions automatically (yes/no); asked once when missing.
        #[arg(long)]
        share_data: Option<bool>,
        /// Do not wait for Enter at the end (used by the autostart entry).
        #[arg(long)]
        quiet: bool,
    },
    /// Stop the background watcher.
    Stop,
    /// Is the background watcher running?
    Status,
    /// Run the watcher in this console (Ctrl+C to quit).
    Watch {
        #[arg(long)]
        server: Option<String>,
        #[arg(long)]
        channel: Option<String>,
        #[arg(long)]
        handle: Option<String>,
        #[arg(long)]
        password: Option<String>,
        #[arg(long)]
        share_data: Option<bool>,
        /// Do not replay the current Game.log on start.
        #[arg(long)]
        no_replay: bool,
        /// Internal: background mode spawned by `start`.
        #[arg(long, hide = true)]
        daemon: bool,
    },
    /// Start the watcher automatically when Windows starts (on|off).
    Autostart {
        #[arg(value_parser = ["on", "off"])]
        mode: String,
    },
    /// Forget the saved server session.
    Logout,
    /// Extract and write a bundle (.zip) per channel by hand, to send on the site.
    Bundle {
        /// Only this channel (LIVE, PTU, ...). Default: every install that has game data.
        #[arg(long)]
        channel: Option<String>,
        /// Explicit Data.p4k path (the channel is then taken from its folder name).
        #[arg(long)]
        p4k: Option<PathBuf>,
        /// Output directory. Default: the directory of the executable.
        #[arg(long)]
        out: Option<PathBuf>,
        /// Name recorded as the author of the bundle. Default: the Windows user name.
        #[arg(long)]
        name: Option<String>,
    },
    /// Extract and print the reproducible content hash of each installed channel, to
    /// compare with what the server shows (nothing is sent).
    Hash {
        /// Only this channel (LIVE, PTU, ...).
        #[arg(long)]
        channel: Option<String>,
        /// Explicit Data.p4k path.
        #[arg(long)]
        p4k: Option<PathBuf>,
    },
    /// List detected Star Citizen installations.
    Installs,
    /// Dump the raw generator record and localization for inspection.
    Spike {
        /// Channel to use (LIVE, PTU, ...). Ignored when --p4k is given.
        #[arg(long, default_value = "LIVE")]
        channel: String,
        /// Explicit Data.p4k path.
        #[arg(long)]
        p4k: Option<PathBuf>,
        /// Output directory.
        #[arg(long, default_value = "tmp/spike")]
        out: PathBuf,
        /// Record name to dump.
        #[arg(long, default_value = wikelo_core::GENERATOR_RECORD_NAME)]
        record: String,
    },
    /// Run the full extraction and write `wikelo-data.json`.
    Extract {
        #[arg(long, default_value = "LIVE")]
        channel: String,
        /// Explicit Data.p4k path (the channel is then taken from its folder name).
        #[arg(long)]
        p4k: Option<PathBuf>,
        /// Output directory.
        #[arg(long, default_value = "tmp/out")]
        out: PathBuf,
        /// Emit one JSON progress object per line on stdout (for the server).
        #[arg(long)]
        json_progress: bool,
    },
    /// List Data.p4k entries whose path contains every given token (case-insensitive).
    P4kLs {
        #[arg(long, default_value = "LIVE")]
        channel: String,
        #[arg(long)]
        p4k: Option<PathBuf>,
        /// Maximum number of entries to print.
        #[arg(long, default_value_t = 200)]
        limit: usize,
        /// Extract matching entries into this directory (raw bytes).
        #[arg(long)]
        extract_to: Option<PathBuf>,
        #[arg(required = true)]
        tokens: Vec<String>,
    },
    /// Dump every main record of a DataForge struct type as JSON (one file each).
    DumpType {
        #[arg(long, default_value = "LIVE")]
        channel: String,
        #[arg(long)]
        p4k: Option<PathBuf>,
        #[arg(long, default_value = "tmp/spike")]
        out: PathBuf,
        /// Struct type name, e.g. `CraftingBlueprintRecord`.
        type_name: String,
        /// Only records whose name contains this (case-insensitive).
        #[arg(long)]
        filter: Option<String>,
    },
    /// Dump arbitrary records (by record name or by file-name suffix) as JSON.
    Dump {
        #[arg(long, default_value = "LIVE")]
        channel: String,
        #[arg(long)]
        p4k: Option<PathBuf>,
        #[arg(long, default_value = "tmp/spike")]
        out: PathBuf,
        /// Record names (e.g. `ContractTemplate.TheCollector_Vehicles`) or file-name
        /// suffixes (e.g. `contracttemplates/thecollector_vehicles_scorpius`), case-insensitive.
        #[arg(required = true)]
        names: Vec<String>,
    },
}

fn main() -> anyhow::Result<()> {
    // Subcommands print; the window does not. We are a GUI executable, so a terminal that
    // launched us is attached by hand (pipes set up by a script are left alone).
    if std::env::args_os().len() > 1 {
        console::attach_parent();
    }
    // No colour codes: the background watcher's output lands in a log file the window shows.
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env().add_directive("info".parse()?))
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();
    let cli = Cli::parse();
    if cli.cmd.is_none() {
        return match gui::run() {
            Ok(()) => Ok(()),
            Err(e) => {
                console::message_box(&format!("{e:#}\n\nSem a janela, use a linha de comando: wikelo-watcher start --server https://..."));
                Err(e)
            }
        };
    }
    // `start` typed in a terminal: keep the result readable before the window closes.
    let pause_at_end = matches!(&cli.cmd, Some(Cmd::Start { quiet, .. }) if !quiet) && console::has_console();
    let result = run(cli);
    if pause_at_end {
        if let Err(e) = &result {
            eprintln!("\nERRO: {e:#}");
        }
        eprintln!("\nPressione Enter para fechar...");
        let mut line = String::new();
        let _ = std::io::stdin().read_line(&mut line);
    }
    result
}

fn run(cli: Cli) -> anyhow::Result<()> {
    match cli.cmd {
        None => unreachable!("the window runs from main"),
        Some(Cmd::Start { server, channel, handle, password, share_data, .. }) => {
            run_start(agent::SetupOptions { server, channel, handle, password, share_data })
        }
        Some(Cmd::Stop) => {
            if agent::stop_background()? {
                eprintln!("watcher parado.");
            } else {
                eprintln!("o watcher não estava rodando.");
            }
            Ok(())
        }
        Some(Cmd::Status) => {
            eprintln!("wikelo-watcher {} ({})", wikelo_core::PKG_VERSION, wikelo_core::GIT_COMMIT);
            match agent::running_pid() {
                Some(pid) => eprintln!("watcher rodando em segundo plano (pid {pid}); log em {}", agent::log_path().display()),
                None => eprintln!("watcher parado."),
            }
            if let Some(cfg) = agent::load_config() {
                eprintln!(
                    "servidor {} · conta {} · canal {} · envio dos dados do jogo em versão nova: {}",
                    cfg.server,
                    cfg.handle,
                    cfg.channel.unwrap_or_else(|| "?".into()),
                    if cfg.share_data == Some(true) { "ativado" } else { "desativado" }
                );
            }
            if let Some(s) = agent::read_status() {
                let state = if s.paused {
                    "pausado pelo site".to_string()
                } else if let Some(e) = &s.last_error {
                    e.clone()
                } else if s.last_ok.is_some() {
                    "conectado ao servidor".to_string()
                } else {
                    "conectando".to_string()
                };
                eprintln!(
                    "{state}{}{}",
                    s.last_ok.map(|t| format!(" · último contato {t}")).unwrap_or_default(),
                    s.data_check.map(|d| format!(" · dados do jogo: {d}")).unwrap_or_default()
                );
                if let (Some(v), Some(u)) = (s.latest_version, s.update_url) {
                    eprintln!("atualização disponível: {v} → {u}");
                }
            }
            Ok(())
        }
        Some(Cmd::Watch { server, channel, handle, password, share_data, no_replay, daemon }) => {
            agent::run(agent::WatchOptions { setup: agent::SetupOptions { server, channel, handle, password, share_data }, no_replay, daemon })
        }
        Some(Cmd::Autostart { mode }) => {
            let on = mode == "on";
            agent::set_autostart(on)?;
            eprintln!("inicialização automática {}.", if on { "ativada: o watcher sobe junto com o Windows" } else { "desativada" });
            Ok(())
        }
        Some(Cmd::Logout) => {
            let _ = agent::stop_background();
            let p = agent::config_path();
            let _ = std::fs::remove_file(&p);
            let _ = std::fs::remove_file(agent::status_path());
            eprintln!("sessão do watcher removida ({}).", p.display());
            Ok(())
        }
        Some(Cmd::Bundle { channel, p4k, out, name }) => run_bundle(channel, p4k, out, name),
        Some(Cmd::Hash { channel, p4k }) => run_hash(channel, p4k),
        Some(cmd) => run_dev(cmd),
    }
}

/// `start`: complete the setup interactively if needed, then leave the watcher running detached.
fn run_start(setup: agent::SetupOptions) -> anyhow::Result<()> {
    if let Some(pid) = agent::running_pid() {
        eprintln!("o watcher já está rodando em segundo plano (pid {pid}). Para parar: wikelo-watcher stop");
        return Ok(());
    }
    let (cfg, name) = agent::ensure_setup(setup, console::has_console())?;
    let pid = agent::start_background()?;
    eprintln!("\nWatcher iniciado em segundo plano (pid {pid}) para {} como {name}.", cfg.server);
    eprintln!("Pode fechar esta janela. Para parar: wikelo-watcher stop · para subir com o Windows: wikelo-watcher autostart on · opções: abra o wikelo-watcher");
    eprintln!("Log: {}", agent::log_path().display());
    Ok(())
}

fn exe_dir() -> PathBuf {
    std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf())).unwrap_or_else(|| PathBuf::from("."))
}

/// Installs selected by `--channel` / `--p4k` that have game data.
fn selected_installs(channel: Option<&str>, p4k: Option<PathBuf>) -> anyhow::Result<Vec<wikelo_install::InstallInfo>> {
    let installs: Vec<wikelo_install::InstallInfo> = if let Some(p) = p4k {
        vec![resolve_install("", Some(p))?]
    } else {
        wikelo_install::discover_installs(&[])
            .into_iter()
            .filter(|i| channel.is_none_or(|c| i.channel.eq_ignore_ascii_case(c)))
            .filter(|i| i.has_p4k())
            .collect()
    };
    if installs.is_empty() {
        bail!(
            "nenhuma instalação do Star Citizen com dados do jogo foi encontrada{}. Use --p4k <caminho do Data.p4k> ou SC_INSTALL_ROOTS.",
            channel.map(|c| format!(" para o canal {c}")).unwrap_or_default()
        );
    }
    Ok(installs)
}

fn print_provenance(built: &bundling::Built) {
    eprintln!("hash do conteúdo: {}", built.content_hash);
    if let Some(x) = &built.extractor {
        eprintln!("extrator {} {} ({}) · Game2.dcb {} · global.ini {}", x.name, x.version, x.commit, &x.dcb_sha256[..16], &x.ini_sha256[..16]);
    }
}

/// Extract every selected install and write one bundle per channel.
fn run_bundle(channel: Option<String>, p4k: Option<PathBuf>, out: Option<PathBuf>, name: Option<String>) -> anyhow::Result<()> {
    let out = out.unwrap_or_else(exe_dir);
    std::fs::create_dir_all(&out)?;
    let author = bundling::author_name(name);
    let installs = selected_installs(channel.as_deref(), p4k)?;
    eprintln!("Autor: {author}");
    for install in installs {
        eprintln!("\n== {} {} ==", install.channel, install.manifest.version);
        let started = Instant::now();
        let on_progress = |p: wikelo_watcher::ExtractProgress| eprintln!("[{:>6.2}s] {:<10} {:>3}% {}", p.elapsed_sec, p.stage, p.pct, p.message);
        let built = bundling::make_bundle(&install, &author, &author, &on_progress)?;
        let path = out.join(&built.file_name);
        std::fs::write(&path, &built.bytes)?;
        eprintln!(
            "pacote: {} — {} contratos, {} itens, {} imagens{}, {:.1} MB em {:.1}s",
            path.display(),
            built.contracts,
            built.items,
            built.images,
            if built.skipped > 0 { format!(" ({} ignoradas)", built.skipped) } else { String::new() },
            built.bytes.len() as f64 / 1048576.0,
            started.elapsed().as_secs_f64()
        );
        print_provenance(&built);
        for w in built.warnings.iter().take(10) {
            eprintln!("aviso: {w}");
        }
    }
    eprintln!("\nPronto. Envie o arquivo em Dados → Enviar pacote no servidor; o administrador vê o mesmo hash lá e ativa.");
    Ok(())
}

/// Extract and print the content hash per channel, so anyone with the game can check
/// what the server received (nothing is uploaded).
fn run_hash(channel: Option<String>, p4k: Option<PathBuf>) -> anyhow::Result<()> {
    let installs = selected_installs(channel.as_deref(), p4k)?;
    for install in installs {
        let started = Instant::now();
        eprintln!("== {} {} ({}) ==", install.channel, wikelo_core::version_label(&install.manifest.branch, &install.manifest.version), install.manifest.version);
        let quiet = |_p: wikelo_watcher::ExtractProgress| {};
        let built = bundling::make_bundle(&install, "", "", &quiet)?;
        eprintln!("{} contratos, {} itens, {} imagens em {:.1}s", built.contracts, built.items, built.images, started.elapsed().as_secs_f64());
        print_provenance(&built);
        println!("{} {} {}", install.channel, install.manifest.version, built.content_hash);
    }
    Ok(())
}

fn run_dev(cmd: Cmd) -> anyhow::Result<()> {
    match cmd {
        Cmd::Start { .. } | Cmd::Stop | Cmd::Status | Cmd::Watch { .. } | Cmd::Autostart { .. } | Cmd::Logout | Cmd::Bundle { .. } | Cmd::Hash { .. } => {
            unreachable!("handled in run")
        }
        Cmd::Installs => {
            for i in wikelo_install::discover_installs(&[]) {
                println!(
                    "{:<13} {:<16} p4change={:<10} p4k={} {}",
                    i.channel,
                    i.manifest.version,
                    i.manifest.requested_p4_change_num,
                    if i.has_p4k() { "yes" } else { "no " },
                    i.root.display()
                );
            }
            Ok(())
        }
        Cmd::Spike { channel, p4k, out, record } => spike(channel, p4k, out, &record),
        Cmd::Dump { channel, p4k, out, names } => dump(channel, p4k, out, &names),
        Cmd::DumpType { channel, p4k, out, type_name, filter } => dump_type(channel, p4k, out, &type_name, filter.as_deref()),
        Cmd::P4kLs { channel, p4k, limit, extract_to, tokens } => p4k_ls(channel, p4k, limit, extract_to, &tokens),
        Cmd::Extract { channel, p4k, out, json_progress } => run_extract(channel, p4k, out, json_progress),
    }
}

fn resolve_install(channel: &str, p4k: Option<PathBuf>) -> anyhow::Result<wikelo_install::InstallInfo> {
    if let Some(p) = p4k {
        let dir = p.parent().context("p4k path has no parent")?;
        return wikelo_install::install_from_channel_dir(dir).context("read build_manifest.id next to Data.p4k");
    }
    let installs = wikelo_install::discover_installs(&[]);
    installs
        .into_iter()
        .find(|i| i.channel.eq_ignore_ascii_case(channel))
        .with_context(|| format!("channel {channel} not found"))
}

fn run_extract(channel: String, p4k: Option<PathBuf>, out: PathBuf, json_progress: bool) -> anyhow::Result<()> {
    let install = resolve_install(&channel, p4k)?;
    std::fs::create_dir_all(&out)?;
    let on_progress = |p: wikelo_watcher::ExtractProgress| {
        if json_progress {
            println!("{}", serde_json::to_string(&p).unwrap_or_default());
        } else {
            eprintln!("[{:>6.2}s] {:<10} {:>3}% {}", p.elapsed_sec, p.stage, p.pct, p.message);
        }
    };
    let output = wikelo_watcher::extract(&install, &on_progress)?;
    let dataset = &output.dataset;
    let path = out.join("wikelo-data.json");
    std::fs::write(&path, serde_json::to_vec_pretty(dataset)?)?;
    let n_img = output.write_images(&out)?;
    if !json_progress {
        eprintln!("{n_img} imagens gravadas em {}", out.join(wikelo_watcher::images::IMG_DIR).display());
    }
    if json_progress {
        println!(
            "{}",
            serde_json::json!({
                "stage": "written",
                "pct": 100,
                "message": path.display().to_string(),
                "contracts": dataset.contracts.len(),
                "items": dataset.items.len(),
                "warnings": dataset.warnings.len(),
            })
        );
    } else {
        eprintln!("written {}", path.display());
        for w in dataset.warnings.iter().take(40) {
            eprintln!("warning: {w}");
        }
        if dataset.warnings.len() > 40 {
            eprintln!("... {} more warnings", dataset.warnings.len() - 40);
        }
    }
    Ok(())
}

fn dump(channel: String, p4k: Option<PathBuf>, out: PathBuf, names: &[String]) -> anyhow::Result<()> {
    let p4k_path = resolve_p4k(&channel, p4k)?;
    std::fs::create_dir_all(&out)?;
    let p4k = MappedP4k::open(&p4k_path).context("open p4k")?;
    let dcb = p4k
        .read_file("Data\\Game2.dcb")
        .or_else(|_| p4k.read_file("Data\\Game.dcb"))
        .context("read Game2.dcb")?;
    let db = Database::from_bytes(&dcb).context("parse DataCore")?;
    for wanted in names {
        let wanted_lc = wanted.to_ascii_lowercase().replace('\\', "/");
        let mut hits = 0;
        for r in db.records() {
            if !db.is_main_record(r) {
                continue;
            }
            let name_offset = r.name_offset;
            let file_offset = r.file_name_offset;
            let name = db.resolve_string2(name_offset);
            let file = db.resolve_string(file_offset).to_ascii_lowercase().replace('\\', "/");
            let file_noext = file.rsplit_once('.').map(|(a, _)| a).unwrap_or(&file);
            if name.eq_ignore_ascii_case(wanted) || file_noext.ends_with(&wanted_lc) {
                let rec = *r;
                let json = starbreaker_datacore::export::to_json(&db, &rec)?;
                let safe: String =
                    name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '_' { c } else { '_' }).collect();
                let path = out.join(format!("{safe}.json"));
                std::fs::write(&path, &json)?;
                let id = rec.id;
                eprintln!("{wanted}: {name} ({id}) -> {} [{} bytes] file={file}", path.display(), json.len());
                hits += 1;
            }
        }
        if hits == 0 {
            eprintln!("{wanted}: no record found");
        }
    }
    Ok(())
}

fn dump_type(channel: String, p4k: Option<PathBuf>, out: PathBuf, type_name: &str, filter: Option<&str>) -> anyhow::Result<()> {
    let p4k_path = resolve_p4k(&channel, p4k)?;
    std::fs::create_dir_all(&out)?;
    let p4k = MappedP4k::open(&p4k_path).context("open p4k")?;
    let dcb = p4k
        .read_file("Data\\Game2.dcb")
        .or_else(|_| p4k.read_file("Data\\Game.dcb"))
        .context("read Game2.dcb")?;
    let db = Database::from_bytes(&dcb).context("parse DataCore")?;
    let filter_lc = filter.map(|f| f.to_ascii_lowercase());
    let mut n = 0;
    for r in db.records_by_type_name(type_name) {
        if !db.is_main_record(r) {
            continue;
        }
        let name = db.resolve_string2(r.name_offset).to_string();
        if let Some(f) = &filter_lc
            && !name.to_ascii_lowercase().contains(f.as_str())
        {
            continue;
        }
        let rec = *r;
        let json = starbreaker_datacore::export::to_json(&db, &rec)?;
        let safe: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '_' { c } else { '_' }).collect();
        std::fs::write(out.join(format!("{safe}.json")), &json)?;
        n += 1;
    }
    eprintln!("{n} registros {type_name} gravados em {}", out.display());
    Ok(())
}

fn p4k_ls(channel: String, p4k: Option<PathBuf>, limit: usize, extract_to: Option<PathBuf>, tokens: &[String]) -> anyhow::Result<()> {
    let p4k_path = resolve_p4k(&channel, p4k)?;
    let started = Instant::now();
    let p4k = MappedP4k::open(&p4k_path)?;
    eprintln!("p4k aberto em {:.1}s ({} entradas)", started.elapsed().as_secs_f64(), p4k.len());
    let mut idx = p4k.search(&tokens.join(" "));
    idx.sort_unstable();
    let entries = p4k.entries();
    eprintln!("{} entradas casam", idx.len());
    for &i in idx.iter().take(limit) {
        let e = &entries[i as usize];
        println!("{:>12} {}", e.uncompressed_size, e.name);
    }
    if let Some(dir) = extract_to {
        std::fs::create_dir_all(&dir)?;
        for &i in idx.iter().take(limit) {
            let e = &entries[i as usize];
            let data = p4k.read(e)?;
            let file = dir.join(e.name.replace(['\\', '/'], "__"));
            std::fs::write(&file, data)?;
        }
        eprintln!("extraído para {}", dir.display());
    }
    Ok(())
}

fn resolve_p4k(channel: &str, p4k: Option<PathBuf>) -> anyhow::Result<PathBuf> {
    if let Some(p) = p4k {
        return Ok(p);
    }
    let installs = wikelo_install::discover_installs(&[]);
    let install = installs
        .iter()
        .find(|i| i.channel.eq_ignore_ascii_case(channel))
        .with_context(|| format!("channel {channel} not found; detected: {:?}", installs.iter().map(|i| &i.channel).collect::<Vec<_>>()))?;
    if !install.has_p4k() {
        bail!("{} has no Data.p4k at {}", install.channel, install.p4k_path.display());
    }
    Ok(install.p4k_path.clone())
}

fn spike(channel: String, p4k: Option<PathBuf>, out: PathBuf, record_name: &str) -> anyhow::Result<()> {
    let p4k_path = resolve_p4k(&channel, p4k)?;
    std::fs::create_dir_all(&out)?;
    eprintln!("p4k: {}", p4k_path.display());

    let t = Instant::now();
    let progress = Progress::new();
    let p4k = MappedP4k::open_with_progress(&p4k_path, Some(&progress)).context("open p4k")?;
    eprintln!("[{:>7.2}s] p4k opened: {} entries", t.elapsed().as_secs_f64(), p4k.len());

    let t1 = Instant::now();
    let dcb = p4k
        .read_file("Data\\Game2.dcb")
        .or_else(|_| p4k.read_file("Data\\Game.dcb"))
        .context("read Game2.dcb")?;
    eprintln!("[{:>7.2}s] dcb read: {} bytes (took {:.2}s)", t.elapsed().as_secs_f64(), dcb.len(), t1.elapsed().as_secs_f64());

    let ini = p4k.read_file("Data\\Localization\\english\\global.ini").context("read global.ini")?;
    std::fs::write(out.join("global.ini"), &ini)?;
    eprintln!("[{:>7.2}s] global.ini: {} bytes", t.elapsed().as_secs_f64(), ini.len());

    let t2 = Instant::now();
    let db = Database::from_bytes(&dcb).context("parse DataCore")?;
    eprintln!(
        "[{:>7.2}s] DataCore parsed: {} records, {} structs (took {:.2}s)",
        t.elapsed().as_secs_f64(),
        db.records().len(),
        db.struct_defs().len(),
        t2.elapsed().as_secs_f64()
    );

    // All ContractGenerator record names, for orientation.
    let mut gen_names: Vec<String> = db
        .records_by_type_name("ContractGenerator")
        .map(|r| {
            let name_offset = r.name_offset;
            db.resolve_string2(name_offset).to_string()
        })
        .collect();
    gen_names.sort();
    std::fs::write(out.join("contract-generators.txt"), gen_names.join("\n"))?;
    eprintln!("ContractGenerator records: {}", gen_names.len());

    let record = db
        .records_by_type_name("ContractGenerator")
        .find(|r| {
            let name_offset = r.name_offset;
            db.resolve_string2(name_offset) == record_name
        })
        .with_context(|| format!("record {record_name} not found"))?;
    let record = *record;
    let t3 = Instant::now();
    let json = starbreaker_datacore::export::to_json(&db, &record).context("export record")?;
    let file = out.join(format!("{record_name}.json"));
    std::fs::write(&file, &json)?;
    eprintln!(
        "[{:>7.2}s] exported {} ({} bytes, took {:.2}s)",
        t.elapsed().as_secs_f64(),
        file.display(),
        json.len(),
        t3.elapsed().as_secs_f64()
    );

    // A few supporting record families, names only, to confirm struct names.
    for ty in ["FactionReputation", "SReputationScopeParams", "MissionLocationTemplate", "ContractTemplate", "Tag"] {
        let names: Vec<String> = db
            .records_by_type_name(ty)
            .map(|r| {
                let name_offset = r.name_offset;
                let id = r.id;
                format!("{}\t{}", id, db.resolve_string2(name_offset))
            })
            .collect();
        std::fs::write(out.join(format!("records-{ty}.txt")), names.join("\n"))?;
        eprintln!("{ty}: {} records", names.len());
    }
    eprintln!("done in {:.2}s", t.elapsed().as_secs_f64());
    Ok(())
}
