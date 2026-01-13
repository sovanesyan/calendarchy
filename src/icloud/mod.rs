mod auth;
mod calendar;
mod types;

pub use auth::ICloudAuth;
pub use calendar::CalDavClient;
pub use types::ICalEvent;

// These are only used in tests
#[cfg(test)]
pub use types::{EventTime, ICalAttendee};
