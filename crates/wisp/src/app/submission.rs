use super::App;
use client_core::workspace_status::WorkspaceAccess;

impl App {
    pub(super) fn submit(&mut self) {
        if self.composer.is_empty() || !self.core.foreground().is_idle() {
            return;
        }

        if self.core.session().workspace_access() == WorkspaceAccess::Remote
            && (!self.composer.selected_mentions().is_empty() || !self.composer.pending_media().is_empty())
        {
            self.notify("Path attachments are unavailable for remote workspaces; remove attachments before sending");
            return;
        }
        let submission = self.composer.take_submission();
        self.update_core(|core| core.submit(submission));
    }

    pub(super) fn send_prompt_search_query(&mut self, query: String) {
        if self.composer.prompt_search().is_none() {
            return;
        }
        self.update_core(|core| core.search_prompts(query));
    }
}
