//! `lila memory view|search` — inspect/repair the manager's memory store from the host.

use crate::cli::MemoryAction;
use crate::config::Config;
use crate::memory::{MemFs, MemFsOptions, SearchHit};

pub fn run(action: MemoryAction) -> i32 {
    let cfg = match Config::load() {
        Ok(cfg) => cfg,
        Err(err) => return fail("CONFIG ERROR", &err.to_string()),
    };
    let mem = match MemFs::open(MemFsOptions {
        dir: cfg.memory_dir.clone().into(),
        fts_path: format!("{}.fts.sqlite", cfg.memory_dir),
    }) {
        Ok(mem) => mem,
        Err(err) => return fail("MEMORY ERROR", &err.to_string()),
    };
    match action {
        MemoryAction::View { path } => view(&mem, &path),
        MemoryAction::Search { query } => search(&mem, &query),
    }
}

fn view(mem: &MemFs, path: &str) -> i32 {
    match mem.view(path, None) {
        Ok(text) => {
            println!("{text}");
            0
        }
        Err(err) => fail("error", &err.to_string()),
    }
}

fn search(mem: &MemFs, query: &str) -> i32 {
    match mem.search(query, 20) {
        Ok(hits) => {
            print_hits(&hits);
            0
        }
        Err(err) => fail("error", &err.to_string()),
    }
}

fn print_hits(hits: &[SearchHit]) {
    if hits.is_empty() {
        println!("(no matches)");
        return;
    }
    for h in hits {
        println!("{}\n    {}", h.path, h.snippet);
    }
}

fn fail(label: &str, msg: &str) -> i32 {
    eprintln!("{label}: {msg}");
    1
}
