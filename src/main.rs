extern crate regex;
extern crate serde;
#[macro_use]
extern crate serde_derive;
extern crate serde_json;

use std::io::{self, BufRead, Read, Result, Write};

use regex::Regex;
use serde_json::Value;

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
        let size = re.captures_iter(&headers).next().unwrap()[1].parse::<usize>().unwrap();
        let mut data = vec![0u8; size];
        input.read_exact(&mut data)?;
        // Less of unwraps
        let value = serde_json::from_slice::<Call>(&data).unwrap();
        let data = serde_json::to_vec(&value).unwrap();
        let output = io::stdout();
        let mut output = output.lock();
        write!(output, "Content-Length: {}\r\n\r\n", data.len())?;
        output.write_all(&data)?;
        output.flush()?;
        eprintln!("Flush");
    }
}

fn main() {
    run().unwrap();
}
