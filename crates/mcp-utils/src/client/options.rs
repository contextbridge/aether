use super::{Elicitation, ElicitationSink};
use aether_auth::OAuthCredentialStorage;
use rmcp::model::{ClientCapabilities, ClientConfig, Implementation};
use std::sync::Arc;
use tokio::sync::mpsc;

#[derive(Clone)]
pub struct ClientOptions {
    pub(super) implementation: Implementation,
    pub(super) elicitation: Option<ElicitationSink>,
    pub(super) oauth_store: Option<Arc<dyn OAuthCredentialStorage>>,
    tasks: bool,
}

impl Default for ClientOptions {
    fn default() -> Self {
        Self {
            implementation: Implementation::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION")),
            elicitation: None,
            oauth_store: None,
            tasks: true,
        }
    }
}

impl ClientOptions {
    pub fn implementation(mut self, implementation: Implementation) -> Self {
        self.implementation = implementation;
        self
    }

    pub fn elicitation(mut self, sink: ElicitationSink) -> Self {
        self.elicitation = Some(sink);
        self
    }

    pub fn oauth_store(mut self, store: Arc<dyn OAuthCredentialStorage>) -> Self {
        self.oauth_store = Some(store);
        self
    }

    #[cfg(any(test, feature = "testing"))]
    pub fn without_tasks(mut self) -> Self {
        self.tasks = false;
        self
    }

    pub(super) fn client_config(&self) -> ClientConfig {
        let mut capabilities = if self.tasks {
            ClientCapabilities::builder().enable_tasks().build()
        } else {
            ClientCapabilities::default()
        };
        capabilities.elicitation = self
            .elicitation
            .as_ref()
            .map(|sink| sink.capability.clone())
            .filter(|capability| capability.form.is_some() || capability.url.is_some());
        ClientConfig::new(capabilities, self.implementation.clone())
    }

    pub(super) fn elicitations(&self) -> Option<&mpsc::Sender<Elicitation>> {
        self.elicitation.as_ref().map(|sink| &sink.events)
    }

    pub(super) fn oauth_prompts(&self) -> Option<&mpsc::Sender<Elicitation>> {
        self.elicitation.as_ref().filter(|sink| sink.capability.url.is_some()).map(|sink| &sink.events)
    }
}
