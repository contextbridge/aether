//! Lifecycle tests for the terminal-mode / ratatui RAII guard and the
//! init -> setup -> body -> teardown lifecycle `run_app` uses.
//!
//! Before this guard existed, a failed mode enable returned straight out of
//! `run_app` and the manual `ratatui::restore()` was skipped, stranding the
//! terminal in raw mode. A later regression did the same for a ratatui init
//! failure: `try_init_with_options(...)?` exited after `enable_raw_mode` had
//! already succeeded. These tests pin both fixes through
//! [`run_terminal_lifecycle`] — the same control flow `run_app` uses — driven by
//! an injectable [`TerminalIo`] so no real TTY is touched.
//!
//! Every setup operation is covered in both fail-before (nothing applied) and
//! fail-after-apply (a partial crossterm write that did land) shapes; teardown
//! resilience is asserted on fake terminal state. The fake does not model
//! ratatui's global panic hook (it is process-global and installed by
//! `init_ratatui` in production), so the panic test asserts the guard's own
//! contribution and documents where production's restore comes from.

use std::cell::RefCell;
use std::io;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;

use crossterm::event::KeyboardEnhancementFlags;
use futures::executor::block_on;
use ratatui::{TerminalOptions, Viewport};
use wisp_next::test_support::terminal::{LifecycleError, TerminalIo, TerminalModes, run_terminal_lifecycle};

/// When an operation should fail relative to mutating fake state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FailMode {
    /// Return `Err` without mutating state: the operation never took effect.
    Before,
    /// Mutate state, then return `Err`: a partial write whose bytes did land.
    After,
}

/// Per-operation failure injection. `None` means the operation succeeds.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Failures {
    init_ratatui: Option<FailMode>,
    push_keyboard_enhancement: Option<FailMode>,
    pop_keyboard_enhancement: Option<FailMode>,
    enable_bracketed_paste: Option<FailMode>,
    disable_bracketed_paste: Option<FailMode>,
    enable_mouse_capture: Option<FailMode>,
    disable_mouse_capture: Option<FailMode>,
}

/// Recorded terminal operation, in order, so teardown ordering is assertable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    InitRatatui,
    RestoreRatatui,
    PushKeyboardEnhancement,
    PopKeyboardEnhancement,
    EnableBracketedPaste,
    DisableBracketedPaste,
    EnableMouseCapture,
    DisableMouseCapture,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct FakeState {
    ratatui_active: bool,
    keyboard_enhancement_depth: usize,
    bracketed_paste: bool,
    mouse_capture: bool,
    log: Vec<Op>,
    failures: Failures,
}

/// Inspectable, in-memory stand-in for crossterm/ratatui. Each op is appended to
/// an ordered log, and any op can fail before or after mutating state to model a
/// partial write. The guard owns a handle; the test keeps a clone to snapshot.
#[derive(Clone, Default)]
struct FakeTerminalIo {
    state: Rc<RefCell<FakeState>>,
}

impl FakeTerminalIo {
    fn configure(&self, f: impl FnOnce(&mut Failures)) {
        f(&mut self.state.borrow_mut().failures);
    }

    fn snapshot(&self) -> FakeState {
        self.state.borrow().clone()
    }

    fn count(&self, op: Op) -> usize {
        self.state.borrow().log.iter().filter(|o| **o == op).count()
    }
}

fn unsupported(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::Unsupported, what)
}

/// Records `op`, then either mutates state and succeeds, mutates and fails
/// (`After`), or fails without mutating (`Before`).
fn try_op(
    state: &mut FakeState,
    op: Op,
    mode: Option<FailMode>,
    mutate: impl FnOnce(&mut FakeState),
    what: &str,
) -> io::Result<()> {
    state.log.push(op);
    match mode {
        Some(FailMode::Before) => Err(unsupported(what)),
        Some(FailMode::After) => {
            mutate(state);
            Err(unsupported(what))
        }
        None => {
            mutate(state);
            Ok(())
        }
    }
}

