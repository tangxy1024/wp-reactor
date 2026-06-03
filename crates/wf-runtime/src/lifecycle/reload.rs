use std::collections::HashSet;
use std::path::Path;

use orion_error::prelude::*;

use wf_config::{
    ClassifiedFusionConfigChange, FusionChangeKind, FusionConfig, FusionReloadDisposition,
    FusionReloadPlan, RawFusionConfigChange, RawFusionConfigTree, WindowConfig,
    validate_over_vs_over_cap,
};
use wf_lang::WindowSchema;

use crate::error::{RuntimeReason, RuntimeResult};

use super::compile::{
    build_pipeline_internal_windows, build_run_rules, build_runtime_var_context,
    collect_intermediate_targets, compile_rules, load_schemas,
};
use super::types::RunRule;

#[derive(::moju_derive::MoJu)]
#[moju(kind = "struct", domain = "Orchestra", module = "Orchestra.HotReload")]
pub struct PreparedRuleReload {
    pub plan: FusionReloadPlan,
    pub next_raw: RawFusionConfigTree,
    pub next_config: FusionConfig,
    #[allow(dead_code)]
    pub(super) next_rules: Vec<RunRule>,
    pub next_intermediate_targets: HashSet<String>,
    pub next_schemas: Vec<WindowSchema>,
}

#[derive(::moju_derive::MoJu)]
#[moju(kind = "state", domain = "Orchestra", module = "Orchestra.HotReload")]
pub enum ReloadPreparation {
    Ready(Box<PreparedRuleReload>),
    Blocked(FusionReloadPlan),
}

pub fn prepare_reload(
    current_raw: &RawFusionConfigTree,
    current_config: &FusionConfig,
    next_raw: RawFusionConfigTree,
    next_config: FusionConfig,
    base_dir: &Path,
) -> RuntimeResult<ReloadPreparation> {
    let mut plan = current_raw.build_reload_plan(&next_raw);
    if plan.has_blockers() {
        return Ok(ReloadPreparation::Blocked(plan));
    }

    append_effective_config_blockers(
        &mut plan,
        current_raw,
        current_config,
        &next_raw,
        &next_config,
    );
    if plan.has_blockers() {
        return Ok(ReloadPreparation::Blocked(plan));
    }

    let current_artifacts = compile_reload_artifacts(current_config, base_dir)?;
    let next_artifacts = compile_reload_artifacts(&next_config, base_dir)?;

    append_topology_blockers(&mut plan, &current_artifacts, &next_artifacts);
    if plan.has_blockers() {
        return Ok(ReloadPreparation::Blocked(plan));
    }

    Ok(ReloadPreparation::Ready(Box::new(PreparedRuleReload {
        plan,
        next_raw,
        next_config,
        next_rules: next_artifacts.run_rules,
        next_intermediate_targets: next_artifacts.intermediate_targets,
        next_schemas: next_artifacts.runtime_schemas,
    })))
}

struct CompiledReloadArtifacts {
    run_rules: Vec<RunRule>,
    intermediate_targets: HashSet<String>,
    runtime_schemas: Vec<WindowSchema>,
    runtime_window_configs: Vec<WindowConfig>,
}

fn compile_reload_artifacts(
    config: &FusionConfig,
    base_dir: &Path,
) -> RuntimeResult<CompiledReloadArtifacts> {
    let all_schemas = load_schemas(&config.runtime.schemas, base_dir)?;
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

    let window_overs = runtime_schemas
        .iter()
        .map(|schema| (schema.name.clone(), schema.over))
        .collect();
    validate_over_vs_over_cap(&runtime_window_configs, &window_overs).source_err(
        RuntimeReason::core_conf(),
        "validate window over vs over_cap",
    )?;

    let run_rules = build_run_rules(&all_rule_plans, &runtime_schemas);
    Ok(CompiledReloadArtifacts {
        run_rules,
        intermediate_targets,
        runtime_schemas,
        runtime_window_configs,
    })
}

