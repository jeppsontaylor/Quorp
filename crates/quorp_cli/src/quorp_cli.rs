#![allow(dead_code)]

#[path = "quorp/cli.rs"]
mod cli;
#[path = "quorp.rs"]
mod quorp;

pub fn dispatch() {
    cli::main();
}

pub fn main() {
    dispatch();
}