/// Viewport is ignored by the fake; any inline options will do.
fn inline_options() -> TerminalOptions {
    TerminalOptions { viewport: Viewport::Inline(1) }
}

/// Drives the lifecycle with a body returning `Result<(), BodyError>`.
fn run_lifecycle(
    fake: FakeTerminalIo,
    body: impl FnOnce((), TerminalModes<FakeTerminalIo>) -> futures::future::LocalBoxFuture<'static, Result<(), BodyError>>,
) -> Result<(), LifecycleError<BodyError>> {
    block_on(run_terminal_lifecycle(fake, inline_options(), body))
}

/// A body error type distinct from `io::Error`, proving a real typed `Err` flows
/// through the lifecycle unchanged.
#[derive(Debug, PartialEq, Eq)]
enum BodyError {
    EventLoop,
}

impl TerminalIo for FakeTerminalIo {
    type Terminal = ();

    fn init_ratatui(&mut self, _options: TerminalOptions) -> io::Result<()> {
        let mut state = self.state.borrow_mut();
        let mode = state.failures.init_ratatui.take();
        try_op(&mut state, Op::InitRatatui, mode, |s| s.ratatui_active = true, "init_ratatui")
    }

    fn restore_ratatui(&mut self) {
        let mut state = self.state.borrow_mut();
        state.log.push(Op::RestoreRatatui);
        state.ratatui_active = false;
    }

    fn push_keyboard_enhancement(&mut self, _flags: KeyboardEnhancementFlags) -> io::Result<()> {
        let mut state = self.state.borrow_mut();
        let mode = state.failures.push_keyboard_enhancement.take();
        try_op(
            &mut state,
            Op::PushKeyboardEnhancement,
            mode,
            |s| s.keyboard_enhancement_depth += 1,
            "push_keyboard_enhancement",
        )
    }

    fn pop_keyboard_enhancement(&mut self) -> io::Result<()> {
        let mut state = self.state.borrow_mut();
        let mode = state.failures.pop_keyboard_enhancement.take();
        try_op(
            &mut state,
            Op::PopKeyboardEnhancement,
            mode,
            |s| s.keyboard_enhancement_depth = s.keyboard_enhancement_depth.saturating_sub(1),
            "pop_keyboard_enhancement",
        )
    }

    fn enable_bracketed_paste(&mut self) -> io::Result<()> {
        let mut state = self.state.borrow_mut();
        let mode = state.failures.enable_bracketed_paste.take();
        try_op(&mut state, Op::EnableBracketedPaste, mode, |s| s.bracketed_paste = true, "enable_bracketed_paste")
    }

    fn disable_bracketed_paste(&mut self) -> io::Result<()> {
        let mut state = self.state.borrow_mut();
        let mode = state.failures.disable_bracketed_paste.take();
        try_op(&mut state, Op::DisableBracketedPaste, mode, |s| s.bracketed_paste = false, "disable_bracketed_paste")
    }

    fn enable_mouse_capture(&mut self) -> io::Result<()> {
        let mut state = self.state.borrow_mut();
        let mode = state.failures.enable_mouse_capture.take();
        try_op(&mut state, Op::EnableMouseCapture, mode, |s| s.mouse_capture = true, "enable_mouse_capture")
    }

    fn disable_mouse_capture(&mut self) -> io::Result<()> {
        let mut state = self.state.borrow_mut();
        let mode = state.failures.disable_mouse_capture.take();
        try_op(&mut state, Op::DisableMouseCapture, mode, |s| s.mouse_capture = false, "disable_mouse_capture")
    }
}

