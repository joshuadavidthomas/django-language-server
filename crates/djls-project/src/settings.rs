mod candidates;
mod composition;

pub(crate) use candidates::settings_candidates;
pub(crate) use candidates::SettingsCandidate;
pub(crate) use candidates::SettingsCandidateSource;
pub(crate) use composition::django_settings;
pub(crate) use composition::PartialListSegment;
