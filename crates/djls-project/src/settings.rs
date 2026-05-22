mod candidates;
mod composition;

pub use candidates::settings_candidates;
pub use candidates::SettingsCandidate;
pub use candidates::SettingsCandidateIssue;
pub use candidates::SettingsCandidateOutcome;
pub use candidates::SettingsCandidateSource;
pub use composition::effective_settings;
pub use composition::EffectiveSettings;
pub use composition::PartialList;
pub use composition::PartialListSegment;
pub use composition::SettingsIssue;
pub use composition::TemplateBackend;
pub use composition::TemplateSettingsResolution;