fn append_effective_config_blockers(
    plan: &mut FusionReloadPlan,
    current_raw: &RawFusionConfigTree,
    current_config: &FusionConfig,
    next_raw: &RawFusionConfigTree,
    next_config: &FusionConfig,
) {
    push_effective_blocker_if_changed(
        plan,
        current_raw,
        next_raw,
        "mode",
        FusionChangeKind::Mode,
        "effective mode changed after variable expansion; lifecycle semantics require restart",
        current_config.mode != next_config.mode,
    );
    push_effective_blocker_if_changed(
        plan,
        current_raw,
        next_raw,
        "sinks",
        FusionChangeKind::Sinks,
        "effective sink root changed after variable expansion; sink topology must be rebuilt",
        current_config.sinks != next_config.sinks,
    );
    push_effective_blocker_if_changed(
        plan,
        current_raw,
        next_raw,
        "work_root",
        FusionChangeKind::Sinks,
        "effective work_root changed after variable expansion; sink runtime paths require restart",
        current_config.work_root != next_config.work_root,
    );
    push_effective_blocker_if_changed(
        plan,
        current_raw,
        next_raw,
        "runtime.executor_parallelism",
        FusionChangeKind::Runtime,
        "effective runtime.executor_parallelism changed after variable expansion; task layout requires restart",
        current_config.runtime.executor_parallelism != next_config.runtime.executor_parallelism,
    );
    push_effective_blocker_if_changed(
        plan,
        current_raw,
        next_raw,
        "runtime.rule_exec_timeout",
        FusionChangeKind::Runtime,
        "effective runtime.rule_exec_timeout changed after variable expansion; rule task behavior requires restart",
        current_config.runtime.rule_exec_timeout != next_config.runtime.rule_exec_timeout,
    );
    push_effective_blocker_if_changed(
        plan,
        current_raw,
        next_raw,
        "runtime.schemas",
        FusionChangeKind::Runtime,
        "effective runtime.schemas changed after variable expansion; schema catalog must be rebuilt",
        current_config.runtime.schemas != next_config.runtime.schemas,
    );
    push_effective_blocker_if_changed(
        plan,
        current_raw,
        next_raw,
        "sources",
        FusionChangeKind::Sources,
        "effective sources changed after variable expansion; receiver tasks require restart",
        current_config.sources != next_config.sources,
    );
    push_effective_blocker_if_changed(
        plan,
        current_raw,
        next_raw,
        "window_defaults",
        FusionChangeKind::Windows,
        "effective window_defaults changed after variable expansion; window lifecycle requires restart",
        current_config.window_defaults != next_config.window_defaults,
    );
    push_effective_blocker_if_changed(
        plan,
        current_raw,
        next_raw,
        "window",
        FusionChangeKind::Windows,
        "effective window config changed after variable expansion; window registry requires restart",
        current_config.windows != next_config.windows,
    );
    push_effective_blocker_if_changed(
        plan,
        current_raw,
        next_raw,
        "logging",
        FusionChangeKind::Logging,
        "effective logging config changed after variable expansion; logging pipeline is not hot-reloadable",
        current_config.logging != next_config.logging,
    );
    push_effective_blocker_if_changed(
        plan,
        current_raw,
        next_raw,
        "metrics",
        FusionChangeKind::Metrics,
        "effective metrics config changed after variable expansion; metrics tasks require restart",
        current_config.metrics != next_config.metrics,
    );
}

fn push_effective_blocker_if_changed(
    plan: &mut FusionReloadPlan,
    current_raw: &RawFusionConfigTree,
    next_raw: &RawFusionConfigTree,
    path: &'static str,
    kind: FusionChangeKind,
    reason: &'static str,
    changed: bool,
) {
    if !changed {
        return;
    }
    plan.requires_restart.push(ClassifiedFusionConfigChange {
        change: RawFusionConfigChange {
            path: path.to_string(),
            old_value: None,
            new_value: None,
            old_origin: current_raw.origin_for(path).map(Path::to_path_buf),
            new_origin: next_raw.origin_for(path).map(Path::to_path_buf),
        },
        kind,
        disposition: FusionReloadDisposition::RequiresRestart,
        reason,
    });
}

fn append_topology_blockers(
    plan: &mut FusionReloadPlan,
    current: &CompiledReloadArtifacts,
    next: &CompiledReloadArtifacts,
) {
    if normalize_schemas(&current.runtime_schemas) != normalize_schemas(&next.runtime_schemas) {
        plan.requires_restart.push(synthetic_restart_change(
            "__derived.runtime_schemas",
            FusionChangeKind::Runtime,
            "compiled runtime schema set changed; router and window registry rebuild is required",
        ));
    }

    if normalize_window_configs(&current.runtime_window_configs)
        != normalize_window_configs(&next.runtime_window_configs)
    {
        plan.requires_restart.push(synthetic_restart_change(
            "__derived.runtime_window_configs",
            FusionChangeKind::Windows,
            "compiled runtime window configs changed; in-memory window layout requires restart",
        ));
    }
}

