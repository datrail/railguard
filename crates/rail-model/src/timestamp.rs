//! A moment as Rail Center's documents write one: RFC 3339 in UTC, with `Z`.

use std::time::{SystemTime, UNIX_EPOCH};

use serde::ser::Error;
use serde::{Serialize, Serializer};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// Written to the nanosecond the clock gives; a time before 1970 or past 9999 is an error.
#[derive(Clone, Copy, Debug)]
pub struct Timestamp(pub SystemTime);

impl Serialize for Timestamp {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // Checked: a clock far off would panic `time`'s `From<SystemTime>`.
        let nanos = match self.0.duration_since(UNIX_EPOCH) {
            Ok(since) => i128::try_from(since.as_nanos()).map_err(S::Error::custom)?,
            Err(before) => {
                -i128::try_from(before.duration().as_nanos()).map_err(S::Error::custom)?
            }
        };
        let written = OffsetDateTime::from_unix_timestamp_nanos(nanos)
            .map_err(S::Error::custom)?
            .format(&Rfc3339)
            .map_err(S::Error::custom)?;
        serializer.serialize_str(&written)
    }
}
