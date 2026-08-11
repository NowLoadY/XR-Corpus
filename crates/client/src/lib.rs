//! Small typed client for XR Corpus.

use std::time::Duration;

use reqwest::StatusCode;
pub use xr_corpus_protocol as protocol;
use xr_corpus_protocol::{
    API_VERSION, CreateSessionRequest, CreateSessionResponse, ErrorResponse, HealthResponse,
    PrepareAsrRequest, PrepareAsrResponse, PrepareTranslationRequest, PrepareTranslationResponse,
    ProviderSnapshotResponse, PublishProviderRequest, RecordTranslationRequest,
    RecordTranslationResponse, SessionStateResponse, VrcxStatusResponse,
};

pub type CorpusResult<T> = Result<T, CorpusClientError>;

#[derive(Debug)]
pub enum CorpusClientError {
    InvalidUrl(String),
    InvalidRequest(String),
    Transport(String),
    InvalidResponse(String),
    IncompatibleApi {
        expected: u16,
        actual: u16,
    },
    Server {
        status: u16,
        code: String,
        message: String,
    },
}

impl std::fmt::Display for CorpusClientError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidUrl(message)
            | Self::InvalidRequest(message)
            | Self::Transport(message)
            | Self::InvalidResponse(message) => formatter.write_str(message),
            Self::IncompatibleApi { expected, actual } => write!(
                formatter,
                "XR Corpus API version {actual} is incompatible; this client requires {expected}"
            ),
            Self::Server {
                status,
                code,
                message,
            } => write!(
                formatter,
                "XR Corpus returned HTTP {status} ({code}): {message}"
            ),
        }
    }
}

impl std::error::Error for CorpusClientError {}

#[derive(Clone)]
pub struct CorpusClient {
    base_url: String,
    http: reqwest::Client,
}

#[derive(Clone)]
pub struct CorpusSessionClient {
    client: CorpusClient,
    session_id: String,
}

impl CorpusClient {
    pub fn new(base_url: impl Into<String>) -> CorpusResult<Self> {
        let requested = base_url.into();
        let parsed = reqwest::Url::parse(requested.trim()).map_err(|error| {
            CorpusClientError::InvalidUrl(format!("invalid XR Corpus URL: {error}"))
        })?;
        if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
            return Err(CorpusClientError::InvalidUrl(
                "XR Corpus URL must be an absolute http:// or https:// URL".into(),
            ));
        }
        if !parsed.host_str().is_some_and(is_loopback_host) {
            return Err(CorpusClientError::InvalidUrl(
                "XR Corpus currently accepts loopback URLs only".into(),
            ));
        }
        if parsed.query().is_some() || parsed.fragment().is_some() || parsed.path() != "/" {
            return Err(CorpusClientError::InvalidUrl(
                "XR Corpus base URL cannot contain a path, query, or fragment".into(),
            ));
        }
        let base_url = parsed.as_str().trim_end_matches('/').to_owned();
        let http = reqwest::Client::builder()
            // XR Corpus is a process-local service. System and environment
            // proxies must never intercept its health or session requests.
            .no_proxy()
            .connect_timeout(Duration::from_secs(2))
            .timeout(Duration::from_secs(8))
            .build()
            .map_err(|error| {
                CorpusClientError::Transport(format!("cannot create XR Corpus client: {error}"))
            })?;
        Ok(Self { base_url, http })
    }

    pub async fn connect(base_url: impl Into<String>) -> CorpusResult<Self> {
        let client = Self::new(base_url)?;
        client.ensure_compatible().await?;
        Ok(client)
    }

    pub async fn health(&self) -> CorpusResult<HealthResponse> {
        self.get("/healthz").await
    }

    pub async fn ensure_compatible(&self) -> CorpusResult<HealthResponse> {
        let health = self.health().await?;
        if health.api_version != API_VERSION {
            return Err(CorpusClientError::IncompatibleApi {
                expected: API_VERSION,
                actual: health.api_version,
            });
        }
        Ok(health)
    }

    pub async fn create_session(&self) -> CorpusResult<CorpusSessionClient> {
        let response: CreateSessionResponse = self
            .post("/v1/sessions", &CreateSessionRequest::default())
            .await?;
        Ok(CorpusSessionClient {
            client: self.clone(),
            session_id: response.session_id,
        })
    }

    pub async fn vrcx_status(&self) -> CorpusResult<VrcxStatusResponse> {
        self.get("/v1/integrations/vrcx/status").await
    }

    pub async fn publish_provider(
        &self,
        provider_id: &str,
        request: &PublishProviderRequest,
    ) -> CorpusResult<ProviderSnapshotResponse> {
        self.put(&provider_path(provider_id)?, request).await
    }

    pub async fn remove_provider(&self, provider_id: &str) -> CorpusResult<()> {
        self.delete(&provider_path(provider_id)?).await
    }

    async fn get<T: serde::de::DeserializeOwned>(&self, path: &str) -> CorpusResult<T> {
        let response = self
            .http
            .get(format!("{}{path}", self.base_url))
            .send()
            .await
            .map_err(request_error)?;
        decode(response).await
    }

    async fn post<B: serde::Serialize, T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> CorpusResult<T> {
        decode(
            self.http
                .post(format!("{}{path}", self.base_url))
                .json(body)
                .send()
                .await
                .map_err(request_error)?,
        )
        .await
    }

    async fn put<B: serde::Serialize, T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> CorpusResult<T> {
        decode(
            self.http
                .put(format!("{}{path}", self.base_url))
                .json(body)
                .send()
                .await
                .map_err(request_error)?,
        )
        .await
    }

    async fn delete(&self, path: &str) -> CorpusResult<()> {
        let response = self
            .http
            .delete(format!("{}{path}", self.base_url))
            .send()
            .await
            .map_err(request_error)?;
        if response.status() == StatusCode::NO_CONTENT || response.status() == StatusCode::NOT_FOUND
        {
            Ok(())
        } else {
            Err(response_error(response).await)
        }
    }
}

