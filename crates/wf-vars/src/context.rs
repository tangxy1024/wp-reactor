use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct ConfigVarContext {
    explicit_vars: HashMap<String, String>,
    env_vars: HashMap<String, String>,
    config_dir: Option<PathBuf>,
    work_dir: Option<PathBuf>,
    work_root: Option<PathBuf>,
}

impl Default for ConfigVarContext {
    fn default() -> Self {
        Self {
            explicit_vars: HashMap::new(),
            env_vars: std::env::vars().collect(),
            config_dir: None,
            work_dir: None,
            work_root: None,
        }
    }
}

impl ConfigVarContext {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_explicit_vars(explicit_vars: HashMap<String, String>) -> Self {
        Self {
            explicit_vars,
            ..Self::default()
        }
    }

    pub fn with_config_dir(mut self, config_dir: impl Into<PathBuf>) -> Self {
        self.config_dir = Some(config_dir.into());
        self
    }

    pub fn with_work_dir(mut self, work_dir: Option<PathBuf>) -> Self {
        self.work_dir = work_dir;
        self
    }

    pub fn with_work_root(mut self, work_root: Option<PathBuf>) -> Self {
        self.work_root = work_root;
        self
    }

    pub fn work_dir(&self) -> Option<&Path> {
        self.work_dir.as_deref()
    }

    pub fn explicit_vars(&self) -> &HashMap<String, String> {
        &self.explicit_vars
    }

    pub fn env_var_value(&self, ident: &str) -> Option<String> {
        self.env_vars.get(ident).cloned()
    }

    pub fn work_root(&self) -> Option<&Path> {
        self.work_root.as_deref()
    }

    pub fn config_dir(&self) -> Option<&Path> {
        self.config_dir.as_deref()
    }

    pub fn for_file(&self, path: &Path) -> Self {
        let mut next = self.clone();
        next.config_dir = path.parent().map(Path::to_path_buf);
        next
    }

    pub fn for_dir(&self, dir: &Path) -> Self {
        let mut next = self.clone();
        next.config_dir = Some(dir.to_path_buf());
        next
    }

    pub fn resolve_external_var(&self, ident: &str) -> Option<String> {
        self.explicit_vars
            .get(ident)
            .cloned()
            .or_else(|| self.builtin_var(ident))
            .or_else(|| self.env_vars.get(ident).cloned())
    }

    pub fn materialize_vars(&self, file_vars: &HashMap<String, String>) -> HashMap<String, String> {
        let mut out = file_vars.clone();
        for (key, value) in self.builtin_vars() {
            out.entry(key).or_insert(value);
        }
        for (key, value) in &self.explicit_vars {
            out.insert(key.clone(), value.clone());
        }
        out
    }

    fn builtin_vars(&self) -> Vec<(String, String)> {
        let mut vars = Vec::new();
        if let Some(config_dir) = &self.config_dir {
            vars.push((
                "CONFIG_DIR".to_string(),
                config_dir.to_string_lossy().to_string(),
            ));
        }
        if let Some(work_dir) = &self.work_dir {
            vars.push((
                "WORK_DIR".to_string(),
                work_dir.to_string_lossy().to_string(),
            ));
        }
        if let Some(work_root) = &self.work_root {
            vars.push((
                "WORK_ROOT".to_string(),
                work_root.to_string_lossy().to_string(),
            ));
        }
        vars
    }

    fn builtin_var(&self, ident: &str) -> Option<String> {
        match ident {
            "CONFIG_DIR" => self
                .config_dir
                .as_ref()
                .map(|path| path.to_string_lossy().to_string()),
            "WORK_DIR" => self
                .work_dir
                .as_ref()
                .map(|path| path.to_string_lossy().to_string()),
            "WORK_ROOT" => self
                .work_root
                .as_ref()
                .map(|path| path.to_string_lossy().to_string()),
            _ => None,
        }
    }

    pub fn builtin_var_value(&self, ident: &str) -> Option<String> {
        self.builtin_var(ident)
    }
}
