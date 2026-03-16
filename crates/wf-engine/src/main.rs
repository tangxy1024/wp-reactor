use std::path::PathBuf;
use std::str::FromStr;

use anyhow::Result;
use clap::{Args, Parser, Subcommand};

use wf_config::{FusionConfig, FusionConfigLoader, HumanDuration, parse_vars};
use wf_runtime::lifecycle::{Reactor, ShutdownTrigger, wait_for_signal};
use wf_runtime::tracing_init::init_tracing;
use wf_vars::ConfigVarContext;

#[derive(Parser)]
#[command(
    name = "wfusion",
    version = env!("CARGO_PKG_VERSION"),
    about = "WarpFusion CEP engine",
    propagate_version = true
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Args, Clone)]
struct ConfigLoadArgs {
    /// Path to wfusion.toml config file (default: conf/wfusion.toml)
    #[arg(short, long, default_value = "conf/wfusion.toml")]
    config: PathBuf,
    /// Apply one or more overlay config files after the base config
    #[arg(long)]
    overlay: Vec<PathBuf>,
    /// Override configuration variables, can be repeated: --var KEY=VALUE
    #[arg(long)]
    var: Vec<String>,
    /// Override the base directory used to resolve relative runtime paths
    #[arg(long)]
    work_dir: Option<PathBuf>,
}

#[derive(Args, Clone, Default)]
struct CompareConfigLoadArgs {
    /// Compare against another config file; defaults to the primary --config
    #[arg(long = "to-config")]
    to_config: Option<PathBuf>,
    /// Apply one or more overlay files to the comparison side
    #[arg(long = "to-overlay")]
    to_overlay: Vec<PathBuf>,
    /// Override configuration variables on the comparison side
    #[arg(long = "to-var")]
    to_var: Vec<String>,
    /// Override the base directory used to resolve relative runtime paths on the comparison side
    #[arg(long = "to-work-dir")]
    to_work_dir: Option<PathBuf>,
}

#[derive(Args, Clone, Default)]
struct PathFilterArgs {
    /// Limit output to one or more config path prefixes, e.g. runtime, sources, window.auth_events
    #[arg(long = "path-prefix")]
    path_prefix: Vec<String>,
}

#[derive(Args, Clone, Default)]
struct VarFilterArgs {
    /// Limit output to one or more variable-name prefixes, e.g. WORK, CASE_, FAIL_
    #[arg(long = "var-prefix")]
    var_prefix: Vec<String>,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the WarpFusion engine
    Run {
        #[command(flatten)]
        load: ConfigLoadArgs,
        /// Enable runtime metrics and periodic snapshot output
        #[arg(long)]
        metrics: bool,
        /// Override metrics report interval (e.g. "2s", "30s", "1m")
        #[arg(long)]
        metrics_interval: Option<String>,
        /// Override metrics listen address for /metrics endpoint
        #[arg(long)]
        metrics_listen: Option<String>,
    },
    /// Render merged configuration after overlay/path rebasing and variable expansion
    Config {
        #[command(subcommand)]
        command: ConfigCommands,
    },
}

#[derive(Subcommand)]
enum ConfigCommands {
    /// Render the effective TOML after applying overlays and variable expansion
    Render {
        #[command(flatten)]
        load: ConfigLoadArgs,
        /// Print merged raw TOML before variable expansion/validation
        #[arg(long)]
        raw: bool,
    },
    /// Show the source file that supplied each final config path
    Origins {
        #[command(flatten)]
        load: ConfigLoadArgs,
        #[command(flatten)]
        filter: PathFilterArgs,
    },
    /// Show the final materialized config variables and their sources
    Vars {
        #[command(flatten)]
        load: ConfigLoadArgs,
        #[command(flatten)]
        filter: VarFilterArgs,
    },
    /// Show field-level differences between two loaded config states
    Diff {
        #[command(flatten)]
        load: ConfigLoadArgs,
        #[command(flatten)]
        compare: CompareConfigLoadArgs,
        #[command(flatten)]
        filter: PathFilterArgs,
        /// Compare expanded values after variable substitution instead of raw merged TOML
        #[arg(long)]
        expanded: bool,
    },
}

