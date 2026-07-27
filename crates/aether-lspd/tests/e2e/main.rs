// `common` stays at `tests/common` so the two binaries kept separate below can
// still share it. Those two configure the fake LSP server differently, and
// `use_fake_rust_server_with_args` applies its configuration process-wide once.
#[path = "../common/mod.rs"]
mod common;

mod e2e_error_handling;
mod e2e_lifecycle;
mod e2e_requests;
