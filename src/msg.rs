use std::error::Error as OldError;
use std::fmt::{Display, Formatter, Result as FmtResult};
use std::io::BufRead;

use failure::Error;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde::de::{Error as DError, Unexpected};
use serde_json::{self, Value};

#[derive(Copy, Clone, Debug)]
pub(crate) struct Version;

impl<'de> Deserialize<'de> for Version {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer)
            .and_then(|value| match &value[..] {
                "2.0" => Ok(Version),
                val => Err(D::Error::invalid_value(Unexpected::Str(val), &"value 2.0")),
            })
    }
}

impl Serialize for Version {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str("2.0")
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) enum Special {

}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct RpcError {
    code: i64,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(untagged, deny_unknown_fields)]
pub(crate) enum Msg {
    Special(Special),
    Reply {
        jsonrpc: Version,
        result: Value,
        id: Value,
    },
    Error {
        jsonrpc: Version,
        error: RpcError,
        id: Value,
    },
    Request {
        jsonrpc: Version,
        method: String,
        id: Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        params: Option<Value>,
    }
}

// TODO: Derive once it is ready
#[derive(Clone, Debug)]
pub(crate) struct HeaderFormat;

impl Display for HeaderFormat {
    fn fmt(&self, f: &mut Formatter) -> FmtResult {
        self.description().fmt(f)
    }
}

impl OldError for HeaderFormat {
    fn description(&self) -> &str {
        "Invalid content-length header format"
    }
}

impl Msg {
    pub(crate) fn from_reader<R: BufRead>(mut reader: R) -> Result<Self, Error> {
        // FIXME: This assumes very specific input format.
        let mut buffer = String::new();
        // Read the line with Content-Length
        reader.read_line(&mut buffer)?;
        let len = buffer.split_whitespace()
            .nth(1)
            .ok_or(HeaderFormat)?
            .parse()?;
        // Get rid of the next empty line
        reader.read_line(&mut buffer)?;
        let data = reader.take(len);
        Ok(serde_json::from_reader(data)?)
    }
}
