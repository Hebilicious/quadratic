use super::error::StoreError;
use jiff::{SignedDuration, Timestamp};

#[derive(Debug, Default)]
pub(crate) struct Clock {
    last: Option<Timestamp>,
}

impl Clock {
    pub(crate) fn after(last: Option<&str>) -> Clock {
        Clock {
            last: last.and_then(|text| text.parse().ok()),
        }
    }

    pub(crate) fn next(&mut self) -> String {
        let now = Timestamp::from_millisecond(Timestamp::now().as_millisecond())
            .unwrap_or(Timestamp::UNIX_EPOCH);
        let next = match self.last {
            Some(last) if now <= last => last + SignedDuration::from_millis(1),
            _ => now,
        };
        self.last = Some(next);
        format(next)
    }
}

pub(crate) fn parse(text: &str) -> Result<String, StoreError> {
    text.parse::<Timestamp>()
        .map(format)
        .map_err(|_| StoreError::InvalidTime(text.to_owned()))
}

pub(crate) fn format(moment: Timestamp) -> String {
    moment.strftime("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}
