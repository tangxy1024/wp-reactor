use orion_error::conversion::ToStructError;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use orion_error::prelude::*;
use wp_core_connectors::sinks::arrow_file::ArrowFileFactory;
use wp_core_connectors::sinks::arrow_ipc::ArrowIpcFactory;
use wp_core_connectors::sinks::blackhole_factory::BlackHoleFactory;
use wp_core_connectors::sinks::file_factory::FileFactory;
use wp_core_connectors::sinks::syslog::SyslogFactory;
use wp_core_connectors::sinks::tcp::TcpFactory;

use wf_config::FusionConfig;
use wf_engine::window::{Router, WindowRegistry};
use wf_config::ConfigVarContext;

use crate::error::{RuntimeReason, RuntimeResult};
use crate::schema_bridge::schemas_to_window_defs;
use crate::sink_build::{SinkFactoryRegistry, build_sink_dispatcher};

use super::compile::{
    build_pipeline_internal_windows, build_run_rules, build_runtime_var_context,
    collect_intermediate_targets, compile_rules, load_schemas, resolve_work_root,
};
use super::types::BootstrapData;

// ---------------------------------------------------------------------------
// Phase 1: load_and_compile — pure data transforms + async sink build
// ---------------------------------------------------------------------------

/// Load schemas, compile rules, validate config, build engines and sink dispatcher.
pub(super) async fn load_and_compile(
    config: &FusionConfig,
    base_dir: &Path,
) -> RuntimeResult<BootstrapData> {
    // 1. Load .wfs files → Vec<WindowSchema>
    let all_schemas = load_schemas(&config.runtime.schemas, base_dir)?;

    // 2. Preprocess .wfl with config.vars → parse → compile → Vec<RulePlan>
    let var_ctx = build_runtime_var_context(config, base_dir);
    let (all_rule_plans, effective_schemas) =
        compile_rules(&config.runtime.rules, base_dir, &var_ctx, &all_schemas)?;
    let intermediate_targets = collect_intermediate_targets(&all_rule_plans);
    let (pipeline_schemas, pipeline_window_configs) = build_pipeline_internal_windows(
        &all_rule_plans,
        &effective_schemas,
        &config.window_defaults,
    );
    let mut runtime_schemas = effective_schemas;
    runtime_schemas.extend(pipeline_schemas);
    let mut runtime_window_configs = config.windows.clone();
    runtime_window_configs.extend(pipeline_window_configs);

    // 3. Cross-validate over vs over_cap
    let window_overs: HashMap<String, Duration> = runtime_schemas
        .iter()
        .map(|ws| (ws.name.clone(), ws.over))
        .collect();
    wf_config::validate_over_vs_over_cap(&runtime_window_configs, &window_overs).source_err(
        RuntimeReason::core_conf(),
        "validate window over vs over_cap",
    )?;
    wf_debug!(
        conf,
        windows = config.windows.len(),
        "over vs over_cap validation passed"
    );

    // 4. Schema bridge: WindowSchema × WindowConfig → Vec<WindowDef>
    let window_defs = schemas_to_window_defs(&runtime_schemas, &runtime_window_configs)
        .source_err(RuntimeReason::Bootstrap, "build window definitions")?;

    // 5. WindowRegistry::build → registry
    let registry = WindowRegistry::build(window_defs).conv_err()?;

    // 5.5. Auto-load knowdb.toml if present in config directory
    let knowdb_path = base_dir.join("knowdb.toml");
    if knowdb_path.exists() {
        load_knowledge_into_windows(&knowdb_path, base_dir, &registry)?;
    }

    // 6. Router::new(registry)
    let router = Arc::new(Router::new(registry));

    // 7. Build RunRules (precompute stream_name → alias routing)
    let rules = build_run_rules(&all_rule_plans, &runtime_schemas);

    // 8. Build connector-based sink dispatcher
    let sinks_dir = base_dir.join(&config.sinks);
    let work_root = resolve_work_root(config, base_dir);
    let mut scoped_vars = config.vars.clone();
    scoped_vars
        .entry("WORK_DIR".to_string())
        .or_insert_with(|| base_dir.to_string_lossy().to_string());
    scoped_vars
        .entry("WORK_ROOT".to_string())
        .or_insert_with(|| work_root.to_string_lossy().to_string());
    let bundle_ctx = ConfigVarContext::from_explicit_vars(scoped_vars);
    let bundle =
        wf_config::sink::load_sink_config_with_context(&sinks_dir, &bundle_ctx, Some(base_dir))
            .source_err(RuntimeReason::core_conf(), "load sink config")?;
    let mut factory_registry = SinkFactoryRegistry::new();
    factory_registry.register(Arc::new(FileFactory));
    factory_registry.register(Arc::new(ArrowIpcFactory));
    factory_registry.register(Arc::new(ArrowFileFactory));
    factory_registry.register(Arc::new(SyslogFactory));
    factory_registry.register(Arc::new(TcpFactory));
    factory_registry.register(Arc::new(BlackHoleFactory));
    let window_names: Vec<String> = config.windows.iter().map(|w| w.name.clone()).collect();
    let dispatcher = Arc::new(
        build_sink_dispatcher(&bundle, &factory_registry, &work_root, &window_names)
            .await
            .source_err(RuntimeReason::Bootstrap, "build sink dispatcher")?,
    );

    let schema_count = runtime_schemas.len();
    Ok(BootstrapData {
        rules,
        router,
        dispatcher,
        schema_count,
        schemas: runtime_schemas,
        intermediate_targets,
    })
}

