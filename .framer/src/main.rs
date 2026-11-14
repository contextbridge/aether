use framer::Project;
use framer::rust::{Level, Lints, Toolchain};
use framer::toml_edit::{Array, InlineTable, Value};
use framer_presets::{Dist, Release, ReleasePlz, Rust, RustMonorepo, Rustfmt, Tools, Workspace};
use std::process::ExitCode;

const MSRV: &str = "1.100";

fn main() -> ExitCode {
    let mut project = Project::new();
    project.apply(
        RustMonorepo::new()
            .rust(Rust::new(MSRV).targets(["wasm32-unknown-unknown"]))
            .workspace(
                Workspace::new()
                    .members(["crates/*", "packages/aether-browser"])
                    .extra_manifests([".framer/Cargo.toml"])
                    .repository("https://github.com/contextbridge/aether"),
            )
            .lints(lints())
            .rustfmt(
                Rustfmt::new()
                    .set("max_width", 120)
                    .set("newline_style", "Unix")
                    .set("use_small_heuristics", "Max"),
            )
            .tools(
                Tools::new()
                    .tool("git-cliff", "2.12.0")
                    .tool("nodejs", "24.15.0")
                    .tool("pnpm", "12.16.1")
                    .tool("aqua:wasm-bindgen/wasm-pack", "0.15.0"),
            )
            // ci.yml is hand-written: zizmor requires SHA-pinned actions, which the preset's CI doesn't use.
            .without_ci()
            .without_conventions()
            .release(Release::new().release_plz(release_plz()).dist(dist())),
    );

    let toolchain = project.get_mut::<Toolchain>().expect("RustMonorepo adds a toolchain");
    *toolchain = toolchain.clone().components(["rust-analyzer"]);

    // release-plz.yml is hand-written: it uses the ContextBridge app's client id and the `ci` environment's secrets.
    project.unmanage(".github/workflows/release-plz.yml");

    project.toml("clippy.toml").set("absolute-paths-max-segments", 3);
    project
        .toml(".config/nextest.toml")
        .set("profile.default.default-filter", "not package(internal-evals)")
        .set("test-groups.evals", inline([("max-threads", Value::from(5))]))
        .set("profile.default.overrides", Array::from_iter([eval_override()]));

    project.synth()
}

fn lints() -> Lints {
    Lints::clippy_pedantic()
        .allow_clippy([
            "missing_errors_doc",
            "missing_panics_doc",
            "must_use_candidate",
            "return_self_not_must_use",
            "pub_underscore_fields",
        ])
        .clippy("absolute_paths", Level::Warn)
        .rust("unreachable_pub", Level::Warn)
}

fn release_plz() -> ReleasePlz {
    ReleasePlz::new().set("release_always", true).set("dependencies_update", false).set("semver_check", false)
}

fn dist() -> Dist {
    Dist::new()
        .version("0.34.0")
        .installers(["shell", "npm", "homebrew"])
        .homebrew_tap("contextbridge/homebrew-tap")
        .allow_dirty_ci()
        .set("publish-jobs", Array::from_iter(["homebrew", "npm"]))
        .set("packages", Array::from_iter(["aether-agent-cli"]))
        .set("npm-scope", "@aether-agent")
        .set("npm-package", "cli")
        .set("macos-sign", true)
        .set("dependencies.apt.libdbus-1-dev", "*")
}

fn eval_override() -> Value {
    inline([
        ("filter", Value::from("package(internal-evals) and test(/_eval$/)")),
        ("test-group", Value::from("evals")),
        ("slow-timeout", inline([("period", Value::from("180s")), ("terminate-after", Value::from(3))])),
        ("failure-output", Value::from("immediate-final")),
    ])
}

fn inline<const N: usize>(entries: [(&str, Value); N]) -> Value {
    let mut table = InlineTable::new();
    for (key, value) in entries {
        table.insert(key, value);
    }
    Value::InlineTable(table)
}
