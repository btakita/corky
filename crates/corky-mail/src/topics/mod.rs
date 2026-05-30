//! Topic management module.

pub mod add;
pub mod info;
pub mod list;
pub mod suggest;

use anyhow::Result;

pub fn run_list(verbose: bool) -> Result<()> {
    list::run(verbose)
}

pub fn run_add(name: &str, keywords: &[String], description: Option<&str>) -> Result<()> {
    add::run(name, keywords, description)
}

pub fn run_info(name: &str) -> Result<()> {
    info::run(name)
}

pub fn run_suggest(limit: usize, mailbox: Option<&str>) -> Result<()> {
    suggest::run(limit, mailbox)
}
