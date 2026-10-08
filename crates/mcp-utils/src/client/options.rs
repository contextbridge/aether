use super::Elicitation;
use aether_auth::OAuthCredentialStorage;
use rmcp::model::{
    ClientCapabilities, ClientConfig, ElicitationCapability, FormElicitationCapability, Implementation,
    UrlElicitationCapability,
};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;

#[derive(Clone)]
pub struct ClientOptions {
    pub(super) implementation: Implementation,
    pub(super) elicitations: Option<mpsc::Sender<Elicitation>>,
    pub(super) elicitation_capability: ElicitationCapability,
    pub(super) oauth_store: Option<Arc<dyn OAuthCredentialStorage>>,
    pub(super) oauth_client_metadata_url: Option<String>,
    pub(super) oauth_callback_port: u16,
    pub(super) cwd: Option<PathBuf>,
}

impl Default for ClientOptions {
    fn default() -> Self {
        Self {
            implementation: Implementation::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION")),
            elicitations: None,
            elicitation_capability: ElicitationCapability::new()
                .with_form(FormElicitationCapability::new())
                .with_url(UrlElicitationCapability::new()),
            oauth_store: None,
            oauth_client_metadata_url: None,
            oauth_callback_port: 0,
            cwd: None,
        }
    }
}

impl ClientOptions {
    pub fn implementation(mut self, implementation: Implementation) -> Self {
        self.implementation = implementation;
        self
    }

    pub fn elicitation(mut self, events: mpsc::Sender<Elicitation>) -> Self {
        self.elicitations = Some(events);
        self
    }

    pub fn elicitation_capability(mut self, capability: ElicitationCapability) -> Self {
        self.elicitation_capability = capability;
        self
    }

    pub fn oauth_store(mut self, store: Arc<dyn OAuthCredentialStorage>) -> Self {
        self.oauth_store = Some(store);
        self
    }

    pub fn oauth_client_metadata_url(mut self, url: impl Into<String>) -> Self {
        self.oauth_client_metadata_url = Some(url.into());
        self
    }

    pub fn oauth_callback_port(mut self, port: u16) -> Self {
        self.oauth_callback_port = port;
        self
    }

    pub fn cwd(mut self, dir: impl Into<PathBuf>) -> Self {
        self.cwd = Some(dir.into());
        self
    }

    pub fn client_config(&self) -> ClientConfig {
        let mut capabilities = ClientCapabilities::builder().enable_tasks().build();
        capabilities.elicitation = self
            .elicitations
            .as_ref()
            .map(|_| self.elicitation_capability.clone())
            .filter(|capability| capability.form.is_some() || capability.url.is_some());
        ClientConfig::new(capabilities, self.implementation.clone())
    }

    pub(super) fn oauth_prompts(&self) -> Option<&mpsc::Sender<Elicitation>> {
        self.elicitations.as_ref().filter(|_| self.elicitation_capability.url.is_some())
    }
}