/// Load knowdb CSV tables directly into matching static windows.
fn load_knowledge_into_windows(
    knowdb_path: &Path,
    _base_dir: &Path,
    registry: &WindowRegistry,
) -> RuntimeResult<()> {
    let content = std::fs::read_to_string(knowdb_path)
        .source_err(RuntimeReason::Bootstrap, format!("read {}", knowdb_path.display()))?;
    let config: toml::Value = toml::from_str(&content)
        .source_raw_err(RuntimeReason::Bootstrap, format!("parse {}", knowdb_path.display()))?;

    let tables = config.get("tables").and_then(|t| t.as_array());
    let Some(tables) = tables else { return Ok(()); };

    let base = config.get("base_dir").and_then(|b| b.as_str()).unwrap_or(".");
    let data_base_dir = knowdb_path.parent().unwrap_or(Path::new(".")).join(base);

    for table in tables {
        let name = table.get("name").and_then(|n| n.as_str()).unwrap_or("");
        let enabled = table.get("enabled").and_then(|e| e.as_bool()).unwrap_or(true);
        if !enabled || name.is_empty() { continue; }

        let Some(window_arc) = registry.get_window(name) else { continue; };

        let dir = table.get("dir").and_then(|d| d.as_str()).unwrap_or(name);
        let data_file = table.get("data_file").and_then(|d| d.as_str()).unwrap_or("data.csv");
        let csv_path = data_base_dir.join(dir).join(data_file);
        if !csv_path.exists() { continue; }

        let schema = { let win = window_arc.read().expect("lock"); win.schema().clone() };

        let mut reader = csv::ReaderBuilder::new()
            .has_headers(true).flexible(true)
            .from_path(&csv_path)
            .map_err(|e| RuntimeReason::Bootstrap.to_err()
                .with_detail(format!("open csv {}: {}", csv_path.display(), e)))?;

        let headers: Vec<String> = reader.headers().map_err(|e| {
            RuntimeReason::Bootstrap.to_err().with_detail(format!("csv headers: {}", e))
        })?.iter().map(|h| h.to_string()).collect();

        let mut rows: Vec<serde_json::Map<String, serde_json::Value>> = Vec::with_capacity(1024);
        for result in reader.records() {
            let record = result.map_err(|e| {
                RuntimeReason::Bootstrap.to_err().with_detail(format!("csv row: {}", e))
            })?;
            let mut map = serde_json::Map::new();
            for (i, value) in record.iter().enumerate() {
                let field = headers.get(i).cloned().unwrap_or_else(|| format!("col_{}", i));
                map.insert(field, serde_json::Value::String(value.to_string()));
            }
            rows.push(map);
        }
        if rows.is_empty() { continue; }

        let batch = crate::receiver::build_record_batch_from_json(&schema, &rows)?;
        let mut win = window_arc.write().expect("lock");
        win.append_with_watermark(batch)
            .source_err(RuntimeReason::Bootstrap, "append knowdb data")?;

        wf_info!(conf, table = %name, rows = rows.len(), "knowdb data loaded");
    }
    Ok(())
}
