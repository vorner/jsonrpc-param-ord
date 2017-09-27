#![feature(splice)]

#[macro_use]
extern crate error_chain;
#[macro_use]
extern crate serde_derive;
extern crate serde_json;

use std::collections::HashSet;
use std::env;
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

error_chain! {
    foreign_links {
        Io(io::Error);
        Json(serde_json::Error);
    }

    errors {
        MissingDatabasePath {
            description("Missing the database path")
            display("Missing the database path")
        }
    }
}

#[derive(Clone, Deserialize, Serialize)]
struct Command {
    arguments: Vec<String>,
    directory: String,
    file: String,
    // TODO: Cache?
    #[serde(default = "SystemTime::now", skip_serializing)]
    created: SystemTime,
}

impl Command {
    fn read<P: AsRef<Path>>(path: P) -> Result<Vec<Self>> {
        let f = File::open(path)?;
        Ok(serde_json::from_reader(f)?)
    }
    fn write<P: AsRef<Path>>(path: P, commands: &Vec<Command>) -> Result<()> {
        let f = File::create(path)?;
        Ok(serde_json::to_writer_pretty(f, commands)?)
    }
    fn full_path(&self) -> Option<PathBuf> {
        Path::new(&self.directory)
            .join(&self.file)
            .canonicalize()
            .ok()
    }
    fn fix_name(&mut self, old: &str) {
        for param in &mut self.arguments {
            if param == old {
                *param = self.file.clone();
            }
        }
    }
    fn set_ext(&mut self, ext: &str) -> bool {
        let len = self.file.len();
        if self.file.ends_with(".c") {
            self.file.splice(len - 1.., ext);
            true
        } else if self.file.ends_with(".cpp") {
            self.file.splice(len - 3.., ext);
            true
        } else {
            false
        }
    }
}

fn run() -> Result<()> {
    let fname = env::args_os()
        .nth(1)
        .ok_or(ErrorKind::MissingDatabasePath)?;
    let mut commands = Command::read(&fname)?;
    let timeout = SystemTime::now() - Duration::from_secs(30 * 24 * 3600);
    commands.retain(|c| c.created >= timeout);
    let index = commands
        .iter()
        .filter_map(Command::full_path)
        .collect::<HashSet<_>>();
    let new_commands = commands
        .iter()
        .flat_map(|c| {
            let gen = |ext| {
                let mut cloned = c.clone();
                let old = cloned.file.clone();
                if cloned.set_ext(ext) {
                    cloned.fix_name(&old);
                    Some(cloned)
                } else {
                    None
                }
            };
            gen("h").into_iter().chain(gen("hpp").into_iter())
        })
        .filter(|c| {
            c.full_path()
                .map(|p| !index.contains(&p))
                .unwrap_or(false)
        })
        .collect::<Vec<_>>();
    commands.extend(new_commands);
    commands.retain(|c| {
        c.full_path()
            .map(|p| p.exists())
            .unwrap_or(false)
    });
    let mut tmp = fname.clone();
    tmp.push(".tmp");
    Command::write(&tmp, &commands)?;
    fs::rename(&tmp, &fname)?;
    Ok(())
}

fn main() {
    run().unwrap();
}
