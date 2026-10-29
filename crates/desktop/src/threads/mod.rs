mod driver;
mod manager;
mod status;
mod thread;

pub use manager::{Attention, ThreadManager, ThreadRef};
pub use status::ThreadStatus;
pub use thread::{Thread, ThreadEvent};

use gpui::{Context, Entity, Subscription};

pub(crate) fn observe_thread<T: 'static, U: PartialEq + 'static>(
    thread: &Entity<Thread>,
    cx: &mut Context<T>,
    project: impl Fn(&Thread) -> U + 'static,
    on_change: impl Fn(&mut T, &mut Context<T>) + 'static,
) -> Subscription {
    let mut seen = project(thread.read(cx));
    cx.observe(thread, move |this, thread, cx| {
        let now = project(thread.read(cx));
        if now != seen {
            seen = now;
            on_change(this, cx);
        }
    })
}