#[test]
fn normal_lifecycle_undoes_modes_before_restoring_ratatui_once() {
    let fake = FakeTerminalIo::default();

    run_lifecycle(fake.clone(), |_terminal, mut modes| {
        Box::pin(async move {
            modes.set_mouse_capture(false);
            Ok::<(), BodyError>(())
        })
    })
    .unwrap();

    let after = fake.snapshot();
    let restore_at = after.log.iter().position(|o| *o == Op::RestoreRatatui).expect("ratatui restored");
    // Modes are torn down while raw mode is still on, i.e. before the restore.
    assert!(after.log[..restore_at].contains(&Op::DisableBracketedPaste));
    assert!(after.log[..restore_at].contains(&Op::PopKeyboardEnhancement));
    assert_eq!(fake.count(Op::RestoreRatatui), 1);
    assert!(!after.ratatui_active);
    assert_eq!(after.keyboard_enhancement_depth, 0);
    assert!(!after.bracketed_paste);
}

#[test]
fn lifecycle_returns_ok_and_restores_on_completion() {
    let fake = FakeTerminalIo::default();

    let result = run_lifecycle(fake.clone(), |_terminal, mut modes| {
        Box::pin(async move {
            modes.set_mouse_capture(true);
            Ok::<(), BodyError>(())
        })
    });

    assert!(result.is_ok());
    let after = fake.snapshot();
    assert!(!after.mouse_capture, "mouse enabled during the run is disabled by teardown");
    assert!(!after.bracketed_paste);
    assert_eq!(after.keyboard_enhancement_depth, 0);
    assert!(!after.ratatui_active);
    assert_eq!(fake.count(Op::RestoreRatatui), 1);
}

#[test]
fn lifecycle_preserves_typed_runtime_error_and_restores() {
    let fake = FakeTerminalIo::default();

    let result = run_lifecycle(fake.clone(), |_terminal, mut modes| {
        Box::pin(async move {
            modes.set_mouse_capture(true);
            Err::<(), BodyError>(BodyError::EventLoop)
        })
    });

    match result {
        Err(LifecycleError::Runtime(BodyError::EventLoop)) => {}
        other => panic!("expected Runtime(EventLoop), got {other:?}"),
    }
    let after = fake.snapshot();
    assert!(!after.mouse_capture, "teardown still runs on the error path");
    assert!(!after.bracketed_paste);
    assert_eq!(after.keyboard_enhancement_depth, 0);
    assert!(!after.ratatui_active);
    assert_eq!(fake.count(Op::RestoreRatatui), 1);
}

/// A panic unwinding out of the body still undoes modes. Ratatui restore on the
/// panic path is owned by ratatui's global panic hook (installed by
/// `init_ratatui` in production), which fires before unwinding reaches this
/// guard; the fake does not model that process-global hook, so it records zero
/// restores here. Production restores exactly once via the hook.
#[test]
fn panic_during_body_undoes_modes_and_defers_restore_to_panic_hook() {
    let fake = FakeTerminalIo::default();

    let outcome = catch_unwind(AssertUnwindSafe(|| {
        run_lifecycle(fake.clone(), |_terminal, mut modes| {
            Box::pin(async move {
                modes.set_mouse_capture(true);
                panic!("event loop panicked");
            })
        })
    }));
    assert!(outcome.is_err(), "panic must propagate out of the lifecycle");

    let after = fake.snapshot();
    assert!(!after.mouse_capture, "mouse disabled during unwind");
    assert!(!after.bracketed_paste, "bracketed paste disabled during unwind");
    assert_eq!(after.keyboard_enhancement_depth, 0, "keyboard enhancement popped during unwind");
    // The guard does not restore here: it leaves that to ratatui's panic hook so
    // ratatui is restored exactly once rather than twice on a panic.
    assert_eq!(fake.count(Op::RestoreRatatui), 0);
}

