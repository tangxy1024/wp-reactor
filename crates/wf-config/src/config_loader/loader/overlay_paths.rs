use std::ffi::OsString;
use std::path::{Path, PathBuf};

use toml::Value as TomlValue;

pub(crate) fn rebase_overlay_paths(
    value: &mut TomlValue,
    overlay_dir: &Path,
    target_base_dir: &Path,
) {
    rebase_top_level_path(value, "sinks", overlay_dir, target_base_dir);
    rebase_top_level_path(value, "work_root", overlay_dir, target_base_dir);
    rebase_nested_path(value, &["runtime", "schemas"], overlay_dir, target_base_dir);
    rebase_nested_path(value, &["runtime", "rules"], overlay_dir, target_base_dir);
    rebase_nested_path(value, &["logging", "file"], overlay_dir, target_base_dir);
    rebase_source_paths(value, overlay_dir, target_base_dir);
}

fn rebase_top_level_path(
    value: &mut TomlValue,
    key: &str,
    overlay_dir: &Path,
    target_base_dir: &Path,
) {
    let Some(table) = value.as_table_mut() else {
        return;
    };
    let Some(entry) = table.get_mut(key) else {
        return;
    };
    rebase_path_string_value(entry, overlay_dir, target_base_dir);
}

fn rebase_nested_path(
    value: &mut TomlValue,
    path: &[&str],
    overlay_dir: &Path,
    target_base_dir: &Path,
) {
    if path.is_empty() {
        return;
    }
    let mut current = value;
    for key in &path[..path.len() - 1] {
        let Some(next) = current.get_mut(*key) else {
            return;
        };
        current = next;
    }
    let Some(last) = path.last() else {
        return;
    };
    let Some(entry) = current.get_mut(*last) else {
        return;
    };
    rebase_path_string_value(entry, overlay_dir, target_base_dir);
}

fn rebase_source_paths(value: &mut TomlValue, overlay_dir: &Path, target_base_dir: &Path) {
    let Some(sources) = value.get_mut("sources").and_then(TomlValue::as_array_mut) else {
        return;
    };
    for source in sources {
        let Some(path_value) = source.get_mut("path") else {
            continue;
        };
        rebase_path_string_value(path_value, overlay_dir, target_base_dir);
    }
}

fn rebase_path_string_value(value: &mut TomlValue, overlay_dir: &Path, target_base_dir: &Path) {
    let Some(raw) = value.as_str() else {
        return;
    };
    let Some(rebased) = rebase_relative_path_string(raw, overlay_dir, target_base_dir) else {
        return;
    };
    *value = TomlValue::String(rebased);
}

fn rebase_relative_path_string(
    raw: &str,
    overlay_dir: &Path,
    target_base_dir: &Path,
) -> Option<String> {
    if raw.contains('$') || raw.is_empty() {
        return None;
    }
    let candidate = Path::new(raw);
    if candidate.is_absolute() {
        return None;
    }

    let overlay_target = normalize_path(overlay_dir.join(candidate));
    let rebased = diff_paths(&overlay_target, target_base_dir)
        .unwrap_or(overlay_target)
        .to_string_lossy()
        .to_string();
    Some(rebased)
}

fn normalize_path(path: PathBuf) -> PathBuf {
    use std::path::Component;

    let mut prefix: Option<OsString> = None;
    let mut has_root = false;
    let mut parts: Vec<OsString> = Vec::new();

    for component in path.components() {
        match component {
            Component::Prefix(p) => prefix = Some(p.as_os_str().to_os_string()),
            Component::RootDir => has_root = true,
            Component::CurDir => {}
            Component::ParentDir => {
                if let Some(last) = parts.last()
                    && last != ".."
                {
                    parts.pop();
                    continue;
                }
                if !has_root {
                    parts.push(OsString::from(".."));
                }
            }
            Component::Normal(part) => parts.push(part.to_os_string()),
        }
    }

    let mut out = PathBuf::new();
    if let Some(prefix) = prefix {
        out.push(prefix);
    }
    if has_root {
        out.push(Path::new("/"));
    }
    for part in parts {
        out.push(part);
    }
    if out.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        out
    }
}

fn diff_paths(path: &Path, base: &Path) -> Option<PathBuf> {
    use std::path::Component;

    let path_components: Vec<Component<'_>> = path.components().collect();
    let base_components: Vec<Component<'_>> = base.components().collect();

    let same_prefix_kind = matches!(
        (path_components.first(), base_components.first()),
        (Some(Component::Prefix(a)), Some(Component::Prefix(b))) if a == b
    ) || !matches!(
        (path_components.first(), base_components.first()),
        (Some(Component::Prefix(_)), _) | (_, Some(Component::Prefix(_)))
    );
    if !same_prefix_kind {
        return None;
    }

    let mut common = 0usize;
    while common < path_components.len()
        && common < base_components.len()
        && path_components[common] == base_components[common]
    {
        common += 1;
    }

    let mut result = PathBuf::new();
    for component in &base_components[common..] {
        if matches!(component, Component::Normal(_)) {
            result.push("..");
        }
    }
    for component in &path_components[common..] {
        result.push(component.as_os_str());
    }

    if result.as_os_str().is_empty() {
        Some(PathBuf::from("."))
    } else {
        Some(result)
    }
}