fn synthetic_restart_change(
    path: &'static str,
    kind: FusionChangeKind,
    reason: &'static str,
) -> ClassifiedFusionConfigChange {
    ClassifiedFusionConfigChange {
        change: RawFusionConfigChange {
            path: path.to_string(),
            old_value: None,
            new_value: None,
            old_origin: None,
            new_origin: None,
        },
        kind,
        disposition: FusionReloadDisposition::RequiresRestart,
        reason,
    }
}

fn normalize_schemas(schemas: &[WindowSchema]) -> Vec<WindowSchema> {
    let mut out = schemas.to_vec();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn normalize_window_configs(configs: &[WindowConfig]) -> Vec<WindowConfig> {
    let mut out = configs.to_vec();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use wf_config::FusionConfigLoader;
    use wf_vars::ConfigVarContext;

    use super::*;

    fn make_temp_dir(name: &str) -> PathBuf {
        let unique = format!(
            "wf-runtime-reload-{}-{}-{}",
            name,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time before unix epoch")
                .as_nanos()
        );
        let dir = std::env::temp_dir().join(unique);
        std::fs::create_dir_all(&dir).expect("failed to create temp dir");
        dir
    }

    fn write_file(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("failed to create parent dir");
        }
        std::fs::write(path, content).expect("failed to write test file");
    }

    fn base_config(runtime_schemas: &str, runtime_rules: &str, vars_block: &str) -> String {
        format!(
            r#"
mode = "daemon"
sinks = "sinks"

[[sources]]
type = "tcp"
name = "ingress"
listen = "tcp://127.0.0.1:0"

[runtime]
executor_parallelism = 2
rule_exec_timeout = "30s"
schemas = "{runtime_schemas}"
rules = "{runtime_rules}"

[window_defaults]
evict_interval = "30s"
max_window_bytes = "256MB"
max_total_bytes = "2GB"
evict_policy = "time_first"
watermark = "5s"
allowed_lateness = "0s"
late_policy = "drop"

[window.auth_events]
mode = "local"
max_window_bytes = "256MB"
over_cap = "30m"

[window.security_alerts]
mode = "local"
max_window_bytes = "64MB"
over_cap = "1h"
{vars_block}
"#
        )
    }

    fn security_schema() -> &'static str {
        r#"
window auth_events {
    stream = "syslog"
    time = event_time
    over = 5m

    fields {
        sip: ip
        username: chars
        action: chars
        event_time: time
    }
}

window security_alerts {
    over = 0
    fields {
        sip: ip
        fail_count: digit
        message: chars
    }
}
"#
    }

    fn simple_rule() -> &'static str {
        r#"
rule brute_force_then_scan {
  events {
    fail : auth_events && action == "failed"
  }

  match<sip:5m> {
    on event {
      fail | count >= ${FAIL_THRESHOLD:3};
    }
    and close {
      fail | count >= 1;
    }
  } -> score(70.0)

  entity(ip, fail.sip)

  yield security_alerts (
    sip = fail.sip,
    fail_count = count(fail),
    message = fmt("{} brute force detected", fail.sip)
  )
}
"#
    }

    fn pipeline_rule() -> &'static str {
        r#"
