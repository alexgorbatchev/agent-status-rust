use super::{SummaryInput, SummaryResult, build_user_prompt, fallback, parse_output};
use agent_common::types::ActivityState;
use genai::{
    Client, ModelIden, ServiceTarget,
    adapter::AdapterKind,
    chat::{ChatMessage, ChatOptions, ChatRequest},
    resolver::{AuthData, Endpoint},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    sync::{Mutex, PoisonError},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio_util::sync::CancellationToken;
use url::Url;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DisableSource {
    Flag,
    Environment,
}

/// Explicit native provider configuration. Keys are never inferred from the process environment.
pub struct ProviderSpec {
    pub name: String,
    pub adapter: AdapterKind,
    pub model: String,
    pub api_key: String,
    pub endpoint: Option<String>,
}

pub struct Config {
    pub disabled: Option<DisableSource>,
    pub debounce: Duration,
    pub timeout: Duration,
    pub providers: Vec<ProviderSpec>,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            disabled: None,
            debounce: Duration::from_secs(10),
            timeout: Duration::from_secs(10),
            providers: Vec::new(),
        }
    }
}

impl Config {
    /// Resolve providers from an explicit environment lookup, in service fallback order.
    /// A disabled configuration performs no lookup, and `Default` never consults the environment.
    pub fn from_lookup(
        disabled: Option<DisableSource>,
        lookup: impl Fn(&str) -> Option<String>,
    ) -> Self {
        let mut config = Self {
            disabled,
            ..Self::default()
        };
        if disabled.is_some() {
            return config;
        }
        let first = |names: &[&str]| {
            names
                .iter()
                .find_map(|name| crate::config::nonempty(&lookup, name))
        };
        let url = first(&["LITELLM_BASE_URL", "OPENAI_BASE_URL"]);
        let key = first(&["LITELLM_API_KEY", "LITELLM_KEY", "OPENAI_API_KEY"]);
        let chosen = first(&["AGENT_SUMMARIZER_MODEL"]);
        let model = |fallback: &str| chosen.clone().unwrap_or_else(|| fallback.into());
        if url.is_some() || key.is_some() {
            config.providers.push(ProviderSpec {
                name: "openai".into(),
                adapter: AdapterKind::OpenAI,
                model: model("gpt-4o-mini"),
                api_key: key.unwrap_or_default(),
                endpoint: url,
            });
        }
        for (environment, name, adapter, default) in [
            (
                "GEMINI_API_KEY",
                "gemini",
                AdapterKind::Gemini,
                "gemini-2.0-flash",
            ),
            (
                "ANTHROPIC_API_KEY",
                "anthropic",
                AdapterKind::Anthropic,
                "claude-3-5-haiku-latest",
            ),
        ] {
            if let Some(key) = crate::config::nonempty(&lookup, environment) {
                config.providers.push(ProviderSpec {
                    name: name.into(),
                    adapter,
                    model: model(default),
                    api_key: key,
                    endpoint: None,
                });
            }
        }
        config
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderInfo {
    pub name: String,
    pub model: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub endpoint: String,
}

#[derive(Debug, thiserror::Error)]
pub enum ProviderInitError {
    #[error("provider endpoint must be an absolute HTTP URL")]
    Endpoint,
    #[error("provider initialization failed")]
    Resolver(#[source] genai::Error),
}

struct Provider {
    info: ProviderInfo,
    client: Client,
    target: ServiceTarget,
}

pub struct Summarizer {
    providers: Vec<Provider>,
    init_errors: BTreeMap<String, ProviderInitError>,
    allowed: Mutex<BTreeMap<String, i64>>,
    debounce: Duration,
    timeout: Duration,
    disabled: Option<DisableSource>,
}

impl Summarizer {
    pub async fn new(config: Config) -> Result<Self, reqwest::Error> {
        let transport = reqwest::Client::builder()
            .retry(reqwest::retry::never())
            .build()?;
        let mut result = Self {
            providers: Vec::new(),
            init_errors: BTreeMap::new(),
            allowed: Mutex::new(BTreeMap::new()),
            debounce: if config.debounce.is_zero() {
                Duration::from_secs(10)
            } else {
                config.debounce
            },
            timeout: if config.timeout.is_zero() {
                Duration::from_secs(10)
            } else {
                config.timeout
            },
            disabled: config.disabled,
        };
        for spec in config.providers {
            let name = spec.name.clone();
            match initialize(spec, transport.clone()).await {
                Ok(provider) => result.providers.push(provider),
                Err(error) => {
                    result.init_errors.insert(name, error);
                }
            }
        }
        Ok(result)
    }

    pub fn active_providers(&self) -> Vec<ProviderInfo> {
        self.providers
            .iter()
            .map(|provider| provider.info.clone())
            .collect()
    }
    pub fn init_errors(&self) -> &BTreeMap<String, ProviderInitError> {
        &self.init_errors
    }
    pub fn disabled_source(&self) -> Option<DisableSource> {
        self.disabled
    }
    pub fn is_disabled(&self) -> bool {
        self.disabled.is_some() || self.providers.is_empty()
    }
    pub fn debounce_window(&self) -> Duration {
        self.debounce
    }

    pub fn allow(&self, agent_id: &str) -> bool {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .min(i64::MAX as u128) as i64;
        self.allow_at(agent_id, now)
    }

    pub fn allow_at(&self, agent_id: &str, timestamp_ms: i64) -> bool {
        let mut allowed = self.allowed.lock().unwrap_or_else(PoisonError::into_inner);
        let interval = self.debounce.as_millis().min(i64::MAX as u128) as i64;
        if allowed
            .get(agent_id)
            .is_some_and(|last| timestamp_ms.saturating_sub(*last) < interval)
        {
            return false;
        }
        allowed.insert(agent_id.into(), timestamp_ms);
        true
    }

    pub async fn summarize(
        &self,
        cancel: &CancellationToken,
        input: &SummaryInput,
    ) -> SummaryResult {
        let fallback = fallback(input);
        if self.is_disabled()
            || input.activity == ActivityState::RateLimited
            || cancel.is_cancelled()
        {
            return fallback;
        }
        let request = ChatRequest::default()
            .with_system(SYSTEM_PROMPT)
            .append_message(ChatMessage::user(build_user_prompt(input)));
        let options = ChatOptions::default()
            .with_temperature(0.1)
            .with_max_tokens(150);
        for (index, provider) in self.providers.iter().enumerate() {
            let response = tokio::select! {
                biased;
                _ = cancel.cancelled() => return fallback,
                response = tokio::time::timeout(self.timeout, provider.client.exec_chat(provider.target.clone(), request.clone(), Some(&options))) => response,
            };
            if let Ok(Ok(response)) = response
                && let Some(text) = response.first_text()
                && let Ok(mut result) = parse_output(text)
            {
                if result.phase.is_none() {
                    result.phase = fallback.phase;
                }
                if result.active_target.is_none() {
                    result.active_target.clone_from(&input.active_target);
                }
                return result;
            }
            if index + 1 < self.providers.len() {
                let delay = Duration::from_millis(rand::random_range(10..50));
                tokio::select! { biased; _ = cancel.cancelled() => return fallback, _ = tokio::time::sleep(delay) => {} }
            }
        }
        fallback
    }
}

async fn initialize(
    spec: ProviderSpec,
    transport: reqwest::Client,
) -> Result<Provider, ProviderInitError> {
    let endpoint = spec
        .endpoint
        .as_deref()
        .filter(|value| !value.is_empty())
        .map(|value| {
            if value.chars().any(char::is_control) {
                return Err(ProviderInitError::Endpoint);
            }
            let mut endpoint = Url::parse(value).map_err(|_| ProviderInitError::Endpoint)?;
            if !matches!(endpoint.scheme(), "http" | "https") || endpoint.host_str().is_none() {
                return Err(ProviderInitError::Endpoint);
            }
            if !endpoint.path().ends_with('/') {
                endpoint.set_path(&format!("{}/", endpoint.path()));
            }
            Ok(Endpoint::from_owned(endpoint.to_string()))
        })
        .transpose()?;
    let auth = AuthData::from_single(spec.api_key);
    let client = Client::builder()
        .with_reqwest(transport)
        .with_adapter_kind(spec.adapter)
        .with_auth_resolver_fn(move |_| Ok(Some(auth.clone())))
        .build();
    let mut target = client
        .resolve_service_target(ModelIden::new(spec.adapter, spec.model.clone()))
        .await
        .map_err(ProviderInitError::Resolver)?;
    if let Some(endpoint) = endpoint {
        target.endpoint = endpoint;
    }
    Ok(Provider {
        info: ProviderInfo {
            name: spec.name,
            model: spec.model,
            endpoint: sanitize_endpoint(spec.endpoint.as_deref().unwrap_or_default()),
        },
        client,
        target,
    })
}

pub fn sanitize_endpoint(value: &str) -> String {
    if value.is_empty() || value.chars().any(char::is_control) {
        return String::new();
    }
    let has_scheme = value.contains("://");
    let input = if has_scheme {
        value.to_owned()
    } else {
        format!("http://{value}")
    };
    let Ok(mut url) = Url::parse(&input) else {
        return String::new();
    };
    if url.host_str().is_none() {
        return String::new();
    }
    let _ = url.set_username("");
    let _ = url.set_password(None);
    url.set_query(None);
    url.set_fragment(None);
    let mut output = url.to_string();
    if url.path() == "/"
        && !value
            .split(['?', '#'])
            .next()
            .unwrap_or_default()
            .ends_with('/')
    {
        output.pop();
    }
    if has_scheme {
        output
    } else {
        output.trim_start_matches("http://").into()
    }
}

const SYSTEM_PROMPT: &str = "You are an AI activity summarizer for a software engineering agent dashboard.\nAnalyze the agent's prompt, tools, thoughts, and state, and respond with ONLY a single JSON object:\n{\"action\":\"<concise phrase in present continuous tense describing current task, max 100 chars, e.g. 'Fixing TypeScript compilation errors in build.ts'>\",\"phase\":\"<one of: planning, editing, testing, debugging, investigating>\",\"activeTarget\":\"<path or target resource, or empty string if none>\"}\nDo not output any reasoning, markdown formatting, or preamble.";