struct ResolvedConfigLoad {
    config_path: PathBuf,
    overlay_paths: Vec<PathBuf>,
    runtime_base_dir: PathBuf,
    config_ctx: ConfigVarContext,
}

fn resolve_config_load(load: ConfigLoadArgs) -> Result<ResolvedConfigLoad> {
    resolve_config_load_parts(load.config, load.overlay, load.var, load.work_dir)
}

fn resolve_config_load_parts(
    config: PathBuf,
    overlay: Vec<PathBuf>,
    var: Vec<String>,
    work_dir: Option<PathBuf>,
) -> Result<ResolvedConfigLoad> {
    let config_path = config
        .canonicalize()
        .map_err(|e| anyhow::anyhow!("config path '{}': {e}", config.display()))?;
    let overlay_paths: Vec<PathBuf> = overlay
        .into_iter()
        .map(|path| {
            path.canonicalize()
                .map_err(|e| anyhow::anyhow!("overlay path '{}': {e}", path.display()))
        })
        .collect::<Result<_>>()?;
    let default_base_dir = config_path
        .parent()
        .expect("config path must have a parent directory");
    let runtime_base_dir = if let Some(work_dir) = work_dir {
        let path = work_dir
            .canonicalize()
            .map_err(|e| anyhow::anyhow!("work-dir path '{}': {e}", work_dir.display()))?;
        if !path.is_dir() {
            anyhow::bail!("work-dir path '{}' is not a directory", path.display());
        }
        path
    } else {
        default_base_dir.to_path_buf()
    };
    let cli_vars = parse_vars(&var)?;
    let config_ctx = ConfigVarContext::from_explicit_vars(cli_vars)
        .with_work_dir(Some(runtime_base_dir.clone()));

    Ok(ResolvedConfigLoad {
        config_path,
        overlay_paths,
        runtime_base_dir,
        config_ctx,
    })
}

fn resolve_compare_config_load(
    base: &ResolvedConfigLoad,
    compare: CompareConfigLoadArgs,
) -> Result<ResolvedConfigLoad> {
    resolve_config_load_parts(
        compare
            .to_config
            .unwrap_or_else(|| base.config_path.clone()),
        compare.to_overlay,
        compare.to_var,
        compare.to_work_dir,
    )
}

fn format_value<T: std::fmt::Display>(value: &T) -> String {
    value.to_string()
}

fn matches_any_prefix(path: &str, prefixes: &[String]) -> bool {
    prefixes.is_empty()
        || prefixes
            .iter()
            .any(|prefix| path_matches_prefix(path, prefix))
}

fn path_matches_prefix(path: &str, prefix: &str) -> bool {
    path == prefix
        || path
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with('.') || rest.starts_with('['))
}

