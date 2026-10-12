use super::{FakeMcpServer, FakeTool, FakeToolResponse, Gate, completed_task_payload};
use rmcp::model::{
    CallToolResponse, CallToolResult, ContentBlock, CreateTaskResult, DetailedTask, ElicitRequest, ElicitRequestParams,
    ElicitResult, ElicitationAction, InputRequest, InputRequests, InputRequiredResult, Task, TaskPayload, TaskStatus,
};
use serde_json::json;

#[derive(Clone)]
pub struct FakeTask {
    seed: Task,
    states: Vec<DetailedTask>,
    progress: Vec<(f64, Option<f64>)>,
    gate: Option<Gate>,
    get_failures: usize,
    update_failures: usize,
}

impl FakeTask {
    pub fn new(task_id: &str) -> Self {
        Self {
            seed: task(task_id, TaskStatus::Working),
            states: vec![working_task(task_id)],
            progress: Vec::new(),
            gate: None,
            get_failures: 0,
            update_failures: 0,
        }
    }

    pub fn seed(mut self, seed: Task) -> Self {
        self.seed = seed;
        self
    }

    pub fn states(mut self, states: impl IntoIterator<Item = DetailedTask>) -> Self {
        self.states = states.into_iter().collect();
        self
    }

    pub fn progress(mut self, progress: f64, total: Option<f64>) -> Self {
        self.progress.push((progress, total));
        self
    }

    pub fn gated(mut self, gate: Gate) -> Self {
        self.gate = Some(gate);
        self
    }

    pub fn failing_gets(mut self, failures: usize) -> Self {
        self.get_failures = failures;
        self
    }

    pub fn failing_updates(mut self, failures: usize) -> Self {
        self.update_failures = failures;
        self
    }

    pub fn server(self, tool: &str) -> FakeMcpServer {
        let task_id = self.seed.task_id.clone();
        let mut response = FakeToolResponse::task(CreateTaskResult::new(self.seed));
        for (progress, total) in self.progress {
            response = response.progress(progress, total);
        }
        if let Some(gate) = self.gate {
            response = response.gated(gate);
        }
        FakeMcpServer::new()
            .with_tool(FakeTool::new(tool).responds(response))
            .with_task(task_id, self.states)
            .with_task_get_failures(self.get_failures)
            .with_task_update_failures(self.update_failures)
    }
}

pub fn task(task_id: &str, status: TaskStatus) -> Task {
    let now = chrono::Utc::now().to_rfc3339();
    Task::new(task_id, status, now.clone(), now).with_poll_interval_ms(10)
}

pub fn working_task(task_id: &str) -> DetailedTask {
    DetailedTask::new(task(task_id, TaskStatus::Working), TaskPayload::Working)
}

pub fn completed_task(task_id: &str, text: &str) -> DetailedTask {
    let result = CallToolResult::success(vec![ContentBlock::text(text)]);
    DetailedTask::new(task(task_id, TaskStatus::Completed), completed_task_payload(result))
}

pub fn failed_task(task_id: &str, message: &str) -> DetailedTask {
    let error = serde_json::from_value(json!({ "code": "boom", "message": message })).expect("an error object");
    DetailedTask::new(task(task_id, TaskStatus::Failed), TaskPayload::Failed { error })
}

pub fn cancelled_task(task_id: &str) -> DetailedTask {
    DetailedTask::new(task(task_id, TaskStatus::Cancelled), TaskPayload::Cancelled)
}

pub fn input_required_task(task_id: &str, keys: &[&str]) -> DetailedTask {
    let input_requests = keys.iter().map(|key| ((*key).to_string(), form_request("Provide a name"))).collect();
    DetailedTask::new(task(task_id, TaskStatus::InputRequired), TaskPayload::InputRequired { input_requests })
}

pub fn name_input(state: &str) -> CallToolResponse {
    InputRequiredResult::new(Some(input_requests("name", form_request("Name?"))), Some(state.to_string())).into()
}

pub fn accept_with_name(name: &str) -> ElicitResult {
    ElicitResult::new(ElicitationAction::Accept).with_content(json!({ "name": name }))
}

pub fn input_requests(key: &str, request: InputRequest) -> InputRequests {
    InputRequests::from([(key.to_string(), request)])
}

pub fn form_request(message: &str) -> InputRequest {
    let requested_schema = serde_json::from_value(json!({
        "type": "object",
        "properties": { "name": { "type": "string" } },
        "required": ["name"]
    }))
    .expect("a valid elicitation schema");
    InputRequest::Elicitation(ElicitRequest::new(ElicitRequestParams::FormElicitationParams {
        meta: None,
        message: message.to_string(),
        requested_schema,
    }))
}

pub fn url_request(message: &str, url: &str, elicitation_id: &str) -> InputRequest {
    InputRequest::Elicitation(ElicitRequest::new(ElicitRequestParams::UrlElicitationParams {
        meta: None,
        message: message.to_string(),
        url: url.to_string(),
        elicitation_id: elicitation_id.to_string(),
    }))
}
