use framer::TextFile;
use framer::github::{Job, Step, Triggers, Workflow};
use framer::yaml::Yaml;

pub fn workflow() -> TextFile {
    let workflow = Workflow::new("ci", "CI")
        .on(Triggers::new().push(["main"]).pull_request())
        .permission("contents", "read")
        .env("CARGO_INCREMENTAL", "0")
        .job(tooling_job("fmt", "Formatting").step(run("Check formatting", "just fmt-check")))
        .job(compiling_job("lint", "Clippy", 15).step(run("Clippy check", "just lint")))
        .job(compiling_job("test", "Test", 30).step(run("Run tests", "just test")))
        .job(sdk_job())
        .job(compiling_job("docs", "Docs", 15).step(run("Check documentation", "just doc-check")))
        .job(website_job())
        .job(Job::new("framer").name("Generated files").timeout_minutes(15).step(checkout()).step(run(
            "Check generated files are up to date",
            "cargo run --locked --manifest-path .framer/Cargo.toml -- --check",
        )));

    // Framer's typed concurrency setter only accepts a boolean, not an Actions expression.
    let yaml = workflow.to_yaml().with(
        "concurrency",
        Yaml::map()
            .with("group", "ci-${{ github.workflow }}-${{ github.ref }}")
            .with("cancel-in-progress", "${{ github.event_name == 'pull_request' }}"),
    );
    TextFile::new(workflow.path(), yaml.to_yaml_string())
}

fn sdk_job() -> Job {
    compiling_job("sdk", "TypeScript SDK", 20)
        .step(run("Install dependencies", "pnpm install --frozen-lockfile"))
        .step(run("Check formatting", "pnpm fmt-check"))
        .step(run("Typecheck", "pnpm sdk:typecheck"))
        .step(run("Build SDK", "pnpm sdk:build"))
        .step(run("Build aether CLI", "cargo build -p aether-agent-cli"))
        .step(run("Test", "pnpm sdk:test"))
        .step(run("Typecheck evals", "pnpm evals:typecheck"))
        .step(run("Build evals", "pnpm evals:build"))
        .step(run("Test evals", "pnpm evals:test"))
}

fn website_job() -> Job {
    tooling_job("website", "Website")
        .step(run("Install dependencies", "pnpm install --frozen-lockfile"))
        .step(run("Check formatting", "pnpm fmt-check"))
        .step(run("Typecheck website", "pnpm website:check"))
        .step(run("Build website", "pnpm website:build"))
}

fn tooling_job(id: &str, name: &str) -> Job {
    Job::new(id).name(name).timeout_minutes(15).step(checkout()).step(mise())
}

fn compiling_job(id: &str, name: &str, timeout_minutes: u32) -> Job {
    Job::new(id)
        .name(name)
        .timeout_minutes(timeout_minutes)
        .env("RUSTFLAGS", "-Clink-arg=-fuse-ld=mold")
        .step(checkout())
        .step(Step::uses("$/.github/actions/setup-workspace"))
        .step(mise())
        .step(
            // PR caches previously raced eviction by exceeding GitHub's 10 GiB limit.
            Step::uses("Swatinem/rust-cache@d1527c8f67ef9af357290475d4af4b0fab8f12e0")
                .with("save-if", "${{ github.ref == 'refs/heads/main' }}"),
        )
}

fn checkout() -> Step {
    Step::uses("actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1")
        .name("Checkout repository")
        .with("persist-credentials", "false")
}

fn mise() -> Step {
    Step::uses("jdx/mise-action@b44d12e7d0d8ee7461bb893e1b1993500433185b")
}

fn run(name: &str, command: &str) -> Step {
    Step::run(command).name(name)
}
