//! La Suite service clients (People, Docs, Meet, Drive, Messages, Calendars, Find).

pub mod people;
pub mod docs;
pub mod meet;
pub mod drive;
pub mod messages;
pub mod calendars;
pub mod find;
pub mod types;

pub use people::PeopleClient;
pub use docs::DocsClient;
pub use meet::MeetClient;
pub use drive::DriveClient;
pub use messages::MessagesClient;
pub use calendars::CalendarsClient;
pub use find::FindClient;
