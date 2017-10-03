#![feature(clippy, never_type, conservative_impl_trait, generators, plugin, proc_macro)]
extern crate bytes;
#[macro_use]
extern crate error_chain;
extern crate futures_await as futures;
#[macro_use]
extern crate lazy_static;
extern crate regex;
#[macro_use]
extern crate serde_derive;
#[macro_use]
extern crate serde_json;
extern crate tokio_core;
extern crate tokio_io;
extern crate tokio_file_unix;
extern crate tokio_process;

use std::io::{self, Cursor, BufRead, ErrorKind as IoErrorKind, Result as IoResult};
use std::num::ParseIntError;
use std::process::{Command, Stdio};

use bytes::BytesMut;
use futures::Sink;
use futures::prelude::*;
use regex::Regex;
use serde_json::Value;
use tokio_core::reactor::{Core, Handle};
use tokio_io::codec::{Decoder, Encoder, FramedRead, FramedWrite};
use tokio_file_unix::{File as TokioFile, StdFile};
use tokio_process::CommandExt;

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
        ClangFailed {
            description("ClangD failed")
            display("ClangD failed")
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Call {
    jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    method: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<Value>,
}


struct ContentLengthPrefixed {
    re: Regex,
}

impl ContentLengthPrefixed {
    fn new() -> Self {
        let re = Regex::new("Content-Length: \\s*(\\d+)").unwrap();
        ContentLengthPrefixed {
            re
        }
    }
}

impl Encoder for ContentLengthPrefixed {
    type Error = Error;
    type Item = Call;
    fn encode(&mut self, item: Call, dst: &mut BytesMut) -> Result<()> {
        let data = serde_json::to_vec(&item)?;
        let header = format!("Content-Length: {}\r\n\r\n", data.len());
        dst.extend_from_slice(header.as_bytes());
        dst.extend_from_slice(&data);
        Ok(())
    }
}

impl Decoder for ContentLengthPrefixed {
    type Error = Error;
    type Item = Call;
    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Call>> {
        let mut cursor = Cursor::new(src);
        let len = {
            let mut read_hdrs = || -> IoResult<String> {
                let mut headers = String::new();
                while cursor.read_line(&mut headers)? > 2 { }
                Ok(headers)
            };
            match read_hdrs() {
                Ok(headers) => {
                    if !headers.ends_with("\n\n") && !headers.ends_with("\r\n\r\n") {
                        return Ok(None);
                    }
                    self.re.captures_iter(&headers)
                        .next()
                        .ok_or(ErrorKind::HeaderMissing)?[1]
                        .parse::<usize>()?
                },
                Err(ref e) if e.kind() == IoErrorKind::UnexpectedEof => return Ok(None),
                Err(e) => return Err(e.into()),
            }
        };
        let pos = cursor.position() as usize;
        let src = cursor.into_inner();
        if len + pos > src.len() {
            return Ok(None);
        }
        src.split_to(pos);
        Ok(Some(serde_json::from_slice(&src.split_to(len))?))
    }
}

lazy_static! {
    static ref STDIN: io::Stdin = io::stdin();
    static ref STDOUT: io::Stdout = io::stdout();
}

#[async]
fn vim2clang<R, W>(reader: R, mut writer: W) -> Result<()>
where
    R: Stream<Item = Call, Error = Error> + 'static,
    W: Sink<SinkItem = Call, SinkError = Error> + 'static,
{
    #[async]
    for mut call in reader {
        let meta = if call.method == Some("textDocument/didOpen".to_owned()) {
            let lang_id = call.params
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
            let meta = vec![
                meta,
                "-Wall",
                "-Wextra",
                "-pedantic",
                "-DUNIT_TESTS=1",
                "-I/usr/include/catch"
            ];
            call.params
                .as_mut()
                .and_then(Value::as_object_mut)
                .unwrap()
                .insert("metadata".to_owned(), json!({"extraFlags": meta}));
        }
        writer = await!(writer.send(call))?;
    }
    Ok(())
}

#[async]
fn clang2vim<R, W>(reader: R, mut writer: W) -> Result<()>
where
    R: Stream<Item = Call, Error = Error> + 'static,
    W: Sink<SinkItem = Call, SinkError = Error> + 'static,
{
    #[async]
    for response in reader {
        writer = await!(writer.send(response))?;
    }
    Ok(())
}

#[async]
fn run(handle: Handle) -> Result<()> {
    let stdin = TokioFile::new_nb(StdFile(STDIN.lock()))?
        .into_io(&handle)?;
    let reader = FramedRead::new(stdin, ContentLengthPrefixed::new());
    let stdout = TokioFile::new_nb(StdFile(STDOUT.lock()))?
        .into_io(&handle)?;
    let writer = FramedWrite::new(stdout, ContentLengthPrefixed::new());
    let mut clangd = Command::new("clangd")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn_async(&handle)?;
    let clangd_writer = FramedWrite::new(clangd.stdin().take().unwrap(),
                                         ContentLengthPrefixed::new());
    let clangd_reader = FramedRead::new(clangd.stdout().take().unwrap(),
                                        ContentLengthPrefixed::new());
    await!(vim2clang(reader, clangd_writer).join(clang2vim(clangd_reader, writer)))?;
    if await!(clangd)?.success() {
        Ok(())
    } else {
        Err(ErrorKind::ClangFailed.into())
    }
}

fn main() {
    let mut core = Core::new().unwrap();
    let handle = core.handle();
    core.run(run(handle)).unwrap();
}
