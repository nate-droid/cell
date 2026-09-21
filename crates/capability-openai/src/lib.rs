use std::{fmt, sync::Arc, time::Duration};

use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use wasmtime::component::Linker;

pub use bindings::cell::openai::chat::{ChatRequest, ChatResponse, Message, Usage};

mod bindings {
    wasmtime::component::bindgen!({
        world: "host",
        path: "wit",
    });
}

const DEFAULT_OPENAI_BASE_URL: &str = "https://api.openai.com/v1";
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);

/// A chat model capability backed by an OpenAI-compatible HTTP API.
///
/// This capability calls `POST {base_url}/chat/completions` using the common
/// OpenAI chat-completions JSON shape. It can target OpenAI itself or local
/// servers that implement the same API, such as Ollama, llama.cpp, vLLM, or
/// LocalAI.
#[derive(Clone, Debug)]
pub struct OpenAiChat {
    inner: Arc<OpenAiChatInner>,
}

struct OpenAiChatInner {
    client: Client,
    base_url: String,
    api_key: Option<String>,
    model: String,
}

impl fmt::Debug for OpenAiChatInner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenAiChatInner")
            .field("base_url", &self.base_url)
            .field("api_key", &self.api_key.as_ref().map(|_| "[REDACTED]"))
            .field("model", &self.model)
            .finish_non_exhaustive()
    }
}

impl OpenAiChat {
    /// Create a capability that targets OpenAI's public API.
    pub fn new(model: impl Into<String>, api_key: impl Into<String>) -> anyhow::Result<Self> {
        Self::builder(model)
            .api_key(api_key)
            .base_url(DEFAULT_OPENAI_BASE_URL)
            .build()
    }

    /// Start building an OpenAI-compatible chat capability.
    pub fn builder(model: impl Into<String>) -> OpenAiChatBuilder {
        OpenAiChatBuilder::new(model)
    }

    /// Create a capability for a local OpenAI-compatible server without an API key.
    pub fn local(model: impl Into<String>, base_url: impl Into<String>) -> anyhow::Result<Self> {
        Self::builder(model).base_url(base_url).build()
    }

    pub fn base_url(&self) -> &str {
        &self.inner.base_url
    }

    pub fn model(&self) -> &str {
        &self.inner.model
    }

    pub fn complete(&self, request: ChatRequest) -> Result<ChatResponse, String> {
        self.complete_inner(request)
    }

    fn complete_inner(&self, request: ChatRequest) -> Result<ChatResponse, String> {
        if request.messages.is_empty() {
            return Err("chat request must include at least one message".to_string());
        }

        let url = format!(
            "{}/chat/completions",
            self.inner.base_url.trim_end_matches('/')
        );

        let body = OpenAiChatCompletionsRequest {
            model: self.inner.model.clone(),
            messages: request.messages.into_iter().map(Into::into).collect(),
            temperature: request.temperature,
            max_tokens: request.max_tokens,
        };

        let mut http_request = self.inner.client.post(url).json(&body);

        if let Some(api_key) = &self.inner.api_key {
            http_request = http_request.bearer_auth(api_key);
        }

        let response = http_request
            .send()
            .map_err(|error| format!("failed to call OpenAI-compatible chat API: {error}"))?;

        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .unwrap_or_else(|error| format!("<failed to read error body: {error}>"));
            return Err(format!(
                "OpenAI-compatible chat API returned {status}: {body}"
            ));
        }

        let response: OpenAiChatCompletionsResponse = response.json().map_err(|error| {
            format!("failed to decode OpenAI-compatible chat response: {error}")
        })?;

        let choice = response.choices.into_iter().next().ok_or_else(|| {
            "OpenAI-compatible chat response did not include any choices".to_string()
        })?;

        Ok(ChatResponse {
            content: choice.message.content.unwrap_or_default(),
            model: response.model,
            usage: response.usage.map(Into::into),
        })
    }
}

/// Builder for [`OpenAiChat`].
#[derive(Clone)]
pub struct OpenAiChatBuilder {
    base_url: String,
    api_key: Option<String>,
    model: String,
    timeout: Duration,
}

impl fmt::Debug for OpenAiChatBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenAiChatBuilder")
            .field("base_url", &self.base_url)
            .field("api_key", &self.api_key.as_ref().map(|_| "[REDACTED]"))
            .field("model", &self.model)
            .field("timeout", &self.timeout)
            .finish()
    }
}

impl OpenAiChatBuilder {
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            base_url: DEFAULT_OPENAI_BASE_URL.to_string(),
            api_key: None,
            model: model.into(),
            timeout: DEFAULT_TIMEOUT,
        }
    }

    pub fn base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    pub fn api_key(mut self, api_key: impl Into<String>) -> Self {
        self.api_key = Some(api_key.into());
        self
    }

    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn build(self) -> anyhow::Result<OpenAiChat> {
        let model = self.model.trim().to_string();
        if model.is_empty() {
            anyhow::bail!("model must not be empty");
        }

        let base_url = self.base_url.trim().trim_end_matches('/').to_string();
        if base_url.is_empty() {
            anyhow::bail!("base_url must not be empty");
        }

        let client = Client::builder().timeout(self.timeout).build()?;

        Ok(OpenAiChat {
            inner: Arc::new(OpenAiChatInner {
                client,
                base_url,
                api_key: self.api_key,
                model,
            }),
        })
    }
}

