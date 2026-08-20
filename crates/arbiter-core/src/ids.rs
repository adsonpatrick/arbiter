//! Strongly typed Arbiter identifiers.

use std::fmt;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

macro_rules! typed_id {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(Uuid);

        impl $name {
            #[must_use]
            pub fn new() -> Self {
                Self(Uuid::now_v7())
            }

            #[must_use]
            pub const fn as_uuid(&self) -> &Uuid {
                &self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl From<Uuid> for $name {
            fn from(value: Uuid) -> Self {
                Self(value)
            }
        }

        impl From<$name> for Uuid {
            fn from(value: $name) -> Self {
                value.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }
    };
}

typed_id!(RequestId);
typed_id!(AttemptId);
typed_id!(EventId);

#[cfg(test)]
mod tests {
    use super::{AttemptId, EventId, RequestId};
    use uuid::Version;

    #[test]
    fn generated_ids_are_distinct_uuid_v7_values() {
        let request_a = RequestId::new();
        let request_b = RequestId::new();
        let attempt_a = AttemptId::new();
        let attempt_b = AttemptId::new();
        let event_a = EventId::new();
        let event_b = EventId::new();

        assert_ne!(request_a, request_b);
        assert_ne!(attempt_a, attempt_b);
        assert_ne!(event_a, event_b);
        assert_eq!(request_a.as_uuid().get_version(), Some(Version::SortRand));
        assert_eq!(attempt_a.as_uuid().get_version(), Some(Version::SortRand));
        assert_eq!(event_a.as_uuid().get_version(), Some(Version::SortRand));
    }
}
