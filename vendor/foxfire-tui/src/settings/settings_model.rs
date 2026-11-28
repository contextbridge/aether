use super::{ThemeSettings, UiSettings};

pub(crate) struct SettingsModel {
    ui: UiSettings,
    theme: ThemeApplication,
}

impl SettingsModel {
    pub(crate) fn new(ui: UiSettings) -> Self {
        Self { ui, theme: ThemeApplication::default() }
    }

    pub(crate) fn ui(&self) -> &UiSettings {
        &self.ui
    }

    pub(crate) fn request_theme_change(&mut self, selection: ThemeSettings) -> Option<UiSettings> {
        if self.theme.in_flight {
            self.theme.queued = Some(selection);
            None
        } else {
            Some(self.start_theme_change(selection))
        }
    }

    pub(crate) fn finish_theme_change(&mut self, settings: Option<UiSettings>) -> Option<UiSettings> {
        self.theme.in_flight = false;
        if let Some(settings) = settings {
            self.ui = settings;
        }
        self.theme.queued.take().map(|selection| self.start_theme_change(selection))
    }

    fn start_theme_change(&mut self, selection: ThemeSettings) -> UiSettings {
        let mut settings = self.ui.clone();
        settings.theme = selection;
        self.theme.in_flight = true;
        settings
    }
}

#[derive(Default)]
struct ThemeApplication {
    in_flight: bool,
    queued: Option<ThemeSettings>,
}
