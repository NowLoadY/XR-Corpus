//! Small typed client for XR Corpus.

use std::time::Duration;

use reqwest::StatusCode;
use xr_corpus_protocol::{
    CreateSessionRequest, CreateSessionResponse, ErrorResponse, HealthResponse, PrepareAsrRequest,
    PrepareAsrResponse, PrepareTranslationRequest, PrepareTranslationResponse,
    RecordTranslationRequest, RecordTranslationResponse, VrcxStatusResponse,
};

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
    pub fn new(base_url: impl Into<String>) -> Result<Self, String> {
        let base_url = base_url.into().trim_end_matches('/').to_owned();
        if base_url.is_empty() {
            return Err("XR Corpus URL cannot be empty".into());
        }
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(2))
            .timeout(Duration::from_secs(8))
            .build()
            .map_err(|error| format!("cannot create XR Corpus client: {error}"))?;
        Ok(Self { base_url, http })
    }

    pub async fn health(&self) -> Result<HealthResponse, String> {
        self.get("/healthz").await
    }

    pub async fn create_session(&self) -> Result<CorpusSessionClient, String> {
        let response: CreateSessionResponse = self
            .post("/v1/sessions", &CreateSessionRequest::default())
            .await?;
        Ok(CorpusSessionClient {
            client: self.clone(),
            session_id: response.session_id,
        })
    }

    pub async fn vrcx_status(&self) -> Result<VrcxStatusResponse, String> {
        self.get("/v1/integrations/vrcx/status").await
    }

    async fn get<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T, String> {
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
    ) -> Result<T, String> {
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
}

impl CorpusSessionClient {
    pub fn id(&self) -> &str {
        &self.session_id
    }

    pub async fn prepare_asr(
        &self,
        request: &PrepareAsrRequest,
    ) -> Result<PrepareAsrResponse, String> {
        self.client
            .post(&format!("/v1/sessions/{}/asr", self.session_id), request)
            .await
    }

    pub async fn prepare_translation(
        &self,
        request: &PrepareTranslationRequest,
    ) -> Result<PrepareTranslationResponse, String> {
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
    ) -> Result<RecordTranslationResponse, String> {
        self.client
            .post(
                &format!("/v1/sessions/{}/results", self.session_id),
                request,
            )
            .await
    }

    pub async fn close(self) -> Result<(), String> {
        let response = self
            .client
            .http
            .delete(format!(
                "{}/v1/sessions/{}",
                self.client.base_url, self.session_id
            ))
            .send()
            .await
            .map_err(|error| format!("XR Corpus request failed: {error}"))?;
        if response.status() == StatusCode::NO_CONTENT || response.status() == StatusCode::NOT_FOUND
        {
            Ok(())
        } else {
            Err(response_error(response).await)
        }
    }
}

async fn decode<T: serde::de::DeserializeOwned>(response: reqwest::Response) -> Result<T, String> {
    if response.status().is_success() {
        response
            .json()
            .await
            .map_err(|error| format!("invalid XR Corpus response: {error}"))
    } else {
        Err(response_error(response).await)
    }
}

async fn response_error(response: reqwest::Response) -> String {
    let status = response.status();
    match response.json::<ErrorResponse>().await {
        Ok(body) => format!("XR Corpus returned {status}: {}", body.error),
        Err(error) => format!("XR Corpus returned {status} with an invalid error body: {error}"),
    }
}

fn request_error(error: reqwest::Error) -> String {
    format!("XR Corpus request failed: {error}")
}
