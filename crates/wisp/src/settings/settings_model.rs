use super::UiSettings;

pub(crate) struct SettingsModel {
    ui: UiSettings,
    theme: ThemeApplication,
}

#[derive(Default)]
struct ThemeApplication {
    in_flight: bool,
    queued: Option<String>,
}

pub(crate) struct ThemeChangeRequest {
    pub(crate) settings: UiSettings,
    pub(crate) value: String,
}

impl SettingsModel {
    pub(crate) fn new(ui: UiSettings) -> Self {
        Self { ui, theme: ThemeApplication::default() }
    }

    pub(crate) fn ui(&self) -> &UiSettings {
        &self.ui
    }

    pub(crate) fn request_theme_change(&mut self, value: String) -> Option<ThemeChangeRequest> {
        if self.theme.in_flight {
            self.theme.queued = Some(value);
            None
        } else {
            Some(self.start_theme_change(value))
        }
    }

    pub(crate) fn finish_theme_change(&mut self, settings: UiSettings) -> Option<ThemeChangeRequest> {
        self.theme.in_flight = false;
        if let Some(value) = self.theme.queued.take() {
            Some(self.start_theme_change(value))
        } else {
            self.ui = settings;
            None
        }
    }

    fn start_theme_change(&mut self, value: String) -> ThemeChangeRequest {
        let mut settings = self.ui.clone();
        settings.theme.file = (!value.is_empty()).then(|| value.clone());
        self.theme.in_flight = true;
        ThemeChangeRequest { settings, value }
    }
}