/// A ratatui init failure after raw mode is active (`Terminal::with_options`
/// failed once `enable_raw_mode` succeeded) must still restore, with the original
/// init error preserved.
#[test]
fn init_failure_after_raw_mode_restores_ratatui_and_preserves_error() {
    let fake = FakeTerminalIo::default();
    fake.configure(|f| f.init_ratatui = Some(FailMode::After));

    let result = run_lifecycle(fake.clone(), |_terminal, _modes| Box::pin(async move { Ok::<(), BodyError>(()) }));

    match result {
        Err(LifecycleError::Init(error)) => assert_eq!(error.kind(), io::ErrorKind::Unsupported),
        other => panic!("expected Init error, got {other:?}"),
    }
    let after = fake.snapshot();
    assert_eq!(fake.count(Op::InitRatatui), 1);
    assert_eq!(fake.count(Op::RestoreRatatui), 1, "raw mode was on, so init must restore before propagating");
    assert!(!after.ratatui_active, "raw mode cleared despite the init failure");
    // The body never ran, so no extra modes were touched.
    assert_eq!(fake.count(Op::EnableBracketedPaste), 0);
}

/// Even when init fails before raw mode is active, the lifecycle restores
/// best-effort (idempotent): it cannot tell how far init got.
#[test]
fn init_failure_before_raw_mode_still_restores_best_effort() {
    let fake = FakeTerminalIo::default();
    fake.configure(|f| f.init_ratatui = Some(FailMode::Before));

    let result = run_lifecycle(fake.clone(), |_terminal, _modes| Box::pin(async move { Ok::<(), BodyError>(()) }));

    match result {
        Err(LifecycleError::Init(error)) => assert_eq!(error.kind(), io::ErrorKind::Unsupported),
        other => panic!("expected Init error, got {other:?}"),
    }
    assert_eq!(fake.count(Op::RestoreRatatui), 1, "lifecycle restores unconditionally on init failure");
    assert!(!fake.snapshot().ratatui_active);
}

#[test]
fn keyboard_push_fail_before_is_swallowed_but_popped() {
    let fake = FakeTerminalIo::default();
    fake.configure(|f| f.push_keyboard_enhancement = Some(FailMode::Before));

    run_lifecycle(fake.clone(), |_terminal, _modes| Box::pin(async move { Ok::<(), BodyError>(()) })).unwrap();

    let after = fake.snapshot();
    assert_eq!(after.keyboard_enhancement_depth, 0);
    assert_eq!(fake.count(Op::PushKeyboardEnhancement), 1);
    // The push reported failure, so setup pops best-effort even though it never recorded success.
    assert_eq!(fake.count(Op::PopKeyboardEnhancement), 1);
    assert_eq!(fake.count(Op::RestoreRatatui), 1);
}

#[test]
fn keyboard_push_fail_after_apply_is_popped() {
    let fake = FakeTerminalIo::default();
    fake.configure(|f| f.push_keyboard_enhancement = Some(FailMode::After));

    run_lifecycle(fake.clone(), |_terminal, _modes| Box::pin(async move { Ok::<(), BodyError>(()) })).unwrap();

    let after = fake.snapshot();
    // The partial push landed (depth 1) and setup's best-effort pop undid it (depth 0).
    assert_eq!(after.keyboard_enhancement_depth, 0);
    assert_eq!(fake.count(Op::PopKeyboardEnhancement), 1);
    assert_eq!(fake.count(Op::RestoreRatatui), 1);
}

#[test]
fn bracketed_paste_fail_before_propagates_as_setup_and_restores_once() {
    let fake = FakeTerminalIo::default();
    fake.configure(|f| f.enable_bracketed_paste = Some(FailMode::Before));

    let result = run_lifecycle(fake.clone(), |_terminal, _modes| Box::pin(async move { Ok::<(), BodyError>(()) }));

    match result {
        Err(LifecycleError::Setup(error)) => assert_eq!(error.kind(), io::ErrorKind::Unsupported),
        other => panic!("expected Setup error, got {other:?}"),
    }
    let after = fake.snapshot();
    assert_eq!(after.keyboard_enhancement_depth, 0, "keyboard pushed then popped on teardown");
    assert!(!after.bracketed_paste, "bracketed paste never landed");
    assert_eq!(fake.count(Op::RestoreRatatui), 1);
    assert!(!after.ratatui_active);
}

