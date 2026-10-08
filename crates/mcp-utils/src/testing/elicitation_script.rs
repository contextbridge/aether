use crate::client::{Elicitation, ElicitationRequest, cancelled};
use futures::future::BoxFuture;
use rmcp::model::{ElicitRequestParams, ElicitResult};
use std::collections::VecDeque;
use std::future::Future;
use std::sync::{Arc, Mutex, PoisonError};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// Scripts the user's side of elicitation round trips: answers each
/// incoming request with the next queued response (Cancel once the queue
/// is empty) and records what arrived for assertions.
pub struct ElicitationScript {
    captured: Arc<Mutex<Vec<CapturedElicitation>>>,
    task: JoinHandle<()>,
}

#[derive(Default)]
pub struct ElicitationScriptBuilder {
    responses: VecDeque<ElicitResult>,
    on_url: Option<UrlHandler>,
}

#[derive(Clone)]
pub struct CapturedElicitation {
    pub server_name: String,
    pub request: ElicitRequestParams,
}

pub fn elicitation(
    server: impl Into<String>,
    request: ElicitRequestParams,
) -> (Box<ElicitationRequest>, impl Future<Output = ElicitResult> + Send + 'static) {
    let (elicitation, response) = ElicitationRequest::new(server.into(), request);
    (Box::new(elicitation), async move { response.await.unwrap_or_else(|_| cancelled()) })
}

impl ElicitationScript {
    pub fn builder() -> ElicitationScriptBuilder {
        ElicitationScriptBuilder::default()
    }

    pub fn spawn(event_rx: mpsc::Receiver<Elicitation>, responses: impl IntoIterator<Item = ElicitResult>) -> Self {
        Self::builder().responses(responses).spawn(event_rx)
    }

    pub fn captured(&self) -> Vec<CapturedElicitation> {
        self.captured.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

impl Drop for ElicitationScript {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl ElicitationScriptBuilder {
    pub fn response(mut self, response: ElicitResult) -> Self {
        self.responses.push_back(response);
        self
    }

    pub fn responses(mut self, responses: impl IntoIterator<Item = ElicitResult>) -> Self {
        self.responses.extend(responses);
        self
    }

    pub fn on_url<F, Fut>(mut self, handler: F) -> Self
    where
        F: Fn(String, String) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        self.on_url = Some(Arc::new(move |url, id| Box::pin(handler(url, id))));
        self
    }

    pub fn spawn(self, mut event_rx: mpsc::Receiver<Elicitation>) -> ElicitationScript {
        let Self { mut responses, on_url } = self;
        let captured = Arc::new(Mutex::new(Vec::new()));
        let recorder = Arc::clone(&captured);
        let task = tokio::spawn(async move {
            while let Some(event) = event_rx.recv().await {
                let Elicitation::Request(elicitation) = event else { continue };
                recorder.lock().unwrap_or_else(PoisonError::into_inner).push(CapturedElicitation {
                    server_name: elicitation.server.clone(),
                    request: elicitation.request.clone(),
                });
                if let (Some(on_url), ElicitRequestParams::UrlElicitationParams { url, elicitation_id, .. }) =
                    (&on_url, &elicitation.request)
                {
                    on_url(url.clone(), elicitation_id.clone()).await;
                }
                elicitation.respond(responses.pop_front().unwrap_or_else(cancelled));
            }
        });
        ElicitationScript { captured, task }
    }
}

type UrlHandler = Arc<dyn Fn(String, String) -> BoxFuture<'static, ()> + Send + Sync>;
