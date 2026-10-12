use crate::client::{ClientOptions, Elicitation, ElicitationSink, cancelled};
use futures::future::BoxFuture;
use rmcp::model::{ElicitRequestParams, ElicitResult};
use std::collections::VecDeque;
use std::future::Future;
use std::sync::{Arc, Mutex, PoisonError};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

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

impl ElicitationScript {
    pub fn builder() -> ElicitationScriptBuilder {
        ElicitationScriptBuilder::default()
    }

    pub fn answering(responses: impl IntoIterator<Item = ElicitResult>) -> (ClientOptions, Self) {
        let (sink, script) = Self::builder().responses(responses).sink();
        (ClientOptions::default().elicitation(sink), script)
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

    pub fn sink(self) -> (ElicitationSink, ElicitationScript) {
        let (sink, events) = elicitation_sink();
        (sink, self.spawn(events))
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

pub fn elicitation_sink() -> (ElicitationSink, mpsc::Receiver<Elicitation>) {
    let (events, host) = mpsc::channel(32);
    (ElicitationSink::form_and_url(events), host)
}

pub fn elicitation_host() -> (ClientOptions, mpsc::Receiver<Elicitation>) {
    let (sink, host) = elicitation_sink();
    (ClientOptions::default().elicitation(sink), host)
}

type UrlHandler = Arc<dyn Fn(String, String) -> BoxFuture<'static, ()> + Send + Sync>;
