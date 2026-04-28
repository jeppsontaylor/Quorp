#![allow(dead_code)]

#[path = "quorp.rs"]
mod quorp;
#[path = "quorp/cli.rs"]
mod cli;

pub fn dispatch() {
    cli::main();
}

pub fn main() {
    dispatch();
}