rule repeated_fail_bursts {
  events {
    e : auth_events && action == "failed"
  }

  match<sip,username:5m:fixed> {
    on event {
      e | count >= 1;
    }
    and close {
      burst: e | count >= 3;
    }
  }
  |> match<sip:30m:fixed> {
    on event {
      _in | count >= 1;
    }
    and close {
      users: _in.username | distinct | count >= 2;
    }
  } -> score(85.0)

  entity(ip, _in.sip)

  yield security_alerts (
    sip = _in.sip,
    fail_count = 2,
    message = fmt("{} multi-user fail bursts", _in.sip)
  )
}
"#
    }

    fn load_state(
        base_path: &Path,
        overlay_paths: &[PathBuf],
    ) -> (RawFusionConfigTree, FusionConfig) {
        let ctx = ConfigVarContext::new();
        let loader = FusionConfigLoader::new(base_path, overlay_paths, &ctx, None);
        let raw = loader.load_raw().expect("load raw config");
        let config = loader.load().expect("load config");
        (raw, config)
    }

    #[test]
    fn prepare_reload_accepts_vars_only_rule_recompile() {
        let root = make_temp_dir("vars-ready");
        let base_path = root.join("conf/wfusion.toml");
        let next_overlay = root.join("env/dev/vars.toml");
        write_file(
            &base_path,
            &base_config(
                "../schemas/*.wfs",
                "../rules/current/*.wfl",
                r#"
[vars]
FAIL_THRESHOLD = "3"
"#,
            ),
        );
        write_file(&root.join("schemas/security.wfs"), security_schema());
        write_file(&root.join("rules/current/brute_force.wfl"), simple_rule());
        write_file(
            &next_overlay,
            r#"
[vars]
FAIL_THRESHOLD = "5"
"#,
        );

        let (current_raw, current_config) = load_state(&base_path, &[]);
        let (next_raw, next_config) = load_state(&base_path, std::slice::from_ref(&next_overlay));

        let prepared = prepare_reload(
            &current_raw,
            &current_config,
            next_raw,
            next_config,
            base_path.parent().expect("base config dir"),
        )
        .expect("prepare reload");

        match prepared {
            ReloadPreparation::Ready(reload) => {
                assert_eq!(reload.plan.hot_reload.len(), 1);
                assert!(reload.plan.requires_restart.is_empty());
                assert!(reload.plan.unsupported.is_empty());
                assert_eq!(reload.next_rules.len(), 1);
                assert!(reload.next_intermediate_targets.is_empty());
                assert_eq!(reload.next_schemas.len(), 2);
            }
            ReloadPreparation::Blocked(plan) => {
                panic!(
                    "expected hot reload to be ready, blockers: {:?}",
                    plan.requires_restart
                );
            }
        }

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn prepare_reload_blocks_vars_that_change_effective_runtime_settings() {
        let root = make_temp_dir("vars-blocked-effective");
        let base_path = root.join("conf/wfusion.toml");
        let next_overlay = root.join("env/dev/vars.toml");
        write_file(
            &base_path,
            &base_config(
                "${SCHEMA_GLOB}",
                "../rules/current/*.wfl",
                r#"
[vars]
SCHEMA_GLOB = "../schemas/*.wfs"
FAIL_THRESHOLD = "3"
"#,
            ),
        );
        write_file(&root.join("schemas/security.wfs"), security_schema());
        write_file(&root.join("schemas_alt/security.wfs"), security_schema());
        write_file(&root.join("rules/current/brute_force.wfl"), simple_rule());
        write_file(
            &next_overlay,
            r#"
[vars]
SCHEMA_GLOB = "../schemas_alt/*.wfs"
"#,
        );

        let (current_raw, current_config) = load_state(&base_path, &[]);
        let (next_raw, next_config) = load_state(&base_path, std::slice::from_ref(&next_overlay));

        let prepared = prepare_reload(
            &current_raw,
            &current_config,
            next_raw,
            next_config,
            base_path.parent().expect("base config dir"),
        )
        .expect("prepare reload");

        match prepared {
            ReloadPreparation::Ready(_) => {
                panic!("expected effective runtime.schemas change to require restart");
            }
            ReloadPreparation::Blocked(plan) => {
                assert!(plan.requires_restart.iter().any(|change| {
                    change.change.path == "runtime.schemas"
                        && change.kind == FusionChangeKind::Runtime
                }));
            }
        }

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn prepare_reload_blocks_rule_change_that_alters_runtime_topology() {
        let root = make_temp_dir("rules-blocked-topology");
        let base_path = root.join("conf/wfusion.toml");
        let next_overlay = root.join("env/dev/rules.toml");
        write_file(
            &base_path,
            &base_config("../schemas/*.wfs", "../rules/v1/*.wfl", ""),
        );
        write_file(&root.join("schemas/security.wfs"), security_schema());
        write_file(&root.join("rules/v1/brute_force.wfl"), simple_rule());
        write_file(
            &root.join("rules/v2/repeated_fail_bursts.wfl"),
            pipeline_rule(),
        );
        write_file(
            &next_overlay,
            r#"
[runtime]
rules = "../../rules/v2/*.wfl"
"#,
        );

        let (current_raw, current_config) = load_state(&base_path, &[]);
        let (next_raw, next_config) = load_state(&base_path, std::slice::from_ref(&next_overlay));

        let prepared = prepare_reload(
            &current_raw,
            &current_config,
            next_raw,
            next_config,
            base_path.parent().expect("base config dir"),
        )
        .expect("prepare reload");

        match prepared {
            ReloadPreparation::Ready(_) => {
                panic!("expected topology-changing rule reload to be blocked");
            }
            ReloadPreparation::Blocked(plan) => {
                assert!(plan.requires_restart.iter().any(|change| {
                    change.change.path == "__derived.runtime_schemas"
                        || change.change.path == "__derived.runtime_window_configs"
                }));
            }
        }

        let _ = std::fs::remove_dir_all(root);
    }
}
