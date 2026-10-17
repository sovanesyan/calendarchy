mod calendar;
mod types;

pub use calendar::CalDavClient;
pub use types::{EventTime, ICalEvent};

// These are only used in tests
#[cfg(test)]
pub use types::ICalAttendee;
