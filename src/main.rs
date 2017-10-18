#![feature(clippy, never_type, conservative_impl_trait, generators, plugin, proc_macro)]
extern crate bytes;
#[macro_use]
extern crate error_chain;
extern crate futures_await as futures;
extern crate glob;
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
extern crate url;

use std::cell::RefCell;
use std::env;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Cursor, ErrorKind as IoErrorKind, Result as IoResult};
use std::num::ParseIntError;
use std::process::{self, Command, Stdio};
use std::rc::Rc;

use bytes::BytesMut;
use futures::Sink;
use futures::unsync::mpsc;
use futures::prelude::*;
use glob::Pattern;
use regex::Regex;
use serde_json::Value;
use tokio_core::reactor::{Core, Handle};
use tokio_io::codec::{Decoder, Encoder, FramedRead, FramedWrite};
use tokio_file_unix::{File as TokioFile, StdFile};
use tokio_process::CommandExt;
use url::Url;

error_chain! {
    foreign_links {
        Io(io::Error);
        SerdeJson(serde_json::Error);
        NumParse(ParseIntError);
        Send(mpsc::SendError<Call>);
        Glob(glob::PatternError);
        Url(url::ParseError);
    }
    errors {
        NoOptsParam {
            description("Missing the options file paramater")
            display("Missing the options file paramater")
        }
        OptSyntax(s: &'static str) {
            description("Syntax error in the opts file")
            display("Syntax error in the opts file: {}", s)
        }
        HeaderMissing {
            description("Important header missing")
            display("Important header missing")
        }
        ClangFailed {
            description("ClangD failed")
            display("ClangD failed")
        }
        Unspecified {
            description("Stupid unspecified () error")
            display("Stupid unspecified () error")
        }
    }
}

impl From<()> for Error {
    fn from(_: ()) -> Error {
        ErrorKind::Unspecified.into()
    }
}

#[derive(Serialize, Deserialize)]
pub struct Call {
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

struct WaitingChange {
    call: Option<Call>,
    waiting: bool,
}

type Waiting = Rc<RefCell<WaitingChange>>;

#[async]
fn vim2clang<R, W>(reader: R, mut writer: W, waiting: Waiting, opts: Vec<Opts>) -> Result<()>
where
    R: Stream<Item = Call, Error = Error> + 'static,
    W: Sink<SinkItem = Call, SinkError = mpsc::SendError<Call>> + 'static,
{
    #[async]
    for mut call in reader {
        /* TODO Disabled for now. Doesn't seem to work very well.
        let mut pre_call = None;
        if call.method == Some("textDocument/didChange".to_owned()) {
            let mut borrow = waiting.borrow_mut();
            // Nothing waiting for an answer, send it through
            if !borrow.waiting {
                borrow.waiting = true;
                eprintln!("Sending right away");
            } else {
                // We wait for an answer. Therefore, delay this one (and overwrite any delayed one
                // if there is).
                borrow.call = Some(call);
                eprintln!("Postponing");
                continue;
            }
        } else {
            let mut borrow = waiting.borrow_mut();
            pre_call = borrow.call.take();
        }
        if let Some(pre) = pre_call {
            eprintln!("Pre-send");
            writer = await!(writer.send(pre))?;
        }
        */
        let meta = if call.method == Some("textDocument/didOpen".to_owned()) {
            let url = call.params
                .as_ref()
                .and_then(|p| p.pointer("/textDocument/uri"))
                .and_then(Value::as_str);
            if let Some(url) = url {
                let path = Url::parse(url)?.to_file_path().unwrap();
                eprintln!("URL {:?}/{:?}", url, path);
                let mut result = Vec::new();
                for opt in &opts {
                    if !opt.glob.matches_path(&path) {
                        continue;
                    }
                    eprintln!("Match: {:?}", opt);
                    let new = opt.opts.iter().cloned();
                    match opt.mode {
                        OptsMode::Append => result.extend(new),
                        OptsMode::Replace => result = new.collect(),
                    }
                }
                Some(result)
            } else {
                None
            }
        } else {
            None
        };
        if let Some(meta) = meta {
            eprintln!("Meta: {:?}", meta);
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
fn clang2vim<R, VW, CW>(reader: R, mut vim_writer: VW, mut clang_writer: CW, waiting: Waiting)
    -> Result<()>
where
    R: Stream<Item = Call, Error = Error> + 'static,
    VW: Sink<SinkItem = Call, SinkError = Error> + 'static,
    CW: Sink<SinkItem = Call, SinkError = mpsc::SendError<Call>> + 'static,
{
    #[async]
    for response in reader {
        let mut call = None;
        if response.method == Some("textDocument/publishDiagnostics".to_owned()) {
            let mut borrow = waiting.borrow_mut();
            call = borrow.call.take();
            if call.is_none() {
                borrow.waiting = false;
                eprintln!("Nothing is waiting now");
            }
        }
        let send_vim = vim_writer.send(response);
        if let Some(call) = call {
            eprintln!("Sending waiting change");
            let res = await!(clang_writer.send(call).map_err(Error::from).join(send_vim))?;
            clang_writer = res.0;
            vim_writer = res.1;
        } else {
            vim_writer = await!(send_vim)?;
        }
    }
    Ok(())
}

#[async]
fn run(opts: Vec<Opts>, handle: Handle) -> Result<()> {
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
    let (sender, receiver) = mpsc::channel(1);
    let forward = clangd_writer.send_all(receiver);
    let waiting = WaitingChange {
        call: None,
        waiting: false,
    };
    let waiting = Rc::new(RefCell::new(waiting));
    let done = vim2clang(reader, sender.clone(), waiting.clone(), opts)
        .join(clang2vim(clangd_reader, writer, sender, waiting))
        .join(forward);
    await!(done)?;
    if await!(clangd)?.success() {
        Ok(())
    } else {
        Err(ErrorKind::ClangFailed.into())
    }
}

#[derive(Debug)]
enum OptsMode {
    Append,
    Replace,
}

#[derive(Debug)]
struct Opts {
    glob: Pattern,
    opts: Vec<String>,
    mode: OptsMode,
}

fn opts_load() -> Result<Vec<Opts>> {
    let path = env::args()
        .nth(1)
        .ok_or(ErrorKind::NoOptsParam)?;
    let f = File::open(path)?;
    let buf = BufReader::new(f);
    let mut result = Vec::new();
    for l in buf.lines() {
        let l = l?;
        if l.is_empty() {
            continue;
        }
        let mode = match &l[..1] {
            "+" => OptsMode::Append,
            "=" => OptsMode::Replace,
            "#" => continue,
            _ => bail!(ErrorKind::OptSyntax("Unknown sigil")),
        };
        let mut content = l[1..].split_whitespace();
        let glob = content.next()
            .map(Pattern::new)
            .ok_or(ErrorKind::OptSyntax("Missing pattern"))??;
        let opts = content
            .map(str::to_owned)
            .collect();
        result.push(Opts {
            glob,
            opts,
            mode,
        });
    }
    Ok(result)
}

fn run_all() -> Result<()> {
    let opts = opts_load()?;
    let mut core = Core::new()?;
    let handle = core.handle();
    core.run(run(opts, handle))
}

fn main() {
    match run_all() {
        Ok(()) => (),
        Err(e) => {
            eprintln!("{}", e);
            process::exit(1);
        },
    }
}
