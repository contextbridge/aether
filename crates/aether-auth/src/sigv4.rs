use aws_config::Region;
use aws_config::default_provider::credentials::DefaultCredentialsChain;
use aws_credential_types::Credentials;
use aws_credential_types::provider::{ProvideCredentials, SharedCredentialsProvider};
use aws_sigv4::http_request::{SignableBody, SignableRequest, SigningSettings, sign};
use aws_sigv4::sign::v4;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use std::collections::HashMap;
use std::error::Error;
use std::sync::{Arc, PoisonError};
use std::time::{Duration, SystemTime};
use tokio::sync::{Mutex, OnceCell};

#[derive(Debug, Clone)]
pub struct SigV4Signer {
    state: Arc<SignerState>,
}

#[derive(Debug, Clone, Default)]
pub struct SigV4Signers {
    signers: Arc<std::sync::Mutex<HashMap<SignerKey, SigV4Signer>>>,
}

#[derive(Debug, Clone)]
pub struct SigV4Config {
    pub service: String,
    pub region: String,
    pub credentials: SigV4Credentials,
    pub refresh_before_expiry: Duration,
}

#[derive(Debug, Clone)]
pub enum SigV4Credentials {
    DefaultChain { profile: Option<String> },
    Provider(SharedCredentialsProvider),
}

#[derive(Debug, thiserror::Error)]
pub enum SigV4Error {
    #[error("Could not resolve AWS credentials: {0}")]
    Credentials(String),

    #[error("SigV4 signing failed: {0}")]
    Signing(String),
}

impl SigV4Signer {
    pub fn new(config: SigV4Config) -> Self {
        Self { state: Arc::new(SignerState { config, default_chain: OnceCell::new(), cached: Mutex::default() }) }
    }

    pub fn config(&self) -> &SigV4Config {
        &self.state.config
    }

    pub async fn headers(&self, method: &str, url: &str, body: &[u8]) -> Result<HeaderMap, SigV4Error> {
        let identity = self.credentials().await?.into();
        let params = v4::SigningParams::builder()
            .identity(&identity)
            .region(&self.config().region)
            .name(&self.config().service)
            .time(SystemTime::now())
            .settings(SigningSettings::default())
            .build()
            .map_err(signing_error)?
            .into();

        let signable =
            SignableRequest::new(method, url, std::iter::empty(), SignableBody::Bytes(body)).map_err(signing_error)?;

        let (instructions, _signature) = sign(signable, &params).map_err(signing_error)?.into_parts();
        instructions.headers().map(|(name, value)| sensitive_header(name, value)).collect()
    }

    async fn credentials(&self) -> Result<Credentials, SigV4Error> {
        let mut cached = self.state.cached.lock().await;
        if let Some(credentials) = cached.as_ref().filter(|credentials| self.is_fresh(credentials)) {
            return Ok(credentials.clone());
        }
        let credentials = self
            .provider()
            .await
            .provide_credentials()
            .await
            .map_err(|error| SigV4Error::Credentials(error_chain(&error)))?;

        *cached = Some(credentials.clone());
        Ok(credentials)
    }

    async fn provider(&self) -> &SharedCredentialsProvider {
        let config = self.config();
        match &config.credentials {
            SigV4Credentials::Provider(provider) => provider,
            SigV4Credentials::DefaultChain { profile } => {
                self.state.default_chain.get_or_init(|| default_chain(&config.region, profile.as_deref())).await
            }
        }
    }

    fn is_fresh(&self, credentials: &Credentials) -> bool {
        credentials.expiry().is_none_or(|expiry| expiry > SystemTime::now() + self.config().refresh_before_expiry)
    }
}

impl SigV4Signers {
    pub fn default_chain(&self, service: String, region: String, profile: Option<String>) -> SigV4Signer {
        let mut signers = self.signers.lock().unwrap_or_else(PoisonError::into_inner);
        let signer = signers.entry((service, region, profile)).or_insert_with_key(|(service, region, profile)| {
            SigV4Signer::new(SigV4Config {
                service: service.clone(),
                region: region.clone(),
                credentials: SigV4Credentials::DefaultChain { profile: profile.clone() },
                refresh_before_expiry: Duration::from_mins(5),
            })
        });
        signer.clone()
    }
}

type SignerKey = (String, String, Option<String>);

#[derive(Debug)]
struct SignerState {
    config: SigV4Config,
    default_chain: OnceCell<SharedCredentialsProvider>,
    cached: Mutex<Option<Credentials>>,
}

async fn default_chain(region: &str, profile: Option<&str>) -> SharedCredentialsProvider {
    let chain = DefaultCredentialsChain::builder().region(Region::new(region.to_string()));
    let chain = match profile {
        Some(profile) => chain.profile_name(profile),
        None => chain,
    };
    SharedCredentialsProvider::new(chain.build().await)
}

fn sensitive_header(name: &str, value: &str) -> Result<(HeaderName, HeaderValue), SigV4Error> {
    let invalid = || SigV4Error::Signing(format!("invalid signed header `{name}`"));
    let mut value = HeaderValue::from_str(value).map_err(|_| invalid())?;
    value.set_sensitive(true);
    Ok((HeaderName::try_from(name).map_err(|_| invalid())?, value))
}

fn signing_error(error: impl Error + 'static) -> SigV4Error {
    SigV4Error::Signing(error_chain(&error))
}

fn error_chain(error: &(dyn Error + 'static)) -> String {
    std::iter::successors(Some(error), |&error| error.source()).map(ToString::to_string).collect::<Vec<_>>().join(": ")
}
