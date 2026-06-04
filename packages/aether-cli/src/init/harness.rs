use aether_project::PromptSource;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, clap::ValueEnum)]
#[clap(rename_all = "kebab-case")]
pub enum HarnessIntegration {
    Claude,
    Agents,
}

impl HarnessIntegration {
    fn dir_name(self) -> &'static str {
        match self {
            Self::Claude => ".claude",
            Self::Agents => ".agents",
        }
    }

    fn file_name(self) -> &'static str {
        match self {
            Self::Claude => "CLAUDE",
            Self::Agents => "AGENTS",
        }
    }

    pub fn title(self) -> String {
        format!("{}.md + {}/", self.file_name(), self.dir_name())
    }

    pub fn description(self) -> String {
        format!("load {}.md, {}/skills, and {}/rules", self.file_name(), self.dir_name(), self.dir_name())
    }

    pub fn prompt_source(self) -> Option<PromptSource> {
        Some(PromptSource::file(format!("${{WORKSPACE}}/{}.md", self.file_name())).optional())
    }

    pub fn skills_dirs(self) -> Vec<String> {
        self.dirs("skills")
    }

    pub fn rules_dirs(self) -> Vec<String> {
        self.dirs("rules")
    }

    fn dirs(self, subdir: &str) -> Vec<String> {
        let dir = self.dir_name();
        vec![format!("${{HOME}}/{dir}/{subdir}"), format!("${{WORKSPACE}}/{dir}/{subdir}")]
    }
}
