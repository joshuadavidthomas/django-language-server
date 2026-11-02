//! Transport-independent LSP pieces shared by the native server and the browser server:
//! client negotiation, advertised capabilities, open-document buffers, the overlay
//! filesystem that exposes them to the database, and the synchronous session core that
//! applies mutations, classifies Project work, and resolves document requests.

mod capabilities;
mod client;
mod document;
mod ext;
mod session;
mod workspace;

pub use capabilities::server_capabilities;
pub use capabilities::server_info;
pub use client::Client;
pub use client::ClientInfo;
pub use client::ClientOptions;
pub use document::TextDocument;
pub use ext::InitializeParamsExt;
pub use ext::UriExt;
pub use session::Analysis;
pub use session::DocumentUpdate;
pub use session::ProjectCoverage;
pub use session::ProjectWork;
pub use session::SessionCore;
pub use workspace::Workspace;

/// The released `djls` version, reported to clients and in issue reports.
pub const VERSION: &str = env!("DJLS_VERSION");
