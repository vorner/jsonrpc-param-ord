#[macro_use]
extern crate error_chain;
extern crate regex;
#[macro_use]
extern crate serde_derive;
#[macro_use]
extern crate serde_json;

use std::io::{self, BufRead, Read, Write};
use std::num::ParseIntError;

use regex::Regex;
use serde_json::Value;

error_chain! {
    foreign_links {
        Io(io::Error);
        SerdeJson(serde_json::Error);
        NumParse(ParseIntError);
    }
    errors {
        HeaderMissing {
            description("Important header missing")
            display("Important header missing")
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Call {
    jsonrpc: String,
    method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<Value>,
}

fn run() -> Result<()> {
    let input = io::stdin();
    let mut input = input.lock();
    let re = Regex::new("Content-Length:\\s*(\\d+)").unwrap();
    loop {
        let mut headers = String::new();
        while input.read_line(&mut headers)? > 2 { }
        eprintln!("{:?}", headers);
        // Less of unwraps
        let size = re.captures_iter(&headers)
            .next()
            .ok_or(ErrorKind::HeaderMissing)?[1]
            .parse::<usize>()?;
        let mut data = vec![0u8; size];
        input.read_exact(&mut data)?;
        if let Ok(mut value) = serde_json::from_slice::<Call>(&data) {
            let meta = if value.method == "textDocument/didOpen" {
                let lang_id = value.params
                    .as_ref()
                    .and_then(|p| p.pointer("/textDocument/languageId"))
                    .and_then(Value::as_str);
                eprintln!("lagID: {:?}", lang_id);
                match lang_id {
                    Some("c") => Some("--std=gnu99"),
                    Some("cpp") => Some("--std=c++1z"),
                    _ => None,
                }
            } else {
                None
            };
            if let Some(meta) = meta {
                let meta = vec![meta, "-Wall", "-Wextra", "-pedantic"];
                value.params
                    .as_mut()
                    .and_then(Value::as_object_mut)
                    .unwrap()
                    .insert("metadata".to_owned(), json!({"extraFlags": meta}));
            }
            let data = serde_json::to_vec(&value)?;
            let output = io::stdout();
            let mut output = output.lock();
            write!(output, "Content-Length: {}\r\n\r\n", data.len())?;
            output.write_all(&data)?;
            output.flush()?;
            eprintln!("Flush");
        } else {
            eprintln!("Format error, ignoring");
        }
    }
}

fn main() {
    run().unwrap();
}
