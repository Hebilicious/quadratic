use serde_json::Value;

use super::Refused;

pub(super) struct Listing {
    digest: String,
}

impl Listing {
    pub(super) fn of(parameters: &Value) -> Listing {
        Listing {
            digest: fnv1a(&parameters.to_string()),
        }
    }

    pub(super) fn bind(&self, position: String) -> String {
        format!("{}.{position}", self.digest)
    }

    pub(super) fn position(&self, cursor: Option<String>) -> Result<Option<String>, Refused> {
        let Some(cursor) = cursor else {
            return Ok(None);
        };
        match cursor.split_once('.') {
            Some((digest, position)) if digest == self.digest => Ok(Some(position.to_owned())),
            _ => Err(Refused::tagged("BadCursor")),
        }
    }
}

fn fnv1a(text: &str) -> String {
    let hash = text.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    });
    format!("{hash:016x}")
}