fn matches_any_var_prefix(key: &str, prefixes: &[String]) -> bool {
    prefixes.is_empty() || prefixes.iter().any(|prefix| key.starts_with(prefix))
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Run {
            load,
            metrics,
            metrics_interval,
            metrics_listen,
        } => {
            let resolved = resolve_config_load(load)?;
            let mut fusion_config = FusionConfig::load_with_overlays(
                &resolved.config_path,
                &resolved.overlay_paths,
                &resolved.config_ctx,
            )?;
            if metrics || metrics_interval.is_some() || metrics_listen.is_some() {
                fusion_config.metrics.enabled = true;
            }
            if let Some(interval) = metrics_interval {
                fusion_config.metrics.report_interval = HumanDuration::from_str(&interval)
                    .map_err(|e| anyhow::anyhow!("invalid --metrics-interval '{interval}': {e}"))?;
            }
            if let Some(listen) = metrics_listen {
                fusion_config.metrics.prometheus_listen = listen;
            }
            let metrics_enabled = fusion_config.metrics.enabled;
            let metrics_interval = fusion_config.metrics.report_interval;
            let metrics_listen = fusion_config.metrics.prometheus_listen.clone();

            let _guard = init_tracing(&fusion_config.logging, &resolved.runtime_base_dir)?;

            let reactor = Reactor::start(fusion_config, &resolved.runtime_base_dir)
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            if let Some(listen_addr) = reactor.listen_addr() {
                tracing::info!(domain = "sys", listen = %listen_addr, "WarpFusion reactor started");
            } else {
                tracing::info!(
                    domain = "sys",
                    "WarpFusion reactor started without tcp listener"
                );
            }
            if metrics_enabled {
                tracing::info!(
                    domain = "res",
                    interval = %metrics_interval,
                    listen = %metrics_listen,
                    "runtime metrics enabled"
                );
            }

            if wait_for_signal(reactor.cancel_token()).await == ShutdownTrigger::Signal {
                reactor.shutdown();
            }
            reactor.wait().await.map_err(|e| anyhow::anyhow!("{e}"))?;
        }
        Commands::Config { command } => match command {
            ConfigCommands::Render { load, raw } => {
                let resolved = resolve_config_load(load)?;
                let loader = FusionConfigLoader::new(
                    &resolved.config_path,
                    &resolved.overlay_paths,
                    &resolved.config_ctx,
                );
                let rendered = if raw {
                    loader.load_merged_toml()?
                } else {
                    loader.load_expanded_toml()?
                };
                print!("{rendered}");
            }
            ConfigCommands::Origins { load, filter } => {
                let resolved = resolve_config_load(load)?;
                let raw = FusionConfigLoader::new(
                    &resolved.config_path,
                    &resolved.overlay_paths,
                    &resolved.config_ctx,
                )
                .load_raw()?;
                let mut matched = 0usize;
                for (path, origin) in raw.origin_entries() {
                    if !matches_any_prefix(&path, &filter.path_prefix) {
                        continue;
                    }
                    matched += 1;
                    println!("{path}\t{}", origin.display());
                }
                if matched == 0 {
                    println!("no matching paths");
                }
            }
            ConfigCommands::Vars { load, filter } => {
                let resolved = resolve_config_load(load)?;
                let vars = FusionConfigLoader::new(
                    &resolved.config_path,
                    &resolved.overlay_paths,
                    &resolved.config_ctx,
                )
                .load_effective_vars()?;
                let mut matched = 0usize;
                for entry in vars {
                    if !matches_any_var_prefix(&entry.key, &filter.var_prefix) {
                        continue;
                    }
                    matched += 1;
                    println!("{}\t{}\t{}", entry.key, entry.value, entry.source);
                }
                if matched == 0 {
                    println!("no matching vars");
                }
            }
            ConfigCommands::Diff {
                load,
                compare,
                filter,
                expanded,
            } => {
                let resolved = resolve_config_load(load)?;
                let compare_resolved = resolve_compare_config_load(&resolved, compare)?;
                let left_loader = FusionConfigLoader::new(
                    &resolved.config_path,
                    &resolved.overlay_paths,
                    &resolved.config_ctx,
                );
                let right_loader = FusionConfigLoader::new(
                    &compare_resolved.config_path,
                    &compare_resolved.overlay_paths,
                    &compare_resolved.config_ctx,
                );
                let left = if expanded {
                    left_loader.load_expanded_raw()?
                } else {
                    left_loader.load_raw()?
                };
                let right = if expanded {
                    right_loader.load_expanded_raw()?
                } else {
                    right_loader.load_raw()?
                };

                let changes: Vec<_> = left
                    .diff(&right)
                    .into_iter()
                    .filter(|change| matches_any_prefix(&change.path, &filter.path_prefix))
                    .collect();
                if changes.is_empty() {
                    println!("no changes");
                    return Ok(());
                }

                for change in changes {
                    println!("path: {}", change.path);
                    println!(
                        "  old: {}",
                        change
                            .old_value
                            .as_ref()
                            .map(format_value)
                            .unwrap_or_else(|| "<none>".to_string())
                    );
                    println!(
                        "  new: {}",
                        change
                            .new_value
                            .as_ref()
                            .map(format_value)
                            .unwrap_or_else(|| "<none>".to_string())
                    );
                    println!(
                        "  old_origin: {}",
                        change
                            .old_origin
                            .as_deref()
                            .map(|path| path.display().to_string())
                            .unwrap_or_else(|| "<none>".to_string())
                    );
                    println!(
                        "  new_origin: {}",
                        change
                            .new_origin
                            .as_deref()
                            .map(|path| path.display().to_string())
                            .unwrap_or_else(|| "<none>".to_string())
                    );
                }
            }
        },
    }

    Ok(())
}