impl CorpusSessionClient {
    pub fn id(&self) -> &str {
        &self.session_id
    }

    pub async fn prepare_asr(
        &self,
        request: &PrepareAsrRequest,
    ) -> CorpusResult<PrepareAsrResponse> {
        self.client
            .post(&format!("/v1/sessions/{}/asr", self.session_id), request)
            .await
    }

    pub async fn prepare_translation(
        &self,
        request: &PrepareTranslationRequest,
    ) -> CorpusResult<PrepareTranslationResponse> {
        self.client
            .post(
                &format!("/v1/sessions/{}/translation", self.session_id),
                request,
            )
            .await
    }

    pub async fn record_translation(
        &self,
        request: &RecordTranslationRequest,
    ) -> CorpusResult<RecordTranslationResponse> {
        self.client
            .post(
                &format!("/v1/sessions/{}/results", self.session_id),
                request,
            )
            .await
    }

    pub async fn state(&self) -> CorpusResult<SessionStateResponse> {
        self.client
            .get(&format!("/v1/sessions/{}", self.session_id))
            .await
    }

    pub async fn close(self) -> CorpusResult<()> {
        self.client
            .delete(&format!("/v1/sessions/{}", self.session_id))
            .await
    }
}

async fn decode<T: serde::de::DeserializeOwned>(response: reqwest::Response) -> CorpusResult<T> {
    if response.status().is_success() {
        response.json().await.map_err(|error| {
            CorpusClientError::InvalidResponse(format!("invalid XR Corpus response: {error}"))
        })
    } else {
        Err(response_error(response).await)
    }
}

async fn response_error(response: reqwest::Response) -> CorpusClientError {
    let status = response.status();
    let body = match response.text().await {
        Ok(body) => body,
        Err(error) => {
            return CorpusClientError::InvalidResponse(format!(
                "XR Corpus returned {status}, but its response body could not be read: {error}"
            ));
        }
    };
    match serde_json::from_str::<ErrorResponse>(&body) {
        Ok(error) => CorpusClientError::Server {
            status: status.as_u16(),
            code: error.code,
            message: error.error,
        },
        Err(_) => CorpusClientError::Server {
            status: status.as_u16(),
            code: "non_json_response".into(),
            message: response_body_summary(&body),
        },
    }
}

fn request_error(error: reqwest::Error) -> CorpusClientError {
    CorpusClientError::Transport(format!("XR Corpus request failed: {error}"))
}

fn is_loopback_host(host: &str) -> bool {
    let host = host
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .unwrap_or(host);
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

fn response_body_summary(body: &str) -> String {
    const MAX_CHARS: usize = 512;
    let normalized = body.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        return "the service returned an empty non-JSON response".into();
    }
    let mut characters = normalized.chars();
    let summary = characters.by_ref().take(MAX_CHARS).collect::<String>();
    if characters.next().is_some() {
        format!("{summary}...")
    } else {
        summary
    }
}

fn provider_path(provider_id: &str) -> CorpusResult<String> {
    if provider_id.is_empty()
        || provider_id.len() > 64
        || !provider_id
            .bytes()
            .all(|value| value.is_ascii_lowercase() || value.is_ascii_digit() || value == b'-')
    {
        return Err(CorpusClientError::InvalidRequest(
            "provider ID must contain only lowercase ASCII letters, digits, and hyphens".into(),
        ));
    }
    Ok(format!("/v1/providers/{provider_id}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_url_is_validated_before_any_network_request() {
        assert!(CorpusClient::new("http://127.0.0.1:7766").is_ok());
        assert!(CorpusClient::new("http://[::1]:7766").is_ok());
        assert!(CorpusClient::new("https://corpus.example").is_err());
        assert!(CorpusClient::new("ws://127.0.0.1:7766").is_err());
        assert!(CorpusClient::new("http://127.0.0.1:7766/v1").is_err());
        assert!(CorpusClient::new("not a url").is_err());
        assert!(provider_path("my-game").is_ok());
        assert!(provider_path("../other").is_err());
        assert_eq!(
            response_body_summary("  upstream\n unavailable  "),
            "upstream unavailable"
        );
    }
}
