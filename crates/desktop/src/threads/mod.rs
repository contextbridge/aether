mod driver;
mod manager;
mod thread;

pub use manager::{Attention, ThreadManager, ThreadRef};
pub use thread::{Thread, ThreadEvent, ThreadStatus};

use gpui::{App, Context, Entity, Subscription};

pub(crate) fn observe_thread<T: 'static, U: PartialEq + 'static>(
    thread: &Entity<Thread>,
    cx: &mut Context<T>,
    project: impl Fn(&Thread) -> U + 'static,
    on_change: impl Fn(&mut T, &mut Context<T>) + 'static,
) -> Subscription {
    observe_changes(thread, cx, move |thread, _| project(thread), on_change)
}

pub(crate) fn observe_changes<T: 'static, U: 'static, V: PartialEq + 'static>(
    entity: &Entity<U>,
    cx: &mut Context<T>,
    project: impl Fn(&U, &App) -> V + 'static,
    on_change: impl Fn(&mut T, &mut Context<T>) + 'static,
) -> Subscription {
    let mut seen = project(entity.read(cx), cx);
    cx.observe(entity, move |this, entity, cx| {
        let now = project(entity.read(cx), cx);
        if now != seen {
            seen = now;
            on_change(this, cx);
        }
    })
}
