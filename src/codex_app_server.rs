use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};

use crate::backend::apply_profile_environment;
use crate::registry::{
    AgentDefinition, IndividualQuotaLimit, QuotaLimit, QuotaLimitBucket, QuotaLimits,
};
use crate::storage::Database;

const RESPONSE_TIMEOUT: Duration = Duration::from_secs(10);
const QUOTA_SOURCE: &str = "codex_app_server";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuotaSnapshot {
    pub remaining_percent: i64,
    pub reset_at: Option<i64>,
    pub window_duration_mins: Option<i64>,
    pub rate_limit_reached_type: Option<String>,
    pub credits: Option<CreditsSnapshot>,
    pub reset_credits_available: Option<i64>,
    pub limits: QuotaLimits,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CreditsSnapshot {
    pub has_credits: bool,
    pub unlimited: bool,
    pub balance: Option<String>,
}

pub trait RateLimitProvider {
    fn read(&self, profile_path: &Path) -> Result<QuotaSnapshot, String>;
}

pub type AgentSyncResult = (String, Result<QuotaSnapshot, String>);

pub struct CodexAppServer;

impl RateLimitProvider for CodexAppServer {
    fn read(&self, profile_path: &Path) -> Result<QuotaSnapshot, String> {
        let mut client = StdioClient::start(profile_path)?;
        client.initialize()?;
        client.read_rate_limits()
    }
}

pub fn initialization_request() -> Value {
    json!({
        "id": 1,
        "method": "initialize",
        "params": {
            "clientInfo": { "name": "orc", "version": env!("CARGO_PKG_VERSION") },
            "capabilities": {}
        }
    })
}

pub fn initialized_notification() -> Value {
    json!({ "method": "initialized" })
}

pub fn rate_limits_request() -> Value {
    json!({ "id": 2, "method": "account/rateLimits/read", "params": null })
}

pub fn parse_rate_limits_response(value: Value) -> Result<QuotaSnapshot, String> {
    if let Some(error) = value.get("error") {
        return Err(format!("app-server rate-limit request failed: {error}"));
    }
    let response: RpcResponse = serde_json::from_value(value)
        .map_err(|error| format!("malformed app-server response: {error}"))?;
    if response.id != 2 {
        return Err(format!(
            "unexpected app-server response id: {}",
            response.id
        ));
    }
    let result: RateLimitsResponse = serde_json::from_value(response.result)
        .map_err(|error| format!("malformed rate-limit response: {error}"))?;
    let primary = result.rate_limits.primary.map(QuotaLimit::from);
    let secondary = result.rate_limits.secondary.map(QuotaLimit::from);
    let individual_limit = result
        .rate_limits
        .individual_limit
        .map(IndividualQuotaLimit::from);
    let (remaining_percent, reset_at, window_duration_mins, effective_name) =
        if let Some(limit) = individual_limit.as_ref() {
            (
                limit.remaining_percent,
                Some(limit.reset_at),
                None,
                "individualLimit",
            )
        } else {
            let (limit, name) = match (primary.as_ref(), secondary.as_ref()) {
                (Some(primary), Some(secondary))
                    if secondary.remaining_percent < primary.remaining_percent =>
                {
                    (secondary, "secondary")
                }
                (Some(primary), _) => (primary, "primary"),
                (None, Some(secondary)) => (secondary, "secondary"),
                (None, None) => {
                    return Err(
                    "Codex rate-limit response has no individualLimit, secondary, or primary limit"
                        .to_owned(),
                );
                }
            };
            (
                limit.remaining_percent,
                limit.reset_at,
                limit.window_duration_mins,
                name,
            )
        };
    Ok(QuotaSnapshot {
        remaining_percent,
        reset_at,
        window_duration_mins,
        rate_limit_reached_type: result.rate_limits.rate_limit_reached_type,
        credits: result.rate_limits.credits,
        reset_credits_available: result
            .rate_limit_reset_credits
            .map(|summary| summary.available_count),
        limits: QuotaLimits {
            primary,
            secondary,
            individual_limit,
            by_limit_id: result
                .rate_limits_by_limit_id
                .unwrap_or_default()
                .into_iter()
                .map(|(id, snapshot)| (id, QuotaLimitBucket::from(snapshot)))
                .collect(),
            effective: effective_name.to_owned(),
        },
    })
}

pub fn sync_agent(
    db: &Database,
    agent: &AgentDefinition,
    provider: &dyn RateLimitProvider,
) -> Result<QuotaSnapshot, String> {
    if agent.backend != "codex" {
        return Err(format!(
            "agent '{}' uses unsupported backend '{}' for quota sync",
            agent.id, agent.backend
        ));
    }
    let profile_path = agent.profile_path.as_deref().ok_or_else(|| {
        format!(
            "Codex agent '{}' requires a configured profile path; run `orc agent profile {} <path>`",
            agent.id, agent.id
        )
    })?;
    let snapshot = provider.read(Path::new(profile_path))?;
    db.set_agent_synced_quota(
        &agent.id,
        snapshot.remaining_percent,
        snapshot.reset_at.map(|value| value.to_string()).as_deref(),
        QUOTA_SOURCE,
        &snapshot.limits,
    )
    .map_err(|error| format!("failed to store quota for '{}': {error}", agent.id))?
    .then_some(())
    .ok_or_else(|| format!("agent '{}' is not registered", agent.id))?;
    Ok(snapshot)
}

pub fn sync_enabled_agents(
    db: &Database,
    provider: &dyn RateLimitProvider,
) -> Result<Vec<AgentSyncResult>, String> {
    let agents = db.list_agents().map_err(|error| error.to_string())?;
    Ok(agents
        .into_iter()
        .filter(|agent| agent.enabled && agent.backend == "codex")
        .map(|agent| {
            let result = sync_agent(db, &agent, provider);
            (agent.id, result)
        })
        .collect())
}

pub fn sync_enabled_agents_after_automated_run(
    db: &Database,
    provider: &dyn RateLimitProvider,
) -> Result<Vec<AgentSyncResult>, String> {
    let agents = db.list_agents().map_err(|error| error.to_string())?;
    Ok(agents
        .into_iter()
        .filter(|agent| agent.enabled && agent.backend == "codex" && agent.profile_path.is_some())
        .map(|agent| {
            let result = sync_agent(db, &agent, provider);
            (agent.id, result)
        })
        .collect())
}

#[derive(Deserialize)]
struct RpcResponse {
    id: i64,
    result: Value,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RateLimitsResponse {
    rate_limits: RateLimitSnapshot,
    rate_limits_by_limit_id: Option<BTreeMap<String, RateLimitSnapshot>>,
    rate_limit_reset_credits: Option<ResetCreditsSummary>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RateLimitSnapshot {
    primary: Option<RateLimitWindow>,
    secondary: Option<RateLimitWindow>,
    individual_limit: Option<SpendControlLimitSnapshot>,
    rate_limit_reached_type: Option<String>,
    credits: Option<CreditsSnapshot>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SpendControlLimitSnapshot {
    limit: String,
    used: String,
    remaining_percent: i64,
    resets_at: i64,
}

impl From<SpendControlLimitSnapshot> for IndividualQuotaLimit {
    fn from(value: SpendControlLimitSnapshot) -> Self {
        Self {
            limit: value.limit,
            used: value.used,
            remaining_percent: value.remaining_percent.clamp(0, 100),
            reset_at: value.resets_at,
        }
    }
}

impl From<RateLimitSnapshot> for QuotaLimitBucket {
    fn from(value: RateLimitSnapshot) -> Self {
        Self {
            primary: value.primary.map(QuotaLimit::from),
            secondary: value.secondary.map(QuotaLimit::from),
            individual_limit: value.individual_limit.map(IndividualQuotaLimit::from),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RateLimitWindow {
    used_percent: i64,
    window_duration_mins: Option<i64>,
    resets_at: Option<i64>,
}

impl From<RateLimitWindow> for QuotaLimit {
    fn from(value: RateLimitWindow) -> Self {
        Self {
            remaining_percent: (100_i64 - value.used_percent).clamp(0, 100),
            reset_at: value.resets_at,
            window_duration_mins: value.window_duration_mins,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResetCreditsSummary {
    available_count: i64,
}

struct StdioClient {
    child: Child,
    stdin: ChildStdin,
    messages: Receiver<Result<Value, String>>,
}

impl StdioClient {
    fn start(profile_path: &Path) -> Result<Self, String> {
        let mut command = Command::new("codex");
        command
            .args(["app-server", "--stdio"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        apply_profile_environment(&mut command, profile_path);
        let mut child = command
            .spawn()
            .map_err(|error| format!("failed to start `codex app-server --stdio`: {error}"))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "failed to open stdin for `codex app-server --stdio`".to_owned())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "failed to open stdout for `codex app-server --stdio`".to_owned())?;
        let (sender, messages) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let parsed = line
                    .map_err(|error| format!("failed reading app-server output: {error}"))
                    .and_then(|line| {
                        serde_json::from_str(&line)
                            .map_err(|error| format!("invalid JSON from app-server: {error}"))
                    });
                if sender.send(parsed).is_err() {
                    break;
                }
            }
            let _ = sender.send(Err(
                "app-server closed stdout before completing the requested response".to_owned(),
            ));
        });
        Ok(Self {
            child,
            stdin,
            messages,
        })
    }

    fn initialize(&mut self) -> Result<(), String> {
        self.send(&initialization_request())?;
        validate_initialization_response(self.response_for(1)?)?;
        self.send(&initialized_notification())
    }

    fn read_rate_limits(&mut self) -> Result<QuotaSnapshot, String> {
        self.send(&rate_limits_request())?;
        parse_rate_limits_response(self.response_for(2)?)
    }

    fn send(&mut self, message: &Value) -> Result<(), String> {
        serde_json::to_writer(&mut self.stdin, message)
            .map_err(|error| format!("failed to encode app-server request: {error}"))?;
        self.stdin
            .write_all(b"\n")
            .and_then(|_| self.stdin.flush())
            .map_err(|error| format!("failed to write app-server request: {error}"))
    }

    fn response_for(&self, expected_id: i64) -> Result<Value, String> {
        loop {
            let message =
                self.messages
                    .recv_timeout(RESPONSE_TIMEOUT)
                    .map_err(|error| match error {
                        mpsc::RecvTimeoutError::Timeout => {
                            format!("app-server timed out waiting for response {expected_id}")
                        }
                        mpsc::RecvTimeoutError::Disconnected => {
                            "app-server closed stdout before responding".to_owned()
                        }
                    })??;
            if message.get("id").and_then(Value::as_i64) == Some(expected_id) {
                return Ok(message);
            }
        }
    }
}

fn validate_initialization_response(response: Value) -> Result<(), String> {
    if let Some(error) = response.get("error") {
        return Err(format!("app-server initialization failed: {error}"));
    }
    if response.get("result").is_none() {
        return Err("malformed app-server initialization response: missing result".into());
    }
    Ok(())
}

impl Drop for StdioClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::AVAILABLE;
    use std::collections::HashMap;
    use std::sync::Mutex;
    use tempfile::tempdir;

    struct FakeProvider {
        results: HashMap<String, Result<QuotaSnapshot, String>>,
        profiles: Mutex<Vec<String>>,
    }

    impl RateLimitProvider for FakeProvider {
        fn read(&self, profile_path: &Path) -> Result<QuotaSnapshot, String> {
            let profile = profile_path.display().to_string();
            self.profiles.lock().unwrap().push(profile.clone());
            self.results
                .get(&profile)
                .cloned()
                .unwrap_or_else(|| Err("fixture missing".into()))
        }
    }

    fn agent(id: &str, backend: &str, profile: &str) -> AgentDefinition {
        AgentDefinition {
            id: id.into(),
            backend: backend.into(),
            execution_mode: "automated".into(),
            display_name: id.into(),
            enabled: true,
            priority: 0,
            capabilities: vec![],
            status: AVAILABLE.into(),
            unavailable_reason: None,
            profile_path: Some(profile.into()),
            model: None,
            reasoning_effort: None,
            config_metadata: None,
            quota_remaining_percent: None,
            quota_reset_at: None,
            quota_checked_at: None,
            quota_source: None,
            quota_limits: None,
            actions: vec![crate::registry::AgentAction::Code],
        }
    }

    fn snapshot(remaining_percent: i64, reset_at: i64) -> QuotaSnapshot {
        QuotaSnapshot {
            remaining_percent,
            reset_at: Some(reset_at),
            window_duration_mins: Some(10080),
            rate_limit_reached_type: None,
            credits: None,
            reset_credits_available: None,
            limits: QuotaLimits {
                primary: Some(QuotaLimit {
                    remaining_percent,
                    reset_at: Some(reset_at),
                    window_duration_mins: Some(10080),
                }),
                secondary: None,
                individual_limit: None,
                by_limit_id: BTreeMap::new(),
                effective: "primary".into(),
            },
        }
    }

    #[test]
    fn request_shapes_match_protocol() {
        assert_eq!(initialization_request()["method"], "initialize");
        assert_eq!(
            initialization_request()["params"]["clientInfo"]["name"],
            "orc"
        );
        assert_eq!(initialized_notification(), json!({"method": "initialized"}));
        assert_eq!(
            rate_limits_request(),
            json!({"id": 2, "method": "account/rateLimits/read", "params": null})
        );
    }

    #[test]
    fn initialization_rejects_missing_result() {
        let error = validate_initialization_response(json!({"id": 1})).unwrap_err();
        assert!(error.contains("missing result"));
    }

    #[test]
    fn parses_primary_limit_and_optional_account_fields() {
        let result = parse_rate_limits_response(json!({
            "id": 2,
            "result": {
                "rateLimits": {
                    "primary": {"usedPercent": 97, "windowDurationMins": 10080, "resetsAt": 1787416740},
                    "rateLimitReachedType": "rate_limit_reached",
                    "credits": {"hasCredits": false, "unlimited": false, "balance": "0"}
                },
                "rateLimitResetCredits": {"availableCount": 2, "credits": null}
            }
        })).unwrap();
        assert_eq!(result.remaining_percent, 3);
        assert_eq!(result.reset_at, Some(1787416740));
        assert_eq!(result.window_duration_mins, Some(10080));
        assert_eq!(
            result.rate_limit_reached_type.as_deref(),
            Some("rate_limit_reached")
        );
        assert_eq!(result.reset_credits_available, Some(2));
    }

    #[test]
    fn individual_limit_overrides_primary_and_preserves_limit_buckets() {
        let result = parse_rate_limits_response(json!({
            "id": 2,
            "result": {"rateLimits": {
                "primary": {"usedPercent": 99, "windowDurationMins": 300, "resetsAt": 1787000000},
                "individualLimit": {"limit": "1000", "used": "10", "remainingPercent": 99, "resetsAt": 1789000000}
            }, "rateLimitsByLimitId": {
                "codex": {"primary": {"usedPercent": 25, "windowDurationMins": 300, "resetsAt": 1787100000}}
            }}
        }))
        .unwrap();

        assert_eq!(result.remaining_percent, 99);
        assert_eq!(result.reset_at, Some(1789000000));
        assert_eq!(result.window_duration_mins, None);
        assert_eq!(result.limits.effective, "individualLimit");
        assert_eq!(result.limits.primary.as_ref().unwrap().remaining_percent, 1);
        assert_eq!(
            result
                .limits
                .individual_limit
                .as_ref()
                .unwrap()
                .remaining_percent,
            99
        );
        assert_eq!(
            result.limits.by_limit_id["codex"]
                .primary
                .as_ref()
                .unwrap()
                .remaining_percent,
            75
        );
    }

    #[test]
    fn paid_account_uses_weekly_secondary_limit() {
        let result = parse_rate_limits_response(json!({
            "id": 2,
            "result": {"rateLimits": {
                "primary": {"usedPercent": 8, "windowDurationMins": 300, "resetsAt": 1787000000},
                "secondary": {"usedPercent": 42, "windowDurationMins": 10080, "resetsAt": 1787600000}
            }}
        }))
        .unwrap();

        assert_eq!(result.remaining_percent, 58);
        assert_eq!(result.reset_at, Some(1787600000));
        assert_eq!(result.window_duration_mins, Some(10080));
        assert_eq!(result.limits.effective, "secondary");
    }

    #[test]
    fn dual_windows_use_the_more_restrictive_five_hour_primary_limit() {
        let result = parse_rate_limits_response(json!({
            "id": 2,
            "result": {"rateLimits": {
                "primary": {"usedPercent": 75, "windowDurationMins": 300, "resetsAt": 1787000000},
                "secondary": {"usedPercent": 10, "windowDurationMins": 10080, "resetsAt": 1787600000}
            }}
        }))
        .unwrap();

        assert_eq!(result.remaining_percent, 25);
        assert_eq!(result.reset_at, Some(1787000000));
        assert_eq!(result.window_duration_mins, Some(300));
        assert_eq!(result.limits.effective, "primary");
        assert_eq!(
            result.limits.secondary.as_ref().unwrap().remaining_percent,
            90
        );
    }

    #[test]
    fn dual_windows_survive_reopen_with_the_effective_limit() {
        let directory = tempdir().unwrap();
        let database_path = directory.path().join("orc.db");
        let db = Database::init(&database_path).unwrap();
        let registered = agent("codex-main", "codex", "/profiles/main");
        db.insert_agent(&registered).unwrap();
        let parsed = parse_rate_limits_response(json!({
            "id": 2,
            "result": {"rateLimits": {
                "primary": {"usedPercent": 75, "windowDurationMins": 300, "resetsAt": 1787000000},
                "secondary": {"usedPercent": 42, "windowDurationMins": 10080, "resetsAt": 1787600000}
            }}
        }))
        .unwrap();
        let provider = FakeProvider {
            results: HashMap::from([("/profiles/main".into(), Ok(parsed))]),
            profiles: Mutex::new(vec![]),
        };
        sync_agent(&db, &registered, &provider).unwrap();
        drop(db);

        let stored = Database::open(&database_path)
            .unwrap()
            .get_agent("codex-main")
            .unwrap()
            .unwrap();
        assert_eq!(stored.quota_remaining_percent, Some(25));
        assert_eq!(stored.quota_reset_at.as_deref(), Some("1787000000"));
        let limits = stored.quota_limits.unwrap();
        assert_eq!(limits.effective, "primary");
        assert_eq!(limits.primary.unwrap().window_duration_mins, Some(300));
        assert_eq!(limits.secondary.unwrap().window_duration_mins, Some(10080));
    }

    #[test]
    fn clamps_usage_and_rejects_missing_or_malformed_primary() {
        let over = parse_rate_limits_response(json!({
            "id": 2, "result": {"rateLimits": {"primary": {"usedPercent": 120}}}
        }))
        .unwrap();
        assert_eq!(over.remaining_percent, 0);
        assert!(
            parse_rate_limits_response(json!({
                "id": 2, "result": {"rateLimits": {"primary": null}}
            }))
            .unwrap_err()
            .contains("no individualLimit")
        );
        assert!(parse_rate_limits_response(json!({"id": 2, "result": []})).is_err());
    }

    #[test]
    fn reached_limit_maps_to_zero_remaining() {
        let result = parse_rate_limits_response(json!({
            "id": 2,
            "result": {"rateLimits": {
                "primary": {"usedPercent": 100, "resetsAt": null},
                "rateLimitReachedType": "rate_limit_reached"
            }}
        }))
        .unwrap();
        assert_eq!(result.remaining_percent, 0);
        assert_eq!(result.reset_at, None);
        assert_eq!(
            result.rate_limit_reached_type.as_deref(),
            Some("rate_limit_reached")
        );
    }

    #[test]
    fn sync_isolates_profiles_and_updates_agents_independently() {
        let directory = tempdir().unwrap();
        let db = Database::init(directory.path().join("orc.db")).unwrap();
        let first = agent("codex-main", "codex", "/profiles/main");
        let second = agent("codex-work", "codex", "/profiles/work");
        db.insert_agent(&first).unwrap();
        db.insert_agent(&second).unwrap();
        let provider = FakeProvider {
            results: HashMap::from([
                ("/profiles/main".into(), Ok(snapshot(3, 1787416740))),
                ("/profiles/work".into(), Ok(snapshot(72, 1787500000))),
            ]),
            profiles: Mutex::new(vec![]),
        };

        let results = sync_enabled_agents(&db, &provider).unwrap();
        assert!(results.iter().all(|(_, result)| result.is_ok()));
        let main = db.get_agent("codex-main").unwrap().unwrap();
        let work = db.get_agent("codex-work").unwrap().unwrap();
        assert_eq!(main.quota_remaining_percent, Some(3));
        assert_eq!(main.quota_reset_at.as_deref(), Some("1787416740"));
        assert_eq!(main.quota_source.as_deref(), Some(QUOTA_SOURCE));
        assert_eq!(
            main.quota_limits
                .as_ref()
                .map(|limits| limits.effective.as_str()),
            Some("primary")
        );
        assert!(main.quota_checked_at.is_some());
        assert_eq!(work.quota_remaining_percent, Some(72));
        assert_eq!(work.quota_reset_at.as_deref(), Some("1787500000"));
        assert_eq!(
            provider.profiles.into_inner().unwrap(),
            vec!["/profiles/main", "/profiles/work"]
        );
    }

    #[test]
    fn sync_rejects_codex_agent_without_profile_before_provider_call() {
        let directory = tempdir().unwrap();
        let db = Database::init(directory.path().join("orc.db")).unwrap();
        let mut missing = agent("codex-missing", "codex", "/profiles/unused");
        missing.profile_path = None;
        db.insert_agent(&missing).unwrap();
        let provider = FakeProvider {
            results: HashMap::new(),
            profiles: Mutex::new(vec![]),
        };

        let error = sync_agent(&db, &missing, &provider).unwrap_err();
        assert!(error.contains("codex-missing"));
        assert!(error.contains("profile path"));
        assert!(provider.profiles.into_inner().unwrap().is_empty());
    }

    #[test]
    fn structured_individual_quota_survives_reopen_and_scheduler_uses_flattened_value() {
        use crate::scheduler;
        use crate::task::{Task, TaskPriority, TaskStatus};

        let directory = tempdir().unwrap();
        let database_path = directory.path().join("orc.db");
        let db = Database::init(&database_path).unwrap();
        let mut registered = agent("codex-main", "codex", "/profiles/main");
        registered.capabilities = vec!["code".into(), "terminal".into()];
        db.insert_agent(&registered).unwrap();
        let parsed = parse_rate_limits_response(json!({
            "id": 2,
            "result": {"rateLimits": {
                "primary": {"usedPercent": 99, "windowDurationMins": 300, "resetsAt": 1787000000},
                "individualLimit": {"limit": "1000", "used": "10", "remainingPercent": 99, "resetsAt": 1789000000}
            }}
        }))
        .unwrap();
        let provider = FakeProvider {
            results: HashMap::from([("/profiles/main".into(), Ok(parsed))]),
            profiles: Mutex::new(vec![]),
        };
        sync_agent(&db, &registered, &provider).unwrap();
        drop(db);

        let reopened = Database::open(&database_path).unwrap();
        let stored = reopened.get_agent("codex-main").unwrap().unwrap();
        assert_eq!(stored.quota_remaining_percent, Some(99));
        assert_eq!(
            stored.quota_limits.as_ref().unwrap().effective,
            "individualLimit"
        );
        assert_eq!(
            stored
                .quota_limits
                .as_ref()
                .unwrap()
                .individual_limit
                .as_ref()
                .unwrap()
                .reset_at,
            1789000000
        );

        let task = Task {
            id: "T-quota".into(),
            title: "quota regression".into(),
            objective: "verify scheduling".into(),
            role: "developer".into(),
            priority: TaskPriority::Normal,
            status: TaskStatus::Ready,
            cancellation_reason: None,
            required_capabilities: vec![],
            scope_mode: None,
            context_files: vec![],
            expected_changes: vec![],
            reasoning_effort: None,
            effort_reason: None,
            risk_factors: vec![],
        };
        let decision = scheduler::schedule(&task, &[stored], None).unwrap();
        assert_eq!(decision.selected_agent_id.as_deref(), Some("codex-main"));
    }

    #[test]
    fn unsupported_backend_rejects_sync() {
        let directory = tempdir().unwrap();
        let db = Database::init(directory.path().join("orc.db")).unwrap();
        let unsupported = agent("copilot", "copilot", "/profiles/copilot");
        db.insert_agent(&unsupported).unwrap();
        let provider = FakeProvider {
            results: HashMap::new(),
            profiles: Mutex::new(vec![]),
        };
        assert!(
            sync_agent(&db, &unsupported, &provider)
                .unwrap_err()
                .contains("unsupported backend")
        );
        assert!(provider.profiles.into_inner().unwrap().is_empty());
    }

    #[test]
    fn one_failed_agent_does_not_stop_enabled_sync() {
        let directory = tempdir().unwrap();
        let db = Database::init(directory.path().join("orc.db")).unwrap();
        db.insert_agent(&agent("codex-bad", "codex", "/profiles/bad"))
            .unwrap();
        db.insert_agent(&agent("codex-good", "codex", "/profiles/good"))
            .unwrap();
        let provider = FakeProvider {
            results: HashMap::from([
                ("/profiles/bad".into(), Err("protocol failed".into())),
                ("/profiles/good".into(), Ok(snapshot(55, 1787500000))),
            ]),
            profiles: Mutex::new(vec![]),
        };
        let results = sync_enabled_agents(&db, &provider).unwrap();
        assert_eq!(results.len(), 2);
        assert!(results[0].1.is_err());
        assert!(results[1].1.is_ok());
        assert_eq!(
            db.get_agent("codex-good")
                .unwrap()
                .unwrap()
                .quota_remaining_percent,
            Some(55)
        );
    }
}
