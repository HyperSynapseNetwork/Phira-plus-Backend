//! Controlled deployment adapter.
//!
//! This is deliberately not a remote shell. Operators configure fixed argv
//! arrays through environment variables; clients may only provide values for
//! explicitly allowlisted structured startup arguments.

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::process::Command;

#[derive(Debug, Clone)]
struct FixedCommand {
    argv: Vec<String>,
    cwd: Option<PathBuf>,
}

#[derive(Debug, Clone, Deserialize, Serialize, utoipa::ToSchema)]
pub struct StartupArgSpec {
    pub key: String,
    pub flag: String,
    #[serde(default = "default_arg_kind")]
    pub kind: String,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub min: Option<i64>,
    #[serde(default)]
    pub max: Option<i64>,
    #[serde(default)]
    pub max_len: Option<usize>,
    #[serde(default)]
    pub allowed_values: Vec<String>,
}

fn default_arg_kind() -> String { "string".to_string() }

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct DeploymentCapabilities {
    pub supervisor_start: bool,
    pub supervisor_stop: bool,
    pub startup_args: Vec<StartupArgSpec>,
    pub ppf_build: bool,
    pub backup: bool,
}

#[derive(Debug, Clone)]
pub struct DeploymentAdapter {
    supervisor_start: Option<FixedCommand>,
    supervisor_stop: Option<FixedCommand>,
    startup_args: Vec<StartupArgSpec>,
    ppf_build: Option<FixedCommand>,
    backup: Option<FixedCommand>,
    env_allowlist: Vec<String>,
    timeout: Duration,
}

impl DeploymentAdapter {
    pub fn from_env() -> Result<Self, anyhow::Error> {
        let timeout_secs = env::var("PPB_DEPLOYMENT_COMMAND_TIMEOUT_SECS")
            .ok().and_then(|v| v.parse::<u64>().ok()).unwrap_or(120).clamp(5, 3600);
        let startup_args = match non_empty_env("PPB_PMP_SUPERVISOR_ARG_SCHEMA_JSON") {
            Some(raw) => {
                let specs: Vec<StartupArgSpec> = serde_json::from_str(&raw)
                    .map_err(|e| anyhow::anyhow!("PPB_PMP_SUPERVISOR_ARG_SCHEMA_JSON invalid JSON: {e}"))?;
                validate_arg_specs(&specs)?;
                specs
            }
            None => Vec::new(),
        };
        let env_allowlist = non_empty_env("PPB_DEPLOYMENT_ENV_ALLOWLIST")
            .map(|raw| raw.split(',').map(str::trim)
                .filter(|name| !name.is_empty() && is_safe_env_name(name))
                .map(str::to_string).collect())
            .unwrap_or_default();
        Ok(Self {
            supervisor_start: command_from_env("PPB_PMP_SUPERVISOR_START_JSON", "PPB_PMP_SUPERVISOR_WORKDIR")?,
            supervisor_stop: command_from_env("PPB_PMP_SUPERVISOR_STOP_JSON", "PPB_PMP_SUPERVISOR_WORKDIR")?,
            startup_args,
            ppf_build: command_from_env("PPB_PPF_BUILD_COMMAND_JSON", "PPB_PPF_BUILD_WORKDIR")?,
            backup: command_from_env("PPB_BACKUP_COMMAND_JSON", "PPB_BACKUP_WORKDIR")?,
            env_allowlist,
            timeout: Duration::from_secs(timeout_secs),
        })
    }

    #[cfg(test)]
    pub fn disabled() -> Self {
        Self { supervisor_start: None, supervisor_stop: None, startup_args: vec![], ppf_build: None, backup: None, env_allowlist: vec![], timeout: Duration::from_secs(5) }
    }

    pub fn capabilities(&self) -> DeploymentCapabilities {
        DeploymentCapabilities {
            supervisor_start: self.supervisor_start.is_some(),
            supervisor_stop: self.supervisor_stop.is_some(),
            startup_args: self.startup_args.clone(),
            ppf_build: self.ppf_build.is_some(),
            backup: self.backup.is_some(),
        }
    }

    pub fn job_configured(&self, job_type: &str) -> bool {
        match job_type { "ppf.build" => self.ppf_build.is_some(), "backup" => self.backup.is_some(), _ => true }
    }