#[test]
fn bracketed_paste_fail_after_apply_is_disabled_and_error_preserved() {
    let fake = FakeTerminalIo::default();
    fake.configure(|f| f.enable_bracketed_paste = Some(FailMode::After));

    let result = run_lifecycle(fake.clone(), |_terminal, _modes| Box::pin(async move { Ok::<(), BodyError>(()) }));

    match result {
        Err(LifecycleError::Setup(error)) => assert_eq!(error.kind(), io::ErrorKind::Unsupported),
        other => panic!("expected Setup error, got {other:?}"),
    }
    let after = fake.snapshot();
    // The partial enable landed and setup's best-effort disable undid it.
    assert!(!after.bracketed_paste);
    assert_eq!(fake.count(Op::DisableBracketedPaste), 1);
    assert_eq!(after.keyboard_enhancement_depth, 0);
    assert_eq!(fake.count(Op::RestoreRatatui), 1);
}

#[test]
fn bracketed_paste_cleanup_failure_still_preserves_original_error() {
    let fake = FakeTerminalIo::default();
    fake.configure(|f| {
        f.enable_bracketed_paste = Some(FailMode::After);
        f.disable_bracketed_paste = Some(FailMode::Before);
    });

    let result = run_lifecycle(fake.clone(), |_terminal, _modes| Box::pin(async move { Ok::<(), BodyError>(()) }));

    match result {
        // The original enable error is returned even though cleanup also failed.
        Err(LifecycleError::Setup(error)) => {
            assert_eq!(error.to_string(), unsupported("enable_bracketed_paste").to_string());
        }
        other => panic!("expected Setup error, got {other:?}"),
    }
    let after = fake.snapshot();
    assert_eq!(fake.count(Op::DisableBracketedPaste), 2, "teardown retried cleanup");
    assert_eq!(fake.count(Op::PopKeyboardEnhancement), 1);
    assert!(!after.bracketed_paste);
    assert_eq!(fake.count(Op::RestoreRatatui), 1, "ratatui still restored exactly once");
    assert!(!after.ratatui_active);
}

#[test]
fn mouse_enable_failure_rolls_back_for_fail_before_and_after() {
    for mode in [FailMode::Before, FailMode::After] {
        let fake = FakeTerminalIo::default();
        fake.configure(|f| f.enable_mouse_capture = Some(mode));

        run_lifecycle(fake.clone(), |_terminal, mut modes| {
            Box::pin(async move {
                modes.set_mouse_capture(true);
                Ok::<(), BodyError>(())
            })
        })
        .unwrap();

        let after = fake.snapshot();
        assert!(!after.mouse_capture, "mouse not left captured for {mode:?}");
        assert_eq!(fake.count(Op::EnableMouseCapture), 1);
        // Enable was attempted and a conservative rollback disable followed.
        assert_eq!(fake.count(Op::DisableMouseCapture), 1);
        assert_eq!(fake.count(Op::RestoreRatatui), 1);
    }
}

#[test]
fn mouse_enable_failure_with_rollback_failure_retries_in_teardown() {
    let fake = FakeTerminalIo::default();
    fake.configure(|f| {
        f.enable_mouse_capture = Some(FailMode::After);
        f.disable_mouse_capture = Some(FailMode::After);
    });

    run_lifecycle(fake.clone(), |_terminal, mut modes| {
        Box::pin(async move {
            modes.set_mouse_capture(true);
            Ok::<(), BodyError>(())
        })
    })
    .unwrap();

    let after = fake.snapshot();
    // The rollback disable failed (After still applies), so the flag stayed set and teardown retried it.
    assert!(fake.count(Op::DisableMouseCapture) >= 2, "teardown retried the failed rollback");
    assert_eq!(fake.count(Op::RestoreRatatui), 1);
    assert!(!after.mouse_capture);
}

