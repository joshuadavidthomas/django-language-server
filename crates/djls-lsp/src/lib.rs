//! Transport-independent LSP pieces shared by the native server and the browser server:
//! client negotiation, advertised capabilities, open-document buffers, the overlay
//! filesystem that exposes them to the database, and the synchronous [`AnalysisHost`] that
//! applies mutations, classifies Project work, and resolves document requests.

mod analysis;
mod capabilities;
mod client;
mod document;
mod ext;
mod workspace;

pub use analysis::Analysis;
pub use analysis::AnalysisHost;
pub use analysis::DocumentUpdate;
pub use analysis::ProjectCoverage;
pub use analysis::ProjectWork;
pub use capabilities::server_capabilities;
pub use capabilities::server_info;
pub use client::Client;
pub use client::ClientInfo;
pub use client::ClientOptions;
pub use document::TextDocument;
pub use ext::InitializeParamsExt;
pub use ext::UriExt;
pub use workspace::Workspace;

/// The released `djls` version, reported to clients and in issue reports.
pub const VERSION: &str = env!("DJLS_VERSION");