    pub async fn start_pmp(&self, args: &Value) -> Result<Value, String> {
        let spec = self.supervisor_start.as_ref().ok_or_else(|| "CAPABILITY_NOT_SUPPORTED: PMP supervisor start is not configured".to_string())?;
        let extra = self.validate_startup_args(args)?;
        let result = self.run_with_env(spec, &extra, &BTreeMap::new()).await?;
        Ok(json!({"started":true,"adapter":"fixed-command","result":result}))
    }

    pub async fn stop_pmp(&self) -> Result<Value, String> {
        let spec = self.supervisor_stop.as_ref().ok_or_else(|| "CAPABILITY_NOT_SUPPORTED: PMP supervisor stop is not configured".to_string())?;
        let result = self.run_with_env(spec, &[], &BTreeMap::new()).await?;
        Ok(json!({"stopped":true,"adapter":"fixed-command","result":result}))
    }

    pub async fn run_job(&self, job_type: &str, ppf_config: Option<&Value>) -> Result<(), String> {
        let spec = match job_type { "ppf.build" => self.ppf_build.as_ref(), "backup" => self.backup.as_ref(), _ => None }
            .ok_or_else(|| format!("CAPABILITY_NOT_SUPPORTED: {job_type} deployment command is not configured"))?;
        let env = if job_type == "ppf.build" { ppf_build_environment(ppf_config) } else { BTreeMap::new() };
        self.run_with_env(spec, &[], &env).await.map(|_| ())
    }

    fn validate_startup_args(&self, args: &Value) -> Result<Vec<String>, String> {
        let object = args.as_object().ok_or_else(|| "server.start args must be an object".to_string())?;
        let by_key: BTreeMap<&str, &StartupArgSpec> = self.startup_args.iter().map(|spec| (spec.key.as_str(), spec)).collect();
        for key in object.keys() {
            if !by_key.contains_key(key.as_str()) { return Err(format!("startup arg `{key}` is not allowlisted")); }
        }
        let mut argv = Vec::new();
        for spec in &self.startup_args {
            let value = object.get(&spec.key);
            if value.is_none() {
                if spec.required { return Err(format!("startup arg `{}` is required", spec.key)); }
                continue;
            }
            let value = value.expect("checked");
            match spec.kind.as_str() {
                "boolean" => {
                    if value.as_bool().ok_or_else(|| format!("startup arg `{}` must be boolean", spec.key))? { argv.push(spec.flag.clone()); }
                }
                "integer" => {
                    let n = value.as_i64().ok_or_else(|| format!("startup arg `{}` must be integer", spec.key))?;
                    if spec.min.is_some_and(|min| n < min) || spec.max.is_some_and(|max| n > max) { return Err(format!("startup arg `{}` is outside its allowed range", spec.key)); }
                    argv.push(spec.flag.clone()); argv.push(n.to_string());
                }
                "string" => {
                    let text = value.as_str().ok_or_else(|| format!("startup arg `{}` must be string", spec.key))?;
                    let max_len = spec.max_len.unwrap_or(256).clamp(1,4096);
                    if text.is_empty() || text.len() > max_len || text.contains('\0') { return Err(format!("startup arg `{}` has invalid length/content", spec.key)); }
                    if !spec.allowed_values.is_empty() && !spec.allowed_values.iter().any(|v| v == text) { return Err(format!("startup arg `{}` is not an allowed value", spec.key)); }
                    argv.push(spec.flag.clone()); argv.push(text.to_string());
                }
                other => return Err(format!("startup arg `{}` has unsupported kind `{other}`", spec.key)),
            }
        }
        Ok(argv)
    }

    async fn run_with_env(&self, spec: &FixedCommand, extra: &[String], injected_env: &BTreeMap<String, String>) -> Result<Value, String> {
        let (program, fixed_args) = spec.argv.split_first().ok_or_else(|| "deployment command has no executable".to_string())?;
        let mut command = Command::new(program);
        command.args(fixed_args).args(extra).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true).env_clear();
        if let Some(cwd) = &spec.cwd { command.current_dir(cwd); }
        copy_safe_environment(&mut command, &self.env_allowlist);
        for (name, value) in injected_env {
            if name.starts_with("NUXT_") && is_safe_env_name(name) {
                command.env(name, value);
            }
        }
        let output = tokio::time::timeout(self.timeout, command.output()).await.map_err(|_| "deployment command timed out".to_string())?
            .map_err(|e| format!("deployment command failed to start: {e}"))?;
        let stdout = truncate_utf8(&output.stdout,4096); let stderr=truncate_utf8(&output.stderr,4096);
        if !output.status.success() { return Err(format!("deployment command exited with {}: {}", output.status, if stderr.is_empty(){stdout}else{stderr})); }
        Ok(json!({"exit_code":output.status.code(),"stdout":stdout,"stderr":stderr}))
    }
}