#[test]
fn mouse_disable_failure_retains_flag_for_teardown_retry() {
    let fake = FakeTerminalIo::default();
    fake.configure(|f| f.disable_mouse_capture = Some(FailMode::Before));

    run_lifecycle(fake.clone(), |_terminal, mut modes| {
        Box::pin(async move {
            modes.set_mouse_capture(true);
            modes.set_mouse_capture(false);
            Ok::<(), BodyError>(())
        })
    })
    .unwrap();

    let after = fake.snapshot();
    // The explicit disable failed, so the flag stayed set and teardown retried it.
    assert!(fake.count(Op::DisableMouseCapture) >= 2, "teardown retried the failed disable");
    assert_eq!(fake.count(Op::RestoreRatatui), 1);
    assert!(!after.mouse_capture);
}

#[test]
fn mouse_toggle_cycles_and_is_disabled_on_teardown() {
    let fake = FakeTerminalIo::default();

    run_lifecycle(fake.clone(), |_terminal, mut modes| {
        Box::pin(async move {
            modes.set_mouse_capture(true);
            modes.set_mouse_capture(false);
            modes.set_mouse_capture(true);
            Ok::<(), BodyError>(())
        })
    })
    .unwrap();

    let after = fake.snapshot();
    assert!(!after.mouse_capture);
    assert_eq!(fake.count(Op::RestoreRatatui), 1);
}

/// A stuck mouse disable must not strand bracketed-paste disable, keyboard pop,
/// or ratatui restore.
#[test]
fn failed_mouse_disable_does_not_block_other_teardown() {
    let fake = FakeTerminalIo::default();
    fake.configure(|f| f.disable_mouse_capture = Some(FailMode::Before));

    run_lifecycle(fake.clone(), |_terminal, mut modes| {
        Box::pin(async move {
            modes.set_mouse_capture(true);
            Ok::<(), BodyError>(())
        })
    })
    .unwrap();

    let after = fake.snapshot();
    assert!(after.mouse_capture, "mouse is genuinely stuck, but teardown continues");
    assert!(!after.bracketed_paste);
    assert_eq!(after.keyboard_enhancement_depth, 0);
    assert!(!after.ratatui_active);
    assert_eq!(fake.count(Op::RestoreRatatui), 1);
}

/// A stuck bracketed-paste disable must not strand keyboard pop or ratatui restore.
#[test]
fn failed_bracketed_paste_disable_does_not_block_other_teardown() {
    let fake = FakeTerminalIo::default();
    fake.configure(|f| f.disable_bracketed_paste = Some(FailMode::Before));

    run_lifecycle(fake.clone(), |_terminal, _modes| Box::pin(async move { Ok::<(), BodyError>(()) })).unwrap();

    let after = fake.snapshot();
    assert!(after.bracketed_paste, "bracketed paste is genuinely stuck, but teardown continues");
    assert_eq!(after.keyboard_enhancement_depth, 0);
    assert!(!after.ratatui_active);
    assert_eq!(fake.count(Op::RestoreRatatui), 1);
}

/// A stuck keyboard pop must not strand ratatui restore.
#[test]
fn failed_keyboard_pop_does_not_block_ratatui_restore() {
    let fake = FakeTerminalIo::default();
    fake.configure(|f| f.pop_keyboard_enhancement = Some(FailMode::Before));

    run_lifecycle(fake.clone(), |_terminal, _modes| Box::pin(async move { Ok::<(), BodyError>(()) })).unwrap();

    let after = fake.snapshot();
    assert_eq!(after.keyboard_enhancement_depth, 1, "keyboard pop is genuinely stuck, but teardown continues");
    assert!(!after.bracketed_paste);
    assert!(!after.ratatui_active);
    assert_eq!(fake.count(Op::RestoreRatatui), 1);
}