impl bindings::cell::openai::chat::Host for OpenAiChat {
    fn complete(&mut self, request: ChatRequest) -> Result<ChatResponse, String> {
        OpenAiChat::complete(self, request)
    }
}

impl<T> cell::Capability<T> for OpenAiChat
where
    T: Send + 'static,
{
    fn name(&self) -> &'static str {
        "cell:openai/chat@0.1.0"
    }

    fn add_to_linker(self, linker: &mut Linker<T>) -> anyhow::Result<()> {
        let chat = self.clone();

        linker
            .instance("cell:openai/chat@0.1.0")?
            .func_wrap("complete", move |_store, (request,): (ChatRequest,)| {
                Ok((chat.complete(request),))
            })?;

        Ok(())
    }
}

#[derive(Debug, Serialize)]
struct OpenAiChatCompletionsRequest {
    model: String,
    messages: Vec<OpenAiMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
}

#[derive(Debug, Serialize)]
struct OpenAiMessage {
    role: String,
    content: String,
}

impl From<Message> for OpenAiMessage {
    fn from(message: Message) -> Self {
        Self {
            role: message.role,
            content: message.content,
        }
    }
}

#[derive(Debug, Deserialize)]
struct OpenAiChatCompletionsResponse {
    model: String,
    choices: Vec<OpenAiChoice>,
    usage: Option<OpenAiUsage>,
}

#[derive(Debug, Deserialize)]
struct OpenAiChoice {
    message: OpenAiResponseMessage,
}

#[derive(Debug, Deserialize)]
struct OpenAiResponseMessage {
    content: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenAiUsage {
    prompt_tokens: Option<u32>,
    completion_tokens: Option<u32>,
    total_tokens: Option<u32>,
}

impl From<OpenAiUsage> for Usage {
    fn from(usage: OpenAiUsage) -> Self {
        Self {
            prompt_tokens: usage.prompt_tokens,
            completion_tokens: usage.completion_tokens,
            total_tokens: usage.total_tokens,
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use cell::Capability;

    fn test_chat() -> anyhow::Result<OpenAiChat> {
        OpenAiChat::local(
            std::env::var("APFEL_TEST_MODEL").unwrap_or_else(|_| "apple-foundationmodel".into()),
            std::env::var("APFEL_TEST_BASE_URL")
                .unwrap_or_else(|_| "http://localhost:11434/v1".into()),
        )
    }

    #[test]
    fn debug_redacts_api_key() -> anyhow::Result<()> {
        let secret = "sk-debug-regression-sentinel";
        let builder = OpenAiChat::builder("debug-test-model")
            .base_url("https://example.com/v1")
            .api_key(secret)
            .timeout(Duration::from_secs(7));
        let chat = builder.clone().build()?;

        for output in [
            format!("{builder:?}"),
            format!("{builder:#?}"),
            format!("{chat:?}"),
            format!("{chat:#?}"),
            format!("{:?}", chat.clone()),
            format!("{:?}", chat.inner),
        ] {
            assert!(!output.contains(secret));
            assert!(output.contains("[REDACTED]"));
            assert!(output.contains("https://example.com/v1"));
            assert!(output.contains("debug-test-model"));
        }
        assert!(format!("{builder:?}").contains("7s"));

        Ok(())
    }

    #[test]
    fn debug_distinguishes_absent_api_key() -> anyhow::Result<()> {
        let builder = OpenAiChat::builder("debug-test-model");
        let chat = builder.clone().build()?;

        for output in [format!("{builder:?}"), format!("{chat:?}")] {
            assert!(output.contains("api_key: None"));
            assert!(!output.contains("[REDACTED]"));
        }

        Ok(())
    }

    #[test]
    fn builder_rejects_empty_model() {
        assert!(OpenAiChat::builder(" ").build().is_err());
    }

    #[test]
    fn builder_normalizes_base_url() -> anyhow::Result<()> {
        let chat = OpenAiChat::builder("apple-foundationmodel")
            .base_url("http://localhost:11434/v1/")
            .build()?;

        assert_eq!(chat.base_url(), "http://localhost:11434/v1");
        assert_eq!(chat.model(), "apple-foundationmodel");

        Ok(())
    }

    #[test]
    fn rejects_empty_message_list_before_http_call() -> anyhow::Result<()> {
        let chat = test_chat()?;

        let result = chat.complete(ChatRequest {
            messages: Vec::new(),
            temperature: None,
            max_tokens: None,
        });

        assert_eq!(
            result.unwrap_err(),
            "chat request must include at least one message"
        );

        Ok(())
    }

    #[test]
    fn adds_chat_function_to_cell_linker() -> anyhow::Result<()> {
        let mut cell = cell::Cell::<()>::empty()?;
        test_chat()?.add_to_linker(cell.linker_mut())?;
        Ok(())
    }

    #[test]
    #[ignore = "requires a running OpenAI-compatible chat API; set APFEL_TEST_BASE_URL/APFEL_TEST_MODEL and run explicitly"]
    fn agent_responds() -> anyhow::Result<()> {
        let chat = test_chat()?;

        let response = chat
            .complete(ChatRequest {
                messages: vec![Message {
                    role: "user".to_string(),
                    content: "Where does the rain in Spain remain?".to_string(),
                }],
                temperature: Some(0.0),
                max_tokens: Some(16),
            })
            .map_err(anyhow::Error::msg)?;

        assert!(
            !response.content.trim().is_empty(),
            "agent returned an empty response"
        );

        Ok(())
    }
}