fn ppf_build_environment(config: Option<&Value>) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let Some(obj) = config.and_then(Value::as_object) else { return out };
    let mappings = [
        ("site_name", "NUXT_PUBLIC_SITE_NAME"),
        ("site_description", "NUXT_PUBLIC_SITE_DESCRIPTION"),
        ("canonical_url", "NUXT_PUBLIC_SITE_URL"),
        ("analytics_provider", "NUXT_PUBLIC_ANALYTICS_PROVIDER"),
        ("plausible_domain", "NUXT_PUBLIC_PLAUSIBLE_DOMAIN"),
        ("ga_id", "NUXT_PUBLIC_GA_ID"),
        ("search_verification_google", "NUXT_PUBLIC_SEARCH_VERIFICATION_GOOGLE"),
        ("search_verification_bing", "NUXT_PUBLIC_SEARCH_VERIFICATION_BING"),
    ];
    for (key, env_name) in mappings {
        if let Some(value) = obj.get(key).and_then(Value::as_str).map(str::trim).filter(|v| !v.is_empty()) {
            out.insert(env_name.to_string(), value.to_string());
        }
    }
    out
}

fn command_from_env(command_name:&str,cwd_name:&str)->Result<Option<FixedCommand>,anyhow::Error>{
    let Some(raw)=non_empty_env(command_name) else{return Ok(None)};
    let argv:Vec<String>=serde_json::from_str(&raw).map_err(|e| anyhow::anyhow!("{command_name} must be a JSON string array: {e}"))?;
    if argv.is_empty()||argv[0].trim().is_empty()||argv.iter().any(|arg|arg.contains('\0')){anyhow::bail!("{command_name} must contain a valid executable and NUL-free argv")}
    Ok(Some(FixedCommand{argv,cwd:non_empty_env(cwd_name).map(PathBuf::from)}))
}
fn validate_arg_specs(specs:&[StartupArgSpec])->Result<(),anyhow::Error>{
    let mut keys=BTreeSet::new();
    for spec in specs{
        if spec.key.is_empty()||spec.flag.is_empty()||spec.flag.contains('\0'){anyhow::bail!("startup arg schema key/flag must be non-empty and NUL-free")}
        if !keys.insert(spec.key.as_str()){anyhow::bail!("duplicate startup arg schema key: {}",spec.key)}
        if !matches!(spec.kind.as_str(),"string"|"integer"|"boolean"){anyhow::bail!("unsupported startup arg kind: {}",spec.kind)}
        if spec.min.zip(spec.max).is_some_and(|(min,max)|min>max){anyhow::bail!("startup arg {} has min > max",spec.key)}
    }
    Ok(())
}
fn non_empty_env(name:&str)->Option<String>{env::var(name).ok().map(|v|v.trim().to_string()).filter(|v|!v.is_empty())}
fn is_safe_env_name(name:&str)->bool{!name.starts_with("PPB_")&&name.chars().all(|ch|ch.is_ascii_alphanumeric()||ch=='_')}
fn copy_safe_environment(command:&mut Command,extra:&[String]){
    const BASE:&[&str]=&["PATH","HOME","USERPROFILE","SYSTEMROOT","WINDIR","TEMP","TMP","TMPDIR","PNPM_HOME","XDG_CACHE_HOME"];
    for name in BASE.iter().copied().chain(extra.iter().map(String::as_str)){if let Ok(value)=env::var(name){command.env(name,value);}}
}
fn truncate_utf8(bytes:&[u8],max:usize)->String{
    let text=String::from_utf8_lossy(bytes);
    if text.len()<=max{return text.into_owned()}
    let mut end=max; while end>0 && !text.is_char_boundary(end){end-=1}
    format!("{}…",&text[..end])
}
