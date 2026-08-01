//! Explicit facade for integration tests.

pub use crate::terminal::inline_viewport_height;

pub mod terminal {
    pub use crate::terminal::{
        LifecycleError, TerminalIo, TerminalModes, inline_viewport_height, run_terminal_lifecycle,
    };
}

pub mod app {
    pub use crate::app::*;
}

pub mod attachments {
    pub use crate::attachments::*;
}

pub mod composer {
    pub use crate::composer::*;
}

pub mod elicitation {
    pub use crate::elicitation::*;
}

pub mod filterable_list {
    pub use crate::filterable_list::*;
}

pub mod generation {
    pub use crate::generation::*;
}

pub mod git_diff {
    pub use crate::git_diff::*;
}

pub mod picker {
    pub use crate::picker::*;
}

pub mod plan_review {
    pub use crate::plan_review::*;
}

pub mod progress_indicator {
    pub use crate::progress_indicator::*;
}

pub mod renderer {
    pub use crate::renderer::*;
}

pub mod screens {
    pub mod git_diff {
        pub use crate::screens::git_diff::*;
    }

    pub mod plan_review {
        pub use crate::screens::plan_review::*;
    }
}

pub mod selection {
    pub use crate::selection::*;
}

pub mod session_config_view {
    pub use crate::session_config_view::*;
}

pub mod settings {
    pub use crate::settings::*;
}

pub mod settings_overlay {
    pub use crate::settings_overlay::*;
}

pub mod surface {
    pub use crate::surface::*;
}

pub mod syntax {
    pub use crate::syntax::*;
}

pub mod tasks {
    pub use crate::tasks::*;
}

pub mod theme {
    pub use crate::theme::*;
}

pub mod tool_calls {
    pub use crate::tool_calls::*;
}

pub mod workspace_status {
    pub use crate::workspace_status::*;
}
