mod ci;

use framer::rust::{CargoWorkspace, Level, Lints, Profile, Toolchain};
use framer::{Project, TomlFile};
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut project = Project::new();
    project.add(workspace());
    project.add(Toolchain::pinned("1.100").components(["rustfmt", "clippy", "rust-analyzer"]));
    project.add(
        TomlFile::new("rustfmt.toml")
            .set("edition", "2024")
            .set("max_width", 120)
            .set("newline_style", "Unix")
            .set("use_field_init_shorthand", true)
            .set("use_small_heuristics", "Max")
            .set("use_try_shorthand", true),
    );
    project.add(ci::workflow());
    project.synth()
}

fn workspace() -> CargoWorkspace {
    CargoWorkspace::new()
        .resolver(Some("3"))
        .members(["crates/*"])
        .lints(
            Lints::new()
                .rust("unreachable_pub", Level::Warn)
                .clippy_group("pedantic", Level::Warn)
                .clippy_group("all", Level::Warn)
                .allow_clippy([
                    "must_use_candidate",
                    "missing_errors_doc",
                    "missing_panics_doc",
                    "return_self_not_must_use",
                    "pub_underscore_fields",
                ]),
        )
        .profile(Profile::new("dev").set("debug", 1))
        .profile(Profile::new("dist").inherits("release").lto("thin"))
}
