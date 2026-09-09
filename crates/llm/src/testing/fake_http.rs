use std::collections::HashMap;
use std::future::{Ready, ready};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use reqwest::{Method, Request, Response};
use tower::Service;

#[derive(Clone, Default)]
pub(crate) struct FakeHttpService {
    state: Arc<Mutex<State>>,
}

impl FakeHttpService {
    pub(crate) fn route(&self, method: Method, url: &str, response: impl Fn() -> Response + Send + Sync + 'static) {
        self.state.lock().unwrap().routes.insert((method, url.to_string()), Arc::new(response));
    }

    pub(crate) fn take_requests(&self) -> Vec<Request> {
        std::mem::take(&mut self.state.lock().unwrap().requests)
    }
}

impl Service<Request> for FakeHttpService {
    type Response = Response;
    type Error = reqwest::Error;
    type Future = Ready<Result<Response, reqwest::Error>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: Request) -> Self::Future {
        let mut state = self.state.lock().unwrap();
        let route = state.routes.get(&(request.method().clone(), request.url().to_string())).cloned();
        state.requests.push(request);
        drop(state);
        ready(Ok(route.map_or_else(
            || http::Response::builder().status(404).body("Not found").unwrap().into(),
            |response| response(),
        )))
    }
}

#[derive(Default)]
struct State {
    routes: HashMap<(Method, String), Arc<dyn Fn() -> Response + Send + Sync>>,
    requests: Vec<Request>,
}
